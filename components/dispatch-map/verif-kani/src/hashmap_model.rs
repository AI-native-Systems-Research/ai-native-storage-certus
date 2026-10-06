// A STUB MODEL of the six `std::collections::HashMap` methods dispatch-map calls on its one map,
// `Inner::entries: HashMap<CacheKey, DispatchEntry>`.
//
// WHY THIS EXISTS — measured on this crate, cargo-kani 0.67.0, cadical/kissat/minisat:
//   * real HashMap<u64,u64>: one insert + one get, unwind 5, unwinding checks ON:
//       SUCCESSFUL, 120-126 s (66-69 s of it symbolic execution, not the solver; all three
//       solvers within 5 %). unwind 6: 178 s. Insert alone: 63 s.
//   * real component map (one symbolic DispatchEntry seeded + read back), unwind 5: >300 s TIMEOUT
//     (cadical >400 s). Every IDispatchMap obligation needs at least 2-4 map operations.
//   * `--no-unwinding-checks` does NOT rescue it: at --unwind 4 hashbrown's
//     `swap_nonoverlapping_chunks::<8>` loop (core/src/ptr/mod.rs) needs 5 iterations, so the cut
//     PRUNES every path through `insert` — the anti-vacuity mutant of a HashMap<u64,u64> harness
//     PASSES there (vacuous), and a `kani::cover!` placed after the insert is reported UNREACHABLE
//     while the harness still reports SUCCESSFUL. At --unwind 8 the same 2-op harness costs 316 s.
//   So the real hashbrown table is out of reach at any bound the gate's 60/300 s caps allow, and
//   the bounded-shallow lever is vacuous on it. This model is the stub lever
//   (lever_battery_kani.yaml `stub`) applied to the container, NOT to dispatch-map's own code:
//   every line of `components/dispatch-map/src/**` still executes for real.
//
// WHAT IS MODELLED, and the contract it follows (std docs, HashMap):
//   new()                       real (an empty hashbrown table; cheap, never populated)
//   insert(k, v) -> Option<V>   replaces the value of an existing key and returns the old one,
//                               else adds (k, v) and returns None
//   get(&k) / get_mut(&k)       Some(&value) for a present key, else None
//   remove(&k) -> Option<V>     removes and returns the value of a present key, else None
//   contains_key(&k)            presence
//   len()                       number of distinct present keys
// Keys are compared by value (CacheKey = u64), which is exactly `Eq` on u64. Nothing here models
// iteration order or hashing, and dispatch-map never iterates its map.
//
// SOUNDNESS GUARDS (each a hard assertion inside the stub, so a misuse FAILS the harness rather
// than proving something about the wrong object):
//   * only the instantiation HashMap<u64, DispatchEntry> may reach the stub (size + type-name
//     check on K and V);
//   * only ONE map instance may use it per harness (address recorded on first use and asserted
//     thereafter) — there is exactly one `Inner` per component and harnesses build one component;
//   * capacity MAXS = 3 distinct keys; exceeding it asserts (never silently drops an insert).
//   `check_hashmap_model_*` in proofs_model.rs re-assert each method's contract on the model and
//   that the stubbed paths are REACHED and return values (the skill's "a stub can blind the code"
//   diagnostic), and `check_real_hashmap_agrees_*` run small scenarios against the REAL table.
//
// FIDELITY: `representative` (a trusted, disclosed stub; the component logic is real).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

use crate::entry::DispatchEntry;

pub const MAXS: usize = 3;

static mut SLOTS: [Option<(u64, DispatchEntry)>; MAXS] = [None, None, None];
static mut OWNER: usize = 0;

fn guard<K, V, S>(m: *const HashMap<K, V, S>) {
    assert!(std::mem::size_of::<K>() == std::mem::size_of::<u64>());
    assert!(std::mem::size_of::<V>() == std::mem::size_of::<DispatchEntry>());
    // type-name LENGTHS rather than contents: a string compare is a memcmp loop that would set
    // the harness unwind bound; sizes + name lengths already single out the one instantiation.
    assert!(std::any::type_name::<K>().len() == std::any::type_name::<u64>().len());
    assert!(std::any::type_name::<V>().len() == std::any::type_name::<DispatchEntry>().len());
    let addr = m as *const u8 as usize;
    unsafe {
        if OWNER == 0 {
            OWNER = addr;
        }
        assert!(OWNER == addr, "HashMap model used by a second map instance");
    }
}

