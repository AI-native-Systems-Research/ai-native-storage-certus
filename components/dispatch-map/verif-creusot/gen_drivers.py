#!/usr/bin/env python3
"""Generate components/dispatch-map/verif-creusot/src/drivers.rs + verif/creusot_advisory.yaml.
Each row: id, sig, requires[], ensures[], mutant ensures[] (or None), extra modules, note, fidelity."""
import yaml, sys, re

OUT = "/home/cornel/cv-dispatch-map-20261005/components/dispatch-map/verif-creusot/src/drivers.rs"
ADV = "/home/cornel/cv-dispatch-map-20261005/components/dispatch-map/verif/creusot_advisory.yaml"

SIG = {
 "init": ("d: &mut Dm", "Out<Result<(), DmError>>", "dm_initialize(d)", ["dm_initialize", "get_pool_id"]),
 "lookup": ("d: &mut Dm, key: u64", "(Result<LookupResult, DmError>, Snapshot<Dm>)", "dm_lookup(d, key)", ["dm_lookup", "wait_for", "check_pred"]),
 "cts": ("d: &mut Dm, key: u64, offset: u64", "Result<(), DmError>", "dm_convert_to_storage(d, key, offset)", ["dm_convert_to_storage"]),
 "tr": ("d: &mut Dm, key: u64", "(Result<(), DmError>, Snapshot<Dm>)", "dm_take_read(d, key)", ["dm_take_read", "wait_for", "check_pred"]),
 "tw": ("d: &mut Dm, key: u64", "(Result<(), DmError>, Snapshot<Dm>)", "dm_take_write(d, key)", ["dm_take_write", "wait_for", "check_pred"]),
 "rr": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_release_read(d, key)", ["dm_release_read"]),
 "rw": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_release_write(d, key)", ["dm_release_write"]),
 "dg": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_downgrade_reference(d, key)", ["dm_downgrade_reference"]),
 "rm": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_remove(d, key)", ["dm_remove"]),
 "touch": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_touch(d, key)", ["dm_touch"]),
 "es": ("d: &mut Dm, key: u64", "Out<Result<u32, DmError>>", "dm_entry_size(d, key)", ["dm_entry_size"]),
 "ok": ("d: &mut Dm, n: usize", "Out<Vec<u64>>", "dm_oldest_keys(d, n)", ["dm_oldest_keys", "get_pool_id"]),
 "cr": ("d: &mut Dm, key: u64, pointer: *mut u8, size: u32", "Out<Result<(), DmError>>", "dm_create_memory_tier_entry(d, key, pointer, size)", ["dm_create_memory_tier_entry", "get_pool_id"]),
 "cmb": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_convert_memory_tier_to_block(d, key)", ["dm_convert_memory_tier_to_block"]),
 "pr": ("d: &mut Dm, key: u64, pointer: *mut u8, size: u32", "Result<(), DmError>", "dm_promote_block_to_memory_tier(d, key, pointer, size)", ["dm_promote_block_to_memory_tier"]),
 "ie": ("d: &mut Dm, key: u64", "bool", "dm_is_evictable(d, key)", ["dm_is_evictable"]),
 "te": ("d: &mut Dm, key: u64", "Result<(), DmError>", "dm_try_evict_to_block(d, key)", ["dm_try_evict_to_block"]),
 "re": ("d: &mut Dm, key: u64, offset: u64, size_blocks: u32", "Out<Result<(), DmError>>", "dm_recover_extent(d, key, offset, size_blocks)", ["dm_recover_extent", "get_pool_id"]),
 "sc": ("d: &mut Dm, key: u64, checksum: u32", "Result<(), DmError>", "dm_set_checksum(d, key, checksum)", ["dm_set_checksum"]),
 "gc": ("d: &mut Dm, key: u64", "Option<u32>", "dm_get_checksum(d, key)", ["dm_get_checksum"]),
 "all": ("d: &mut Dm, op: Op", "(St, Snapshot<Dm>)", "step!(d, op)", None),
}
ALL_MODS = ["dm_new", "get_pool_id", "wait_for", "check_pred", "dm_initialize", "dm_lookup",
            "dm_convert_to_storage", "dm_take_read", "dm_take_write", "dm_release_read",
            "dm_release_write", "dm_downgrade_reference", "dm_remove", "dm_touch", "dm_entry_size",
            "dm_oldest_keys", "dm_create_memory_tier_entry", "dm_convert_memory_tier_to_block",
            "dm_promote_block_to_memory_tier", "dm_is_evictable", "dm_try_evict_to_block",
            "dm_recover_extent", "dm_set_checksum", "dm_get_checksum"]

O, N, M = "(*d)", "(^d)", "(*result.1)"
WF = "wf(*d)"

def E(k):  # entry field at key in state
    return k

ROWS = []
def row(pid, sig, req, ens, mut=None, mods=None, note="", fid="ghost-mirror", custom=None, refute=None, wf=False,
        attrs=None, deleg=None):
    ROWS.append(dict(id=pid, sig=sig, req=req, ens=ens, mut=mut, mods=mods or [], note=note, fid=fid,
                     custom=custom, refute=refute, wf=wf, attrs=attrs or [], deleg=deleg))

# The eviction ORDER (which key is least recently used) is decided by the connected eviction-policy
# component; dispatch-map's own part is to report each use to it and to return its answer verbatim.
EP_ORDER = ("for one pool, track(pool, key) registers the key as the most recently used and touch(h) on a "
            "tracked handle makes it the most recently used, so get_eviction_candidates(pool, n) returns the "
            "pool's tracked keys least recently used (by that track/touch history) first")
ORDER_HALF = (" ORDERING HALF DELEGATED: the order in which oldest_keys lists keys is the order "
              "IEvictionPolicy::get_eviction_candidates returns (left abstract here as cands(policy state, pool, n)); "
              "that it is least-recently-used order is the eviction-policy component's obligation (delegate_to).")

OKR = "Out::Ret(Ok(()))"
EXT = "em.extents@[i]"
def init_forall(body):
    return f"match result {{ Out::Ret(Ok(())) => match (*d).em {{ Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> {body}, None => true }}, _ => true }}"

# ---------------- initialize ----------------
row("DM-INITIALIZE-RECOVERS-ALL-EXTENTS", "init", ["a_fr020(*d)"],
    [init_forall(f"has(^d, {EXT}.key) && ent(^d, {EXT}.key).location == Location::BlockDevice {{ offset: {EXT}.offset }} && lr_of(ent(^d, {EXT}.key).location) == LookupResult::BlockDevice {{ offset: {EXT}.offset }}")],
    [init_forall(f"!has(^d, {EXT}.key)")], mods=["dm_lookup"],
    note="assumes: [D-RANGE-FR-020-fd9bf8] (read at callback time = no reported key pre-exists AND reported keys are distinct). 'lookup returns that offset' = lr_of(location), which is exactly what dm_lookup returns for a found entry (dm_lookup contract). IExtentManager::for_each_extent is modelled as one visit per element of the reported Vec<Extent>, in order (trusted boundary).")
row("DM-INITIALIZE-RECOVERED-SIZE", "init", ["a_fr020(*d)"],
    [init_forall(f"ent(^d, {EXT}.key).size_blocks == {EXT}.size")],
    [init_forall(f"ent(^d, {EXT}.key).size_blocks@ == {EXT}.size@ + 1")],
    note="assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INITIALIZE-EMPTY-EXTENT-MANAGER", "init",
    ["a_fr012(*d)", "match (*d).em { Some(em) => em.extents@.len() == 0, None => false }"],
    ["result == Out::Ret(Ok(())) && same_map(^d, *d)"],
    ["result == Out::Ret(Ok(())) && !same_map(^d, *d)"],
    note="assumes: [D-RANGE-FR-012-437b1e]. The second requires is the obligation's own premise (an extent manager that reports no extents).")
row("DM-INITIALIZE-NO-EXTENT-MANAGER-OK", "init", ["a_fr012(*d)", "(*d).em == None"],
    ["result == Out::Ret(Ok(())) && same_map(^d, *d)"],
    ["result != Out::Ret(Ok(()))"],
    note="assumes: [D-RANGE-FR-012-437b1e]. em == None is the obligation's own premise.")
row("DM-INITIALIZE-RESTORED-ZERO-REFS", "init", ["a_fr020(*d)"],
    [init_forall(f"ent(^d, {EXT}.key).read_ref@ == 0 && ent(^d, {EXT}.key).write_ref@ == 0")],
    [init_forall(f"ent(^d, {EXT}.key).write_ref@ == 1")],
    note="assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INITIALIZE-TRACKS-EVICTION", "init", ["a_fr020(*d)"],
    [init_forall(f"(^d).pool_id != None && trk(^d).contains(ent(^d, {EXT}.key).eviction_handle) && Some(trk(^d).lookup(ent(^d, {EXT}.key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, {EXT}.key))")],
    [init_forall(f"!trk(^d).contains(ent(^d, {EXT}.key).eviction_handle)")],
    note="assumes: [D-RANGE-FR-020-fd9bf8]. IEvictionPolicy is a trusted consumed-interface model: track(pool,key) returns Ok(h) with h tracking (pool,key) exactly when the pool exists (InvalidPool otherwise).", fid="trusted-boundary")
row("DM-INITIALIZE-FRAME-UNREPORTED-KEYS", "init", [],
    ["forall<k: u64> !reported(*d, k) ==> (^d).entries@.get(k) == (*d).entries@.get(k)"],
    ["forall<k: u64> (^d).entries@.get(k) == (*d).entries@.get(k)"],
    note="Holds on every path, including the receptacle-unwrap panic paths (the mirror proves the post-state at the panic point).")
# DM-INITIALIZE-PRESERVES-REFERENCED-ENTRY: moved to level2_excluded by the orchestrator (2026-10-06) - no driver.

# ---------------- lookup ----------------
P = "pred_holds(*result.1, key, Pred::NoWriter)"
row("DM-LOOKUP-NOT-EXIST", "lookup", ["!has(*d, key)"],
    ["result.0 == Ok(LookupResult::NotExist) && !has(^d, key)"],
    ["result.0 == Ok(LookupResult::NotExist) && has(^d, key)"])
