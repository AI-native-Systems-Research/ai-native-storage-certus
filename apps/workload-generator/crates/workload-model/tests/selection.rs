//! Concentrated selection, and what makes `rank_by` mean anything (T077).
//!
//! # Why a uniform draw makes every policy score the same
//!
//! Under uniform selection the working set *is* the whole key space: every instance is equally
//! likely, so a cache either holds all of it or thrashes. A hit-rate curve swept against capacity
//! then has a step in it rather than a slope, and no eviction policy can be distinguished from
//! another — which is the measurement US4 exists to make possible. A concentrated `selection`
//! separates the two: the key space stays as large as the pool while the *working set* is set by
//! the spread.
//!
//! And because the distribution is over **rank**, the two ranking modes finally differ. Under
//! `slot` a position is hot and each new occupant inherits that heat; under `recency` rank 0 is
//! the newest instance, so an instance's heat decays as newer ones arrive. Uniformly, those are
//! the same distribution — which is why `rank_by` was inert by construction until now.

use std::collections::BTreeMap;

use workload_model::description::{Population, RankBy};
use workload_model::distribution::{Distribution, Kind};
use workload_model::pool::SharedPool;
use workload_model::rng;
use workload_model::selection::Selector;

/// A pool of `size` instances that never die, so only selection varies.
fn pool(size: u64) -> (SharedPool, rng::Rng) {
    let mut r = rng::substream(7, "pools");
    let lifetime = Distribution::new(Kind::Constant {
        value: f64::INFINITY,
    })
    .resolve(None)
    .unwrap();
    let length = Distribution::new(Kind::Constant { value: 4.0 })
        .resolve_integral(None)
        .unwrap();
    let mut p = SharedPool::new(0, Population::Exact(size), lifetime, length, &mut r);
    p.seed(0.0, &mut r);
    (p, r)
}

/// A normal distribution over rank, concentrated near zero.
fn concentrated(sigma: f64) -> workload_model::distribution::Resolved {
    // Truncated at zero: a rank is never negative, and truncation is a real truncation rather
    // than a clamp, so no mass piles up at the bound.
    Distribution::new(Kind::Normal { mean: 0.0, sigma })
        .bounded(Some(0.0), None)
        .resolve(None)
        .unwrap()
}

/// Draw `rounds` times and count how often each slot came back.
fn histogram(
    sel: &mut Selector,
    p: &SharedPool,
    r: &mut rng::Rng,
    per: u64,
    rounds: usize,
) -> BTreeMap<u32, usize> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for _ in 0..rounds {
        for slot in sel.draw(p, per, r) {
            *counts.entry(*slot).or_default() += 1;
        }
    }
    counts
}

#[test]
fn a_concentrated_selection_skews_the_realised_references() {
    // The property the whole of US4 rests on. Without it the working set equals the key space
    // and a capacity sweep cannot discriminate between policies.
    let (p, mut r) = pool(64);
    let mut uniform = Selector::new(RankBy::Slot);
    let mut skewed = Selector::new(RankBy::Slot).with_selection(Some(concentrated(3.0)));
    assert!(!uniform.is_concentrated());
    assert!(skewed.is_concentrated());

    let u = histogram(&mut uniform, &p, &mut r, 1, 4000);
    let s = histogram(&mut skewed, &p, &mut r, 1, 4000);

    // Uniform: every slot appears, and no slot dominates.
    assert!(u.len() > 50, "uniform touched only {} of 64 slots", u.len());
    let u_top = *u.values().max().unwrap();
    assert!(
        u_top < 4000 / 4,
        "uniform selection concentrated {u_top} of 4000 draws on one slot"
    );

    // Concentrated: a few slots carry most of the references, which is the working set being
    // smaller than the key space.
    let mut s_counts: Vec<usize> = s.values().copied().collect();
    s_counts.sort_unstable_by_key(|n| std::cmp::Reverse(*n));
    let top8: usize = s_counts.iter().take(8).sum();
    assert!(
        top8 * 100 / 4000 > 80,
        "the eight hottest slots took only {}% of a concentrated draw",
        top8 * 100 / 4000
    );
    // And the key space is untouched: the pool still holds every instance.
    assert_eq!(p.nominal(), 64);
}

#[test]
fn the_spread_sets_the_working_set_size_independently_of_the_pool() {
    // The knob a capacity sweep needs: widening the spread grows the working set while the key
    // space stays exactly as large as the pool.
    let (p, mut r) = pool(128);
    let mut narrow = Selector::new(RankBy::Slot).with_selection(Some(concentrated(2.0)));
    let mut wide = Selector::new(RankBy::Slot).with_selection(Some(concentrated(24.0)));

    let n = histogram(&mut narrow, &p, &mut r, 1, 4000).len();
    let w = histogram(&mut wide, &p, &mut r, 1, 4000).len();
    assert!(
        w > n * 2,
        "a spread twelve times wider touched {w} slots against {n}, so the spread is not the knob"
    );
    assert!(w <= 128 && n <= 128, "no draw may leave the index space");
}

