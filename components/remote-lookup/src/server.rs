//! Server role: answering peers' KEY_QUERYs and serving their RDMA_REQUESTs.
//!
//! On a KEY_QUERY the actor classifies each `(key, size)` against the
//! dispatch-map (memory / disk / not-available, FR-015) and whispers a
//! KEY_RESPONSE. On an RDMA_REQUEST it pins each value, delegates the write to
//! `IRemoteLookupRdmaInitiator::push`, and whispers per-key RDMA_STATUS.
//! (Disk→memory promotion via `IDispatcher::promote_to_memory_tier` is US4,
//! deferred; a disk-only key reports `KeyNoLongerAvailable` for now.)
//!
//! These are pure functions over the local receptacles — they compute the wire
//! response but do not touch zyre; the actor performs the whisper. A node acts
//! as a server for a peer's operation while simultaneously running its own
//! `batch_lookup`s as a client (the dual-role concurrency the mesh tests
//! exercise).

use std::sync::Arc;

use interfaces::{
    CacheKey, Endpoint, IDispatchMap, IDispatcher, IRemoteLookupRdmaInitiator, LookupResult,
    PushStatus, RemoteRegion,
};

use crate::wire::{Avail, RdmaStatusCode, SlotDesc};

/// Classify each queried `(key, size)` against the local dispatch map (US2,
/// FR-015): memory-resident at the requested size ⇒ [`Avail::Memory`], on the
/// block/disk tier at the requested size ⇒ [`Avail::Disk`], otherwise
/// [`Avail::None`] (absent or size-mismatched — treated as a miss, see
/// `knowledge/size-mismatch-handling.md`).
///
/// Each `lookup` hit pins a read reference; this releases it immediately, since
/// classification does not serve data.
pub(crate) fn classify_query(
    dispatch_map: &Arc<dyn IDispatchMap + Send + Sync>,
    entries: &[(CacheKey, u32)],
) -> Vec<(CacheKey, u32, Avail)> {
    entries
        .iter()
        .map(|&(key, size)| {
            let avail = match dispatch_map.lookup(key) {
                Ok(LookupResult::MemoryTier { size: stored, .. }) => {
                    let _ = dispatch_map.release_read(key);
                    if stored == size {
                        Avail::Memory
                    } else {
                        Avail::None
                    }
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    let stored = dispatch_map.entry_size(key).ok();
                    let _ = dispatch_map.release_read(key);
                    if stored == Some(size) {
                        Avail::Disk
                    } else {
                        Avail::None
                    }
                }
                // NotExist / MismatchSize / error: no pin taken, nothing held.
                _ => Avail::None,
            };
            (key, size, avail)
        })
        .collect()
}

/// Serve a peer's RDMA_REQUEST (US3, FR-016): for each requested slot, pin the
/// value, delegate the one-sided write to `initiator.push`, and map the per-key
/// [`PushStatus`] to an [`RdmaStatusCode`]. A `BlockDevice` key is first promoted
/// to the memory tier via `dispatcher.promote_to_memory_tier` and re-looked-up
/// (US4, FR-016/FR-017); a key that is still not memory-resident at the requested
/// size (evicted, size-mismatched, or promotion failed) reports
/// [`RdmaStatusCode::KeyNoLongerAvailable`]. Pins are released after the push.
///
/// `requester_endpoint`/`rkey` come from the RDMA_REQUEST: the serving peer's
/// initiator connects to the requester's responder and writes into its pool.
pub(crate) fn serve_rdma_request(
    dispatch_map: &Arc<dyn IDispatchMap + Send + Sync>,
    dispatcher: Option<&Arc<dyn IDispatcher + Send + Sync>>,
    initiator: &Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
    requester_endpoint: &Endpoint,
    rkey: u32,
    slots: &[SlotDesc],
) -> Vec<(CacheKey, RdmaStatusCode)> {
    let mut statuses: Vec<(CacheKey, RdmaStatusCode)> = Vec::with_capacity(slots.len());
    let mut pinned: Vec<CacheKey> = Vec::new();
    let mut to_push: Vec<(CacheKey, RemoteRegion)> = Vec::new();

    // Resolve a slot to a memory-tier hit at the requested size (promoting from
    // disk if needed). On a servable hit the returned pin is still held (added to
    // `pinned`); every other outcome releases its pin before returning.
    let resolve = |key: CacheKey, length: u32| -> bool {
        match dispatch_map.lookup(key) {
            Ok(LookupResult::MemoryTier { size, .. }) if size == length => true,
            Ok(LookupResult::MemoryTier { .. }) => {
                let _ = dispatch_map.release_read(key);
                false
            }
            Ok(LookupResult::BlockDevice { .. }) => {
                // US4: try to promote disk → memory, then re-check.
                let _ = dispatch_map.release_read(key);
                let Some(disp) = dispatcher else {
                    return false;
                };
                disp.promote_to_memory_tier(std::slice::from_ref(&key));
                match dispatch_map.lookup(key) {
                    Ok(LookupResult::MemoryTier { size, .. }) if size == length => true,
                    Ok(LookupResult::MemoryTier { .. }) | Ok(LookupResult::BlockDevice { .. }) => {
                        let _ = dispatch_map.release_read(key);
                        false
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    };

    for slot in slots {
        if resolve(slot.key, slot.length) {
            pinned.push(slot.key);
            to_push.push((
                slot.key,
                RemoteRegion {
                    addr: slot.addr,
                    rkey,
                    length: slot.length,
                },
            ));
        } else {
            statuses.push((slot.key, RdmaStatusCode::KeyNoLongerAvailable));
        }
    }

    if !to_push.is_empty() {
        let endpoint = format!("{}:{}", requester_endpoint.ip, requester_endpoint.port);
        match initiator.push(&endpoint, &to_push) {
            Ok(results) => {
                for ((key, _), status) in to_push.iter().zip(results) {
                    statuses.push((*key, map_push_status(status)));
                }
            }
            Err(_) => {
                // A method-level failure applies to the whole batch.
                for (key, _) in &to_push {
                    statuses.push((*key, RdmaStatusCode::UnableToConnect));
                }
            }
        }
    }

    for key in pinned {
        let _ = dispatch_map.release_read(key);
    }

    statuses
}

/// Map an initiator [`PushStatus`] to the wire [`RdmaStatusCode`] (FR-016):
/// `KeyNotFound`/`SizeMismatch` both fold to `KeyNoLongerAvailable`.
fn map_push_status(status: PushStatus) -> RdmaStatusCode {
    match status {
        PushStatus::Success => RdmaStatusCode::Success,
        PushStatus::UnableToConnect => RdmaStatusCode::UnableToConnect,
        PushStatus::KeyNotFound | PushStatus::SizeMismatch => RdmaStatusCode::KeyNoLongerAvailable,
    }
}
