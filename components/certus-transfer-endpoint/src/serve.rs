//! Source side: per-entry lookup, pin, push and outcome accumulation.
//!
//! # Pin discipline (Principle IX — review this module against it)
//!
//! One [`IDispatchMap::lookup`] yields residency, address and pin together
//! (research D6). The dispatch map increments `read_ref` *before* it reads the
//! location, so its answers are:
//!
//! | answer | pinned? | this module |
//! |---|---|---|
//! | `NotExist` | no | `Miss`; **release nothing** |
//! | `MismatchSize` | no | `Miss`; **release nothing** (not returned by the current map, kept in case) |
//! | `MemoryTier`, size matches, inside the pool | **yes** | post a write; the pin travels in the slab and is released by the reaper that owns its `wr_id` |
//! | `MemoryTier`, wrong size | **yes** | `Miss`; **release it now** |
//! | `MemoryTier`, outside the pool | **yes** | `Failed` (map and tier disagree); **release it now** |
//! | `BlockDevice`, a block source bound (`spdk`) | **yes** | read it: the pin waits in the read table while the dispatcher reads the extent into a bounce buffer, and is released when the read finishes — *before* the write is posted; the slab then carries the buffer |
//! | `BlockDevice`, no block source | **yes** | `Miss`; **release it now** |
//! | `Err(..)` (e.g. a 2 s writer timeout) | no | `Failed`; release nothing |
//!
//! The rule is *release exactly what `lookup` pinned, exactly once*. The wrong-
//! size row is the one easy to get backwards: `lookup` takes no size, so a
//! mismatch comes back pinned and must be released, unlike `MismatchSize`.
//!
//! Every pin this module holds is a [`Pin`], whose only way out is
//! [`Pin::release`] — dropping one unreleased panics, so a leaked pin is loud.
//!
//! # Known gaps, accepted for now
//!
//! * **Tier neutrality is not met for memory-tier serving.** `lookup` also
//!   refreshes the entry's LRU position, so serving a peer reorders the
//!   source's cache (invariant 6 / SC-005). No `IDispatchMap` call pins and
//!   returns the location without that touch; a non-touching lookup is the
//!   tracked fix.
//! * **`lookup` blocks up to 2 s** while a writer holds the entry, on the
//!   endpoint thread. Rare and bounded; a timeout reports the entry `Failed`.
//!
//! This path must not phase-barrier: writes post as soon as they are resolved,
//! and the send queue is never drained between entries while work remains
//! (FR-022, D19).

use std::collections::VecDeque;
use std::sync::Arc;

use interfaces::{
    BounceBuffer, CacheKey, EntryOutcome, FetchCompletion, IDispatchMap, LookupResult, RemoteRegion,
};

use crate::connection::transport::SendOp;
use crate::connection::PeerId;

/// A read reference obtained from one `lookup`. Must be released exactly once,
/// through [`Pin::release`]; dropping it unreleased is a defect and panics.
pub(crate) struct Pin {
    key: CacheKey,
    released: bool,
}

impl Pin {
    /// Record a pin `lookup` just took. Only `resolve` constructs these.
    fn taken(key: CacheKey) -> Self {
        Pin {
            key,
            released: false,
        }
    }

    /// Release the read reference. A failure means the ledger is already
    /// wrong — someone else released this pin — so it is fatal.
    pub(crate) fn release(mut self, map: &dyn IDispatchMap) {
        self.released = true;
        if let Err(e) = map.release_read(self.key) {
            panic!(
                "release_read({}) failed: {e}; the pin ledger is corrupt",
                self.key
            );
        }
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if !self.released && !std::thread::panicking() {
            panic!(
                "pin on key {} dropped without release: it would never be released",
                self.key
            );
        }
    }
}

/// The local memory-tier pool, as registered with the transport. Lookups
/// answer with pointers into it; one outside it is refused rather than
/// written from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pool {
    pub(crate) addr: u64,
    pub(crate) len: usize,
}

impl Pool {
    fn contains(&self, addr: u64, len: usize) -> bool {
        addr.checked_sub(self.addr)
            .and_then(|off| off.checked_add(len as u64))
            .is_some_and(|end| end <= self.len as u64)
    }
}

/// One entry ready to post: the write and the pin it carries.
pub(crate) struct Write {
    pub(crate) index: usize,
    pub(crate) local: u64,
    pub(crate) len: u32,
    pub(crate) op: SendOp,
    pub(crate) pin: Pin,
}

/// A block-tier entry to read before it can be written: its pin, held until
/// the read finishes.
pub(crate) struct BlockRead {
    pub(crate) index: usize,
    pub(crate) key: CacheKey,
    pub(crate) offset: u64,
    pub(crate) region: RemoteRegion,
    pub(crate) pin: Pin,
    /// The readers' free count when the read was submitted (see
    /// `Readers::mark_full`).
    pub(crate) frees_at: u64,
}