fn key_of<Q: ?Sized>(k: &Q) -> u64 {
    assert!(std::mem::size_of_val(k) == std::mem::size_of::<u64>());
    assert!(std::any::type_name::<Q>().len() == std::any::type_name::<u64>().len());
    unsafe { *(k as *const Q as *const u8 as *const u64) }
}

/// Index of the slot holding `key`, if any. Unrolled over MAXS = 3 (no loop).
fn find(key: u64) -> Option<usize> {
    unsafe {
        let s = &*std::ptr::addr_of!(SLOTS);
        if let Some((k, _)) = &s[0] {
            if *k == key {
                return Some(0);
            }
        }
        if let Some((k, _)) = &s[1] {
            if *k == key {
                return Some(1);
            }
        }
        if let Some((k, _)) = &s[2] {
            if *k == key {
                return Some(2);
            }
        }
        None
    }
}

fn free_slot() -> usize {
    unsafe {
        let s = &*std::ptr::addr_of!(SLOTS);
        if s[0].is_none() {
            return 0;
        }
        if s[1].is_none() {
            return 1;
        }
        if s[2].is_none() {
            return 2;
        }
    }
    panic!("HashMap model full: raise MAXS");
}

pub fn get<'a, K, V, S, Q: ?Sized>(m: &'a HashMap<K, V, S>, k: &Q) -> Option<&'a V>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    guard(m as *const _);
    let key = key_of(k);
    match find(key) {
        Some(i) => unsafe {
            let s = &*std::ptr::addr_of!(SLOTS);
            let e: &DispatchEntry = &s[i].as_ref().unwrap().1;
            Some(&*(e as *const DispatchEntry as *const V))
        },
        None => None,
    }
}

pub fn get_mut<'a, K, V, S, Q: ?Sized>(m: &'a mut HashMap<K, V, S>, k: &Q) -> Option<&'a mut V>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    guard(m as *const _);
    let key = key_of(k);
    match find(key) {
        Some(i) => unsafe {
            let s = &mut *std::ptr::addr_of_mut!(SLOTS);
            let e: &mut DispatchEntry = &mut s[i].as_mut().unwrap().1;
            Some(&mut *(e as *mut DispatchEntry as *mut V))
        },
        None => None,
    }
}

pub fn contains_key<K, V, S, Q: ?Sized>(m: &HashMap<K, V, S>, k: &Q) -> bool
where
    K: Eq + Hash + Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    guard(m as *const _);
    find(key_of(k)).is_some()
}

pub fn insert<K, V, S>(m: &mut HashMap<K, V, S>, k: K, v: V) -> Option<V>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    guard(m as *const _);
    let key: u64 = unsafe { std::mem::transmute_copy::<K, u64>(&k) };
    let val: DispatchEntry = unsafe { std::mem::transmute_copy::<V, DispatchEntry>(&v) };
    std::mem::forget(v);
    std::mem::forget(k);
    unsafe {
        let s = &mut *std::ptr::addr_of_mut!(SLOTS);
        match find(key) {
            Some(i) => {
                let old = std::mem::replace(&mut s[i].as_mut().unwrap().1, val);
                let r: V = std::mem::transmute_copy::<DispatchEntry, V>(&old);
                std::mem::forget(old);
                Some(r)
            }
            None => {
                let i = free_slot();
                s[i] = Some((key, val));
                None
            }
        }
    }
}

pub fn remove<K, V, S, Q: ?Sized>(m: &mut HashMap<K, V, S>, k: &Q) -> Option<V>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    guard(m as *const _);
    let key = key_of(k);
    match find(key) {
        Some(i) => unsafe {
            let s = &mut *std::ptr::addr_of_mut!(SLOTS);
            let (_, old) = s[i].take().unwrap();
            let r: V = std::mem::transmute_copy::<DispatchEntry, V>(&old);
            std::mem::forget(old);
            Some(r)
        },
        None => None,
    }
}

pub fn len<K, V, S>(m: &HashMap<K, V, S>) -> usize {
    guard(m as *const _);
    unsafe {
        let s = &*std::ptr::addr_of!(SLOTS);
        (s[0].is_some() as usize)
            + (s[1].is_some() as usize)
            + (s[2].is_some() as usize)
    }
}
