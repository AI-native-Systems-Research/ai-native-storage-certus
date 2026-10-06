// drivers.rs — GENERATED from the property table (one verify_<ID> per level-2 property).
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

// ---- DM-INITIALIZE-RECOVERS-ALL-EXTENTS
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> has(^d, em.extents@[i].key) && ent(^d, em.extents@[i].key).location == Location::BlockDevice { offset: em.extents@[i].offset } && lr_of(ent(^d, em.extents@[i].key).location) == LookupResult::BlockDevice { offset: em.extents@[i].offset }, None => true }, _ => true })]
pub fn verify_dm_initialize_recovers_all_extents(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-RECOVERS-ALL-EXTENTS — anti-vacuity twin, MUST FAIL
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> !has(^d, em.extents@[i].key), None => true }, _ => true })]
pub fn verify_dm_initialize_recovers_all_extents__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-RECOVERED-SIZE
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> ent(^d, em.extents@[i].key).size_blocks == em.extents@[i].size, None => true }, _ => true })]
pub fn verify_dm_initialize_recovered_size(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-RECOVERED-SIZE — anti-vacuity twin, MUST FAIL
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> ent(^d, em.extents@[i].key).size_blocks@ == em.extents@[i].size@ + 1, None => true }, _ => true })]
pub fn verify_dm_initialize_recovered_size__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-EMPTY-EXTENT-MANAGER
#[requires(a_fr012(*d))]
#[requires(match (*d).em { Some(em) => em.extents@.len() == 0, None => false })]
#[ensures(result == Out::Ret(Ok(())) && same_map(^d, *d))]
pub fn verify_dm_initialize_empty_extent_manager(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-EMPTY-EXTENT-MANAGER — anti-vacuity twin, MUST FAIL
#[requires(a_fr012(*d))]
#[requires(match (*d).em { Some(em) => em.extents@.len() == 0, None => false })]
#[ensures(result == Out::Ret(Ok(())) && !same_map(^d, *d))]
pub fn verify_dm_initialize_empty_extent_manager__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-NO-EXTENT-MANAGER-OK
#[requires(a_fr012(*d))]
#[requires((*d).em == None)]
#[ensures(result == Out::Ret(Ok(())) && same_map(^d, *d))]
pub fn verify_dm_initialize_no_extent_manager_ok(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-NO-EXTENT-MANAGER-OK — anti-vacuity twin, MUST FAIL
#[requires(a_fr012(*d))]
#[requires((*d).em == None)]
#[ensures(result != Out::Ret(Ok(())))]
pub fn verify_dm_initialize_no_extent_manager_ok__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-RESTORED-ZERO-REFS
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> ent(^d, em.extents@[i].key).read_ref@ == 0 && ent(^d, em.extents@[i].key).write_ref@ == 0, None => true }, _ => true })]
pub fn verify_dm_initialize_restored_zero_refs(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-RESTORED-ZERO-REFS — anti-vacuity twin, MUST FAIL
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> ent(^d, em.extents@[i].key).write_ref@ == 1, None => true }, _ => true })]
pub fn verify_dm_initialize_restored_zero_refs__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-TRACKS-EVICTION
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> (^d).pool_id != None && trk(^d).contains(ent(^d, em.extents@[i].key).eviction_handle) && Some(trk(^d).lookup(ent(^d, em.extents@[i].key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, em.extents@[i].key)), None => true }, _ => true })]
pub fn verify_dm_initialize_tracks_eviction(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-TRACKS-EVICTION — anti-vacuity twin, MUST FAIL
#[requires(a_fr020(*d))]
#[ensures(match result { Out::Ret(Ok(())) => match (*d).em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> !trk(^d).contains(ent(^d, em.extents@[i].key).eviction_handle), None => true }, _ => true })]
pub fn verify_dm_initialize_tracks_eviction__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-FRAME-UNREPORTED-KEYS
#[ensures(forall<k: u64> !reported(*d, k) ==> (^d).entries@.get(k) == (*d).entries@.get(k))]
pub fn verify_dm_initialize_frame_unreported_keys(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-INITIALIZE-FRAME-UNREPORTED-KEYS — anti-vacuity twin, MUST FAIL
#[ensures(forall<k: u64> (^d).entries@.get(k) == (*d).entries@.get(k))]
pub fn verify_dm_initialize_frame_unreported_keys__mutant(d: &mut Dm) -> Out<Result<(), DmError>> {
    dm_initialize(d)
}

// ---- DM-LOOKUP-NOT-EXIST
#[requires(!has(*d, key))]
#[ensures(result.0 == Ok(LookupResult::NotExist) && !has(^d, key))]
pub fn verify_dm_lookup_not_exist(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-NOT-EXIST — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result.0 == Ok(LookupResult::NotExist) && has(^d, key))]
pub fn verify_dm_lookup_not_exist__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-MEMORY-TIER-RESULT
#[ensures(match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::MemoryTier { pointer, size, .. } => r == LookupResult::MemoryTier { pointer, size } && ent(^d, key).location == ent(*result.1, key).location, Location::BlockDevice { .. } => true }, Err(_) => true })]
pub fn verify_dm_lookup_memory_tier_result(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-MEMORY-TIER-RESULT — anti-vacuity twin, MUST FAIL
#[ensures(match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::MemoryTier { pointer, size, .. } => r == LookupResult::NotExist, Location::BlockDevice { .. } => true }, Err(_) => true })]
pub fn verify_dm_lookup_memory_tier_result__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-BLOCK-DEVICE-RESULT
#[ensures(match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::BlockDevice { offset } => r == LookupResult::BlockDevice { offset } && ent(^d, key).location == ent(*result.1, key).location, Location::MemoryTier { .. } => true }, Err(_) => true })]
pub fn verify_dm_lookup_block_device_result(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-BLOCK-DEVICE-RESULT — anti-vacuity twin, MUST FAIL
#[ensures(match result.0 { Ok(r) => has(*result.1, key) ==> match ent(*result.1, key).location { Location::BlockDevice { offset } => r == LookupResult::NotExist, Location::MemoryTier { .. } => true }, Err(_) => true })]
pub fn verify_dm_lookup_block_device_result__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-INCREMENTS-READ-REF
#[ensures(match result.0 { Ok(_) => has(*result.1, key) ==> has(^d, key) && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1, Err(_) => true })]
pub fn verify_dm_lookup_increments_read_ref(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-INCREMENTS-READ-REF — anti-vacuity twin, MUST FAIL
#[ensures(match result.0 { Ok(_) => has(*result.1, key) ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@, Err(_) => true })]
pub fn verify_dm_lookup_increments_read_ref__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-WAITS-FOR-WRITER
#[ensures(match result.0 { Ok(_) => has(*result.1, key) ==> ent(*result.1, key).write_ref@ == 0 && ent(^d, key).write_ref@ == 0, Err(_) => true })]
#[ensures(pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d)]
pub fn verify_dm_lookup_waits_for_writer(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-WAITS-FOR-WRITER — anti-vacuity twin, MUST FAIL
#[ensures(match result.0 { Ok(_) => has(*result.1, key) ==> ent(*result.1, key).write_ref@ == 1, Err(_) => true })]
pub fn verify_dm_lookup_waits_for_writer__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-TIMEOUT
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(result.0 == Err(DmError::Timeout(key)) ==> !pred_holds(*result.1, key, Pred::NoWriter))]
pub fn verify_dm_lookup_timeout(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-TIMEOUT — anti-vacuity twin, MUST FAIL
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Ok(LookupResult::NotExist))]
pub fn verify_dm_lookup_timeout__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-NEVER-MISMATCH-SIZE
#[ensures(result.0 != Ok(LookupResult::MismatchSize))]
pub fn verify_dm_lookup_never_mismatch_size(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-NEVER-MISMATCH-SIZE — anti-vacuity twin, MUST FAIL
#[ensures(result.0 == Ok(LookupResult::MismatchSize))]
pub fn verify_dm_lookup_never_mismatch_size__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-REFCOUNT-OVERFLOW
#[requires(wf(*d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> result.0 == Err(DmError::RefCountOverflow(key)) && ent(^d, key).read_ref == u32::MAX && same_map(^d, *d))]
pub fn verify_dm_lookup_refcount_overflow(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-REFCOUNT-OVERFLOW — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> ent(^d, key).read_ref@ == 0)]
pub fn verify_dm_lookup_refcount_overflow__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-FRAME
#[ensures(others_same(^d, *result.1, key))]
#[ensures(has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).write_ref == ent(*result.1, key).write_ref)]
pub fn verify_dm_lookup_frame(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-LOOKUP-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*result.1, key) ==> ent(^d, key).write_ref@ == ent(*result.1, key).write_ref@ + 1)]
pub fn verify_dm_lookup_frame__mutant(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    dm_lookup(d, key)
}

// ---- DM-CONVERT-TO-STORAGE-SETS-SSD-OFFSET
#[ensures(result == Ok(()) ==> has(^d, key) && ssd_of(ent(^d, key).location) == Some(offset))]
pub fn verify_dm_convert_to_storage_sets_ssd_offset(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-SETS-SSD-OFFSET — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ssd_of(ent(^d, key).location) == None)]
pub fn verify_dm_convert_to_storage_sets_ssd_offset__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-STAYS-MEMORY-TIER
#[ensures(has(*d, key) && is_mt(ent(*d, key).location) ==> has(^d, key) && is_mt(ent(^d, key).location) && lr_of(ent(^d, key).location) == lr_of(ent(*d, key).location))]
pub fn verify_dm_convert_to_storage_stays_memory_tier(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-STAYS-MEMORY-TIER — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) && is_mt(ent(*d, key).location) ==> !is_mt(ent(^d, key).location))]
pub fn verify_dm_convert_to_storage_stays_memory_tier__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-RELEASES-READ-REF
#[ensures(result == Ok(()) ==> ent(^d, key).read_ref@ == (if ent(*d, key).read_ref@ > 0 { ent(*d, key).read_ref@ - 1 } else { 0 }))]
pub fn verify_dm_convert_to_storage_releases_read_ref(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-RELEASES-READ-REF — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ent(^d, key).read_ref == ent(*d, key).read_ref)]
pub fn verify_dm_convert_to_storage_releases_read_ref__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-FRAME
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) ==> has(^d, key) && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle)]
pub fn verify_dm_convert_to_storage_frame(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).write_ref@ == ent(*d, key).write_ref@ + 1)]
pub fn verify_dm_convert_to_storage_frame__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_convert_to_storage_not_found(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_convert_to_storage_not_found__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-BLOCK-DEVICE
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_convert_to_storage_block_device(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-CONVERT-TO-STORAGE-BLOCK-DEVICE — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Ok(()))]
pub fn verify_dm_convert_to_storage_block_device__mutant(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    dm_convert_to_storage(d, key, offset)
}