/// What one lookup resolved an entry to.
pub(crate) enum Resolved {
    Write(Write),
    /// Block tier, with a block source to read it: read, then write.
    Block(BlockRead),
    Miss,
    /// The lookup itself failed: not a statement about residency.
    Failed,
}

/// Why an entry became a miss without a write, for counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissCause {
    Absent,
    WrongSize,
    OutsidePool,
    BlockTier,
    LookupFailed,
}

/// Resolve one entry with **one** `lookup`, applying the module's pin table.
/// `blocks` says whether a block source is bound: without one, a block-tier
/// answer is a miss (D14).
pub(crate) fn resolve(
    map: &dyn IDispatchMap,
    pool: Pool,
    blocks: bool,
    index: usize,
    key: CacheKey,
    region: RemoteRegion,
) -> (Resolved, Option<MissCause>) {
    match map.lookup(key) {
        Ok(LookupResult::NotExist) => (Resolved::Miss, Some(MissCause::Absent)),
        Ok(LookupResult::MismatchSize) => (Resolved::Miss, Some(MissCause::WrongSize)),
        Ok(LookupResult::BlockDevice { offset }) if blocks => (
            Resolved::Block(BlockRead {
                index,
                key,
                offset,
                region,
                pin: Pin::taken(key),
                frees_at: 0,
            }),
            None,
        ),
        Ok(LookupResult::BlockDevice { .. }) => {
            Pin::taken(key).release(map);
            (Resolved::Miss, Some(MissCause::BlockTier))
        }
        Ok(LookupResult::MemoryTier { pointer, size }) => {
            let pin = Pin::taken(key);
            if size != region.length {
                pin.release(map);
                return (Resolved::Miss, Some(MissCause::WrongSize));
            }
            // The map and the tier disagree about where the pool is: a fault,
            // not residency, so it must not read as a cold cache.
            if !pool.contains(pointer as u64, size as usize) {
                pin.release(map);
                return (Resolved::Failed, Some(MissCause::OutsidePool));
            }
            (
                Resolved::Write(Write {
                    index,
                    local: pointer as u64,
                    len: size,
                    op: SendOp::RdmaWrite {
                        remote_addr: region.addr,
                        rkey: region.rkey,
                    },
                    pin,
                }),
                None,
            )
        }
        Err(_) => (Resolved::Failed, Some(MissCause::LookupFailed)),
    }
}

/// What keeps a posted write's source bytes from changing under it.
pub(crate) enum Hold {
    /// A memory-tier entry: its pin, released when the write completes.
    Pin(Pin),
    /// A block-tier entry, already read: the bounce buffer it was read into,
    /// returned when the write completes. Its pin was released after the read.
    Buffer(BounceBuffer),
}

/// What a posted write carries in its slab slot until the reaper frees it.
pub(crate) struct PushWork {
    pub(crate) push: u64,
    pub(crate) index: usize,
    pub(crate) hold: Hold,
}

/// Who a finished push reports to.
pub(crate) enum Finish {
    /// A local caller's `push_async`: fire its callback.
    Callback(FetchCompletion),
    /// A peer's `OP_FETCH`: reply to it, echoing `context_id`, once every
    /// write has completed — the reply ordering guarantee (T034).
    Reply { context_id: u64 },
}

/// One push in progress on the endpoint thread: a local `push_async`, or the
/// serving of a peer's `OP_FETCH`, which is the same work.
pub(crate) struct Push {
    pub(crate) peer: PeerId,
    /// Entries not yet looked up, in request order.
    pub(crate) pending: VecDeque<(usize, CacheKey, RemoteRegion)>,
    /// Block-tier entries put back for want of a free reader or bounce space,
    /// their pins released; looked up afresh when a reader or buffer frees.
    pub(crate) deferred: VecDeque<(usize, CacheKey, RemoteRegion)>,
    /// Block-tier entries read into a buffer and waiting for a send credit.
    pub(crate) ready: VecDeque<(usize, BounceBuffer, RemoteRegion)>,
    /// One outcome per item; `None` until decided.
    pub(crate) outcomes: Vec<Option<EntryOutcome>>,
    /// Writes posted and not yet completed.
    pub(crate) in_flight: usize,
    /// Block-tier reads handed out and not yet back.
    pub(crate) reading: usize,
    /// Its connection was torn down: reads still out are discarded on return.
    pub(crate) dead: bool,
    pub(crate) finish: Option<Finish>,
}