row("DM-LOOKUP-MEMORY-TIER-RESULT", "lookup", [],
    ["match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::MemoryTier { pointer, size, .. } => r == LookupResult::MemoryTier { pointer, size } && ent(^d, key).location == ent(*result.1, key).location, Location::BlockDevice { .. } => true }, Err(_) => true }"],
    ["match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::MemoryTier { pointer, size, .. } => r == LookupResult::NotExist, Location::BlockDevice { .. } => true }, Err(_) => true }"],
    note="'currently recorded' = the entry at the linearization point (the state in which the call holds the lock with the wait predicate satisfied; equal to the pre-state when no writer held the entry at call time); the location is unchanged at return. *mut u8 kept as a plain value.")
row("DM-LOOKUP-BLOCK-DEVICE-RESULT", "lookup", [],
    ["match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::BlockDevice { offset } => r == LookupResult::BlockDevice { offset } && ent(^d, key).location == ent(*result.1, key).location, Location::MemoryTier { .. } => true }, Err(_) => true }"],
    ["match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::BlockDevice { offset } => r == LookupResult::NotExist, Location::MemoryTier { .. } => true }, Err(_) => true }"],
    note="As DM-LOOKUP-MEMORY-TIER-RESULT.")
row("DM-LOOKUP-INCREMENTS-READ-REF", "lookup", [],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> has(^d, key) && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1, Err(_) => true }"],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@, Err(_) => true }"],
    note="'before the call' = the linearization point (see DM-LOOKUP-MEMORY-TIER-RESULT).")
row("DM-LOOKUP-WAITS-FOR-WRITER", "lookup", [],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> ent(*result.1, key).write_ref@ == 0 && ent(^d, key).write_ref@ == 0, Err(_) => true }",
     "pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d"],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> ent(*result.1, key).write_ref@ == 1, Err(_) => true }"],
    note="Proved: a location is returned and a read reference taken only in a state (the one the wait returned holding the lock) whose entry has write_ref == 0; when no writer holds it at call time the call does not wait at all. The Mutex/Condvar are a trusted boundary (condvar_wait_timeout: releases the lock, other callers run whole wf-preserving critical sections, reacquires). That the call keeps waiting (does not return) while the writer holds is a liveness/timing fact NOT proved here.", fid="trusted-boundary")
row("DM-LOOKUP-TIMEOUT", "lookup", [],
    [f"!{P} ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1)",
     f"result.0 == Err(DmError::Timeout(key)) ==> !{P}"],
    [f"!{P} ==> result.0 == Ok(LookupResult::NotExist)"],
    note="Proved: if the writer still holds the entry when the wait gives up, the result is Timeout and the map (so the read-reference count) is unchanged from that state; Timeout is returned only then. That the wait lasts the full 2000 ms period (deadline arithmetic on Instant) and terminates is timing/liveness, not proved.", fid="trusted-boundary")
row("DM-LOOKUP-NEVER-MISMATCH-SIZE", "lookup", [], ["result.0 != Ok(LookupResult::MismatchSize)"],
    ["result.0 == Ok(LookupResult::MismatchSize)"])
row("DM-LOOKUP-REFCOUNT-OVERFLOW", "lookup", [WF],
    ["has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> result.0 == Err(DmError::RefCountOverflow(key)) && ent(^d, key).read_ref == u32::MAX && same_map(^d, *d)"],
    ["has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> ent(^d, key).read_ref@ == 0"], wf=True,
    note="Premise is exactly the statement's (count at its maximum when lookup is called), plus the proved inductive invariant wf. No 'no writer' premise: under wf, DM-INV-READ-WRITE-EXCLUSIVE gives write_ref != 0 ==> read_ref == 0, so read_ref == u32::MAX forces write_ref == 0, the wait predicate holds at call time, lookup does not wait, and returns RefCountOverflow with the map unchanged. A writer-held entry with read_ref == u32::MAX (which makes the code time out first, lib.rs:121-130) is NOT a reachable state, so no refutation is shipped (it would prove at function level and CONTRADICT this wf proof).")
row("DM-LOOKUP-FRAME", "lookup", [],
    ["others_same(^d, *result.1, key)",
     "has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).write_ref == ent(*result.1, key).write_ref"],
    ["has(*result.1, key) ==> ent(^d, key).write_ref@ == ent(*result.1, key).write_ref@ + 1"],
    note="keep_meta = location, size_blocks, eviction_handle, checksum unchanged. Relative to the linearization point.")

row("DM-LOOKUP-REFRESHES-EVICTION-PRIORITY", "lookup", [WF],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> (^d).ep != None && uses(^d) == uses(*result.1).push_back(ent(*result.1, key).eviction_handle) && match (^d).pool_id { Some(p) => trk(^d).get(ent(*result.1, key).eviction_handle) == Some((p, key)), None => false }, Err(_) => true }"],
    ["match result.0 { Ok(_) => has(*result.1, key) ==> uses(^d) == uses(*result.1), Err(_) => true }"], wf=True, fid="trusted-boundary",
    deleg=dict(component="eviction-policy", obligation="touch(h) on a handle tracked in pool p makes it the most recently used entry of p, so get_eviction_candidates(p, n) lists its key after every other key tracked in p (until another track/touch)"),
    note="Dispatch-map's half, proved: a successful lookup of an existing entry (relative to the linearization point) reports exactly one use - that entry's eviction handle, tracking (map pool, key) - to the connected policy via touch (lib.rs:144, :154-157); the policy's touch result is discarded. Needs wf only to know a policy is connected whenever an entry exists. In the real code the policy call happens after the map lock is dropped (lib.rs:153); the read reference just taken keeps the entry from being removed in between (remove refuses ActiveReferences)." + ORDER_HALF)

# ---------------- convert_to_storage ----------------
row("DM-CONVERT-TO-STORAGE-SETS-SSD-OFFSET", "cts", [],
    ["result == Ok(()) ==> has(^d, key) && ssd_of(ent(^d, key).location) == Some(offset)"],
    ["result == Ok(()) ==> ssd_of(ent(^d, key).location) == None"])
row("DM-CONVERT-TO-STORAGE-STAYS-MEMORY-TIER", "cts", [],
    ["has(*d, key) && is_mt(ent(*d, key).location) ==> has(^d, key) && is_mt(ent(^d, key).location) && lr_of(ent(^d, key).location) == lr_of(ent(*d, key).location)"],
    ["has(*d, key) && is_mt(ent(*d, key).location) ==> !is_mt(ent(^d, key).location)"], mods=["dm_lookup"],
    note="'lookup still returns the same pointer and size' = lr_of(location) unchanged, which is what dm_lookup returns.")
row("DM-CONVERT-TO-STORAGE-RELEASES-READ-REF", "cts", [],
    ["result == Ok(()) ==> ent(^d, key).read_ref@ == (if ent(*d, key).read_ref@ > 0 { ent(*d, key).read_ref@ - 1 } else { 0 })"],
    ["result == Ok(()) ==> ent(^d, key).read_ref == ent(*d, key).read_ref"])
row("DM-CONVERT-TO-STORAGE-FRAME", "cts", [],
    ["others_same(^d, *d, key)",
     "has(*d, key) ==> has(^d, key) && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle"],
    ["has(*d, key) ==> ent(^d, key).write_ref@ == ent(*d, key).write_ref@ + 1"])
row("DM-CONVERT-TO-STORAGE-NOT-FOUND", "cts", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"],
    ["result == Ok(())"])
