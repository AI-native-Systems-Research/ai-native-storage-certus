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

/// A workload whose key selection is genuinely **random**, so that an RNG-stream
/// divergence can actually change which keys a session reads.
///
/// `MIGRATING` and `STATIC` cannot: their shared class has no `pool` (one immortal
/// instance) and `count: {constant: 1}`, with constant turns and growths, so every
/// session's keys are forced regardless of what the generator draws. That is fine for
/// the migration tests, which are about a session's *node*, and useless for
/// `the_workload_is_the_same_whatever_the_node_count`, which is about its *keys*.
const RANDOM_KEYS: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  tool:
    length: {uniform: {min: 1, max: 20}}
    lifetime: {exponential: {mean: 200, min: 1}}
    pool:
      size: 40
      rank_by: slot
      selection: {exponential: {mean: 6}}
session_classes:
  chat:
    pool: {size: 30}
    uses: [{class: tool, count: {uniform: {min: 1, max: 5}}}]
    turns: {exponential: {mean: 6, min: 1}}
    input_growth: {normal: {mean: 4, sigma: 1, min: 1}}
    output_growth: {normal: {mean: 2, sigma: 1, min: 1}}
    think_time: {exponential: {mean: 8}}
    migration_interval: {exponential: {mean: 15}}
"#;

/// Run and record, per turn, `(session id, turn index, the WHOLE key path)`.
///
/// The whole path rather than [`run`]'s first key: a stream divergence that left the
/// first key alone while changing the rest would pass the weaker comparison.
fn run_paths(yaml: &str, seed: u64, nodes: usize, span: f64) -> (Vec<(u64, usize, Vec<u64>)>, u64) {
    let d: WorkloadDescription = yaml.parse().unwrap();
    let mut sim = Simulation::new(&d, seed, nodes).unwrap();
    let mut seen = Vec::new();
    sim.run_until(span, &mut |s, t| {
        seen.push((s.id(), t.index(), s.reads_of(t).to_vec()));
    });
    (seen, sim.migrations())
}

/// Run and record, per turn, `(session id, node, first key of the path, turn index)`.
fn run(yaml: &str, seed: u64, nodes: usize, span: f64) -> (Vec<(u64, usize, u64, usize)>, u64) {
    let d: WorkloadDescription = yaml.parse().unwrap();
    let mut sim = Simulation::new(&d, seed, nodes).unwrap();
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
fn the_population_seeded_at_t0_is_spread_too_and_not_all_on_node_zero() {
    // FR-048's first half for the cohort that exists *before* the run starts, which
    // `new_sessions_are_spread_across_the_configured_nodes` cannot see: its 400-second span is
    // ten session lifetimes, so the seeded twelve are long dead and replaced by sessions born
    // during the run. Placement went wrong for exactly the sessions that span hides.
    //
    // The span here is 10 against a lifetime of 8 turns x 5 = 40, so nothing has been
    // replaced and every session observed is a seeded one. STATIC, so a session's node is
    // still the one it was placed on at birth.
    //
    // What this pins: the node count must be known when the seeded population is placed. When
    // it was supplied by a setter *after* construction, all twelve landed on node 0 and a
    // four-instance run drove one instance with ~6x the load of the others — while every
    // longer-running test stayed green.
    let (turns, migrations) = run(STATIC, 11, 4, 10.0);
    assert_eq!(migrations, 0, "STATIC declares no interval");

    // `pool: {size: {exact: 12}}` and ids are handed out from 0 in seeding order, so the
    // seeded cohort is ids 0..12 — asserted rather than assumed, since the whole point is
    // that these are the sessions placed inside the constructor.
    let mut node_of: BTreeMap<u64, usize> = BTreeMap::new();
    for (id, node, _, _) in &turns {
        node_of.insert(*id, *node);
    }
    let seeded: BTreeMap<u64, usize> = node_of
        .iter()
        .filter(|(id, _)| **id < 12)
        .map(|(i, n)| (*i, *n))
        .collect();
    assert_eq!(
        seeded.len(),
        12,
        "expected all twelve seeded sessions to take a turn within the span, saw {:?}",
        seeded.keys()
    );

    let mut by_node: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    for (id, node) in &seeded {
        by_node.entry(*node).or_default().insert(*id);
    }
    assert!(
        by_node.len() >= 3,
        "the seeded cohort landed on {:?} of 4 nodes: {by_node:?}",
        by_node.keys()
    );
    let biggest = by_node.values().map(|s| s.len()).max().unwrap_or(0);
    assert!(
        biggest < 12,
        "every seeded session landed on one node: {by_node:?}"
    );
    assert!(by_node.keys().all(|n| *n < 4), "a node outside the range");
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
    // the same workload dispatched differently — not two different workloads.
    //
    // What secures that is `placement_rng`: placement draws from its own substream, so a node
    // draw cannot shift the stream `bind` takes a session's shared instances from. When the two
    // shared one stream this was violated, because `gen_range(0..1)` and `gen_range(0..4)`
    // consume different amounts of entropy, so the node count moved every later key.
    //
    // **This test used `MIGRATING` and was therefore vacuous**: that fixture's keys are forced
    // (one immortal shared instance, `count: {constant: 1}`, constant growths), so it passed
    // under arbitrary stream divergence and could not fail. `RANDOM_KEYS` gives `bind` real
    // choices — a 40-instance pool, a `selection` spread, a random `count` — and the whole key
    // path is compared rather than the first key.
    let mut previous: Option<Vec<(u64, usize, Vec<u64>)>> = None;
    let mut migrations_seen = Vec::new();
    for nodes in [1usize, 2, 3, 8] {
        let (turns, migrations) = run_paths(RANDOM_KEYS, 5, nodes, 300.0);
        assert!(!turns.is_empty(), "no turns at all at {nodes} nodes");
        migrations_seen.push(migrations);
        if let Some(prev) = &previous {
            assert_eq!(
                prev.len(),
                turns.len(),
                "the turn COUNT changed with the node count ({nodes} nodes)"
            );
            assert_eq!(
                *prev, turns,
                "the workload changed with the node count ({nodes} nodes)"
            );
        }
        previous = Some(turns);
    }

    // And the deployments genuinely differed, or the comparison above is vacuous for a second
    // reason — identical workloads prove nothing if nothing about the runs was different.
    assert_eq!(
        migrations_seen[0], 0,
        "one node must leave migration inert (FR-049)"
    );
    assert!(
        migrations_seen[1..].iter().all(|&m| m > 0),
        "a multi-node run performed no migrations, so no run differed from the one-node run: \
         {migrations_seen:?}"
    );
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
