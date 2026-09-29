//! T028 — sessions with overlapping shared sets produce **nested** chains, not
//! divergent ones (FR-027, FR-028).
//!
//! `session.rs`'s unit tests already check that a bound list is sorted and that a
//! common leading run of instances gives a common leading run of keys. What this
//! file adds is the part those cannot state:
//!
//! - the **named case** from the task — `{0,1}` against `{0,1,4}` — as a concrete
//!   pair rather than a property over random pairs;
//! - **strict nesting** of a three-session family, which is the shape "nested, not
//!   divergent" actually describes;
//! - **position, not membership, is what matters**: `{0,1,4}` and `{0,2,4}` share
//!   only object 0's blocks even though they also hold object 4 in common;
//! - the same across **two shared classes**, where agreeing on a later class buys
//!   nothing if an earlier one differs;
//! - and all of it **with turns taken**, so growth is in the chains and must be
//!   seen to diverge immediately after the shared run.

use std::collections::BTreeMap;

use workload_model::description::{Population, RankBy};
use workload_model::distribution::{Distribution, Kind};
use workload_model::keys::CacheKey;
use workload_model::pool::SharedPool;
use workload_model::rng;
use workload_model::selection::Selector;
use workload_model::session::{bind, take_turn, Growth, SessionIds, SessionPool, Uses};

/// Blocks per shared instance, so an expected key count is `instances * BLOCKS`.
const BLOCKS: usize = 4;

fn integral(v: f64) -> Distribution {
    Distribution::new(Kind::Constant { value: v })
}

fn resolved_int(v: f64) -> workload_model::distribution::Resolved {
    integral(v).resolve_integral(None).unwrap()
}

/// A shared class of `n` immortal instances, so a slot names one instance for the
/// whole test and the ordering is the only thing in play.
fn shared_class(class_id: u64, n: u64, r: &mut rng::Rng) -> (SharedPool, Selector) {
    let life = Distribution::new(Kind::Constant {
        value: f64::INFINITY,
    })
    .resolve(None)
    .unwrap();
    let mut p = SharedPool::new(
        class_id,
        Population::Exact(n),
        life,
        resolved_int(BLOCKS as f64),
        r,
    );
    p.seed(0.0, r);
    (p, Selector::new(RankBy::Slot))
}

/// One session's outcome: the slots it drew per class, in `uses` order, and its
/// whole key chain after every turn has been taken.
struct Outcome {
    slots: Vec<Vec<u32>>,
    chain: Vec<CacheKey>,
    shared_len: usize,
}