row("DM-CONVERT-TO-STORAGE-BLOCK-DEVICE", "cts", ["has(*d, key) && !is_mt(ent(*d, key).location)"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"],
    ["result == Ok(())"])

# ---------------- take_read ----------------
PR_ = "pred_holds(*result.1, key, Pred::NoWriter)"
row("DM-TAKE-READ-INCREMENTS", "tr", [],
    ["result.0 == Ok(()) ==> has(^d, key) && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1"],
    ["result.0 == Ok(()) ==> ent(^d, key).read_ref == ent(*result.1, key).read_ref"],
    note="'before the call' = the linearization point (state at which the wait returned with the lock held; equal to the pre-state when no writer held the entry).")
row("DM-TAKE-READ-WAITS-FOR-WRITER", "tr", [],
    ["result.0 == Ok(()) ==> has(*result.1, key) && ent(*result.1, key).write_ref@ == 0 && ent(^d, key).write_ref@ == 0",
     "pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d"],
    ["result.0 == Ok(()) ==> ent(*result.1, key).write_ref@ == 1"],
    note="As DM-LOOKUP-WAITS-FOR-WRITER: the waiting itself (liveness/timing) is not proved; Mutex/Condvar trusted.", fid="trusted-boundary")
row("DM-TAKE-READ-TIMEOUT", "tr", [],
    [f"!{PR_} ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1)",
     f"result.0 == Err(DmError::Timeout(key)) ==> !{PR_}"],
    [f"!{PR_} ==> result.0 == Ok(())"],
    note="As DM-LOOKUP-TIMEOUT (the full-period timing is not proved).", fid="trusted-boundary")
row("DM-TAKE-READ-NOT-FOUND", "tr", ["!has(*d, key)"],
    ["result.0 == Err(DmError::KeyNotFound(key)) && !has(^d, key)"],
    ["result.0 == Ok(())"])
row("DM-TAKE-READ-REFCOUNT-OVERFLOW", "tr", [WF],
    ["has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> result.0 == Err(DmError::RefCountOverflow(key)) && ent(^d, key).read_ref == u32::MAX && same_map(^d, *d)"],
    ["has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> ent(^d, key).read_ref@ == 0"], wf=True,
    note="As DM-LOOKUP-REFCOUNT-OVERFLOW: premise = the statement's (count at its maximum at call time) + proved invariant wf; the writer-held-and-saturated state that would make take_read time out first (lib.rs:203-209) is unreachable under DM-INV-READ-WRITE-EXCLUSIVE, so no refutation is shipped.")
row("DM-TAKE-READ-FRAME", "tr", [],
    ["others_same(^d, *result.1, key) && (^d).ep == (*result.1).ep",
     "has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).write_ref == ent(*result.1, key).write_ref"],
    ["has(*result.1, key) ==> ent(^d, key).eviction_handle != ent(*result.1, key).eviction_handle"],
    note="'does not change the eviction ranking': take_read makes no IEvictionPolicy call at all, so the eviction-policy state (incl. its ranking) is identical ((^d).ep == linearization .ep).")

# ---------------- take_write ----------------
PW = "pred_holds(*result.1, key, Pred::NoRefs)"
row("DM-TAKE-WRITE-SETS-WRITE-REF", "tw", [],
    ["result.0 == Ok(()) ==> has(^d, key) && ent(^d, key).write_ref@ == 1"],
    ["result.0 == Ok(()) ==> ent(^d, key).write_ref@ == 2"])
row("DM-TAKE-WRITE-REQUIRES-NO-REFS", "tw", [],
    ["result.0 == Ok(()) ==> has(*result.1, key) && ent(*result.1, key).read_ref@ == 0 && ent(*result.1, key).write_ref@ == 0",
     "pred_holds(*d, key, Pred::NoRefs) ==> *result.1 == *d"],
    ["result.0 == Ok(()) ==> ent(*result.1, key).write_ref@ == 1"],
    note="The waiting itself (liveness/timing) is not proved; Mutex/Condvar trusted.", fid="trusted-boundary")
row("DM-TAKE-WRITE-TIMEOUT", "tw", [],
    [f"!{PW} ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1)",
     f"result.0 == Err(DmError::Timeout(key)) ==> !{PW}"],
    [f"!{PW} ==> result.0 == Ok(())"],
    note="As DM-LOOKUP-TIMEOUT.", fid="trusted-boundary")
row("DM-TAKE-WRITE-NOT-FOUND", "tw", ["!has(*d, key)"],
    ["result.0 == Err(DmError::KeyNotFound(key)) && !has(^d, key)"],
    ["result.0 == Ok(())"])
row("DM-TAKE-WRITE-FRAME", "tw", [],
    ["others_same(^d, *result.1, key)",
     "has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).read_ref == ent(*result.1, key).read_ref"],
    ["has(*result.1, key) ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1"])

# ---------------- release_read / release_write ----------------
row("DM-RELEASE-READ-DECREMENTS", "rr", [],
    ["result == Ok(()) ==> has(*d, key) && has(^d, key) && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ - 1"],
    ["result == Ok(()) ==> ent(^d, key).read_ref == ent(*d, key).read_ref"])
row("DM-RELEASE-READ-UNDERFLOW", "rr", ["has(*d, key) && ent(*d, key).read_ref@ == 0"],
    ["result == Err(DmError::RefCountUnderflow(key)) && ent(^d, key).read_ref@ == 0"],
    ["result == Ok(())"])
row("DM-RELEASE-READ-NOT-FOUND", "rr", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
row("DM-RELEASE-READ-FRAME", "rr", [],
    ["others_same(^d, *d, key)", "has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key)) && ent(^d, key).write_ref == ent(*d, key).write_ref"],
    ["has(*d, key) ==> ent(^d, key).write_ref@ == ent(*d, key).write_ref@ + 1"])
row("DM-RELEASE-WRITE-CLEARS", "rw", [], ["result == Ok(()) ==> has(^d, key) && ent(^d, key).write_ref@ == 0"],
    ["result == Ok(()) ==> ent(^d, key).write_ref@ == 1"])
row("DM-RELEASE-WRITE-UNDERFLOW", "rw", ["has(*d, key) && ent(*d, key).write_ref@ == 0"],
    ["result == Err(DmError::RefCountUnderflow(key)) && ent(^d, key).write_ref@ == 0"], ["result == Ok(())"])
row("DM-RELEASE-WRITE-NOT-FOUND", "rw", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
row("DM-RELEASE-WRITE-FRAME", "rw", [],
    ["others_same(^d, *d, key)", "has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key)) && ent(^d, key).read_ref == ent(*d, key).read_ref"],
    ["has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1"])

# ---------------- downgrade ----------------
row("DM-DOWNGRADE-REFERENCE-SWAPS", "dg", [],
    ["result == Ok(()) ==> has(*d, key) && has(^d, key) && ent(^d, key).write_ref@ == 0 && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1"],
    ["result == Ok(()) ==> ent(^d, key).write_ref == ent(*d, key).write_ref"],
    note="Atomicity ('no moment in between'): the swap is one critical section under the map Mutex (trusted boundary), so no other caller can observe or act on the intermediate state.", fid="trusted-boundary")
row("DM-DOWNGRADE-REFERENCE-NO-WRITE-REF", "dg", ["has(*d, key) && ent(*d, key).write_ref@ == 0"],
    ["result == Err(DmError::NoWriteReference(key)) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref"],
    ["result == Ok(())"])
row("DM-DOWNGRADE-REFERENCE-NOT-FOUND", "dg", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
# DM-DOWNGRADE-REFERENCE-REFCOUNT-OVERFLOW: leaves level 2 as unreachable under the proved invariants (orchestrator 2026-10-06) - no driver.
row("DM-DOWNGRADE-REFERENCE-FRAME", "dg", [],
    ["others_same(^d, *d, key)", "has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key))"],
    ["has(*d, key) ==> ent(^d, key).checksum@ == ent(*d, key).checksum@ + 1"])

# ---------------- remove / touch ----------------
row("DM-REMOVE-DELETES", "rm", [], ["result == Ok(()) ==> !has(^d, key)"], ["result == Ok(()) ==> has(^d, key)"],
    mods=["dm_lookup"], note="'lookup reports that it does not exist': dm_lookup returns NotExist for an absent key (DM-LOOKUP-NOT-EXIST).")
row("DM-REMOVE-ACTIVE-REFERENCES", "rm", ["has(*d, key) && (ent(*d, key).read_ref@ > 0 || ent(*d, key).write_ref@ > 0)"],
    ["result == Err(DmError::ActiveReferences(key)) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-REMOVE-NOT-FOUND", "rm", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
row("DM-REMOVE-UNTRACKS", "rm", [WF],
    ["result == Ok(()) ==> !trk(^d).contains(ent(*d, key).eviction_handle)",
     "result == Ok(()) ==> match (^d).pool_id { Some(p) => forall<h: Handle> trk(^d).contains(h) && (trk(^d).lookup(h)).0 == p ==> (trk(^d).lookup(h)).1 != key, None => true }"],
    ["result == Ok(()) ==> trk(^d).contains(ent(*d, key).eviction_handle)"], wf=True,
    note="'never again offered as an eviction candidate': after the call no handle tracked in the map's pool maps to the key, and IEvictionPolicy::get_eviction_candidates returns only keys tracked in the pool (trusted consumed-interface model). Uses the proved maintained invariant wf (DM-INV-ENTRY-TRACKED-FOR-EVICTION etc.).", fid="trusted-boundary")
row("DM-REMOVE-FRAME", "rm", [], ["others_same(^d, *d, key)"], ["forall<j: u64> (^d).entries@.get(j) == None"])
row("DM-TOUCH-FRAME", "touch", [], ["same_map(^d, *d)"], ["has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1"])
row("DM-TOUCH-NOT-FOUND", "touch", ["!has(*d, key)"], ["result == Err(DmError::KeyNotFound(key))"], ["result == Ok(())"])
row("DM-TOUCH-NO-WAIT", "touch", [],
    ["result != Err(DmError::Timeout(key))",
     "has(*d, key) && (ent(*d, key).read_ref@ > 0 || ent(*d, key).write_ref@ > 0) ==> result == Ok(())"],
    ["has(*d, key) && ent(*d, key).write_ref@ > 0 ==> result == Err(DmError::Timeout(key))"],
    attrs=["#[check(terminates)]"], fid="trusted-boundary",
    note="Proved: touch never returns Timeout, and returns Ok(()) for a present entry whatever read/write references are held on it; the driver, dm_touch and the two callees it reaches (Entries::get, IEvictionPolicy::touch, both trusted and declared terminating) carry #[check(terminates)], so the call is machine-checked to complete with no wait or loop on its path (lib.rs:349-361 takes only the map Mutex and never touches the Condvar). The Mutex acquisition itself is the trusted boundary: that the short internal lock is always eventually granted is not proved.")
row("DM-TOUCH-REFRESHES-PRIORITY", "touch", [WF],
    ["result == Ok(()) ==> (*d).ep != None && uses(^d) == uses(*d).push_back(ent(*d, key).eviction_handle) && match (*d).pool_id { Some(p) => trk(^d).get(ent(*d, key).eviction_handle) == Some((p, key)), None => false }"],
    ["result == Ok(()) ==> uses(^d) == uses(*d)"], wf=True, fid="trusted-boundary",
    deleg=dict(component="eviction-policy", obligation="touch(h) on a handle tracked in pool p makes it the most recently used entry of p, so get_eviction_candidates(p, n) lists its key after every other key tracked in p (until another track/touch)"),
    note="Dispatch-map's half, proved: a successful touch reports exactly one use - the entry's own eviction handle, which tracks (map pool, key) - to the connected eviction policy (lib.rs:355-359); the policy's touch result is discarded. Needs the proved invariant wf only to know a policy is connected whenever an entry exists (wf_pool + wf_fwd); without it a touch on a map with no policy would return Ok and report nothing. The use log is a ghost field of the trusted IEvictionPolicy model (track/touch append). The real code releases the map lock BEFORE calling the policy (lib.rs:356), so a concurrent remove of the same key can interleave between the two; that interleaving is not modelled here (Loom territory)." + ORDER_HALF)

# ---------------- entry_size ----------------
row("DM-ENTRY-SIZE-BLOCK-MULTIPLE", "es", [WF],
    ["result != Out::Panic", "match result { Out::Ret(Ok(v)) => has(*d, key) && v@ == ent(*d, key).size_blocks@ * 4096 && v@ % 4096 == 0, _ => true }"],
    ["match result { Out::Ret(Ok(v)) => v@ % 4096 == 1, _ => true }"], wf=True,
    note="`size_blocks * 4096` (lib.rs:369) is unchecked u32 arithmetic, mirrored as checked_mul with the overflow arm = Panic (debug-build semantics). That it never overflows is DM-ENTRY-SIZE-NO-ARITHMETIC-OVERFLOW.")
row("DM-ENTRY-SIZE-ROUNDS-UP-MEMORY-TIER", "custom", ["a_fr003(size)"],
    ["match result.0 { Out::Ret(Ok(())) => match result.1 { Out::Ret(Ok(v)) => v@ >= size@ && v@ < size@ + 4096 && v@ % 4096 == 0, _ => false }, _ => true }"],
    ["match result.0 { Out::Ret(Ok(())) => match result.1 { Out::Ret(Ok(v)) => v@ == size@, _ => false }, _ => true }"],
    custom=("d: &mut Dm, key: u64, pointer: *mut u8, size: u32, promote: bool",
            "(Out<Result<(), DmError>>, Out<Result<u32, DmError>>)",
            "let r = if promote { match dm_promote_block_to_memory_tier(d, key, pointer, size) { Ok(()) => Out::Ret(Ok(())), Err(e) => Out::Ret(Err(e)) } } else { dm_create_memory_tier_entry(d, key, pointer, size) };\n    let s = dm_entry_size(d, key);\n    (r, s)",
            ["dm_create_memory_tier_entry", "dm_promote_block_to_memory_tier", "dm_entry_size", "get_pool_id"]),
    note="assumes: [D-RANGE-FR-003-8e9f6a]. Driver: create (promote=false) or promote (promote=true) with byte size s, then entry_size on the same key.")
row("DM-ENTRY-SIZE-FRAME", "es", [], ["^d == *d"], ["has(*d, key) ==> !has(^d, key)"])
row("DM-ENTRY-SIZE-NOT-FOUND", "es", ["!has(*d, key)"], ["result == Out::Ret(Err(DmError::KeyNotFound(key)))"], ["result != Out::Ret(Err(DmError::KeyNotFound(key)))"])
row("DM-ENTRY-SIZE-NO-ARITHMETIC-OVERFLOW", "es", [WF],
    ["result != Out::Panic", "match result { Out::Ret(Ok(v)) => v@ == ent(*d, key).size_blocks@ * 4096, _ => true }"],
    ["result == Out::Panic"], wf=True,
    note="Holds for every entry that can be in the map: wf (proved maintained invariant; established by dm_new, preserved by every method under the level-2 assumptions D-RANGE-FR-003-8e9f6a and D-RANGE-FR-023-388782) bounds size_blocks <= 1048575, so size_blocks*4096 <= u32::MAX. Without those assumptions the product overflows (the discordances record it).")

# ---------------- oldest_keys ----------------
row("DM-OLDEST-KEYS-AT-MOST-N", "ok", [], ["match result { Out::Ret(v) => v@.len() <= n@, Out::Panic => true }"],
    ["match result { Out::Ret(v) => v@.len() > n@, Out::Panic => true }"], fid="trusted-boundary",
    note="Relies on IEvictionPolicy::get_eviction_candidates returning at most n keys (its interface doc: 'up to n keys'; trusted consumed-interface model). Partial correctness w.r.t. the get_pool_id panic.")
row("DM-OLDEST-KEYS-FRAME", "ok", [], ["same_map(^d, *d) && trk(^d) == trk(*d)"], ["!same_map(^d, *d)"])
row("DM-OLDEST-KEYS-ONLY-PRESENT-KEYS", "ok", [WF],
    ["match result { Out::Ret(v) => forall<i: Int> 0 <= i && i < v@.len() ==> has(^d, v@[i]), Out::Panic => true }"],
    ["match result { Out::Ret(v) => forall<i: Int> 0 <= i && i < v@.len() ==> !has(^d, v@[i]), Out::Panic => true }"], wf=True,
    fid="trusted-boundary", note="From the proved maintained invariant wf_back (every handle tracked in the map's pool belongs to a live entry) plus get_eviction_candidates returning only keys tracked in the pool (trusted consumed-interface model).")
row("DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY", "ok", ["(*d).ep == None"],
    ["match result { Out::Ret(v) => v@.len() == 0, Out::Panic => false }"], None,
    refute=dict(req=["wf(*d)", "(*d).ep == None", "(*d).pool_id == None"], ens=["result == Out::Panic"]),
    note="REFUTED: oldest_keys (lib.rs:373) first calls get_pool_id, which on the first call (no pool yet) does `self.eviction_policy.get().unwrap()` (lib.rs:55) and panics when no eviction policy is connected; the empty-list fallback at lib.rs:376-378 is reachable only once a pool already exists. refute_ proves: no eviction policy and no pool yet ==> the call aborts. Reachable: a freshly constructed component with no eviction-policy receptacle connected.")

row("DM-OLDEST-KEYS-ORDER", "custom", [WF, "op_assume(*d, op)"],
    ["match op { Op::OldestKeys(n) => result.0 == St::Ok ==> match (^d).ep { Some(e) => match (^d).pool_id { Some(p) => result.2@ == cands(e, p, n), None => false }, None => false }, _ => true }",
     "uses_ok(op, result.0, *result.1, ^d)"],
    ["match op { Op::Touch(k) => result.0 == St::Ok ==> uses(^d) == uses(*result.1), _ => true }"],
    custom=("d: &mut Dm, op: Op", "(St, Snapshot<Dm>, Vec<u64>)",
            "match op {\n        Op::OldestKeys(n) => { let lin = snapshot!(*d); match dm_oldest_keys(d, n) { Out::Ret(v) => (St::Ok, lin, v), Out::Panic => (St::Panic, lin, Vec::new()) } }\n        _ => { let (s, lin) = step!(d, op); (s, lin, Vec::new()) }\n    }",
            None), wf=True, fid="trusted-boundary",
    deleg=dict(component="eviction-policy", obligation=EP_ORDER),
    note="Dispatch-map's half, proved over EVERY method (inductive step, in wf states): (1) oldest_keys returns exactly the eviction policy's get_eviction_candidates(map pool, n) answer, unaltered (lib.rs:373-375); (2) the uses the policy is told about are exactly: one per successful create_memory_tier_entry / recover_extent (the new handle, via track), one per extent restored by initialize (in reporting order), one per successful lookup of an existing entry and one per successful touch (the entry's handle, via touch); every other method, and every error/NotExist path, reports none (uses_ok). The use log is a ghost field of the trusted IEvictionPolicy model. assumes: [D-RANGE-FR-020-fd9bf8, D-RANGE-FR-003-8e9f6a, D-RANGE-FR-023-388782] via op_assume." + ORDER_HALF)

# ---------------- create_memory_tier_entry ----------------
row("DM-CREATE-MEMORY-TIER-ENTRY-REGISTERS-EVICTION", "cr", [],
    ["result == Out::Ret(Ok(())) ==> uses(^d) == uses(*d).push_back(ent(^d, key).eviction_handle) && (^d).pool_id != None && trk(^d).contains(ent(^d, key).eviction_handle) && Some(trk(^d).lookup(ent(^d, key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, key))"],
    ["result == Out::Ret(Ok(())) ==> uses(^d) == uses(*d)"], fid="trusted-boundary",
    deleg=dict(component="eviction-policy", obligation="track(p, key) registers key as the most recently used entry of pool p, so get_eviction_candidates(p, n) lists keys in the order they were tracked (absent later touches)"),
    note="Dispatch-map's half, proved: a successful create registers the key with the connected eviction policy in the map's pool (track, lib.rs:399), stores the returned handle in the entry (lib.rs:409), and that registration is the only use reported by the call (uses log extended by exactly the new handle)." + ORDER_HALF)
row("DM-CREATE-MEMORY-TIER-ENTRY-LOCATION", "cr", [],
    ["result == Out::Ret(Ok(())) ==> has(^d, key) && ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: None }"],
    ["result == Out::Ret(Ok(())) ==> ssd_of(ent(^d, key).location) != None"])
row("DM-CREATE-MEMORY-TIER-ENTRY-WRITE-REF", "cr", [],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 1 && ent(^d, key).read_ref@ == 0"],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 0"])
row("DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS", "cr", [WF, "has(*d, key)"],
    ["result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d)"], None, wf=True,
    refute=dict(req=["wf(*d)", "has(*d, key)", "size@ == 0"], ens=["result == Out::Ret(Err(DmError::InvalidSize))"]),
    note="REFUTED AS STATED (an error-precedence conflict between two inventory obligations, not a code defect): the size check at lib.rs:387-389 runs before the existence check at lib.rs:395-397, so for an existing key with size 0 the call returns InvalidSize, not AlreadyExists - exactly what DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE requires. The entry is left unchanged either way. For size != 0 the obligation holds (under wf, which also rules out the receptacle panic); the companion DM-PROMOTE-NOT-FOUND shows the inventory carving out 'with a non-zero size' elsewhere, which this statement lacks.")
row("DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE", "cr", ["size@ == 0"],
    ["result == Out::Ret(Err(DmError::InvalidSize)) && same_map(^d, *d)"], ["result == Out::Ret(Ok(()))"])
row("DM-CREATE-MEMORY-TIER-ENTRY-SIZE-BLOCKS", "cr", [],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).size_blocks@ == (size@ + 4095) / 4096"],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).size_blocks@ == size@ / 4096 + 1"],
    note="`size.div_ceil(4096)` (lib.rs:406) is given its std meaning by an extern_spec (creusot-std ships none).")
row("DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR", "cr", ["(*d).ep == None", "size@ != 0"],
    ["match result { Out::Ret(Err(_)) => same_map(^d, *d), _ => false }"], None,
    refute=dict(req=["wf(*d)", "(*d).ep == None", "size@ != 0"], ens=["result == Out::Panic"]),
    note="REFUTED: with a non-zero size and no eviction policy connected, create_memory_tier_entry aborts instead of returning an error: get_pool_id unwraps the receptacle at lib.rs:55 when no pool exists yet, and lib.rs:392 `self.eviction_policy.get().unwrap()` unwraps it again when one does. refute_ proves the call panics on every such input. Reachable: any component whose eviction-policy receptacle is not connected.")
row("DM-CREATE-MEMORY-TIER-ENTRY-FRAME", "cr", [],
    ["others_same(^d, *d, key)"], ["forall<j: u64> (^d).entries@.get(j) == None"],
    note="Holds on every path, including the receptacle-unwrap panic paths (a panicking call leaves the state unchanged).")

# ---------------- convert_memory_tier_to_block ----------------
row("DM-CONVERT-MEMORY-TIER-TO-BLOCK-TRANSITION", "cmb", [],
    ["result == Ok(()) ==> has(*d, key) && match ssd_of(ent(*d, key).location) { Some(o) => ent(^d, key).location == Location::BlockDevice { offset: o } && lr_of(ent(^d, key).location) == LookupResult::BlockDevice { offset: o }, None => false }"],
    ["result == Ok(()) ==> is_mt(ent(^d, key).location)"], mods=["dm_lookup"])
row("DM-CONVERT-MEMORY-TIER-TO-BLOCK-NO-SSD-OFFSET", "cmb", ["has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-MEMORY-TIER", "cmb", ["has(*d, key) && !is_mt(ent(*d, key).location)"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-FOUND", "cmb", ["!has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
row("DM-CONVERT-MEMORY-TIER-TO-BLOCK-FRAME", "cmb", [],
    ["others_same(^d, *d, key)", "has(*d, key) ==> has(^d, key) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle"],
    ["has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1"])

# ---------------- promote ----------------
row("DM-PROMOTE-TRANSITION", "pr", [],
    ["result == Ok(()) ==> has(*d, key) && match ent(*d, key).location { Location::BlockDevice { offset } => ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: Some(offset) }, Location::MemoryTier { .. } => false }"],
    ["result == Ok(()) ==> ssd_of(ent(^d, key).location) == None"])
row("DM-PROMOTE-RECOMPUTES-SIZE", "custom", ["a_fr003(size)"],
    ["result.0 == Ok(()) ==> match result.1 { Out::Ret(Ok(v)) => v@ == (size@ + 4095) / 4096 * 4096, _ => false }"],
    ["result.0 == Ok(()) ==> match result.1 { Out::Ret(Ok(v)) => v@ == size@, _ => false }"],
    custom=("d: &mut Dm, key: u64, pointer: *mut u8, size: u32", "(Result<(), DmError>, Out<Result<u32, DmError>>)",
            "let r = dm_promote_block_to_memory_tier(d, key, pointer, size);\n    let s = dm_entry_size(d, key);\n    (r, s)",
            ["dm_promote_block_to_memory_tier", "dm_entry_size"]),
    note="assumes: [D-RANGE-FR-003-8e9f6a]. Driver: promote then entry_size on the same key.")
row("DM-PROMOTE-PRESERVES-REFS", "pr", [],
    ["has(*d, key) ==> has(^d, key) && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref",
     "others_same(^d, *d, key)",
     "has(*d, key) && !is_mt(ent(*d, key).location) ==> result == Ok(()) || result == Err(DmError::InvalidSize)"],
    ["has(*d, key) && !is_mt(ent(*d, key).location) && ent(*d, key).read_ref@ > 0 ==> result == Err(DmError::InvalidState)"],
    note="Third clause ('so it succeeds on an entry pinned by readers'): on a block-device entry the outcome never depends on the reference counts - it is Ok, or InvalidSize for a zero byte size (which DM-PROMOTE-ZERO-SIZE mandates). The block-device premise is the statement's subject ('promote_block_to_memory_tier ... succeeds'): a memory-tier entry is DM-PROMOTE-ALREADY-MEMORY-TIER.")
row("DM-INV-HANDLE-STABLE", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).eviction_handle == ent(*result.1, k).eviction_handle"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).eviction_handle != ent(*result.1, k).eviction_handle"],
    note="Inductive step over every method (Op dispatch): no method changes the handle of an entry present before and after its critical section; demotion (convert_memory_tier_to_block, try_evict_to_block) and promotion included. assumes: [D-RANGE-FR-020-fd9bf8] for initialize (which otherwise overwrites).")
row("DM-PROMOTE-ALREADY-MEMORY-TIER", "pr", ["has(*d, key) && is_mt(ent(*d, key).location)"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], None,
    refute=dict(req=["wf(*d)", "has(*d, key)", "is_mt(ent(*d, key).location)", "size@ == 0"], ens=["result == Err(DmError::InvalidSize)"]),
    note="REFUTED AS STATED (an error-precedence conflict between two inventory obligations, not a code defect): lib.rs:473-475 checks size == 0 before the tier check at lib.rs:496-500, so a memory-tier entry promoted with size 0 gets InvalidSize, not InvalidState - exactly what DM-PROMOTE-ZERO-SIZE ('whether or not the key exists') requires. The entry is unchanged either way; for size != 0 the obligation holds.")
row("DM-PROMOTE-NOT-FOUND", "pr", ["size@ != 0 && !has(*d, key)"],
    ["result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d)"], ["result == Ok(())"])
row("DM-PROMOTE-ZERO-SIZE", "pr", ["size@ == 0"], ["result == Err(DmError::InvalidSize) && ^d == *d"], ["result == Ok(())"])

# ---------------- is_evictable / try_evict ----------------
row("DM-IS-EVICTABLE-EXACT-CONDITION", "ie", [],
    ["result == (has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) != None && ent(*d, key).read_ref@ == 0 && ent(*d, key).write_ref@ == 0)"],
    ["result == (has(*d, key) && is_mt(ent(*d, key).location) && ent(*d, key).read_ref@ == 0 && ent(*d, key).write_ref@ == 0)"])
row("DM-IS-EVICTABLE-NOT-FOUND", "ie", ["!has(*d, key)"], ["!result"], ["result"])
row("DM-IS-EVICTABLE-FRAME", "ie", [], ["^d == *d"], ["has(*d, key) ==> !has(^d, key)"],
    note="The call makes no IEvictionPolicy call, so the eviction ordering is untouched too (whole state equal).")
row("DM-IS-EVICTABLE-AGREES-WITH-TRY-EVICT", "custom", [],
    ["result.0 == (result.1 == Ok(()))"], ["result.0 != (result.1 == Ok(()))"],
    custom=("d: &mut Dm, key: u64", "(bool, Result<(), DmError>)",
            "let b = dm_is_evictable(d, key);\n    let r = dm_try_evict_to_block(d, key);\n    (b, r)",
            ["dm_is_evictable", "dm_try_evict_to_block"]),
    note="Driver: is_evictable then try_evict_to_block on the same state (is_evictable leaves the state unchanged).")
row("DM-TRY-EVICT-TO-BLOCK-SUCCESS", "te", [],
    ["result == Ok(()) ==> has(*d, key) && match ssd_of(ent(*d, key).location) { Some(o) => ent(^d, key).location == Location::BlockDevice { offset: o } && !is_mt(ent(^d, key).location), None => false }"],
    ["result == Ok(()) ==> is_mt(ent(^d, key).location)"],
    note="'no later lookup returns the old memory-tier pointer': the entry is a block-device entry, and lookup returns lr_of(location) = BlockDevice{offset}.")
row("DM-TRY-EVICT-TO-BLOCK-ACTIVE-REFS", "te", ["has(*d, key) && (ent(*d, key).read_ref@ != 0 || ent(*d, key).write_ref@ != 0)"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-TRY-EVICT-TO-BLOCK-NO-SSD-OFFSET", "te", ["has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-TRY-EVICT-TO-BLOCK-NOT-MEMORY-TIER", "te", ["has(*d, key) && !is_mt(ent(*d, key).location)"],
    ["result == Err(DmError::InvalidState) && same_map(^d, *d)"], ["result == Ok(())"])
row("DM-TRY-EVICT-TO-BLOCK-NOT-FOUND", "te", ["!has(*d, key)"], ["result == Err(DmError::KeyNotFound(key))"], ["result == Ok(())"])
row("DM-TRY-EVICT-TO-BLOCK-FRAME", "te", [],
    ["others_same(^d, *d, key) && trk(^d) == trk(*d)",
     "has(*d, key) ==> has(^d, key) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle"],
    ["has(*d, key) ==> !trk(^d).contains(ent(*d, key).eviction_handle)"],
    note="'leaves it tracked': try_evict_to_block makes no IEvictionPolicy call, so the tracking map is identical.")

# ---------------- recover_extent ----------------
row("DM-RECOVER-EXTENT-INSERTS", "custom", ["a_fr023(size_blocks)"],
    ["result.0 == Out::Ret(Ok(())) ==> has(^d, key) && ent(^d, key).location == Location::BlockDevice { offset } && lr_of(ent(^d, key).location) == LookupResult::BlockDevice { offset } && ent(^d, key).size_blocks == size_blocks && match result.1 { Out::Ret(Ok(v)) => v@ == size_blocks@ * 4096, _ => false }"],
    ["result.0 == Out::Ret(Ok(())) ==> match result.1 { Out::Ret(Ok(v)) => v@ == size_blocks@, _ => false }"],
    custom=("d: &mut Dm, key: u64, offset: u64, size_blocks: u32", "(Out<Result<(), DmError>>, Out<Result<u32, DmError>>)",
            "let r = dm_recover_extent(d, key, offset, size_blocks);\n    let s = dm_entry_size(d, key);\n    (r, s)",
            ["dm_recover_extent", "dm_entry_size", "get_pool_id", "dm_lookup"]),
    note="assumes: [D-RANGE-FR-023-388782]. Driver: recover_extent then entry_size on the same key; lookup returns lr_of(location).")
row("DM-RECOVER-EXTENT-REGISTERS-EVICTION", "re", [],
    ["result == Out::Ret(Ok(())) ==> (^d).pool_id != None && trk(^d).contains(ent(^d, key).eviction_handle) && Some(trk(^d).lookup(ent(^d, key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, key))"],
    ["result == Out::Ret(Ok(())) ==> !trk(^d).contains(ent(^d, key).eviction_handle)"], fid="trusted-boundary",
    note="IEvictionPolicy::track is a trusted consumed-interface model.")
row("DM-RECOVER-EXTENT-ALREADY-EXISTS", "re", [WF, "has(*d, key)"],
    ["result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d)"], ["result == Out::Ret(Ok(()))"], wf=True,
    note="wf (proved maintained invariant) gives: a non-empty map implies a connected eviction policy and a valid pool, so the receptacle unwraps (lib.rs:55, :573) cannot fire before the existence check.")
row("DM-RECOVER-EXTENT-ZERO-REFS", "re", [],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).read_ref@ == 0 && ent(^d, key).write_ref@ == 0"],
    ["result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 1"])
row("DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR", "re", ["(*d).ep == None"],
    ["match result { Out::Ret(Err(_)) => same_map(^d, *d), _ => false }"], None,
    refute=dict(req=["wf(*d)", "(*d).ep == None"], ens=["result == Out::Panic"]),
    note="REFUTED: with no eviction policy connected, recover_extent aborts instead of returning an error: get_pool_id unwraps the receptacle at lib.rs:55 when no pool exists, and lib.rs:573 `self.eviction_policy.get().unwrap()` unwraps it when one does. refute_ proves the call panics on every such input.")
row("DM-RECOVER-EXTENT-FRAME", "re", [], ["others_same(^d, *d, key)"],
    ["forall<j: u64> (^d).entries@.get(j) == None"], note="Holds on every path, including the receptacle-unwrap panic paths (a panicking call leaves the state unchanged).")

# ---------------- checksums ----------------
row("DM-SET-CHECKSUM-RECORDS", "custom", ["checksum@ != 0"],
    ["result.0 == Ok(()) ==> result.1 == Some(checksum)"], ["result.0 == Ok(()) ==> result.1 == None"],
    custom=("d: &mut Dm, key: u64, checksum: u32", "(Result<(), DmError>, Option<u32>)",
            "let r = dm_set_checksum(d, key, checksum);\n    let g = dm_get_checksum(d, key);\n    (r, g)", ["dm_set_checksum", "dm_get_checksum"]),
    note="Driver: set_checksum then get_checksum. The integrity-check feature is assumed ON (the methods only exist then).")
row("DM-SET-CHECKSUM-NOT-FOUND", "sc", ["!has(*d, key)"], ["result == Err(DmError::KeyNotFound(key)) && !has(^d, key)"], ["result == Ok(())"])
row("DM-SET-CHECKSUM-FRAME", "sc", [],
    ["others_same(^d, *d, key) && rest_same(*d, ^d)",
     "has(*d, key) ==> has(^d, key) && ent(^d, key).location == ent(*d, key).location && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle"],
    ["has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1"])
row("DM-GET-CHECKSUM-NONE-UNSET", "custom", ["a_fr020(*d)"],
    ["result.0 ==> result.1 == None"], ["result.0 ==> result.1 != None"],
    custom=("d: &mut Dm, which: Adder", "(bool, Option<u32>)",
            "match which {\n        Adder::Create(key, pointer, size) => { let r = dm_create_memory_tier_entry(d, key, pointer, size); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }\n        Adder::Recover(key, offset, sb) => { let r = dm_recover_extent(d, key, offset, sb); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }\n        Adder::Init(key) => { let r = dm_initialize(d); let ok = match r { Out::Ret(Ok(())) => reported_prog(d, key), _ => false }; (ok, dm_get_checksum(d, key)) }\n    }",
            ["dm_create_memory_tier_entry", "dm_recover_extent", "dm_initialize", "dm_get_checksum", "get_pool_id", "reported_prog"]),
    note="assumes: [D-RANGE-FR-020-fd9bf8] (initialize case). Driver: add an entry by create_memory_tier_entry, recover_extent or initialize (for a reported key), then get_checksum on it: none (every adder stores checksum 0, lib.rs:104/:412/:588, and 0 reads as none, lib.rs:614).")
row("DM-GET-CHECKSUM-ZERO-IS-NONE", "gc", [],
    ["has(*d, key) && ent(*d, key).checksum@ == 0 ==> result == None", "has(*d, key) && ent(*d, key).checksum@ != 0 ==> result == Some(ent(*d, key).checksum)"],
    ["has(*d, key) && ent(*d, key).checksum@ == 0 ==> result == Some(ent(*d, key).checksum)"])
row("DM-GET-CHECKSUM-ABSENT-KEY", "gc", ["!has(*d, key)"], ["result == None"], ["result != None"])
row("DM-GET-CHECKSUM-FRAME", "gc", [], ["^d == *d"], ["has(*d, key) ==> !has(^d, key)"])

# ---------------- global invariants ----------------
row("DM-INV-RELEASE-WAKES-WAITERS", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && (ent(^d, k).write_ref@ < ent(*result.1, k).write_ref@ || ent(^d, k).read_ref@ < ent(*result.1, k).read_ref@) ==> *(^d).cv.epoch > *(*result.1).cv.epoch"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) ==> *(^d).cv.epoch > *(*result.1).cv.epoch"], wf=True,
    fid="trusted-boundary",
    note="Inductive step over every method: any critical section that lowers a read count or clears a write reference calls condvar.notify_all (modelled as a trusted epoch counter) before returning. Needs the proved invariant wf: at function level downgrade_reference's overflow path (lib.rs:308-312) clears write_ref and returns without notifying, but that path is unreachable under DM-INV-READ-WRITE-EXCLUSIVE (see refute_dm_downgrade_reference_refcount_overflow). 'so they re-check before their deadline' (the waiter side) is Condvar semantics, trusted.")
row("DM-INV-WAIT-TIMES-OUT-ONLY-IF-BLOCKED", "custom", [],
    ["result.0 ==> !pred_holds(*result.1, key, pw(which))", "pred_holds(*d, key, pw(which)) ==> !result.0"],
    ["pred_holds(*d, key, pw(which)) ==> result.0"],
    custom=("d: &mut Dm, key: u64, which: u8", "(bool, Snapshot<Dm>)",
            "if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }\n    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }\n    else { let (r, s) = dm_take_write(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }",
            ["dm_lookup", "dm_take_read", "dm_take_write", "wait_for", "check_pred"]),
    fid="trusted-boundary",
    note="which: 0 = lookup, 1 = take_read, otherwise take_write. Timeout only if the waited-for condition is false in the state where the wait gave up; never if it holds at call time. 'stayed false for the whole period' beyond the check points is timing, not proved.")
for pid, body, mbody in [
    ("DM-INV-WRITE-REF-AT-MOST-ONE", "ent(^d, k).write_ref@ <= 1", "ent(^d, k).write_ref@ == 0"),
    ("DM-INV-READ-WRITE-EXCLUSIVE", "ent(^d, k).write_ref@ != 0 ==> ent(^d, k).read_ref@ == 0", "ent(^d, k).read_ref@ == 0"),
    ("DM-INV-MEMORY-TIER-SIZE-NONZERO", "match ent(^d, k).location { Location::MemoryTier { size, .. } => size@ > 0, Location::BlockDevice { .. } => true }",
       "match ent(^d, k).location { Location::MemoryTier { size, .. } => size@ > 1, Location::BlockDevice { .. } => true }"),
    ("DM-INV-MEMORY-TIER-SIZE-BLOCKS-CONSISTENT", "match ent(^d, k).location { Location::MemoryTier { size, .. } => ent(^d, k).size_blocks@ == (size@ + 4095) / 4096, Location::BlockDevice { .. } => true }",
       "match ent(^d, k).location { Location::MemoryTier { size, .. } => ent(^d, k).size_blocks@ == size@ / 4096, Location::BlockDevice { .. } => true }"),
]:
    row(pid, "all", [WF, "op_assume(*d, op)"],
        [f"forall<k: u64> has(^d, k) ==> {body}"],
        [f"forall<k: u64> has(^d, k) ==> {mbody}"], wf=True,
        note="Maintained invariant (part of wf): holds initially (dm_new: empty map, module dm_new) and every method preserves it (inductive step over the Op dispatch, every method's own module proving its wf clauses); waits are the trusted Condvar boundary, across which other callers' wf-preserving critical sections run. assumes: [D-RANGE-FR-003-8e9f6a, D-RANGE-FR-023-388782, D-RANGE-FR-020-fd9bf8] as op_assume for the methods they constrain.")
row("DM-INV-REFCOUNT-CONSERVATION", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == ent(*result.1, k).read_ref@ + delta(op, result.0, k, ent(*result.1, k).read_ref@)",
     "forall<k: u64> !has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == 0",
     "forall<k: u64> has(*result.1, k) && !has(^d, k) ==> ent(*result.1, k).read_ref@ == 0"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == ent(*result.1, k).read_ref@"],
    fid="trusted-boundary",
    note="Inductive step: every critical section changes an entry's read count by exactly +1 for a successful acquire on that key (lookup found / take_read / downgrade_reference), -1 for a successful release (release_read, convert_to_storage on a positive count), 0 otherwise (errors included); new entries start at 0 and only entries at 0 are removed. The Mutex (trusted boundary) serializes critical sections, so under any interleaving the count equals acquisitions minus releases - no lost update, leak or underflow (counts are u32 and overflow is an error, not a wrap). The interleaving argument itself (Mutex serialization) is not machine-checked here. assumes: [D-RANGE-FR-020-fd9bf8] for initialize.")
row("DM-INV-ENTRY-TRACKED-FOR-EVICTION", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(^d, k) ==> match (^d).pool_id { Some(p) => trk(^d).get(ent(^d, k).eviction_handle) == Some((p, k)), None => false }",
     "match (^d).pool_id { Some(p) => forall<h: Handle> trk(^d).contains(h) && (trk(^d).lookup(h)).0 == p ==> has(^d, (trk(^d).lookup(h)).1), None => true }"],
    ["forall<k: u64> has(^d, k) ==> !trk(^d).contains(ent(^d, k).eviction_handle)"], wf=True,
    fid="trusted-boundary",
    note="Maintained invariant (wf_fwd + wf_back): a key is tracked in the map's pool exactly when it has an entry. IEvictionPolicy is a trusted consumed-interface model (track/remove/create_pool contracts), and the pool is assumed private to this dispatch map (no other client tracks into or evicts from it). assumes: [D-RANGE-FR-003-8e9f6a, D-RANGE-FR-023-388782, D-RANGE-FR-020-fd9bf8].")
row("DM-INV-CHECKSUM-SURVIVES-TRANSITIONS", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) != is_mt(ent(^d, k).location) ==> ent(^d, k).checksum == ent(*result.1, k).checksum"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) != is_mt(ent(^d, k).location) ==> ent(^d, k).checksum != ent(*result.1, k).checksum"],
    mods=["dm_get_checksum"], note="Over every method: any call that moves an entry between tiers leaves its checksum field unchanged, so get_checksum keeps returning it. assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INITIALIZE-EXPLICIT-ONLY", "custom", [],
    ["forall<k: u64> !has(result, k)"], ["exists<k: u64> has(result, k)"],
    custom=("ep: Option<Ep>, em: Option<Em>, cv: Cv", "Dm", "dm_new(ep, em, cv)", ["dm_new"]),
    note="Construction (DispatchMapState::new, state.rs:29-37) builds an empty map and never calls the extent manager, even when one is bound.")