// ---- DM-TAKE-READ-INCREMENTS
#[ensures(result.0 == Ok(()) ==> has(^d, key) && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1)]
pub fn verify_dm_take_read_increments(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-INCREMENTS — anti-vacuity twin, MUST FAIL
#[ensures(result.0 == Ok(()) ==> ent(^d, key).read_ref == ent(*result.1, key).read_ref)]
pub fn verify_dm_take_read_increments__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-WAITS-FOR-WRITER
#[ensures(result.0 == Ok(()) ==> has(*result.1, key) && ent(*result.1, key).write_ref@ == 0 && ent(^d, key).write_ref@ == 0)]
#[ensures(pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d)]
pub fn verify_dm_take_read_waits_for_writer(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-WAITS-FOR-WRITER — anti-vacuity twin, MUST FAIL
#[ensures(result.0 == Ok(()) ==> ent(*result.1, key).write_ref@ == 1)]
pub fn verify_dm_take_read_waits_for_writer__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-TIMEOUT
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(result.0 == Err(DmError::Timeout(key)) ==> !pred_holds(*result.1, key, Pred::NoWriter))]
pub fn verify_dm_take_read_timeout(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-TIMEOUT — anti-vacuity twin, MUST FAIL
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Ok(()))]
pub fn verify_dm_take_read_timeout__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result.0 == Err(DmError::KeyNotFound(key)) && !has(^d, key))]
pub fn verify_dm_take_read_not_found(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result.0 == Ok(()))]
pub fn verify_dm_take_read_not_found__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-REFCOUNT-OVERFLOW
#[requires(wf(*d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> result.0 == Err(DmError::RefCountOverflow(key)) && ent(^d, key).read_ref == u32::MAX && same_map(^d, *d))]
pub fn verify_dm_take_read_refcount_overflow(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-REFCOUNT-OVERFLOW — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref == u32::MAX ==> ent(^d, key).read_ref@ == 0)]
pub fn verify_dm_take_read_refcount_overflow__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-FRAME
#[ensures(others_same(^d, *result.1, key) && (^d).ep == (*result.1).ep)]
#[ensures(has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).write_ref == ent(*result.1, key).write_ref)]
pub fn verify_dm_take_read_frame(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-READ-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*result.1, key) ==> ent(^d, key).eviction_handle != ent(*result.1, key).eviction_handle)]
pub fn verify_dm_take_read_frame__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_read(d, key)
}

// ---- DM-TAKE-WRITE-SETS-WRITE-REF
#[ensures(result.0 == Ok(()) ==> has(^d, key) && ent(^d, key).write_ref@ == 1)]
pub fn verify_dm_take_write_sets_write_ref(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-SETS-WRITE-REF — anti-vacuity twin, MUST FAIL
#[ensures(result.0 == Ok(()) ==> ent(^d, key).write_ref@ == 2)]
pub fn verify_dm_take_write_sets_write_ref__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-REQUIRES-NO-REFS
#[ensures(result.0 == Ok(()) ==> has(*result.1, key) && ent(*result.1, key).read_ref@ == 0 && ent(*result.1, key).write_ref@ == 0)]
#[ensures(pred_holds(*d, key, Pred::NoRefs) ==> *result.1 == *d)]
pub fn verify_dm_take_write_requires_no_refs(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-REQUIRES-NO-REFS — anti-vacuity twin, MUST FAIL
#[ensures(result.0 == Ok(()) ==> ent(*result.1, key).write_ref@ == 1)]
pub fn verify_dm_take_write_requires_no_refs__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-TIMEOUT
#[ensures(!pred_holds(*result.1, key, Pred::NoRefs) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(result.0 == Err(DmError::Timeout(key)) ==> !pred_holds(*result.1, key, Pred::NoRefs))]
pub fn verify_dm_take_write_timeout(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-TIMEOUT — anti-vacuity twin, MUST FAIL
#[ensures(!pred_holds(*result.1, key, Pred::NoRefs) ==> result.0 == Ok(()))]
pub fn verify_dm_take_write_timeout__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result.0 == Err(DmError::KeyNotFound(key)) && !has(^d, key))]
pub fn verify_dm_take_write_not_found(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result.0 == Ok(()))]
pub fn verify_dm_take_write_not_found__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-FRAME
#[ensures(others_same(^d, *result.1, key))]
#[ensures(has(*result.1, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*result.1, key)) && ent(^d, key).read_ref == ent(*result.1, key).read_ref)]
pub fn verify_dm_take_write_frame(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-TAKE-WRITE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*result.1, key) ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1)]
pub fn verify_dm_take_write_frame__mutant(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    dm_take_write(d, key)
}

