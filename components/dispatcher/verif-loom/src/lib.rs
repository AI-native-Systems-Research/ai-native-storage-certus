//! Loom mirror of the concurrency-relevant parts of the `dispatcher` component.
//!
//! * [`dmap`] mirrors the dispatch map the dispatcher drives through its
//!   `IDispatchMap` receptacle (`components/dispatch-map/src/lib.rs`, `state.rs`):
//!   one `Mutex<HashMap>` + `Condvar`, read/write references, and the LRU
//!   eviction-policy order (a separate lock, as `ep.touch` runs after the map
//!   lock is dropped).
//! * [`dispatcher`] mirrors `DispatcherComponent` methods in `src/lib.rs`
//!   (lookup, populate, reserve_memory, batch_lookup, remove, clear_memory_tier,
//!   flush_to_ssd, the eviction helpers and the write-through job with its
//!   `WriteThroughPins` coordination with remove).
//! * [`background`] mirrors `src/background.rs` (the per-drive background writer
//!   and one pass of the memory-tier and SSD evictor loops).
//! * [`stand`] holds deterministic stand-ins for what is not concurrency logic:
//!   crossbeam channels, the memory tier (slot pool + LRU), the extent manager /
//!   NVMe drive and the GPU DMA (a slot copy into a local value).
//!
//! `std::sync` / `std::thread` are replaced by `loom::sync` / `loom::thread`.
//! Wall-clock elements are abstracted: the dispatch-map 2 s wait timeout and the
//! 5 s concurrent-promotion timeout never fire, `thread::sleep` polls become
//! `thread::yield_now`, and `recv_timeout(50ms)` is a blocking receive that ends
//! when the channel is closed. Logging and statistics-only counters are dropped.
//!
//! Every mirrored function carries a `// mirrors src/<file>:<line>` comment
//! (paths relative to `components/dispatcher/`; dispatch-map paths relative to
//! `components/`). `mirrored_sources.sha256` pins the source files.
//!
//! Each mirror takes a [`Mutation`]. `Mutation::None` is the faithful mirror;
//! every other variant changes one requirement-relevant step, marked
//! `// MUTANT(<variant>)`, and is used only by the `__mutant` twin tests.

pub mod background;
pub mod dispatcher;
pub mod dmap;
pub mod stand;

/// Cache key (`interfaces::CacheKey`).
pub type CacheKey = u64;

/// Single-step deviations used by the mutant twins. `None` = faithful mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    /// Faithful mirror of the shipped code.
    None,
    /// LOOKUP-CONCURRENT-READERS twin: `dm.lookup` waits for `read_ref == 0`
    /// too, i.e. readers exclude each other.
    LookupExclusive,
    /// LOOKUP-NO-DATA-CORRUPTION twin: populate registers the entry and enqueues
    /// the write-through before the GPU→DRAM copy has landed.
    PublishBeforeCopy,
    /// RESERVE-MEMORY-BACKPRESSURE twin: `reserve_memory` returns the first
    /// `AllocationFailed` instead of retrying while the deadline is open.
    ReserveFailFast,
    /// BATCH-LOOKUP-SERVEDBY twins: the concurrent-promotion recovery rewrites
    /// the tier too, but to `Remote` (wrong route).
    RecoveryWrongTier,
    /// REMOVE-NO-WAIT twin: `dm.remove` blocks until the key's references drain.
    RemoveWaitsForRefs,
    /// REMOVE-ORDER twin: remove never frees the memory-tier slot.
    RemoveSkipsSlotFree,
    /// REMOVE-RESOURCES-REUSABLE twin: remove never frees the SSD extent of a
    /// block-device entry.
    RemoveSkipsExtentFree,
    /// REMOVE-NO-FOREIGN-PIN-RELEASE twin: remove releases its lookup's read
    /// reference twice; the second release drops one it does not own.
    RemoveDoubleRelease,
    /// REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED twin: `dm.remove` drops the entry
    /// although references are still held, so a remove that must be refused
    /// goes through and frees the resources underneath the holder.
    RemoveIgnoresRefs,
    /// The writer decrements `in_flight` before it processes the job. No longer
    /// used by a twin: since clear_memory_tier retries pinned entries (cccfb328e)
    /// the early decrement cannot make clear drop an entry.
    FlushEarlyDecrement,
    /// FLUSH-THEN-CLEAR twin: a successful write-through releases its pin
    /// without recording the SSD offset (`convert_to_storage` skipped).
    SettleSkipsConvert,
    /// EVICTION-EVENTS twin: the memory-tier evictor tags events `Removed`.
    EvictorWrongReason,
    /// MT-EVICTOR-WATERMARK twin: the evictor stops after the first demotion.
    MtEvictorStopAfterOne,
    /// SSD-EVICTOR-WATERMARK twin: the evictor stops after the first eviction.
    SsdEvictorStopAfterOne,
    /// CLEAR-MT-KEEPS-PINNED twin: clear_memory_tier frees the slot before it
    /// even tries to demote the entry.
    ClearFreesFirst,
    /// FLUSH-REPORTS-FAILED-WRITE twin: flush_to_ssd returns without waiting
    /// for the write queue to drain.
    FlushNoWait,
    /// EVICTOR-NO-FOREIGN-PIN-RELEASE twin: the SSD evictor releases a read
    /// reference even when its candidate lookup answered NotExist (and so took
    /// none), dropping the write-through pin of a concurrently registered
    /// entry (the defect class of spec-sync A-11, 2026-10-23).
    EvictorReleasesForeign,
}