row("DM-INV-SINGLE-WRITER", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && ent(*result.1, k).write_ref@ != 0 && has(^d, k) && ent(^d, k).write_ref@ == 0 ==> releases_write(op, k)",
     "match op { Op::TakeWrite(k) => result.0 == St::Ok ==> has(*result.1, k) && ent(*result.1, k).write_ref@ == 0 && ent(^d, k).write_ref@ == 1, _ => true }"],
    ["match op { Op::TakeWrite(k) => result.0 == St::Ok ==> ent(*result.1, k).write_ref@ == 1, _ => true }"],
    fid="trusted-boundary",
    note="Sequentialized under the trusted Mutex: take_write succeeds only from write_ref == 0 (and sets 1), and the only critical sections that take an entry from a held write reference to none are release_write and downgrade_reference on that key (remove refuses an entry with a writer). Hence between two successful take_write calls on a key one of those must run. Who holds the reference is not modelled (the API has no owner token), and the interleaving argument is Mutex serialization (trusted). assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INV-WAIT-THEN-ACT-ATOMIC", "custom", [],
    ["result.0 ==> pred_holds(*result.1, key, pw(which)) && has(*result.1, key) && has(^d, key)",
     "result.0 && which@ < 2 ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1 && ent(^d, key).write_ref@ == 0",
     "result.0 && which@ >= 2 ==> ent(^d, key).write_ref@ == 1 && ent(^d, key).read_ref@ == 0"],
    ["result.0 && which@ >= 2 ==> ent(*result.1, key).read_ref@ > 0"],
    custom=("d: &mut Dm, key: u64, which: u8", "(bool, Snapshot<Dm>)",
            "if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Ok(LookupResult::NotExist) => false, Ok(_) => true, _ => false }, s) }\n    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Ok(()) => true, _ => false }, s) }\n    else { let (r, s) = dm_take_write(d, key); (match r { Ok(()) => true, _ => false }, s) }",
            ["dm_lookup", "dm_take_read", "dm_take_write", "wait_for", "check_pred"]),
    fid="trusted-boundary",
    note="which: 0 = lookup, 1 = take_read, otherwise take_write. The reference is taken in the very state in which the wait observed its condition (wait_for returns with the guard held, state.rs:53-79), so the condition still holds when the reference is taken. Mutex/Condvar trusted.")