// ---- DM-RELEASE-READ-DECREMENTS
#[ensures(result == Ok(()) ==> has(*d, key) && has(^d, key) && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ - 1)]
pub fn verify_dm_release_read_decrements(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-DECREMENTS — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ent(^d, key).read_ref == ent(*d, key).read_ref)]
pub fn verify_dm_release_read_decrements__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-UNDERFLOW
#[requires(has(*d, key) && ent(*d, key).read_ref@ == 0)]
#[ensures(result == Err(DmError::RefCountUnderflow(key)) && ent(^d, key).read_ref@ == 0)]
pub fn verify_dm_release_read_underflow(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-UNDERFLOW — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && ent(*d, key).read_ref@ == 0)]
#[ensures(result == Ok(()))]
pub fn verify_dm_release_read_underflow__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_release_read_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_release_read_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-FRAME
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key)) && ent(^d, key).write_ref == ent(*d, key).write_ref)]
pub fn verify_dm_release_read_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-READ-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).write_ref@ == ent(*d, key).write_ref@ + 1)]
pub fn verify_dm_release_read_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_read(d, key)
}

// ---- DM-RELEASE-WRITE-CLEARS
#[ensures(result == Ok(()) ==> has(^d, key) && ent(^d, key).write_ref@ == 0)]
pub fn verify_dm_release_write_clears(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-CLEARS — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ent(^d, key).write_ref@ == 1)]
pub fn verify_dm_release_write_clears__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-UNDERFLOW
#[requires(has(*d, key) && ent(*d, key).write_ref@ == 0)]
#[ensures(result == Err(DmError::RefCountUnderflow(key)) && ent(^d, key).write_ref@ == 0)]
pub fn verify_dm_release_write_underflow(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-UNDERFLOW — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && ent(*d, key).write_ref@ == 0)]
#[ensures(result == Ok(()))]
pub fn verify_dm_release_write_underflow__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_release_write_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_release_write_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-FRAME
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key)) && ent(^d, key).read_ref == ent(*d, key).read_ref)]
pub fn verify_dm_release_write_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-RELEASE-WRITE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1)]
pub fn verify_dm_release_write_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_release_write(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-SWAPS
#[ensures(result == Ok(()) ==> has(*d, key) && has(^d, key) && ent(^d, key).write_ref@ == 0 && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1)]
pub fn verify_dm_downgrade_reference_swaps(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-SWAPS — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ent(^d, key).write_ref == ent(*d, key).write_ref)]
pub fn verify_dm_downgrade_reference_swaps__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-NO-WRITE-REF
#[requires(has(*d, key) && ent(*d, key).write_ref@ == 0)]
#[ensures(result == Err(DmError::NoWriteReference(key)) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref)]
pub fn verify_dm_downgrade_reference_no_write_ref(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-NO-WRITE-REF — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && ent(*d, key).write_ref@ == 0)]
#[ensures(result == Ok(()))]
pub fn verify_dm_downgrade_reference_no_write_ref__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_downgrade_reference_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_downgrade_reference_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-FRAME
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) ==> has(^d, key) && keep_meta(ent(^d, key), ent(*d, key)))]
pub fn verify_dm_downgrade_reference_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-DOWNGRADE-REFERENCE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).checksum@ == ent(*d, key).checksum@ + 1)]
pub fn verify_dm_downgrade_reference_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_downgrade_reference(d, key)
}

// ---- DM-REMOVE-DELETES
#[ensures(result == Ok(()) ==> !has(^d, key))]
pub fn verify_dm_remove_deletes(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-DELETES — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> has(^d, key))]
pub fn verify_dm_remove_deletes__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-ACTIVE-REFERENCES
#[requires(has(*d, key) && (ent(*d, key).read_ref@ > 0 || ent(*d, key).write_ref@ > 0))]
#[ensures(result == Err(DmError::ActiveReferences(key)) && same_map(^d, *d))]
pub fn verify_dm_remove_active_references(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-ACTIVE-REFERENCES — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && (ent(*d, key).read_ref@ > 0 || ent(*d, key).write_ref@ > 0))]
#[ensures(result == Ok(()))]
pub fn verify_dm_remove_active_references__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_remove_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_remove_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-UNTRACKS
#[requires(wf(*d))]
#[ensures(result == Ok(()) ==> !trk(^d).contains(ent(*d, key).eviction_handle))]
#[ensures(result == Ok(()) ==> match (^d).pool_id { Some(p) => forall<h: Handle> trk(^d).contains(h) && (trk(^d).lookup(h)).0 == p ==> (trk(^d).lookup(h)).1 != key, None => true })]
pub fn verify_dm_remove_untracks(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-UNTRACKS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(result == Ok(()) ==> trk(^d).contains(ent(*d, key).eviction_handle))]
pub fn verify_dm_remove_untracks__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-FRAME
#[ensures(others_same(^d, *d, key))]
pub fn verify_dm_remove_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-REMOVE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(forall<j: u64> (^d).entries@.get(j) == None)]
pub fn verify_dm_remove_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_remove(d, key)
}

// ---- DM-TOUCH-FRAME
#[ensures(same_map(^d, *d))]
pub fn verify_dm_touch_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_touch(d, key)
}

// ---- DM-TOUCH-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1)]
pub fn verify_dm_touch_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_touch(d, key)
}

// ---- DM-TOUCH-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)))]
pub fn verify_dm_touch_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_touch(d, key)
}

// ---- DM-TOUCH-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_touch_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_touch(d, key)
}