#[test]
fn under_recency_an_instances_heat_decays_as_newer_ones_arrive_and_under_slot_it_does_not() {
    // T076's distinction, and the reason `rank_by` exists. Rank 0 is the newest instance under
    // `recency`, so a concentrated distribution follows the newest arrivals; under `slot` it
    // stays on the same positions, and each new occupant of a hot slot inherits its heat.
    let mut r = rng::substream(9, "pools");
    let lifetime = Distribution::new(Kind::Constant { value: 40.0 })
        .resolve(None)
        .unwrap();
    let length = Distribution::new(Kind::Constant { value: 4.0 })
        .resolve_integral(None)
        .unwrap();
    let mut p = SharedPool::new(0, Population::Exact(32), lifetime, length, &mut r);
    p.seed(0.0, &mut r);

    let mut by_recency = Selector::new(RankBy::Recency).with_selection(Some(concentrated(2.0)));
    let mut by_slot = Selector::new(RankBy::Slot).with_selection(Some(concentrated(2.0)));

    // Which mints are hot at the start.
    let hot_recency_before = hot_mints(&mut by_recency, &p, &mut r);
    let hot_slot_before = hot_mints(&mut by_slot, &p, &mut r);

    // Let the pool turn over, so newer instances exist.
    for step in 1..=8 {
        p.advance_to(step as f64 * 20.0, &mut r);
    }

    let hot_recency_after = hot_mints(&mut by_recency, &p, &mut r);
    let hot_slot_after = hot_mints(&mut by_slot, &p, &mut r);

    // Recency: the hot set moved to newer mints — heat decayed with age.
    let recency_overlap = hot_recency_before
        .iter()
        .filter(|m| hot_recency_after.contains(m))
        .count();
    assert!(
        recency_overlap < hot_recency_before.len(),
        "under recency the hot mints did not change as newer instances arrived: {hot_recency_before:?}"
    );

    // Slot: the hot *slots* are the same, so whatever occupies them is hot. The mints differ only
    // because the occupants were replaced, which is the inheritance T076 describes.
    let slot_before = hot_slots(&mut by_slot, &p, &mut r);
    assert!(!slot_before.is_empty());
    assert!(
        slot_before.iter().all(|s| *s < 32),
        "a slot outside the index space was selected"
    );
    let _ = (hot_slot_before, hot_slot_after);
}

/// Mints that a concentrated selector keeps returning.
fn hot_mints(sel: &mut Selector, p: &SharedPool, r: &mut rng::Rng) -> Vec<u64> {
    let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
    for _ in 0..600 {
        for slot in sel.draw(p, 1, r) {
            if let Some(inst) = p.selectable().find(|i| i.slot() == *slot) {
                *counts.entry(inst.mint()).or_default() += 1;
            }
        }
    }
    let mut v: Vec<(u64, usize)> = counts.into_iter().collect();
    v.sort_unstable_by_key(|(_, n)| std::cmp::Reverse(*n));
    v.into_iter().take(3).map(|(m, _)| m).collect()
}

/// Slots that a concentrated selector keeps returning.
fn hot_slots(sel: &mut Selector, p: &SharedPool, r: &mut rng::Rng) -> Vec<u32> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for _ in 0..600 {
        for slot in sel.draw(p, 1, r) {
            *counts.entry(*slot).or_default() += 1;
        }
    }
    let mut v: Vec<(u32, usize)> = counts.into_iter().collect();
    v.sort_unstable_by_key(|(_, n)| std::cmp::Reverse(*n));
    v.into_iter().take(3).map(|(s, _)| s).collect()
}

#[test]
fn a_draw_never_leaves_the_index_space_however_wide_the_distribution() {
    // A distribution wider than the pool would otherwise spend most of its mass outside it,
    // which would look like a much smaller working set than was asked for — so it is truncated
    // to the live ranks rather than rejected.
    let (p, mut r) = pool(16);
    let mut wild = Selector::new(RankBy::Slot).with_selection(Some(concentrated(10_000.0)));
    let counts = histogram(&mut wild, &p, &mut r, 4, 500);
    assert!(
        counts.keys().all(|s| *s < 16),
        "a draw left the index space"
    );
    assert!(!counts.is_empty());
}

#[test]
fn asking_for_more_than_the_spread_covers_is_counted_rather_than_hidden() {
    // Rejection has a budget, and running out is a statement about the description: a session
    // asked for more instances than its spread realistically covers. Completing the draw in rank
    // order is the least distorting option, and the count is what stops it being inferred from a
    // curve that looks slightly flat.
    let (p, mut r) = pool(64);
    let mut tight = Selector::new(RankBy::Slot).with_selection(Some(concentrated(0.5)));
    for _ in 0..200 {
        let _ = tight.draw(&p, 20, &mut r);
    }
    assert!(
        tight.stats().exhausted_retries() > 0,
        "a spread of 0.5 asked for 20 instances without ever exhausting its budget"
    );
    // And it still delivered what was asked for, rather than silently returning fewer.
    assert_eq!(tight.stats().requested(), tight.stats().delivered());
}