row("DM-INV-ERROR-LEAVES-STATE-UNCHANGED", "all", [WF, "op_assume(*d, op)"],
    ["result.0 == St::Err ==> same_map(^d, *result.1)"], ["result.0 == St::Err ==> !same_map(^d, *result.1)"], wf=True,
    note="Over every method, in states satisfying the proved invariant wf: an error return leaves every entry (location, sizes, counts, handle, checksum) as it was at the linearization point. At FUNCTION level (arbitrary entry state) downgrade_reference's overflow error path is the one exception - it clears write_ref before failing (lib.rs:308-312, see refute_dm_downgrade_reference_refcount_overflow) - but that input state is unreachable under DM-INV-READ-WRITE-EXCLUSIVE. assumes: [D-RANGE-FR-003-8e9f6a, D-RANGE-FR-023-388782, D-RANGE-FR-020-fd9bf8].")
row("DM-INV-SINGLE-POOL", "all", [WF, "op_assume(*d, op)"],
    ["match (*result.1).pool_id { Some(p) => (^d).pool_id == Some(p) && npools(^d) == npools(*result.1), None => true }",
     "forall<h: Handle> trk(^d).contains(h) && !trk(*result.1).contains(h) ==> Some((trk(^d).lookup(h)).0) == (^d).pool_id"],
    ["match (*result.1).pool_id { Some(p) => npools(^d) > npools(*result.1), None => true }"],
    fid="trusted-boundary",
    note="Over every method: once the pool exists it never changes and no further pool is created; every handle a method newly tracks is tracked in that pool. (get_pool_id runs under its own Mutex, trusted.) assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INV-SSD-OFFSET-STICKY", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) && ssd_of(ent(*result.1, k).location) != None && is_mt(ent(^d, k).location) ==> ssd_of(ent(^d, k).location) != None"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(^d, k).location) ==> ssd_of(ent(^d, k).location) != None"],
    note="Over every method. assumes: [D-RANGE-FR-020-fd9bf8].")