// ---- DM-ENTRY-SIZE-BLOCK-MULTIPLE
#[requires(wf(*d))]
#[ensures(result != Out::Panic)]
#[ensures(match result { Out::Ret(Ok(v)) => has(*d, key) && v@ == ent(*d, key).size_blocks@ * 4096 && v@ % 4096 == 0, _ => true })]
pub fn verify_dm_entry_size_block_multiple(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-BLOCK-MULTIPLE — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(match result { Out::Ret(Ok(v)) => v@ % 4096 == 1, _ => true })]
pub fn verify_dm_entry_size_block_multiple__mutant(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-ROUNDS-UP-MEMORY-TIER
#[requires(a_fr003(size))]
#[ensures(match result.0 { Out::Ret(Ok(())) => match result.1 { Out::Ret(Ok(v)) => v@ >= size@ && v@ < size@ + 4096 && v@ % 4096 == 0, _ => false }, _ => true })]
pub fn verify_dm_entry_size_rounds_up_memory_tier(d: &mut Dm, key: u64, pointer: *mut u8, size: u32, promote: bool) -> (Out<Result<(), DmError>>, Out<Result<u32, DmError>>) {
    let r = if promote { match dm_promote_block_to_memory_tier(d, key, pointer, size) { Ok(()) => Out::Ret(Ok(())), Err(e) => Out::Ret(Err(e)) } } else { dm_create_memory_tier_entry(d, key, pointer, size) };
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-ENTRY-SIZE-ROUNDS-UP-MEMORY-TIER — anti-vacuity twin, MUST FAIL
#[requires(a_fr003(size))]
#[ensures(match result.0 { Out::Ret(Ok(())) => match result.1 { Out::Ret(Ok(v)) => v@ == size@, _ => false }, _ => true })]
pub fn verify_dm_entry_size_rounds_up_memory_tier__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32, promote: bool) -> (Out<Result<(), DmError>>, Out<Result<u32, DmError>>) {
    let r = if promote { match dm_promote_block_to_memory_tier(d, key, pointer, size) { Ok(()) => Out::Ret(Ok(())), Err(e) => Out::Ret(Err(e)) } } else { dm_create_memory_tier_entry(d, key, pointer, size) };
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-ENTRY-SIZE-FRAME
#[ensures(^d == *d)]
pub fn verify_dm_entry_size_frame(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> !has(^d, key))]
pub fn verify_dm_entry_size_frame__mutant(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Out::Ret(Err(DmError::KeyNotFound(key))))]
pub fn verify_dm_entry_size_not_found(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result != Out::Ret(Err(DmError::KeyNotFound(key))))]
pub fn verify_dm_entry_size_not_found__mutant(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-NO-ARITHMETIC-OVERFLOW
#[requires(wf(*d))]
#[ensures(result != Out::Panic)]
#[ensures(match result { Out::Ret(Ok(v)) => v@ == ent(*d, key).size_blocks@ * 4096, _ => true })]
pub fn verify_dm_entry_size_no_arithmetic_overflow(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-ENTRY-SIZE-NO-ARITHMETIC-OVERFLOW — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(result == Out::Panic)]
pub fn verify_dm_entry_size_no_arithmetic_overflow__mutant(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    dm_entry_size(d, key)
}

// ---- DM-OLDEST-KEYS-AT-MOST-N
#[ensures(match result { Out::Ret(v) => v@.len() <= n@, Out::Panic => true })]
pub fn verify_dm_oldest_keys_at_most_n(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-AT-MOST-N — anti-vacuity twin, MUST FAIL
#[ensures(match result { Out::Ret(v) => v@.len() > n@, Out::Panic => true })]
pub fn verify_dm_oldest_keys_at_most_n__mutant(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-FRAME
#[ensures(same_map(^d, *d) && trk(^d) == trk(*d))]
pub fn verify_dm_oldest_keys_frame(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(!same_map(^d, *d))]
pub fn verify_dm_oldest_keys_frame__mutant(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-ONLY-PRESENT-KEYS
#[requires(wf(*d))]
#[ensures(match result { Out::Ret(v) => forall<i: Int> 0 <= i && i < v@.len() ==> has(^d, v@[i]), Out::Panic => true })]
pub fn verify_dm_oldest_keys_only_present_keys(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-ONLY-PRESENT-KEYS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[ensures(match result { Out::Ret(v) => forall<i: Int> 0 <= i && i < v@.len() ==> !has(^d, v@[i]), Out::Panic => true })]
pub fn verify_dm_oldest_keys_only_present_keys__mutant(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY
#[requires((*d).ep == None)]
#[ensures(match result { Out::Ret(v) => v@.len() == 0, Out::Panic => false })]
pub fn verify_dm_oldest_keys_no_eviction_policy_empty(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY — NEGATION (machine-checked counterexample family): MUST PROVE
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[requires((*d).pool_id == None)]
#[ensures(result == Out::Panic)]
pub fn refute_dm_oldest_keys_no_eviction_policy_empty(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[requires((*d).pool_id == None)]
#[ensures(!(result == Out::Panic))]
pub fn sanity_refute_dm_oldest_keys_no_eviction_policy_empty(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    dm_oldest_keys(d, n)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-LOCATION
#[ensures(result == Out::Ret(Ok(())) ==> has(^d, key) && ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: None })]
pub fn verify_dm_create_memory_tier_entry_location(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-LOCATION — anti-vacuity twin, MUST FAIL
#[ensures(result == Out::Ret(Ok(())) ==> ssd_of(ent(^d, key).location) != None)]
pub fn verify_dm_create_memory_tier_entry_location__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-WRITE-REF
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 1 && ent(^d, key).read_ref@ == 0)]
pub fn verify_dm_create_memory_tier_entry_write_ref(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-WRITE-REF — anti-vacuity twin, MUST FAIL
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 0)]
pub fn verify_dm_create_memory_tier_entry_write_ref__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[ensures(result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d))]
pub fn verify_dm_create_memory_tier_entry_already_exists(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS — NEGATION (machine-checked counterexample family): MUST PROVE
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[requires(size@ == 0)]
#[ensures(result == Out::Ret(Err(DmError::InvalidSize)))]
pub fn refute_dm_create_memory_tier_entry_already_exists(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[requires(size@ == 0)]
#[ensures(!(result == Out::Ret(Err(DmError::InvalidSize))))]
pub fn sanity_refute_dm_create_memory_tier_entry_already_exists(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE
#[requires(size@ == 0)]
#[ensures(result == Out::Ret(Err(DmError::InvalidSize)) && same_map(^d, *d))]
pub fn verify_dm_create_memory_tier_entry_zero_size(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE — anti-vacuity twin, MUST FAIL
#[requires(size@ == 0)]
#[ensures(result == Out::Ret(Ok(())))]
pub fn verify_dm_create_memory_tier_entry_zero_size__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-SIZE-BLOCKS
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).size_blocks@ == (size@ + 4095) / 4096)]
pub fn verify_dm_create_memory_tier_entry_size_blocks(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-SIZE-BLOCKS — anti-vacuity twin, MUST FAIL
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).size_blocks@ == size@ / 4096 + 1)]
pub fn verify_dm_create_memory_tier_entry_size_blocks__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR
#[requires((*d).ep == None)]
#[requires(size@ != 0)]
#[ensures(match result { Out::Ret(Err(_)) => same_map(^d, *d), _ => false })]
pub fn verify_dm_create_memory_tier_entry_no_eviction_policy_error(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR — NEGATION (machine-checked counterexample family): MUST PROVE
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[requires(size@ != 0)]
#[ensures(result == Out::Panic)]
pub fn refute_dm_create_memory_tier_entry_no_eviction_policy_error(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[requires(size@ != 0)]
#[ensures(!(result == Out::Panic))]
pub fn sanity_refute_dm_create_memory_tier_entry_no_eviction_policy_error(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-FRAME
#[ensures(others_same(^d, *d, key))]
pub fn verify_dm_create_memory_tier_entry_frame(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CREATE-MEMORY-TIER-ENTRY-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(forall<j: u64> (^d).entries@.get(j) == None)]
pub fn verify_dm_create_memory_tier_entry_frame__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    dm_create_memory_tier_entry(d, key, pointer, size)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-TRANSITION
#[ensures(result == Ok(()) ==> has(*d, key) && match ssd_of(ent(*d, key).location) { Some(o) => ent(^d, key).location == Location::BlockDevice { offset: o } && lr_of(ent(^d, key).location) == LookupResult::BlockDevice { offset: o }, None => false })]
pub fn verify_dm_convert_memory_tier_to_block_transition(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-TRANSITION — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> is_mt(ent(^d, key).location))]
pub fn verify_dm_convert_memory_tier_to_block_transition__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NO-SSD-OFFSET
#[requires(has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None)]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_convert_memory_tier_to_block_no_ssd_offset(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NO-SSD-OFFSET — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None)]
#[ensures(result == Ok(()))]
pub fn verify_dm_convert_memory_tier_to_block_no_ssd_offset__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-MEMORY-TIER
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_convert_memory_tier_to_block_not_memory_tier(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-MEMORY-TIER — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Ok(()))]
pub fn verify_dm_convert_memory_tier_to_block_not_memory_tier__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_convert_memory_tier_to_block_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_convert_memory_tier_to_block_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-FRAME
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) ==> has(^d, key) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle)]
pub fn verify_dm_convert_memory_tier_to_block_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-CONVERT-MEMORY-TIER-TO-BLOCK-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1)]
pub fn verify_dm_convert_memory_tier_to_block_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_convert_memory_tier_to_block(d, key)
}