impl Push {
    pub(crate) fn new(peer: PeerId, items: Vec<(CacheKey, RemoteRegion)>, finish: Finish) -> Self {
        Push {
            peer,
            outcomes: vec![None; items.len()],
            pending: items
                .into_iter()
                .enumerate()
                .map(|(i, (k, r))| (i, k, r))
                .collect(),
            deferred: VecDeque::new(),
            ready: VecDeque::new(),
            in_flight: 0,
            reading: 0,
            dead: false,
            finish: Some(finish),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.pending.is_empty()
            && self.deferred.is_empty()
            && self.ready.is_empty()
            && self.in_flight == 0
            && self.reading == 0
    }

    /// Every outcome, and who to report them to — exactly once. Undecided
    /// entries — ones a failed connection never got to — are `Failed`:
    /// nothing arrived for them, and not because the source lacks them.
    pub(crate) fn finish(&mut self) -> (Vec<EntryOutcome>, Option<Finish>) {
        let outcomes: Vec<_> = self
            .outcomes
            .iter()
            .map(|o| o.unwrap_or(EntryOutcome::Failed))
            .collect();
        (outcomes, self.finish.take())
    }
}

/// The dispatch map, cloned out of its receptacle for the endpoint thread.
pub(crate) type Map = Arc<dyn IDispatchMap + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_doubles::LedgerMap;

    const POOL: Pool = Pool {
        addr: 0x1000,
        len: 0x1000,
    };

    fn region(len: u32) -> RemoteRegion {
        RemoteRegion {
            addr: 0x9000,
            rkey: 7,
            length: len,
        }
    }

    /// T026's ledger, at the resolve step: every kind of answer leaves every
    /// key's `read_ref` exactly where it started once the step is done, and
    /// the write's pin is held until released.
    #[test]
    fn every_answer_balances_the_pin_ledger() {
        let m = LedgerMap::new();
        m.memory(1, 0x1000, 64);
        m.memory(2, 0x1000, 32); // wrong size: comes back pinned
        m.memory(3, 0x9_0000, 64); // outside the pool: pinned
        m.block(4);
        m.timeout(5);

        let mut writes = Vec::new();
        let mut causes = Vec::new();
        for key in 0..=5u64 {
            let (r, cause) = resolve(&m, POOL, false, key as usize, key, region(64));
            causes.push(cause);
            if let Resolved::Write(w) = r {
                writes.push(w);
            }
        }
        assert_eq!(
            causes,
            [
                Some(MissCause::Absent),
                None,
                Some(MissCause::WrongSize),
                Some(MissCause::OutsidePool),
                Some(MissCause::BlockTier),
                Some(MissCause::LookupFailed),
            ]
        );
        assert_eq!(writes.len(), 1);
        assert_eq!(m.read_ref(1), 1, "the write's pin is held");
        for key in [0, 2, 3, 4, 5] {
            assert_eq!(m.read_ref(key), 0, "key {key} balanced");
        }
        writes.pop().unwrap().pin.release(&m);
        assert_eq!(m.read_ref(1), 0);
        assert_eq!(m.lookups(), 6, "exactly one lookup per entry");
    }

    /// `MismatchSize`, if a map returns it, is unpinned and must not be
    /// released — releasing it would decrement another reader's pin.
    #[test]
    fn mismatch_size_answer_releases_nothing() {
        let m = LedgerMap::new();
        m.mismatch(9);
        m.preload_reader(9); // someone else's pin
        let (r, cause) = resolve(&m, POOL, false, 0, 9, region(64));
        assert!(matches!(r, Resolved::Miss));
        assert_eq!(cause, Some(MissCause::WrongSize));
        assert_eq!(m.read_ref(9), 1, "the other reader's pin is untouched");
    }

    /// With a block source, a block-tier answer keeps its pin for the read —
    /// still one lookup — and hands back the offset to read.
    #[test]
    fn a_block_answer_with_a_source_keeps_its_pin_for_the_read() {
        let m = LedgerMap::new();
        m.block_at(4, 0x2000);
        let (r, cause) = resolve(&m, POOL, true, 3, 4, region(64));
        let Resolved::Block(b) = r else {
            panic!("expected a block read");
        };
        assert_eq!((b.index, b.key, b.offset, cause), (3, 4, 0x2000, None));
        assert_eq!(m.read_ref(4), 1, "pinned for the read");
        b.pin.release(&m);
        assert_eq!((m.read_ref(4), m.lookups()), (0, 1));
    }

    #[test]
    #[should_panic(expected = "dropped without release")]
    fn an_unreleased_pin_is_loud() {
        drop(Pin::taken(1));
    }

    #[test]
    fn pool_bounds() {
        assert!(POOL.contains(0x1000, 0x1000));
        assert!(!POOL.contains(0x1000, 0x1001));
        assert!(!POOL.contains(0xfff, 1));
        assert!(!POOL.contains(u64::MAX, 2));
    }

    #[test]
    fn finish_reports_once_and_fills_undecided_as_failed() {
        let mut p = Push::new(
            PeerId::parse("10.0.0.2:1").unwrap(),
            vec![(1, region(1)), (2, region(1))],
            Finish::Reply { context_id: 4 },
        );
        p.outcomes[0] = Some(EntryOutcome::HitMemoryTier);
        let (o, f) = p.finish();
        assert_eq!(o, [EntryOutcome::HitMemoryTier, EntryOutcome::Failed]);
        assert!(matches!(f, Some(Finish::Reply { context_id: 4 })));
        assert!(p.finish().1.is_none(), "reported exactly once");
    }
}