row("DM-INV-LEGAL-LOCATION-TRANSITIONS", "all", [WF, "op_assume(*d, op)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) && !is_mt(ent(^d, k).location) ==> ssd_of(ent(*result.1, k).location) != None && Some(ent(^d, k).location) == ssd_of(ent(*result.1, k).location).map_logic(|o| Location::BlockDevice { offset: o }) && demotes(op, k)",
     "forall<k: u64> has(*result.1, k) && has(^d, k) && !is_mt(ent(*result.1, k).location) && is_mt(ent(^d, k).location) ==> promotes(op, k)"],
    ["forall<k: u64> has(*result.1, k) && has(^d, k) && !is_mt(ent(*result.1, k).location) && is_mt(ent(^d, k).location) ==> !promotes(op, k)"],
    note="Over every method: memory tier -> block device only by convert_memory_tier_to_block / try_evict_to_block, only with an SSD copy recorded, landing at that offset; block device -> memory tier only by promote_block_to_memory_tier. assumes: [D-RANGE-FR-020-fd9bf8].")

row("DM-INV-OPERATIONS-ARE-KEY-LOCAL", "all", [],
    ["match op_key(op) { Some(k) => forall<j: u64> j != k ==> (^d).entries@.get(j) == (*result.1).entries@.get(j), None => true }"],
    ["forall<j: u64> (^d).entries@.get(j) == (*result.1).entries@.get(j)"],
    note="Over every method that names a key (all but initialize and oldest_keys): every other key's entry - location, size, both reference counts, handle, checksum - is identical before (at the linearization point) and after the call, on every path (success, error, unwrap-panic). No precondition, not even wf. Concurrency: other callers' critical sections run only inside the trusted Condvar wait, i.e. before the linearization point.")

