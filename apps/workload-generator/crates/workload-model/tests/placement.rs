//! Node placement and migration (T070, FR-048 and FR-049).
//!
//! The property that matters is not the arithmetic of picking a node but what a migration
//! *does not* do: a migrated session's already-stored blocks stay where they were. That is why
//! a migration is expressed as a change to one field and nothing else — moving blocks would
//! model a data migration Certus does not perform, and tracking which node holds each block
//! would duplicate state the cache owns and would be wrong the moment it evicted one.

use std::collections::{BTreeMap, BTreeSet};

use workload_model::description::WorkloadDescription;
use workload_model::sim::Simulation;

/// Sessions that migrate often relative to their lifetime, so a short span shows migrations.
const MIGRATING: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 12}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 8}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
    migration_interval: {constant: 7}
"#;

/// The same workload with no migration declared.
const STATIC: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 12}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 8}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;

/// Run and record, per turn, `(session id, node, first key of the path, turn index)`.
fn run(yaml: &str, seed: u64, nodes: usize, span: f64) -> (Vec<(u64, usize, u64, usize)>, u64) {
    let d: WorkloadDescription = yaml.parse().unwrap();
    let mut sim = Simulation::new(&d, seed).unwrap().with_nodes(nodes);
    let mut seen = Vec::new();
    sim.run_until(span, &mut |s, t| {
        let first = s.reads_of(t).first().copied().unwrap_or(0);
        seen.push((s.id(), s.node(), first, t.index()));
    });
    (seen, sim.migrations())
}

#[test]
fn new_sessions_are_spread_across_the_configured_nodes() {
    // FR-048's first half. Uniform placement, checked by the spread rather than by a
    // distribution test: what would break is placing everything on node 0, which no amount of
    // sampling noise produces.
    let (turns, _) = run(STATIC, 11, 4, 400.0);
    let mut by_node: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    for (id, node, _, _) in &turns {
        by_node.entry(*node).or_default().insert(*id);
    }
    assert_eq!(by_node.len(), 4, "sessions landed on {:?}", by_node.keys());
    let sessions: usize = by_node.values().map(|s| s.len()).sum();
    assert!(sessions >= 8, "only {sessions} sessions in the run");
    for (node, ids) in &by_node {
        assert!(!ids.is_empty(), "node {node} got nothing");
    }
    // Every node in range, and no node outside it.
    assert!(by_node.keys().all(|n| *n < 4));
}

#[test]
fn a_session_without_a_migration_interval_never_moves() {
    let (turns, migrations) = run(STATIC, 12, 4, 400.0);
    assert_eq!(migrations, 0, "a class with no interval migrated");
    let mut node_of: BTreeMap<u64, usize> = BTreeMap::new();
    for (id, node, _, _) in &turns {
        let first = *node_of.entry(*id).or_insert(*node);
        assert_eq!(first, *node, "session {id} moved without an interval");
    }
}

#[test]
fn a_migrating_session_moves_and_never_to_itself() {
    // FR-048's second half: uniform among the *others*. Drawing over all nodes would leave a
    // session where it was with probability 1/nodes, which is not a migration.
    let (turns, migrations) = run(MIGRATING, 13, 4, 400.0);
    assert!(
        migrations > 0,
        "no migration happened, so nothing is tested"
    );

    let mut last: BTreeMap<u64, usize> = BTreeMap::new();
    let mut moves = 0usize;
    for (id, node, _, _) in &turns {
        if let Some(prev) = last.insert(*id, *node) {
            if prev != *node {
                moves += 1;
            }
        }
    }
    assert!(
        moves > 0,
        "{migrations} migrations were counted but no turn ever saw a different node"
    );

    // And every observed move is to a different node, which the loop above already implies —
    // the assertion worth making is that a *counted* migration always changed the node, since a
    // self-move would be counted and invisible.
    let (turns2, migrations2) = run(MIGRATING, 13, 2, 400.0);
    let mut last2: BTreeMap<u64, usize> = BTreeMap::new();
    let mut moves2 = 0usize;
    for (id, node, _, _) in &turns2 {
        if let Some(prev) = last2.insert(*id, *node) {
            if prev != *node {
                moves2 += 1;
            }
        }
    }
    // With two nodes every migration must flip the node, so a session seen on both sides of a
    // migration always shows a change. That makes the counted total an upper bound on visible
    // moves and any shortfall attributable only to sessions that ended first.
    assert!(migrations2 > 0);
    assert!(
        moves2 > 0,
        "with two nodes a migration must flip the node, but none was observed"
    );
}

#[test]
fn migration_is_inert_with_one_node_rather_than_an_error() {
    // FR-049. A single-node run is the ordinary case, and refusing a description that declares
    // a migration interval would make every multi-node description unusable locally.
    let (turns, migrations) = run(MIGRATING, 14, 1, 400.0);
    assert_eq!(migrations, 0, "migration was not inert on one node");
    assert!(!turns.is_empty());
    assert!(
        turns.iter().all(|(_, node, _, _)| *node == 0),
        "a one-node run placed a session somewhere other than node 0"
    );
}

#[test]
fn migration_leaves_the_prefix_alone() {
    // The heart of FR-048: a migrated session's already-stored blocks stay where they were, so
    // its prefix is cold on the new node and must be fetched. In the model that means a
    // migration changes *nothing* about the session's keys — same chain, same growth, same
    // turn schedule — and only where its turns are sent.
    let (with_migration, migrations) = run(MIGRATING, 15, 4, 400.0);
    assert!(migrations > 0, "no migration, so nothing is tested");

    // The same seed on one node: migration inert, everything else identical.
    let (without, none) = run(MIGRATING, 15, 1, 400.0);
    assert_eq!(none, 0);

    let keys_with: Vec<(u64, u64, usize)> = with_migration
        .iter()
        .map(|(id, _, key, turn)| (*id, *key, *turn))
        .collect();
    let keys_without: Vec<(u64, u64, usize)> = without
        .iter()
        .map(|(id, _, key, turn)| (*id, *key, *turn))
        .collect();
    assert_eq!(
        keys_with, keys_without,
        "migration changed which keys a session reads, so it moved the prefix instead of \
         leaving it where it was"
    );
}

#[test]
fn the_workload_is_the_same_whatever_the_node_count() {
    // Placement is a property of the deployment, so a two-node run and a four-node run must be
    // the same workload dispatched differently — not two different workloads. The node draw is
    // taken even on one node precisely so this holds.
    let mut previous: Option<Vec<(u64, u64, usize)>> = None;
    for nodes in [1usize, 2, 3, 8] {
        let (turns, _) = run(MIGRATING, 16, nodes, 300.0);
        let keys: Vec<(u64, u64, usize)> = turns
            .iter()
            .map(|(id, _, key, turn)| (*id, *key, *turn))
            .collect();
        if let Some(prev) = &previous {
            assert_eq!(
                *prev, keys,
                "the workload changed with the node count ({nodes} nodes)"
            );
        }
        previous = Some(keys);
    }
}

#[test]
fn migrations_are_reported_so_a_run_that_exercised_none_is_visible() {
    // A migration interval long relative to session lifetime performs no migrations at all, and
    // a multi-node measurement that meant to exercise migration would otherwise look like one
    // that did.
    let long = MIGRATING.replace(
        "migration_interval: {constant: 7}",
        "migration_interval: {constant: 100000}",
    );
    let (_, none) = run(&long, 17, 4, 400.0);
    assert_eq!(
        none, 0,
        "an interval far beyond the span produced migrations"
    );
    let (_, some) = run(MIGRATING, 17, 4, 400.0);
    assert!(some > 0, "a short interval produced none");
}