/// Run `count` sessions through bind and every turn, and return their outcomes.
///
/// One `SessionPool` for all of them, because constructing one builds a
/// residual-life grid and doing that per session would dominate the test.
fn run_sessions(
    count: u64,
    uses: &[Uses],
    pools: &mut [SharedPool],
    selectors: &mut [Selector],
    r: &mut rng::Rng,
) -> Vec<Outcome> {
    let mut ids = SessionIds::new();
    let mut sessions = SessionPool::new(
        0,
        Population::Exact(count),
        resolved_int(3.0),
        Distribution::new(Kind::Constant { value: 1.0 })
            .resolve(None)
            .unwrap(),
        r,
    );
    sessions.seed(0.0, &mut ids, r);
    let growth = Growth {
        input: resolved_int(2.0),
        output: resolved_int(1.0),
    };

    let mut out = Vec::new();
    for h in sessions.handles().collect::<Vec<_>>() {
        bind(sessions.session_mut(h), uses, pools, selectors, r);
        let shared_len = sessions.session(h).shared_len();

        // Slots per class, in `uses` order. Grouping by class rather than by
        // position, so a class that drew nothing simply contributes an empty list.
        let mut by_class: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
        for b in sessions.session(h).chosen() {
            by_class
                .entry(b.class_id())
                .or_default()
                .push(pools[b.class_id() as usize].held(b.held()).slot());
        }
        let slots: Vec<Vec<u32>> = uses
            .iter()
            .map(|u| {
                by_class
                    .get(&(u.class_index as u64))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();

        while take_turn(sessions.session_mut(h), &growth, r).is_some() {}
        let chain = sessions.session(h).prefix().to_vec();
        for b in sessions.session_mut(h).take_chosen() {
            pools[b.class_id() as usize].release(b.into_held());
        }
        out.push(Outcome {
            slots,
            chain,
            shared_len,
        });
    }
    out
}

/// How many leading keys two chains agree on.
fn common_keys(a: &[CacheKey], b: &[CacheKey]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// How many leading instances two selections agree on **in canonical order**.
///
/// Each class's slots are sorted here, independently of the order they were bound
/// in. That is the point: comparing the bound order against itself would hold
/// whatever that order was, which is precisely how an earlier version of this
/// helper let a missing canonical sort pass unnoticed.
fn common_slots(a: &[Vec<u32>], b: &[Vec<u32>]) -> usize {
    let canonical = |x: &[Vec<u32>]| -> Vec<u32> {
        x.iter()
            .flat_map(|c| {
                let mut v = c.clone();
                v.sort_unstable();
                v
            })
            .collect()
    };
    let fa = canonical(a);
    let fb = canonical(b);
    fa.iter().zip(&fb).take_while(|(x, y)| x == y).count()
}

/// Find the first outcome whose flattened slots are exactly `want`.
fn find<'a>(outcomes: &'a [Outcome], want: &[u32]) -> Option<&'a Outcome> {
    outcomes
        .iter()
        .find(|o| o.slots.iter().flatten().copied().collect::<Vec<u32>>() == want)
}

#[test]
fn the_named_case_zero_one_against_zero_one_four_nests() {
    // The example the requirement is written against: two sessions that agree on
    // their first two objects share exactly those two objects' blocks, and nothing
    // after.
    let mut r = rng::substream(51, "t028");
    let (p, s) = shared_class(0, 6, &mut r);
    let mut pools = vec![p];
    let mut sels = vec![s];

    let mut outcomes = run_sessions(
        300,
        &[Uses {
            class_index: 0,
            count: resolved_int(2.0),
        }],
        &mut pools,
        &mut sels,
        &mut r,
    );
    outcomes.extend(run_sessions(
        300,
        &[Uses {
            class_index: 0,
            count: resolved_int(3.0),
        }],
        &mut pools,
        &mut sels,
        &mut r,
    ));

    let a = find(&outcomes, &[0, 1]).expect("no session drew exactly {0,1}");
    let b = find(&outcomes, &[0, 1, 4]).expect("no session drew exactly {0,1,4}");

    // Nested: the smaller session's shared run is a prefix of the larger's chain.
    assert_eq!(a.shared_len, 2 * BLOCKS);
    assert_eq!(b.shared_len, 3 * BLOCKS);
    assert_eq!(
        &a.chain[..a.shared_len],
        &b.chain[..a.shared_len],
        "the shared run of {{0,1}} is not a prefix of {{0,1,4}}'s chain"
    );

    // And exactly that far: `{0,1}`'s own growth begins where the shared run ends,
    // so the chains part company at the first block after it.
    assert_eq!(
        common_keys(&a.chain, &b.chain),
        2 * BLOCKS,
        "chains agree on {} keys, expected exactly the two shared objects' {}",
        common_keys(&a.chain, &b.chain),
        2 * BLOCKS
    );
}

#[test]
fn an_overlapping_family_is_strictly_nested() {
    // "Nested, not divergent" in its clearest form: for sets that nest, the shared
    // runs nest too, each a strict prefix of the next.
    let mut r = rng::substream(52, "t028");
    let (p, s) = shared_class(0, 6, &mut r);
    let mut pools = vec![p];
    let mut sels = vec![s];

    let mut outcomes = Vec::new();
    for count in [1.0, 2.0, 3.0] {
        outcomes.extend(run_sessions(
            250,
            &[Uses {
                class_index: 0,
                count: resolved_int(count),
            }],
            &mut pools,
            &mut sels,
            &mut r,
        ));
    }

    let family = [
        find(&outcomes, &[0]).expect("no session drew {0}"),
        find(&outcomes, &[0, 1]).expect("no session drew {0,1}"),
        find(&outcomes, &[0, 1, 4]).expect("no session drew {0,1,4}"),
    ];
    for (i, (small, large)) in family.iter().zip(family.iter().skip(1)).enumerate() {
        assert_eq!(
            &small.chain[..small.shared_len],
            &large.chain[..small.shared_len],
            "family member {i}'s shared run is not a prefix of member {}'s",
            i + 1
        );
        assert!(
            large.shared_len > small.shared_len,
            "member {} did not extend member {i}",
            i + 1
        );
    }
}

#[test]
fn an_object_held_in_common_at_a_differing_position_yields_nothing() {
    // The sharp form of FR-027. `{0,1,4}` and `{0,2,4}` both hold object 4, but at
    // a position reached through different parents — so object 4's blocks have
    // different keys in each, and the only reuse is object 0's.
    let mut r = rng::substream(53, "t028");
    let (p, s) = shared_class(0, 6, &mut r);
    let mut pools = vec![p];
    let mut sels = vec![s];
    let outcomes = run_sessions(
        900,
        &[Uses {
            class_index: 0,
            count: resolved_int(3.0),
        }],
        &mut pools,
        &mut sels,
        &mut r,
    );

    let a = find(&outcomes, &[0, 1, 4]).expect("no session drew {0,1,4}");
    let b = find(&outcomes, &[0, 2, 4]).expect("no session drew {0,2,4}");

    assert_eq!(
        common_keys(&a.chain, &b.chain),
        BLOCKS,
        "the sets agree on one leading object, so exactly {BLOCKS} keys should \
         match; membership of object 4 must buy nothing"
    );
    // Object 4's blocks are the last BLOCKS of each shared run, and they must not
    // coincide despite naming the same object.
    let a_tail = &a.chain[a.shared_len - BLOCKS..a.shared_len];
    let b_tail = &b.chain[b.shared_len - BLOCKS..b.shared_len];
    assert!(
        a_tail.iter().all(|k| !b_tail.contains(k)),
        "object 4's blocks coincide across two sessions that reached it through \
         different prefixes"
    );
}

#[test]
fn agreeing_on_a_later_class_buys_nothing_if_an_earlier_one_differs() {
    // Canonical ordering puts classes in `uses` order, and chaining means the first
    // difference ends all reuse. So two sessions with the same instance of class 1
    // share nothing at all when their class-0 instance differs.
    let mut r = rng::substream(54, "t028");
    let (p0, s0) = shared_class(0, 2, &mut r);
    let (p1, s1) = shared_class(1, 2, &mut r);
    let mut pools = vec![p0, p1];
    let mut sels = vec![s0, s1];
    let uses = vec![
        Uses {
            class_index: 0,
            count: resolved_int(1.0),
        },
        Uses {
            class_index: 1,
            count: resolved_int(1.0),
        },
    ];
    let outcomes = run_sessions(400, &uses, &mut pools, &mut sels, &mut r);

    let pick = |c0: u32, c1: u32| {
        outcomes
            .iter()
            .find(|o| o.slots == vec![vec![c0], vec![c1]])
            .unwrap_or_else(|| panic!("no session drew class0={c0}, class1={c1}"))
    };

    // Same later class, different earlier class: no reuse whatsoever.
    let a = pick(0, 1);
    let b = pick(1, 1);
    assert_eq!(
        common_keys(&a.chain, &b.chain),
        0,
        "sessions differing in their first class still shared keys"
    );

    // Same earlier class, different later class: exactly the first class's blocks.
    let c = pick(0, 0);
    assert_eq!(
        common_keys(&a.chain, &c.chain),
        BLOCKS,
        "sessions agreeing on class 0 alone should share exactly its {BLOCKS} keys"
    );
}

#[test]
fn identical_sets_share_the_whole_shared_run_and_no_growth() {
    // The upper bound on reuse. Two sessions drawing the same set agree on every
    // shared block and then diverge immediately, because growth is salted with the
    // session id (FR-030) — which is what keeps cross-session and intra-session
    // reuse separable in the numbers.
    let mut r = rng::substream(55, "t028");
    let (p, s) = shared_class(0, 1, &mut r);
    let mut pools = vec![p];
    let mut sels = vec![s];
    let outcomes = run_sessions(
        6,
        &[Uses {
            class_index: 0,
            count: resolved_int(1.0),
        }],
        &mut pools,
        &mut sels,
        &mut r,
    );

    for (a, b) in outcomes.iter().zip(outcomes.iter().skip(1)) {
        assert_eq!(
            a.slots, b.slots,
            "a pool of one should give everyone slot 0"
        );
        assert_eq!(a.shared_len, BLOCKS);
        assert_eq!(
            common_keys(&a.chain, &b.chain),
            BLOCKS,
            "identical sets should share the shared run and stop there"
        );
        let a_growth: std::collections::BTreeSet<&CacheKey> = a.chain[BLOCKS..].iter().collect();
        let b_growth: std::collections::BTreeSet<&CacheKey> = b.chain[BLOCKS..].iter().collect();
        assert!(
            a_growth.is_disjoint(&b_growth),
            "growth blocks are shared between sessions"
        );
    }
}

#[test]
fn shared_key_agreement_always_matches_shared_instance_agreement() {
    // The general statement the specific cases above are instances of, checked over
    // every pair produced: leading key agreement is exactly BLOCKS times leading
    // instance agreement, never more and never less.
    let mut r = rng::substream(56, "t028");
    let (p, s) = shared_class(0, 5, &mut r);
    let mut pools = vec![p];
    let mut sels = vec![s];
    let mut outcomes = Vec::new();
    for count in [1.0, 2.0, 3.0, 4.0] {
        outcomes.extend(run_sessions(
            60,
            &[Uses {
                class_index: 0,
                count: resolved_int(count),
            }],
            &mut pools,
            &mut sels,
            &mut r,
        ));
    }

    let mut pairs = 0;
    for (i, a) in outcomes.iter().enumerate() {
        for b in &outcomes[i + 1..] {
            let want = common_slots(&a.slots, &b.slots) * BLOCKS;
            let got = common_keys(&a.chain, &b.chain);
            assert_eq!(
                got,
                want,
                "{:?} and {:?} agree on {} leading instances, so {want} keys, not {got}",
                a.slots,
                b.slots,
                want / BLOCKS
            );
            pairs += 1;
        }
    }
    assert!(pairs > 20_000, "only {pairs} pairs compared");
}