# =====================================================================================
HEADER = r'''// drivers.rs — GENERATED from the property table (one verify_<ID> per level-2 property).
// Each driver's #[requires] holds only the obligation's own premises, declared level-2
// assumptions (a_fr003 / a_fr023 / a_fr020 / a_fr012, see core.rs), and — where marked — the
// proved maintained invariant `wf` (established by dm_new, preserved by every method module).

/// Every IDispatchMap method, for the inductive-step (global invariant) drivers.
pub enum Op {
    Initialize,
    Lookup(u64),
    ConvertToStorage(u64, u64),
    TakeRead(u64),
    TakeWrite(u64),
    ReleaseRead(u64),
    ReleaseWrite(u64),
    Downgrade(u64),
    Remove(u64),
    Touch(u64),
    EntrySize(u64),
    OldestKeys(usize),
    CreateMt(u64, *mut u8, u32),
    ConvertMtToBlock(u64),
    Promote(u64, *mut u8, u32),
    IsEvictable(u64),
    TryEvict(u64),
    RecoverExtent(u64, u64, u32),
    SetChecksum(u64, u32),
    GetChecksum(u64),
}

/// Coarse outcome of a method call.
pub enum St {
    Ok,
    Err,
    Panic,
}

/// Entry-adding methods (DM-GET-CHECKSUM-NONE-UNSET).
pub enum Adder {
    Create(u64, *mut u8, u32),
    Recover(u64, u64, u32),
    Init(u64),
}

/// The declared level-2 assumptions that constrain each method's inputs.
#[logic(open)]
pub fn op_assume(d: Dm, op: Op) -> bool {
    pearlite! { match op {
        Op::Initialize => a_fr020(d) && a_fr023_em(d),
        Op::CreateMt(_, _, s) => a_fr003(s),
        Op::Promote(_, _, s) => a_fr003(s),
        Op::RecoverExtent(_, _, sb) => a_fr023(sb),
        _ => true,
    } }
}

/// Read-count change a successful/failed call makes to key k (count r at the linearization point).
#[logic(open)]
pub fn delta(op: Op, s: St, k: u64, r: Int) -> Int {
    pearlite! { match s {
        St::Ok => match op {
            Op::Lookup(j) => if j == k { 1 } else { 0 },
            Op::TakeRead(j) => if j == k { 1 } else { 0 },
            Op::Downgrade(j) => if j == k { 1 } else { 0 },
            Op::ReleaseRead(j) => if j == k { -1 } else { 0 },
            Op::ConvertToStorage(j, _) => if j == k && r > 0 { -1 } else { 0 },
            _ => 0,
        },
        _ => 0,
    } }
}

#[logic(open)]
pub fn releases_write(op: Op, k: u64) -> bool {
    pearlite! { match op { Op::ReleaseWrite(j) => j == k, Op::Downgrade(j) => j == k, _ => false } }
}

#[logic(open)]
pub fn demotes(op: Op, k: u64) -> bool {
    pearlite! { match op { Op::ConvertMtToBlock(j) => j == k, Op::TryEvict(j) => j == k, _ => false } }
}

#[logic(open)]
pub fn promotes(op: Op, k: u64) -> bool {
    pearlite! { match op { Op::Promote(j, _, _) => j == k, _ => false } }
}

/// The key an operation names (None for initialize / oldest_keys).
#[logic(open)]
pub fn op_key(op: Op) -> Option<u64> {
    pearlite! { match op {
        Op::Initialize => None,
        Op::OldestKeys(_) => None,
        Op::Lookup(k) => Some(k),
        Op::ConvertToStorage(k, _) => Some(k),
        Op::TakeRead(k) => Some(k),
        Op::TakeWrite(k) => Some(k),
        Op::ReleaseRead(k) => Some(k),
        Op::ReleaseWrite(k) => Some(k),
        Op::Downgrade(k) => Some(k),
        Op::Remove(k) => Some(k),
        Op::Touch(k) => Some(k),
        Op::EntrySize(k) => Some(k),
        Op::CreateMt(k, _, _) => Some(k),
        Op::ConvertMtToBlock(k) => Some(k),
        Op::Promote(k, _, _) => Some(k),
        Op::IsEvictable(k) => Some(k),
        Op::TryEvict(k) => Some(k),
        Op::RecoverExtent(k, _, _) => Some(k),
        Op::SetChecksum(k, _) => Some(k),
        Op::GetChecksum(k) => Some(k),
    } }
}

/// The uses each method reports to the eviction policy (DM-OLDEST-KEYS-ORDER), relative to the
/// linearization-point state `lin`: creation / recovery / initialize's restored extents (track)
/// and a successful lookup of an existing entry / touch (touch) append that entry's handle;
/// nothing else reports a use.
#[logic(open)]
pub fn uses_ok(op: Op, s: St, lin: Dm, post: Dm) -> bool {
    pearlite! { match op {
        Op::Touch(k) => if s == St::Ok { uses(post) == uses(lin).push_back(ent(lin, k).eviction_handle) } else { uses(post) == uses(lin) },
        Op::Lookup(k) => if s == St::Ok && has(lin, k) { uses(post) == uses(lin).push_back(ent(lin, k).eviction_handle) } else { uses(post) == uses(lin) },
        Op::CreateMt(k, _, _) => if s == St::Ok { uses(post) == uses(lin).push_back(ent(post, k).eviction_handle) } else { uses(post) == uses(lin) },
        Op::RecoverExtent(k, _, _) => if s == St::Ok { uses(post) == uses(lin).push_back(ent(post, k).eviction_handle) } else { uses(post) == uses(lin) },
        Op::Initialize => if s == St::Ok { match lin.em {
            Some(em) => uses(post).len() == uses(lin).len() + em.extents@.len()
                && (forall<j: Int> 0 <= j && j < uses(lin).len() ==> uses(post)[j] == uses(lin)[j])
                && (forall<i: Int> 0 <= i && i < em.extents@.len() ==> uses(post)[uses(lin).len() + i] == ent(post, em.extents@[i].key).eviction_handle),
            None => uses(post) == uses(lin) } } else { uses(post) == uses(lin) },
        _ => uses(post) == uses(lin),
    } }
}

#[logic(open)]
pub fn pw(which: u8) -> Pred {
    pearlite! { if which@ < 2 { Pred::NoWriter } else { Pred::NoRefs } }
}

/// Whether the extent manager reports `key` (program-side, for the initialize checksum driver).
#[ensures(result == reported(*d, key))]
pub fn reported_prog(d: &Dm, key: u64) -> bool {
    match &d.em {
        None => false,
        Some(em) => {
            let mut i: usize = 0;
            #[invariant(i@ <= em.extents@.len())]
            #[invariant(forall<j: Int> 0 <= j && j < i@ ==> em.extents@[j].key != key)]
            while i < em.extents.len() {
                if em.extents[i].key == key {
                    return true;
                }
                i += 1;
            }
            false
        }
    }
}

fn st_out(r: &Out<Result<(), DmError>>) -> St {
    match r { Out::Ret(Ok(_)) => St::Ok, Out::Ret(Err(_)) => St::Err, Out::Panic => St::Panic }
}

/// One critical section of any method, returning (outcome, state at its linearization point).
macro_rules! step {
    ($d:ident, $op:ident) => {
        match $op {
            Op::Initialize => { let lin = snapshot!(*$d); let r = dm_initialize($d); (match r { Out::Ret(Ok(_)) => St::Ok, Out::Ret(Err(_)) => St::Err, Out::Panic => St::Panic }, lin) }
            Op::Lookup(k) => { let (r, lin) = dm_lookup($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::ConvertToStorage(k, o) => { let lin = snapshot!(*$d); let r = dm_convert_to_storage($d, k, o); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::TakeRead(k) => { let (r, lin) = dm_take_read($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::TakeWrite(k) => { let (r, lin) = dm_take_write($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::ReleaseRead(k) => { let lin = snapshot!(*$d); let r = dm_release_read($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::ReleaseWrite(k) => { let lin = snapshot!(*$d); let r = dm_release_write($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::Downgrade(k) => { let lin = snapshot!(*$d); let r = dm_downgrade_reference($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::Remove(k) => { let lin = snapshot!(*$d); let r = dm_remove($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::Touch(k) => { let lin = snapshot!(*$d); let r = dm_touch($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::EntrySize(k) => { let lin = snapshot!(*$d); let r = dm_entry_size($d, k); (match r { Out::Ret(Ok(_)) => St::Ok, Out::Ret(Err(_)) => St::Err, Out::Panic => St::Panic }, lin) }
            Op::OldestKeys(n) => { let lin = snapshot!(*$d); let r = dm_oldest_keys($d, n); (match r { Out::Ret(_) => St::Ok, Out::Panic => St::Panic }, lin) }
            Op::CreateMt(k, p, s) => { let lin = snapshot!(*$d); let r = dm_create_memory_tier_entry($d, k, p, s); (match r { Out::Ret(Ok(_)) => St::Ok, Out::Ret(Err(_)) => St::Err, Out::Panic => St::Panic }, lin) }
            Op::ConvertMtToBlock(k) => { let lin = snapshot!(*$d); let r = dm_convert_memory_tier_to_block($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::Promote(k, p, s) => { let lin = snapshot!(*$d); let r = dm_promote_block_to_memory_tier($d, k, p, s); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::IsEvictable(k) => { let lin = snapshot!(*$d); let _ = dm_is_evictable($d, k); (St::Ok, lin) }
            Op::TryEvict(k) => { let lin = snapshot!(*$d); let r = dm_try_evict_to_block($d, k); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::RecoverExtent(k, o, sb) => { let lin = snapshot!(*$d); let r = dm_recover_extent($d, k, o, sb); (match r { Out::Ret(Ok(_)) => St::Ok, Out::Ret(Err(_)) => St::Err, Out::Panic => St::Panic }, lin) }
            Op::SetChecksum(k, c) => { let lin = snapshot!(*$d); let r = dm_set_checksum($d, k, c); (match r { Ok(_) => St::Ok, Err(_) => St::Err }, lin) }
            Op::GetChecksum(k) => { let lin = snapshot!(*$d); let _ = dm_get_checksum($d, k); (St::Ok, lin) }
        }
    };
}
'''