// ---- DM-PROMOTE-TRANSITION
#[ensures(result == Ok(()) ==> has(*d, key) && match ent(*d, key).location { Location::BlockDevice { offset } => ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: Some(offset) }, Location::MemoryTier { .. } => false })]
pub fn verify_dm_promote_transition(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-TRANSITION — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> ssd_of(ent(^d, key).location) == None)]
pub fn verify_dm_promote_transition__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-RECOMPUTES-SIZE
#[requires(a_fr003(size))]
#[ensures(result.0 == Ok(()) ==> match result.1 { Out::Ret(Ok(v)) => v@ == (size@ + 4095) / 4096 * 4096, _ => false })]
pub fn verify_dm_promote_recomputes_size(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> (Result<(), DmError>, Out<Result<u32, DmError>>) {
    let r = dm_promote_block_to_memory_tier(d, key, pointer, size);
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-PROMOTE-RECOMPUTES-SIZE — anti-vacuity twin, MUST FAIL
#[requires(a_fr003(size))]
#[ensures(result.0 == Ok(()) ==> match result.1 { Out::Ret(Ok(v)) => v@ == size@, _ => false })]
pub fn verify_dm_promote_recomputes_size__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> (Result<(), DmError>, Out<Result<u32, DmError>>) {
    let r = dm_promote_block_to_memory_tier(d, key, pointer, size);
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-PROMOTE-PRESERVES-REFS
#[ensures(has(*d, key) ==> has(^d, key) && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref)]
#[ensures(others_same(^d, *d, key))]
#[ensures(has(*d, key) && !is_mt(ent(*d, key).location) ==> result == Ok(()) || result == Err(DmError::InvalidSize))]
pub fn verify_dm_promote_preserves_refs(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-PRESERVES-REFS — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) && !is_mt(ent(*d, key).location) && ent(*d, key).read_ref@ > 0 ==> result == Err(DmError::InvalidState))]
pub fn verify_dm_promote_preserves_refs__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-INV-HANDLE-STABLE
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).eviction_handle == ent(*result.1, k).eviction_handle)]
pub fn verify_dm_inv_handle_stable(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-HANDLE-STABLE — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).eviction_handle != ent(*result.1, k).eviction_handle)]
pub fn verify_dm_inv_handle_stable__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-PROMOTE-ALREADY-MEMORY-TIER
#[requires(has(*d, key) && is_mt(ent(*d, key).location))]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_promote_already_memory_tier(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-ALREADY-MEMORY-TIER — NEGATION (machine-checked counterexample family): MUST PROVE
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[requires(is_mt(ent(*d, key).location))]
#[requires(size@ == 0)]
#[ensures(result == Err(DmError::InvalidSize))]
pub fn refute_dm_promote_already_memory_tier(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-ALREADY-MEMORY-TIER — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[requires(is_mt(ent(*d, key).location))]
#[requires(size@ == 0)]
#[ensures(!(result == Err(DmError::InvalidSize)))]
pub fn sanity_refute_dm_promote_already_memory_tier(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-NOT-FOUND
#[requires(size@ != 0 && !has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
pub fn verify_dm_promote_not_found(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(size@ != 0 && !has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_promote_not_found__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-ZERO-SIZE
#[requires(size@ == 0)]
#[ensures(result == Err(DmError::InvalidSize) && ^d == *d)]
pub fn verify_dm_promote_zero_size(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-PROMOTE-ZERO-SIZE — anti-vacuity twin, MUST FAIL
#[requires(size@ == 0)]
#[ensures(result == Ok(()))]
pub fn verify_dm_promote_zero_size__mutant(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    dm_promote_block_to_memory_tier(d, key, pointer, size)
}

// ---- DM-IS-EVICTABLE-EXACT-CONDITION
#[ensures(result == (has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) != None && ent(*d, key).read_ref@ == 0 && ent(*d, key).write_ref@ == 0))]
pub fn verify_dm_is_evictable_exact_condition(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-EXACT-CONDITION — anti-vacuity twin, MUST FAIL
#[ensures(result == (has(*d, key) && is_mt(ent(*d, key).location) && ent(*d, key).read_ref@ == 0 && ent(*d, key).write_ref@ == 0))]
pub fn verify_dm_is_evictable_exact_condition__mutant(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(!result)]
pub fn verify_dm_is_evictable_not_found(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result)]
pub fn verify_dm_is_evictable_not_found__mutant(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-FRAME
#[ensures(^d == *d)]
pub fn verify_dm_is_evictable_frame(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> !has(^d, key))]
pub fn verify_dm_is_evictable_frame__mutant(d: &mut Dm, key: u64) -> bool {
    dm_is_evictable(d, key)
}

// ---- DM-IS-EVICTABLE-AGREES-WITH-TRY-EVICT
#[ensures(result.0 == (result.1 == Ok(())))]
pub fn verify_dm_is_evictable_agrees_with_try_evict(d: &mut Dm, key: u64) -> (bool, Result<(), DmError>) {
    let b = dm_is_evictable(d, key);
    let r = dm_try_evict_to_block(d, key);
    (b, r)
}

// ---- DM-IS-EVICTABLE-AGREES-WITH-TRY-EVICT — anti-vacuity twin, MUST FAIL
#[ensures(result.0 != (result.1 == Ok(())))]
pub fn verify_dm_is_evictable_agrees_with_try_evict__mutant(d: &mut Dm, key: u64) -> (bool, Result<(), DmError>) {
    let b = dm_is_evictable(d, key);
    let r = dm_try_evict_to_block(d, key);
    (b, r)
}

// ---- DM-TRY-EVICT-TO-BLOCK-SUCCESS
#[ensures(result == Ok(()) ==> has(*d, key) && match ssd_of(ent(*d, key).location) { Some(o) => ent(^d, key).location == Location::BlockDevice { offset: o } && !is_mt(ent(^d, key).location), None => false })]
pub fn verify_dm_try_evict_to_block_success(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-SUCCESS — anti-vacuity twin, MUST FAIL
#[ensures(result == Ok(()) ==> is_mt(ent(^d, key).location))]
pub fn verify_dm_try_evict_to_block_success__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-ACTIVE-REFS
#[requires(has(*d, key) && (ent(*d, key).read_ref@ != 0 || ent(*d, key).write_ref@ != 0))]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_try_evict_to_block_active_refs(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-ACTIVE-REFS — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && (ent(*d, key).read_ref@ != 0 || ent(*d, key).write_ref@ != 0))]
#[ensures(result == Ok(()))]
pub fn verify_dm_try_evict_to_block_active_refs__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NO-SSD-OFFSET
#[requires(has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None)]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_try_evict_to_block_no_ssd_offset(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NO-SSD-OFFSET — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && is_mt(ent(*d, key).location) && ssd_of(ent(*d, key).location) == None)]
#[ensures(result == Ok(()))]
pub fn verify_dm_try_evict_to_block_no_ssd_offset__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NOT-MEMORY-TIER
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Err(DmError::InvalidState) && same_map(^d, *d))]
pub fn verify_dm_try_evict_to_block_not_memory_tier(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NOT-MEMORY-TIER — anti-vacuity twin, MUST FAIL
#[requires(has(*d, key) && !is_mt(ent(*d, key).location))]
#[ensures(result == Ok(()))]
pub fn verify_dm_try_evict_to_block_not_memory_tier__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)))]
pub fn verify_dm_try_evict_to_block_not_found(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_try_evict_to_block_not_found__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-FRAME
#[ensures(others_same(^d, *d, key) && trk(^d) == trk(*d))]
#[ensures(has(*d, key) ==> has(^d, key) && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle)]
pub fn verify_dm_try_evict_to_block_frame(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-TRY-EVICT-TO-BLOCK-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> !trk(^d).contains(ent(*d, key).eviction_handle))]
pub fn verify_dm_try_evict_to_block_frame__mutant(d: &mut Dm, key: u64) -> Result<(), DmError> {
    dm_try_evict_to_block(d, key)
}

// ---- DM-RECOVER-EXTENT-INSERTS
#[requires(a_fr023(size_blocks))]
#[ensures(result.0 == Out::Ret(Ok(())) ==> has(^d, key) && ent(^d, key).location == Location::BlockDevice { offset } && lr_of(ent(^d, key).location) == LookupResult::BlockDevice { offset } && ent(^d, key).size_blocks == size_blocks && match result.1 { Out::Ret(Ok(v)) => v@ == size_blocks@ * 4096, _ => false })]
pub fn verify_dm_recover_extent_inserts(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> (Out<Result<(), DmError>>, Out<Result<u32, DmError>>) {
    let r = dm_recover_extent(d, key, offset, size_blocks);
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-RECOVER-EXTENT-INSERTS — anti-vacuity twin, MUST FAIL
#[requires(a_fr023(size_blocks))]
#[ensures(result.0 == Out::Ret(Ok(())) ==> match result.1 { Out::Ret(Ok(v)) => v@ == size_blocks@, _ => false })]
pub fn verify_dm_recover_extent_inserts__mutant(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> (Out<Result<(), DmError>>, Out<Result<u32, DmError>>) {
    let r = dm_recover_extent(d, key, offset, size_blocks);
    let s = dm_entry_size(d, key);
    (r, s)
}

// ---- DM-RECOVER-EXTENT-REGISTERS-EVICTION
#[ensures(result == Out::Ret(Ok(())) ==> (^d).pool_id != None && trk(^d).contains(ent(^d, key).eviction_handle) && Some(trk(^d).lookup(ent(^d, key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, key)))]
pub fn verify_dm_recover_extent_registers_eviction(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-REGISTERS-EVICTION — anti-vacuity twin, MUST FAIL
#[ensures(result == Out::Ret(Ok(())) ==> !trk(^d).contains(ent(^d, key).eviction_handle))]
pub fn verify_dm_recover_extent_registers_eviction__mutant(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-ALREADY-EXISTS
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[ensures(result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d))]
pub fn verify_dm_recover_extent_already_exists(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-ALREADY-EXISTS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(has(*d, key))]
#[ensures(result == Out::Ret(Ok(())))]
pub fn verify_dm_recover_extent_already_exists__mutant(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-ZERO-REFS
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).read_ref@ == 0 && ent(^d, key).write_ref@ == 0)]
pub fn verify_dm_recover_extent_zero_refs(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-ZERO-REFS — anti-vacuity twin, MUST FAIL
#[ensures(result == Out::Ret(Ok(())) ==> ent(^d, key).write_ref@ == 1)]
pub fn verify_dm_recover_extent_zero_refs__mutant(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR
#[requires((*d).ep == None)]
#[ensures(match result { Out::Ret(Err(_)) => same_map(^d, *d), _ => false })]
pub fn verify_dm_recover_extent_no_eviction_policy_error(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR — NEGATION (machine-checked counterexample family): MUST PROVE
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[ensures(result == Out::Panic)]
pub fn refute_dm_recover_extent_no_eviction_policy_error(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR — sanity twin of the refutation: same premises, opposite verdict; MUST FAIL (else the refutation's premises are unsatisfiable)
#[requires(wf(*d))]
#[requires((*d).ep == None)]
#[ensures(!(result == Out::Panic))]
pub fn sanity_refute_dm_recover_extent_no_eviction_policy_error(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-FRAME
#[ensures(others_same(^d, *d, key))]
pub fn verify_dm_recover_extent_frame(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-RECOVER-EXTENT-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(forall<j: u64> (^d).entries@.get(j) == None)]
pub fn verify_dm_recover_extent_frame__mutant(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    dm_recover_extent(d, key, offset, size_blocks)
}

// ---- DM-SET-CHECKSUM-RECORDS
#[requires(checksum@ != 0)]
#[ensures(result.0 == Ok(()) ==> result.1 == Some(checksum))]
pub fn verify_dm_set_checksum_records(d: &mut Dm, key: u64, checksum: u32) -> (Result<(), DmError>, Option<u32>) {
    let r = dm_set_checksum(d, key, checksum);
    let g = dm_get_checksum(d, key);
    (r, g)
}

// ---- DM-SET-CHECKSUM-RECORDS — anti-vacuity twin, MUST FAIL
#[requires(checksum@ != 0)]
#[ensures(result.0 == Ok(()) ==> result.1 == None)]
pub fn verify_dm_set_checksum_records__mutant(d: &mut Dm, key: u64, checksum: u32) -> (Result<(), DmError>, Option<u32>) {
    let r = dm_set_checksum(d, key, checksum);
    let g = dm_get_checksum(d, key);
    (r, g)
}

// ---- DM-SET-CHECKSUM-NOT-FOUND
#[requires(!has(*d, key))]
#[ensures(result == Err(DmError::KeyNotFound(key)) && !has(^d, key))]
pub fn verify_dm_set_checksum_not_found(d: &mut Dm, key: u64, checksum: u32) -> Result<(), DmError> {
    dm_set_checksum(d, key, checksum)
}

// ---- DM-SET-CHECKSUM-NOT-FOUND — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result == Ok(()))]
pub fn verify_dm_set_checksum_not_found__mutant(d: &mut Dm, key: u64, checksum: u32) -> Result<(), DmError> {
    dm_set_checksum(d, key, checksum)
}

// ---- DM-SET-CHECKSUM-FRAME
#[ensures(others_same(^d, *d, key) && rest_same(*d, ^d))]
#[ensures(has(*d, key) ==> has(^d, key) && ent(^d, key).location == ent(*d, key).location && ent(^d, key).size_blocks == ent(*d, key).size_blocks && ent(^d, key).read_ref == ent(*d, key).read_ref && ent(^d, key).write_ref == ent(*d, key).write_ref && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle)]
pub fn verify_dm_set_checksum_frame(d: &mut Dm, key: u64, checksum: u32) -> Result<(), DmError> {
    dm_set_checksum(d, key, checksum)
}

// ---- DM-SET-CHECKSUM-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1)]
pub fn verify_dm_set_checksum_frame__mutant(d: &mut Dm, key: u64, checksum: u32) -> Result<(), DmError> {
    dm_set_checksum(d, key, checksum)
}

// ---- DM-GET-CHECKSUM-NONE-UNSET
#[requires(a_fr020(*d))]
#[ensures(result.0 ==> result.1 == None)]
pub fn verify_dm_get_checksum_none_unset(d: &mut Dm, which: Adder) -> (bool, Option<u32>) {
    match which {
        Adder::Create(key, pointer, size) => { let r = dm_create_memory_tier_entry(d, key, pointer, size); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }
        Adder::Recover(key, offset, sb) => { let r = dm_recover_extent(d, key, offset, sb); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }
        Adder::Init(key) => { let r = dm_initialize(d); let ok = match r { Out::Ret(Ok(())) => reported_prog(d, key), _ => false }; (ok, dm_get_checksum(d, key)) }
    }
}

// ---- DM-GET-CHECKSUM-NONE-UNSET — anti-vacuity twin, MUST FAIL
#[requires(a_fr020(*d))]
#[ensures(result.0 ==> result.1 != None)]
pub fn verify_dm_get_checksum_none_unset__mutant(d: &mut Dm, which: Adder) -> (bool, Option<u32>) {
    match which {
        Adder::Create(key, pointer, size) => { let r = dm_create_memory_tier_entry(d, key, pointer, size); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }
        Adder::Recover(key, offset, sb) => { let r = dm_recover_extent(d, key, offset, sb); let ok = match r { Out::Ret(Ok(())) => true, _ => false }; (ok, dm_get_checksum(d, key)) }
        Adder::Init(key) => { let r = dm_initialize(d); let ok = match r { Out::Ret(Ok(())) => reported_prog(d, key), _ => false }; (ok, dm_get_checksum(d, key)) }
    }
}

// ---- DM-GET-CHECKSUM-ZERO-IS-NONE
#[ensures(has(*d, key) && ent(*d, key).checksum@ == 0 ==> result == None)]
#[ensures(has(*d, key) && ent(*d, key).checksum@ != 0 ==> result == Some(ent(*d, key).checksum))]
pub fn verify_dm_get_checksum_zero_is_none(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-GET-CHECKSUM-ZERO-IS-NONE — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) && ent(*d, key).checksum@ == 0 ==> result == Some(ent(*d, key).checksum))]
pub fn verify_dm_get_checksum_zero_is_none__mutant(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-GET-CHECKSUM-ABSENT-KEY
#[requires(!has(*d, key))]
#[ensures(result == None)]
pub fn verify_dm_get_checksum_absent_key(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-GET-CHECKSUM-ABSENT-KEY — anti-vacuity twin, MUST FAIL
#[requires(!has(*d, key))]
#[ensures(result != None)]
pub fn verify_dm_get_checksum_absent_key__mutant(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-GET-CHECKSUM-FRAME
#[ensures(^d == *d)]
pub fn verify_dm_get_checksum_frame(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-GET-CHECKSUM-FRAME — anti-vacuity twin, MUST FAIL
#[ensures(has(*d, key) ==> !has(^d, key))]
pub fn verify_dm_get_checksum_frame__mutant(d: &mut Dm, key: u64) -> Option<u32> {
    dm_get_checksum(d, key)
}

// ---- DM-INV-RELEASE-WAKES-WAITERS
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && (ent(^d, k).write_ref@ < ent(*result.1, k).write_ref@ || ent(^d, k).read_ref@ < ent(*result.1, k).read_ref@) ==> *(^d).cv.epoch > *(*result.1).cv.epoch)]
pub fn verify_dm_inv_release_wakes_waiters(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-RELEASE-WAKES-WAITERS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) ==> *(^d).cv.epoch > *(*result.1).cv.epoch)]
pub fn verify_dm_inv_release_wakes_waiters__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-WAIT-TIMES-OUT-ONLY-IF-BLOCKED
#[ensures(result.0 ==> !pred_holds(*result.1, key, pw(which)))]
#[ensures(pred_holds(*d, key, pw(which)) ==> !result.0)]
pub fn verify_dm_inv_wait_times_out_only_if_blocked(d: &mut Dm, key: u64, which: u8) -> (bool, Snapshot<Dm>) {
    if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
    else { let (r, s) = dm_take_write(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
}

// ---- DM-INV-WAIT-TIMES-OUT-ONLY-IF-BLOCKED — anti-vacuity twin, MUST FAIL
#[ensures(pred_holds(*d, key, pw(which)) ==> result.0)]
pub fn verify_dm_inv_wait_times_out_only_if_blocked__mutant(d: &mut Dm, key: u64, which: u8) -> (bool, Snapshot<Dm>) {
    if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
    else { let (r, s) = dm_take_write(d, key); (match r { Err(DmError::Timeout(_)) => true, _ => false }, s) }
}

// ---- DM-INV-WRITE-REF-AT-MOST-ONE
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> ent(^d, k).write_ref@ <= 1)]
pub fn verify_dm_inv_write_ref_at_most_one(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-WRITE-REF-AT-MOST-ONE — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> ent(^d, k).write_ref@ == 0)]
pub fn verify_dm_inv_write_ref_at_most_one__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-READ-WRITE-EXCLUSIVE
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> ent(^d, k).write_ref@ != 0 ==> ent(^d, k).read_ref@ == 0)]
pub fn verify_dm_inv_read_write_exclusive(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-READ-WRITE-EXCLUSIVE — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> ent(^d, k).read_ref@ == 0)]
pub fn verify_dm_inv_read_write_exclusive__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-MEMORY-TIER-SIZE-NONZERO
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> match ent(^d, k).location { Location::MemoryTier { size, .. } => size@ > 0, Location::BlockDevice { .. } => true })]
pub fn verify_dm_inv_memory_tier_size_nonzero(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-MEMORY-TIER-SIZE-NONZERO — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> match ent(^d, k).location { Location::MemoryTier { size, .. } => size@ > 1, Location::BlockDevice { .. } => true })]
pub fn verify_dm_inv_memory_tier_size_nonzero__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-MEMORY-TIER-SIZE-BLOCKS-CONSISTENT
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> match ent(^d, k).location { Location::MemoryTier { size, .. } => ent(^d, k).size_blocks@ == (size@ + 4095) / 4096, Location::BlockDevice { .. } => true })]
pub fn verify_dm_inv_memory_tier_size_blocks_consistent(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-MEMORY-TIER-SIZE-BLOCKS-CONSISTENT — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> match ent(^d, k).location { Location::MemoryTier { size, .. } => ent(^d, k).size_blocks@ == size@ / 4096, Location::BlockDevice { .. } => true })]
pub fn verify_dm_inv_memory_tier_size_blocks_consistent__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-REFCOUNT-CONSERVATION
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == ent(*result.1, k).read_ref@ + delta(op, result.0, k, ent(*result.1, k).read_ref@))]
#[ensures(forall<k: u64> !has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == 0)]
#[ensures(forall<k: u64> has(*result.1, k) && !has(^d, k) ==> ent(*result.1, k).read_ref@ == 0)]
pub fn verify_dm_inv_refcount_conservation(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-REFCOUNT-CONSERVATION — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) ==> ent(^d, k).read_ref@ == ent(*result.1, k).read_ref@)]
pub fn verify_dm_inv_refcount_conservation__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-ENTRY-TRACKED-FOR-EVICTION
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> match (^d).pool_id { Some(p) => trk(^d).get(ent(^d, k).eviction_handle) == Some((p, k)), None => false })]
#[ensures(match (^d).pool_id { Some(p) => forall<h: Handle> trk(^d).contains(h) && (trk(^d).lookup(h)).0 == p ==> has(^d, (trk(^d).lookup(h)).1), None => true })]
pub fn verify_dm_inv_entry_tracked_for_eviction(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-ENTRY-TRACKED-FOR-EVICTION — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(^d, k) ==> !trk(^d).contains(ent(^d, k).eviction_handle))]
pub fn verify_dm_inv_entry_tracked_for_eviction__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-CHECKSUM-SURVIVES-TRANSITIONS
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) != is_mt(ent(^d, k).location) ==> ent(^d, k).checksum == ent(*result.1, k).checksum)]
pub fn verify_dm_inv_checksum_survives_transitions(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-CHECKSUM-SURVIVES-TRANSITIONS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) != is_mt(ent(^d, k).location) ==> ent(^d, k).checksum != ent(*result.1, k).checksum)]
pub fn verify_dm_inv_checksum_survives_transitions__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INITIALIZE-EXPLICIT-ONLY
#[ensures(forall<k: u64> !has(result, k))]
pub fn verify_dm_initialize_explicit_only(ep: Option<Ep>, em: Option<Em>, cv: Cv) -> Dm {
    dm_new(ep, em, cv)
}

// ---- DM-INITIALIZE-EXPLICIT-ONLY — anti-vacuity twin, MUST FAIL
#[ensures(exists<k: u64> has(result, k))]
pub fn verify_dm_initialize_explicit_only__mutant(ep: Option<Ep>, em: Option<Em>, cv: Cv) -> Dm {
    dm_new(ep, em, cv)
}

// ---- DM-INV-SINGLE-WRITER
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && ent(*result.1, k).write_ref@ != 0 && has(^d, k) && ent(^d, k).write_ref@ == 0 ==> releases_write(op, k))]
#[ensures(match op { Op::TakeWrite(k) => result.0 == St::Ok ==> has(*result.1, k) && ent(*result.1, k).write_ref@ == 0 && ent(^d, k).write_ref@ == 1, _ => true })]
pub fn verify_dm_inv_single_writer(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-SINGLE-WRITER — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(match op { Op::TakeWrite(k) => result.0 == St::Ok ==> ent(*result.1, k).write_ref@ == 1, _ => true })]
pub fn verify_dm_inv_single_writer__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-WAIT-THEN-ACT-ATOMIC
#[ensures(result.0 ==> pred_holds(*result.1, key, pw(which)) && has(*result.1, key) && has(^d, key))]
#[ensures(result.0 && which@ < 2 ==> ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1 && ent(^d, key).write_ref@ == 0)]
#[ensures(result.0 && which@ >= 2 ==> ent(^d, key).write_ref@ == 1 && ent(^d, key).read_ref@ == 0)]
pub fn verify_dm_inv_wait_then_act_atomic(d: &mut Dm, key: u64, which: u8) -> (bool, Snapshot<Dm>) {
    if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Ok(LookupResult::NotExist) => false, Ok(_) => true, _ => false }, s) }
    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Ok(()) => true, _ => false }, s) }
    else { let (r, s) = dm_take_write(d, key); (match r { Ok(()) => true, _ => false }, s) }
}