def fn_name(pid, suffix=""):
    return ("verify_" + pid.lower().replace("-", "_")) + suffix

def emit_fn(name, params, ret, body, req, ens, comment, attrs=()):
    lines = [f"// ---- {comment}"] + list(attrs)
    for r in req:
        lines.append(f"#[requires({r})]")
    for e in ens:
        lines.append(f"#[ensures({e})]")
    lines.append(f"pub fn {name}({params}) -> {ret} {{")
    lines.append(f"    {body}")
    lines.append("}")
    return "\n".join(lines) + "\n"

def main():
    d = yaml.safe_load(open("/home/cornel/cv-dispatch-map-20261005/components/dispatch-map/verif/unified_properties.yaml"))
    excl = set(d["level2_excluded"])
    excl |= {"DM-DOWNGRADE-REFERENCE-REFCOUNT-OVERFLOW"}  # explicit skip: unreachable under wf (bundle edit pending)
    l2 = [p["id"] for p in d["properties"] if p.get("verifiable") and p["id"] not in excl]
    ids = [r["id"] for r in ROWS]
    missing = [i for i in l2 if i not in ids]
    extra = [i for i in ids if i not in l2]
    dup = [i for i in ids if ids.count(i) > 1]
    if missing or extra or dup:
        sys.exit(f"coverage mismatch: missing={missing} extra={extra} dup={set(dup)}")
    out = [HEADER]
    adv = {}
    for r in ROWS:
        if r["custom"]:
            params, ret, body, mods = r["custom"]
        else:
            params, ret, body, mods = SIG[r["sig"]]
        if mods is None:
            mods = ALL_MODS
        mods = list(mods) + [m for m in r["mods"] if m not in mods]
        if r["wf"] or r["sig"] == "all":
            mods = mods + [m for m in ALL_MODS if m not in mods]
        out.append(emit_fn(fn_name(r["id"]), params, ret, body, r["req"], r["ens"], r["id"], r["attrs"]))
        if r["mut"]:
            out.append(emit_fn(fn_name(r["id"], "__mutant"), params, ret, body, r["req"], r["mut"], r["id"] + " — anti-vacuity twin, MUST FAIL", r["attrs"]))
        if r["refute"]:
            rf = r["refute"]
            out.append(emit_fn("refute_" + r["id"].lower().replace("-", "_"), params, ret, body, rf["req"], rf["ens"],
                               r["id"] + " — NEGATION (machine-checked counterexample family): MUST PROVE"))
            out.append(emit_fn("sanity_refute_" + r["id"].lower().replace("-", "_"), params, ret, body, rf["req"], ["!(" + rf["ens"][0] + ")"],
                               r["id"] + " — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)"))
        own = fn_name(r["id"])
        a = {"fidelity": r["fid"], "evidence": {"modules": [own] + mods}}
        if r["deleg"]:
            a["delegate_to"] = dict(r["deleg"])
        note = r["note"]
        base = ("Proved against a line-faithful standalone mirror (verif-creusot/src/core.rs) of src/lib.rs + src/state.rs; "
                "std HashMap = FMap ghost mirror via #[trusted] wrappers (KD-STD-CONTAINER-NO-SPECS lever); Mutex = trusted "
                "(each method body = one critical section); logger calls not modelled; integrity-check feature ON. "
                "evidence.modules lists the callee mirror modules whose own proofs the driver relies on.")
        a["note"] = (note + " " if note else "") + base
        adv[r["id"]] = a
    open(OUT, "w").write("\n".join(out))
    hdr = ("# creusot_advisory.yaml — advisory fields ONLY (fidelity, note, evidence.modules pointer), keyed by property id.\n"
           "# Written by the Role-2 Creusot agent; folded into unified_properties.yaml by the orchestrator.\n"
           "# The scorer (scorer_creusot.py) owns status/symbol/evidence results. Crate: components/dispatch-map/verif-creusot\n")
    with open(ADV, "w") as f:
        f.write(hdr)
        yaml.safe_dump(adv, f, sort_keys=False, width=110, allow_unicode=True)
    print(f"{len(ROWS)} drivers, {sum(1 for r in ROWS if r['mut'])} mutants, {sum(1 for r in ROWS if r['refute'])} refutes")

if __name__ == "__main__":
    main()