// ---- DM-INV-WAIT-THEN-ACT-ATOMIC — anti-vacuity twin, MUST FAIL
#[ensures(result.0 && which@ >= 2 ==> ent(*result.1, key).read_ref@ > 0)]
pub fn verify_dm_inv_wait_then_act_atomic__mutant(d: &mut Dm, key: u64, which: u8) -> (bool, Snapshot<Dm>) {
    if which == 0 { let (r, s) = dm_lookup(d, key); (match r { Ok(LookupResult::NotExist) => false, Ok(_) => true, _ => false }, s) }
    else if which == 1 { let (r, s) = dm_take_read(d, key); (match r { Ok(()) => true, _ => false }, s) }
    else { let (r, s) = dm_take_write(d, key); (match r { Ok(()) => true, _ => false }, s) }
}

// ---- DM-INV-ERROR-LEAVES-STATE-UNCHANGED
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(result.0 == St::Err ==> same_map(^d, *result.1))]
pub fn verify_dm_inv_error_leaves_state_unchanged(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-ERROR-LEAVES-STATE-UNCHANGED — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(result.0 == St::Err ==> !same_map(^d, *result.1))]
pub fn verify_dm_inv_error_leaves_state_unchanged__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-SINGLE-POOL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(match (*result.1).pool_id { Some(p) => (^d).pool_id == Some(p) && npools(^d) == npools(*result.1), None => true })]
#[ensures(forall<h: Handle> trk(^d).contains(h) && !trk(*result.1).contains(h) ==> Some((trk(^d).lookup(h)).0) == (^d).pool_id)]
pub fn verify_dm_inv_single_pool(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-SINGLE-POOL — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(match (*result.1).pool_id { Some(p) => npools(^d) > npools(*result.1), None => true })]
pub fn verify_dm_inv_single_pool__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-SSD-OFFSET-STICKY
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) && ssd_of(ent(*result.1, k).location) != None && is_mt(ent(^d, k).location) ==> ssd_of(ent(^d, k).location) != None)]
pub fn verify_dm_inv_ssd_offset_sticky(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-SSD-OFFSET-STICKY — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(^d, k).location) ==> ssd_of(ent(^d, k).location) != None)]
pub fn verify_dm_inv_ssd_offset_sticky__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-LEGAL-LOCATION-TRANSITIONS
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && is_mt(ent(*result.1, k).location) && !is_mt(ent(^d, k).location) ==> ssd_of(ent(*result.1, k).location) != None && Some(ent(^d, k).location) == ssd_of(ent(*result.1, k).location).map_logic(|o| Location::BlockDevice { offset: o }) && demotes(op, k))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && !is_mt(ent(*result.1, k).location) && is_mt(ent(^d, k).location) ==> promotes(op, k))]
pub fn verify_dm_inv_legal_location_transitions(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}

// ---- DM-INV-LEGAL-LOCATION-TRANSITIONS — anti-vacuity twin, MUST FAIL
#[requires(wf(*d))]
#[requires(op_assume(*d, op))]
#[ensures(forall<k: u64> has(*result.1, k) && has(^d, k) && !is_mt(ent(*result.1, k).location) && is_mt(ent(^d, k).location) ==> !promotes(op, k))]
pub fn verify_dm_inv_legal_location_transitions__mutant(d: &mut Dm, op: Op) -> (St, Snapshot<Dm>) {
    step!(d, op)
}
