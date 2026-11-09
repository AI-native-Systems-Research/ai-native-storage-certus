//! Symmetric RDMA endpoint for explicit certus-to-certus KV cache transfer.
//!
//! A destination reserves landing slots in its own memory tier, names a single
//! peer (chosen externally by llm-d), and asks that peer for a run of block
//! keys; the source RDMA-writes the values it holds directly into those slots
//! and replies with a per-key outcome. There is no broadcast, no peer polling
//! and no reap: exactly one peer is asked, exactly once.
//!
//! The endpoint is symmetric at the connection level — one queue pair per peer
//! *instance* pair, either end able to post work requests — but **not** at the
//! API level. A destination drives an explicit `fetch()`; a source is reactive,
//! answering inbound requests from its own thread, which is why there is
//! deliberately no `serve()` method.
//!
//! # Build configuration
//!
//! Nothing is enabled by default, and the default build is the one CI runs:
//!
//! | feature | adds |
//! |---|---|
//! | *(none)* | connection table, framing, credits and pin logic over an in-process seam |
//! | `rdma` | the real libibverbs/librdmacm transport |
//! | `spdk` | block-tier serving: a `dispatcher` receptacle that reads entries into the memory tier's bounce reserve |
//! | `telemetry` | extra metrics beyond `stats()` (which counts in every build) |
//!
//! Without `spdk` a block-tier lookup answer is reported as a miss rather than
//! an error (research D14). Hardware-dependent tests are `#[ignore]`d, so a
//! plain `cargo test` never needs a NIC or an NVMe device (constitution
//! Principle VIII).
//!
//! # Status
//!
//! Tasks T001–T083 of
//! `specs/001-rdma-cache-transfer/tasks.md` are complete: the crate builds as a
//! workspace default member, peer identity is implemented, and the verbs FFI, its
//! C shim and the safe wrappers over them exist behind `rdma`, and the transport
//! seam with its in-process mock, the connection table over it, and its per-link
//! `wr_id` slab, send-queue credits and counters exist in every build, owned by
//! [`CertusTransferEndpoint`] on its endpoint thread. Every transfer method works
//! end to end over the mock: the legacy `push_async` and `push`, landing
//! regions, and `fetch` over the request/reply messaging, with a fail-loud reply
//! deadline; and with `spdk`, block-tier entries are read by the dispatcher into
//! the memory tier's bounce reserve and served from there. A fetch of any length
//! is chunked against the peer's `max_entries`. An `rdma` build runs over the
//! verbs transport. Phase 11, the cutover, is out of scope here.

#![deny(missing_docs)]

mod connection;
mod endpoint;
mod message;
mod msgpool;
mod serve;
mod session;
mod telemetry;
#[cfg(test)]
mod test_doubles;

#[cfg(feature = "rdma")]
mod ffi;
#[cfg(feature = "rdma")]
mod rdma;

// In every build, so the read path is tested over a mock source (Principle
// VIII); only the dispatcher-backed source is `spdk`-gated.
mod bounce;

#[cfg(all(test, feature = "rdma"))]
mod loopback_test;

use std::sync::mpsc;

use component_core::actor::{Actor, ActorHandle};
use component_core::numa::CpuSet;
use interfaces::{
    CacheKey, EndpointConfig, EntryOutcome, FetchCompletion, ILogger, ITransferEndpoint,
    ITransferEndpointAdmin, LandingRegion, LocalSlot, RemoteRegion, TransferContextId,
    TransferEndpointError, TransferStats,
};

use crate::connection::table::ConnectionTable;
use crate::connection::transport::{select_transport, Access, QpCaps, TransportChoice};
use crate::connection::PeerId;
use crate::endpoint::{Command, EndpointActor};

/// The endpoint's lifecycle state. Public only because `define_component!`
/// makes every field public; it has no public constructor or variants, so
/// nothing outside this crate can read or change it.
pub struct Lifecycle(State);

impl Default for Lifecycle {
    fn default() -> Self {
        Lifecycle(State::Unconfigured)
    }
}

/// Where block-tier entries are read from.
type Source = std::sync::Arc<dyn bounce::BlockSource>;

/// Configured, then running, then stopped.
enum State {
    /// No configuration yet.
    Unconfigured,
    /// Configured; [`ITransferEndpointAdmin::initialize`] starts it. The
    /// transport choice is kept here so tests can substitute an isolated mock.
    Configured(EndpointConfig, Option<TransportChoice>, Option<Source>),
    /// The endpoint thread is running, landing fetches in its registered pool.
    Running(ActorHandle<Command>, PoolKey),
    /// Shut down; not restartable.
    Stopped,
}

/// The memory-tier pool as registered: where a fetch's slots must lie, and
/// the remote key a peer writes them with.
#[derive(Clone, Copy)]
struct PoolKey {
    addr: u64,
    len: u64,
    rkey: u32,
}

impl PoolKey {
    /// The region a peer writes `slot` with, if the slot lies wholly inside
    /// the pool.
    fn region(&self, slot: LocalSlot) -> Option<RemoteRegion> {
        let off = slot.addr.checked_sub(self.addr)?;
        (off.checked_add(slot.length as u64)? <= self.len).then_some(RemoteRegion {
            addr: slot.addr,
            rkey: self.rkey,
            length: slot.length,
        })
    }
}

/// A logger for when the `logger` receptacle is unbound: a missing logger is
/// never a reason for a transfer to fail.
struct NoopLogger;

impl ILogger for NoopLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// Forwards to the bound logger, so the endpoint thread owns a logger it can
/// keep for its whole life.
struct SharedLogger(std::sync::Arc<dyn ILogger + Send + Sync>);

impl ILogger for SharedLogger {
    fn error(&self, msg: &str) {
        self.0.error(msg)
    }
    fn warn(&self, msg: &str) {
        self.0.warn(msg)
    }
    fn info(&self, msg: &str) {
        self.0.info(msg)
    }
    fn debug(&self, msg: &str) {
        self.0.debug(msg)
    }
}

/// The component. Generated by `define_component!`, which emits undocumented
/// fields, so `missing_docs` is relaxed for this module alone; everything here
/// is re-exported and documented at the re-export.
#[allow(missing_docs)]
mod component {
    use std::sync::Mutex;

    use component_framework::define_component;
    #[cfg(feature = "spdk")]
    use interfaces::IDispatcher;
    use interfaces::{
        IDispatchMap, ILogger, IMemoryTier, ITransferEndpoint, ITransferEndpointAdmin,
    };

    use super::Lifecycle;

    #[cfg(not(feature = "spdk"))]
    define_component! {
        pub CertusTransferEndpoint {
            version: "0.1.0",
            provides: [ITransferEndpoint, ITransferEndpointAdmin],
            receptacles: {
                logger: ILogger,
                dispatch_map: IDispatchMap,
                memory_tier: IMemoryTier,
            },
            fields: {
                lifecycle: Mutex<Lifecycle>,
            },
        }
    }

    #[cfg(feature = "spdk")]
    define_component! {
        pub CertusTransferEndpoint {
            version: "0.1.0",
            provides: [ITransferEndpoint, ITransferEndpointAdmin],
            receptacles: {
                logger: ILogger,
                dispatch_map: IDispatchMap,
                memory_tier: IMemoryTier,
                dispatcher: IDispatcher,
            },
            fields: {
                lifecycle: Mutex<Lifecycle>,
            },
        }
    }
}

/// A symmetric RDMA endpoint for explicit certus-to-certus KV cache transfer:
/// provides [`ITransferEndpoint`] and [`ITransferEndpointAdmin`]. Receptacles:
/// `logger` (optional); `dispatch_map`, for atomic lookup-and-pin (D6, D13);
/// `memory_tier`, whose pool is registered **once**, at `initialize`, and held
/// for the endpoint's life so its `rkey` survives every reconnect (D17) — the
/// responder's approach, not the initiator's register-per-connect; and, with
/// `spdk`, `dispatcher`, which reads a pinned block-tier entry into the memory
/// tier's bounce reserve (D13, D15) — the endpoint never addresses a device.
///
/// # Example
///
/// Two endpoints, wired and started as an application would, then a push of
/// one cached entry from one into the other's pool. The default
/// build runs this over the in-process mock. An `rdma` build runs the same
/// code over the NIC, which needs real RDMA addresses — so there the example
/// is compiled but not run (`loopback_test` is its hardware counterpart).
///
#[cfg_attr(not(feature = "rdma"), doc = "```")]
#[cfg_attr(feature = "rdma", doc = "```no_run")]
/// use std::sync::{Arc, Mutex};
/// use certus_transfer_endpoint::CertusTransferEndpoint;
/// use interfaces::{
///     CacheKey, DispatchMapError, EndpointConfig, EntryOutcome, IDispatchMap, IMemoryTier,
///     ITransferEndpoint, ITransferEndpointAdmin, LookupResult,
/// };
/// # use interfaces::{BounceError, BounceBuffer, MemoryTierError, MemoryTierTelemetrySnapshot};
///
/// // A dispatch map holding one entry, pinned per lookup as the real one is.
/// struct OneEntry { addr: u64, pins: Mutex<i64> }
/// impl IDispatchMap for OneEntry {
///     fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
///         if key != 7 { return Ok(LookupResult::NotExist) }
///         *self.pins.lock().unwrap() += 1;
///         Ok(LookupResult::MemoryTier { pointer: self.addr as *mut u8, size: 5 })
///     }
///     fn release_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
///         *self.pins.lock().unwrap() -= 1;
///         Ok(())
///     }
///     // ... the rest of the interface, unused here
/// #   fn initialize(&self) -> Result<(), DispatchMapError> { Ok(()) }
/// #   fn convert_to_storage(&self, _: CacheKey, _: u64) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn take_read(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn release_write(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn downgrade_reference(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn remove(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn touch(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn entry_size(&self, _: CacheKey) -> Result<u32, DispatchMapError> { unimplemented!() }
/// #   fn oldest_keys(&self, _: usize) -> Vec<CacheKey> { unimplemented!() }
/// #   fn create_memory_tier_entry(&self, _: CacheKey, _: *mut u8, _: u32) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn promote_block_to_memory_tier(&self, _: CacheKey, _: *mut u8, _: u32) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn is_evictable(&self, _: CacheKey) -> bool { unimplemented!() }
/// #   fn try_evict_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> { unimplemented!() }
/// #   fn recover_extent(&self, _: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> { unimplemented!() }
/// }
///
/// // A memory tier reporting its pool, which the endpoint registers once.
/// struct Pool(usize, usize);
/// // SAFETY: only the pool's address and length are read.
/// unsafe impl Send for Pool {}
/// unsafe impl Sync for Pool {}
/// impl IMemoryTier for Pool {
///     fn pool_info(&self) -> Option<(*mut u8, usize)> { Some((self.0 as *mut u8, self.1)) }
///     // ... the rest of the interface, unused here
/// #   fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> { unimplemented!() }
/// #   fn insert(&self, _: CacheKey, _: u32) -> Result<*mut u8, MemoryTierError> { unimplemented!() }
/// #   fn get(&self, _: CacheKey) -> Option<(*mut u8, u32)> { unimplemented!() }
/// #   fn peek(&self, _: CacheKey) -> Option<(*mut u8, u32)> { unimplemented!() }
/// #   fn evict_next(&self) -> Option<CacheKey> { unimplemented!() }
/// #   fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> { unimplemented!() }
/// #   fn oldest_keys(&self, _: usize) -> Vec<CacheKey> { unimplemented!() }
/// #   fn remove(&self, _: CacheKey) -> Result<(), MemoryTierError> { unimplemented!() }
/// #   fn touch(&self, _: CacheKey) { unimplemented!() }
/// #   fn batch_touch(&self, _: &[CacheKey]) { unimplemented!() }
/// #   fn contains(&self, _: CacheKey) -> bool { unimplemented!() }
/// #   fn capacity(&self) -> usize { self.1 }
/// #   fn used(&self) -> usize { unimplemented!() }
/// #   fn is_dma_capable(&self) -> bool { unimplemented!() }
/// #   fn clear(&self) -> Result<usize, MemoryTierError> { unimplemented!() }
/// #   fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot { unimplemented!() }
/// #   fn set_bounce_reserve(&self, _: usize) -> Result<(), MemoryTierError> { unimplemented!() }
/// #   fn bounce_alloc(&self, _: u32) -> Result<BounceBuffer, BounceError> { unimplemented!() }
/// #   fn bounce_free(&self, _: BounceBuffer) { unimplemented!() }
/// #   fn bounce_usage(&self) -> (usize, usize) { unimplemented!() }
/// }
///
/// fn endpoint(local: &str, pool: &mut [u8], map: Arc<OneEntry>) -> Arc<CertusTransferEndpoint> {
///     let ep = CertusTransferEndpoint::create();
///     ep.dispatch_map.connect(map).unwrap();
///     ep.memory_tier
///         .connect(Arc::new(Pool(pool.as_mut_ptr() as usize, pool.len())))
///         .unwrap();
///     ep.configure(EndpointConfig::new(local)).unwrap();
///     ep.initialize().unwrap();
///     ep
/// }
///
/// let mut src_pool = *b"hello, peer.....";
/// let map = Arc::new(OneEntry { addr: src_pool.as_ptr() as u64, pins: Mutex::new(0) });
/// let src = endpoint("10.20.30.1:7000", &mut src_pool, Arc::clone(&map));
/// let mut dst_pool = [0u8; 16];
/// let dst = endpoint("10.20.30.2:7000", &mut dst_pool, Arc::new(OneEntry { addr: 0, pins: Mutex::new(0) }));
///
/// // The destination's pool, as its control plane would hand it to a pusher.
/// let pool = dst.landing_region().unwrap();
///
/// // Key 7 is held; key 8 is not.
/// let out = src
///     .push("10.20.30.2:7000", &[(7, pool.region_at(0, 5).unwrap()),
///                                (8, pool.region_at(8, 5).unwrap())])
///     .unwrap();
/// assert_eq!(out, [EntryOutcome::HitMemoryTier, EntryOutcome::Miss]);
/// assert_eq!(&dst_pool[..5], b"hello");
/// assert_eq!(*map.pins.lock().unwrap(), 0, "the pin was released");
///
/// src.shutdown().unwrap();
/// dst.shutdown().unwrap();
/// ```
pub use component::CertusTransferEndpoint;

/// The IPv4 address of this host's first active RDMA (RoCE v2) port, for an
/// endpoint whose operator names no address. `rdma` builds only: without it
/// there is no NIC to ask.
///
/// # Errors
///
/// A message if no device, or no active port with an IPv4-mapped GID, exists.
#[cfg(feature = "rdma")]
pub fn first_active_rdma_ipv4() -> Result<String, String> {
    rdma::first_active_rdma_ipv4()
}

impl CertusTransferEndpoint {
    /// An endpoint awaiting [`ITransferEndpointAdmin::configure`]. The same
    /// as the generated `new_default`.
    ///
    /// ```
    /// use certus_transfer_endpoint::CertusTransferEndpoint;
    /// use interfaces::{ITransferEndpoint, TransferEndpointError};
    ///
    /// let ep = CertusTransferEndpoint::create();
    /// // Nothing runs until configured and initialized.
    /// assert!(matches!(ep.disconnect("10.0.0.2:7000"),
    ///                  Err(TransferEndpointError::NotInitialized(_))));
    /// assert_eq!(ep.stats().requests_attempted, 0);
    /// ```
    pub fn create() -> std::sync::Arc<Self> {
        Self::new_default()
    }

    fn lifecycle(&self) -> LifecycleGuard<'_> {
        LifecycleGuard(self.lifecycle.lock().expect("lifecycle lock poisoned"))
    }

    /// Serve block-tier entries from `source` instead of the bound dispatcher.
    /// Test-only: the default build has no dispatcher receptacle.
    #[cfg(test)]
    fn use_block_source(&self, source: Source) {
        if let State::Configured(_, _, slot) = &mut *self.lifecycle() {
            *slot = Some(source);
        }
    }

    /// The bound dispatcher, as a block source (`spdk` builds).
    #[cfg(feature = "spdk")]
    fn block_source(&self) -> Option<Source> {
        let d = self.dispatcher.get().ok()?;
        Some(std::sync::Arc::new(bounce::DispatcherSource(d)))
    }

    /// No block source without `spdk`: block-tier answers are misses (D14).
    #[cfg(not(feature = "spdk"))]
    fn block_source(&self) -> Option<Source> {
        None
    }

    /// Run in an isolated mock fabric instead of the build's default, so tests
    /// in any build cannot collide on an instance name. Call after `configure`.
    #[cfg(test)]
    fn use_transport(&self, choice: TransportChoice) {
        if let State::Configured(_, slot, _) = &mut *self.lifecycle() {
            *slot = Some(choice);
        }
    }

    /// Send `cmd` to the endpoint thread and wait for its reply.
    fn call<R>(
        &self,
        make: impl FnOnce(mpsc::Sender<R>) -> Command,
    ) -> Result<R, TransferEndpointError> {
        let (tx, rx) = mpsc::channel();
        {
            let guard = self.lifecycle();
            let State::Running(handle, _) = &*guard else {
                return Err(not_running());
            };
            handle
                .send(make(tx))
                .map_err(|e| TransferEndpointError::Transport(format!("endpoint thread: {e}")))?;
        }
        rx.recv().map_err(|_| {
            TransferEndpointError::Transport("endpoint thread exited without replying".into())
        })
    }
}

/// The lock on [`Lifecycle`], dereferencing to its state.
struct LifecycleGuard<'a>(std::sync::MutexGuard<'a, Lifecycle>);

impl std::ops::Deref for LifecycleGuard<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        &self.0 .0
    }
}

impl std::ops::DerefMut for LifecycleGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.0 .0
    }
}

/// Ids for `fetch`'s `TransferContextId`: unique per process, so a log line
/// or a `Deadline` names one fetch unambiguously.
fn next_fetch_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn not_running() -> TransferEndpointError {
    TransferEndpointError::NotInitialized("endpoint is not running".into())
}

fn parse_peer(s: &str) -> Result<PeerId, TransferEndpointError> {
    PeerId::parse(s).map_err(|e| TransferEndpointError::InvalidEndpoint(e.to_string()))
}

impl ITransferEndpointAdmin for CertusTransferEndpoint {
    fn configure(&self, config: EndpointConfig) -> Result<(), TransferEndpointError> {
        parse_peer(&config.local)?;
        // Each connection posts one receive per message credit (the reply
        // reserve) and one per request the peer may send (the quota); the
        // device must be asked for at least that many.
        if config.message_credits == 0 || config.inbound_quota == 0 {
            return Err(TransferEndpointError::Transport(
                "message_credits and inbound_quota must each be at least 1".into(),
            ));
        }
        if config.block_readers == 0 {
            return Err(TransferEndpointError::Transport(
                "block_readers must be at least 1".into(),
            ));
        }
        if config.deadline_action != interfaces::DeadlineAction::FailLoud {
            // FR-016: the degrading action is deferred. Refused here, never
            // quietly run as FailLoud (Principle XI).
            return Err(TransferEndpointError::Transport(format!(
                "deadline_action {:?} is deferred and not implemented; only FailLoud is",
                config.deadline_action
            )));
        }
        let receives = config.message_credits.saturating_add(config.inbound_quota);
        if config.max_recv_wr < receives {
            return Err(TransferEndpointError::Transport(format!(
                "max_recv_wr {} is below the {receives} receives each connection posts \
                 (message_credits {} + inbound_quota {})",
                config.max_recv_wr, config.message_credits, config.inbound_quota
            )));
        }
        let mut guard = self.lifecycle();
        match &*guard {
            State::Unconfigured | State::Configured(..) => {
                *guard = State::Configured(config, None, None);
                Ok(())
            }
            State::Running(..) | State::Stopped => Err(TransferEndpointError::NotInitialized(
                "already initialized".into(),
            )),
        }
    }

    fn initialize(&self) -> Result<(), TransferEndpointError> {
        let mut guard = self.lifecycle();
        // Check the state before moving anything out of it: a call in the
        // wrong state must leave that state exactly as it was.
        match &*guard {
            State::Configured(..) => {}
            State::Unconfigured => {
                return Err(TransferEndpointError::NotInitialized(
                    "not configured".into(),
                ))
            }
            State::Running(..) | State::Stopped => {
                return Err(TransferEndpointError::NotInitialized(
                    "already initialized".into(),
                ))
            }
        }
        let State::Configured(config, choice, source) =
            std::mem::replace(&mut *guard, State::Stopped)
        else {
            unreachable!("checked above, under the same lock");
        };
        let start = || -> Result<(ActorHandle<Command>, PoolKey), TransferEndpointError> {
            let local = parse_peer(&config.local)?;
            let transport = select_transport(choice.unwrap_or(TransportChoice::BuildDefault))
                .map_err(|e| TransferEndpointError::Transport(e.to_string()))?;
            let caps = QpCaps {
                max_send_wr: config.max_send_wr,
                max_recv_wr: config.max_recv_wr,
            };
            let map = self.dispatch_map.get().map_err(|_| {
                TransferEndpointError::NotInitialized("dispatch_map receptacle not bound".into())
            })?;
            let tier = self.memory_tier.get().map_err(|_| {
                TransferEndpointError::NotInitialized("memory_tier receptacle not bound".into())
            })?;
            let (pool_base, pool_len) = tier.pool_info().ok_or_else(|| {
                TransferEndpointError::NotInitialized(
                    "memory tier pool not initialized (pool_info returned None)".into(),
                )
            })?;
            let mut table = ConnectionTable::new(transport, local, caps, config.send_credits)
                .map_err(|e| TransferEndpointError::Transport(e.to_string()))?;
            // Once, for the endpoint's life: the source of every push and,
            // with fetch, where peers land data — so both write flags.
            // SAFETY: the memory tier owns the pool for the process's life;
            // the region is dropped only after the table, so after every write
            // it backs has been drained. Pool memory is never held as a Rust
            // reference by this crate.
            let pool_region = unsafe {
                table.transport().register(
                    pool_base,
                    pool_len,
                    Access::LOCAL_WRITE | Access::REMOTE_WRITE,
                )
            }
            .map_err(|e| TransferEndpointError::Transport(format!("registering the pool: {e}")))?;
            let key = PoolKey {
                addr: pool_region.addr(),
                len: pool_region.len() as u64,
                rkey: pool_region.rkey(),
            };
            let logger: Box<dyn ILogger + Send + Sync> = match self.logger.get() {
                Ok(l) => Box::new(SharedLogger(l)),
                Err(_) => Box::new(NoopLogger),
            };
            let limits = session::Limits {
                message_credits: config.message_credits,
                inbound_quota: config.inbound_quota,
            };
            let readers = match source.clone().or_else(|| self.block_source()) {
                Some(src) => Some(
                    bounce::Readers::start(src, config.block_readers as usize).map_err(|e| {
                        TransferEndpointError::Transport(format!("starting block readers: {e}"))
                    })?,
                ),
                None => None,
            };
            let actor = Actor::new(
                EndpointActor::new(table, logger, map, pool_region, limits)
                    .with_deadline(config.fetch_deadline)
                    .with_readers(readers),
                |_| {
                    // Unreachable: every handler path aborts on panic itself.
                },
            );
            let actor = match config.actor_cpu {
                Some(cpu) => actor.with_cpu_affinity(
                    CpuSet::from_cpu(cpu)
                        .map_err(|e| TransferEndpointError::Transport(e.to_string()))?,
                ),
                None => actor,
            };
            actor
                .activate()
                .map(|h| (h, key))
                .map_err(|e| TransferEndpointError::Transport(format!("starting thread: {e}")))
        };
        match start() {
            Ok((handle, key)) => {
                *guard = State::Running(handle, key);
                Ok(())
            }
            Err(e) => {
                // Leave it configured, so a fixed configuration can retry.
                *guard = State::Configured(config, None, source);
                Err(e)
            }
        }
    }

    fn shutdown(&self) -> Result<(), TransferEndpointError> {
        let handle = {
            let mut guard = self.lifecycle();
            match std::mem::replace(&mut *guard, State::Stopped) {
                State::Running(h, _) => h,
                // Never started, or already stopped: nothing to drain.
                _ => return Ok(()),
            }
        };
        let (tx, rx) = mpsc::channel();
        let drained = match handle.send(Command::Shutdown(tx)) {
            Ok(()) => rx
                .recv()
                .unwrap_or_else(|_| Err("endpoint thread exited before finishing shutdown".into())),
            Err(e) => Err(format!("endpoint thread: {e}")),
        };
        let joined = handle.deactivate();
        drained.map_err(TransferEndpointError::Transport)?;
        joined.map_err(|e| TransferEndpointError::Transport(format!("joining thread: {e}")))
    }
}

impl ITransferEndpoint for CertusTransferEndpoint {
    /// Ask `peer` for `items`: it writes each entry it holds into the named
    /// slot of this endpoint's pool, then replies once with one outcome per
    /// item, which `on_complete` receives in request order. Returns once
    /// queued; a slot outside the pool is refused first, with nothing sent.
    ///
    /// A hit means the entry's write completed before the reply was sent, so
    /// its slot is safe to commit; every other outcome means abort it. Any
    /// length is accepted: the endpoint chunks against the peer's
    /// `max_entries`.
    fn fetch(
        &self,
        peer: &str,
        items: &[(CacheKey, LocalSlot)],
        on_complete: FetchCompletion,
    ) -> Result<TransferContextId, TransferEndpointError> {
        let peer = parse_peer(peer)?;
        let guard = self.lifecycle();
        let State::Running(handle, pool) = &*guard else {
            return Err(not_running());
        };
        let items = items
            .iter()
            .map(|&(key, slot)| {
                pool.region(slot).map(|r| (key, r)).ok_or_else(|| {
                    TransferEndpointError::Transport(format!(
                        "key {key}: slot 0x{:x}+{} is not inside the memory-tier pool \
                         0x{:x}+{}",
                        slot.addr, slot.length, pool.addr, pool.len
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let id = next_fetch_id();
        handle
            .send(Command::Fetch {
                id,
                peer,
                items,
                on_complete,
            })
            .map_err(|e| TransferEndpointError::Transport(format!("endpoint thread: {e}")))?;
        Ok(TransferContextId(id))
    }

    /// The pool's registration, as recorded at `initialize`; no round trip.
    fn landing_region(&self) -> Result<LandingRegion, TransferEndpointError> {
        let guard = self.lifecycle();
        let State::Running(_, pool) = &*guard else {
            return Err(not_running());
        };
        Ok(LandingRegion {
            addr: pool.addr,
            len: pool.len,
            rkey: pool.rkey,
        })
    }

    /// Push each entry this endpoint holds into the peer's named region,
    /// then fire `on_complete` once, on the endpoint thread, with one outcome
    /// per item in request order (invariant 1). Returns once queued.
    ///
    /// Each entry is looked up — and pinned — only when a send credit is free
    /// to post its write at once, so pins held never exceed the credits
    /// (SC-004), plus one per block reader. An entry the source does not serve
    /// is a `Miss`: absent, the wrong size, or block-tier in a build without
    /// `spdk` (with it, block-tier entries are read through the dispatcher and
    /// served as `HitBlockTier`). An entry not
    /// delivered for any other reason — a connection that could not be made
    /// or was lost, a refused or flushed write, a failed lookup — is `Failed`.
    fn push_async(
        &self,
        endpoint: &str,
        items: &[(CacheKey, RemoteRegion)],
        on_complete: FetchCompletion,
    ) -> Result<(), TransferEndpointError> {
        let peer = parse_peer(endpoint)?;
        let guard = self.lifecycle();
        let State::Running(handle, _) = &*guard else {
            return Err(not_running());
        };
        handle
            .send(Command::Push {
                peer,
                items: items.to_vec(),
                on_complete,
            })
            .map_err(|e| TransferEndpointError::Transport(format!("endpoint thread: {e}")))
    }

    /// [`push_async`](Self::push_async), waiting for its outcomes: one per
    /// item, in request order.
    ///
    /// Refused on an endpoint thread — from a completion callback, say — where
    /// waiting would deadlock: the thread it waits on is itself.
    fn push(
        &self,
        endpoint: &str,
        items: &[(CacheKey, RemoteRegion)],
    ) -> Result<Vec<EntryOutcome>, TransferEndpointError> {
        if endpoint::on_endpoint_thread() {
            return Err(TransferEndpointError::Transport(
                "push called on an endpoint thread (from a completion callback?) would \
                 wait on itself; use push_async"
                    .into(),
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.push_async(
            endpoint,
            items,
            Box::new(move |outcomes| {
                // The caller may have stopped waiting; nothing to do then.
                let _ = tx.send(outcomes);
            }),
        )?;
        // push_async guarantees the callback fires exactly once — on
        // completion, failure, teardown or shutdown — so this returns.
        rx.recv().map_err(|_| {
            TransferEndpointError::Transport("endpoint thread exited without completing".into())
        })?
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransferEndpointError> {
        let peer = parse_peer(peer)?;
        self.call(|reply| Command::Disconnect(peer, reply))?
            .map_err(TransferEndpointError::Transport)
    }

    /// Counters in every build. Before `initialize`, and after `shutdown`,
    /// all zero: nothing has run.
    fn stats(&self) -> TransferStats {
        self.call(Command::Stats).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::mock::MockFabric;
    use crate::test_doubles::{LedgerMap, PoolTier};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// An endpoint plus the memory it serves from: a pool, and a ledger
    /// dispatch map answering into it.
    struct Node {
        ep: Arc<CertusTransferEndpoint>,
        map: Arc<LedgerMap>,
        pool: Box<[u8]>,
    }

    impl Node {
        fn pool_addr(&self, off: usize) -> u64 {
            self.pool.as_ptr() as u64 + off as u64
        }
    }

    /// Where a node's pool begins its landing half: a fetch lands in the
    /// destination's own pool, in buffers it has not published. Entries a
    /// test caches stay below.
    const LANDING: usize = 4096;

    /// A node's landing half, as a fetch names it and a test reads it back.
    struct Landing {
        base: *const u8,
        len: usize,
    }

    impl Landing {
        fn of(n: &Node) -> Landing {
            Landing {
                base: n.pool[LANDING..].as_ptr(),
                len: n.pool.len() - LANDING,
            }
        }

        /// The slot of `len` bytes at `off` into the landing half.
        fn slot(&self, off: usize, len: u32) -> LocalSlot {
            assert!(off + len as usize <= self.len, "inside the landing half");
            LocalSlot {
                addr: self.base as u64 + off as u64,
                length: len,
            }
        }

        /// The region a push names for the slot: the node's pool rkey.
        fn region(&self, ep: &CertusTransferEndpoint, off: usize, len: u32) -> RemoteRegion {
            let (_, rkey) = pool_key(ep);
            let s = self.slot(off, len);
            RemoteRegion {
                addr: s.addr,
                rkey,
                length: s.length,
            }
        }

        fn as_ptr(&self) -> *const u8 {
            self.base
        }
    }

    impl std::ops::Deref for Landing {
        type Target = [u8];
        fn deref(&self) -> &[u8] {
            // SAFETY: inside the node's pool, which outlives the test; the
            // tests read it only once the writes they check have completed.
            unsafe { std::slice::from_raw_parts(self.base, self.len) }
        }
    }

    fn node(fabric: &MockFabric, local: &str, send_credits: u32) -> Node {
        node_with(fabric, local, |c| c.send_credits = send_credits)
    }

    /// A node whose configuration `tweak` adjusts before `configure`.
    fn node_with(
        fabric: &MockFabric,
        local: &str,
        tweak: impl FnOnce(&mut EndpointConfig),
    ) -> Node {
        let mut pool = vec![0u8; 2 * LANDING].into_boxed_slice();
        let ep = CertusTransferEndpoint::create();
        let map = Arc::new(LedgerMap::new());
        ep.dispatch_map
            .connect(Arc::clone(&map) as Arc<dyn interfaces::IDispatchMap + Send + Sync>)
            .unwrap();
        ep.memory_tier
            .connect(Arc::new(PoolTier {
                base: pool.as_mut_ptr(),
                len: pool.len(),
            })
                as Arc<dyn interfaces::IMemoryTier + Send + Sync>)
            .unwrap();
        let mut config = EndpointConfig::new(local);
        tweak(&mut config);
        ep.configure(config).unwrap();
        ep.use_transport(TransportChoice::Mock(fabric.clone()));
        Node { ep, map, pool }
    }

    /// An endpoint with its receptacles bound; the memory behind them is
    /// leaked, which is harmless in a test that never pushes.
    fn endpoint(fabric: &MockFabric, local: &str) -> Arc<CertusTransferEndpoint> {
        let n = node(fabric, local, 8);
        std::mem::forget(n.pool);
        n.ep
    }

    /// The outcomes of a request that got a reply (a fetch) or ran (a push).
    fn hits(r: interfaces::FetchResult) -> Vec<EntryOutcome> {
        r.unwrap_or_else(|e| panic!("expected outcomes, got {e:?}"))
    }

    /// Wait for `cond`, polling: the endpoint threads connect on their own.
    fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn lifecycle_is_enforced() {
        let ep = endpoint(&MockFabric::new(), "10.0.0.9:1");
        let ep = {
            // Start from unconfigured, but with receptacles bound.
            let fresh = CertusTransferEndpoint::create();
            fresh
                .dispatch_map
                .connect(ep.dispatch_map.get().unwrap())
                .unwrap();
            fresh
                .memory_tier
                .connect(ep.memory_tier.get().unwrap())
                .unwrap();
            fresh
        };
        let not_init = |r: Result<(), TransferEndpointError>| {
            matches!(r, Err(TransferEndpointError::NotInitialized(_)))
        };
        assert!(not_init(ep.initialize()), "not configured");
        assert!(not_init(ep.disconnect("10.0.0.2:7000")), "not running");
        assert!(matches!(
            ep.configure(EndpointConfig::new("10.0.0.1")),
            Err(TransferEndpointError::InvalidEndpoint(_))
        ));
        ep.configure(EndpointConfig::new("10.0.0.1:7000")).unwrap();
        ep.use_transport(TransportChoice::Mock(MockFabric::new()));
        ep.initialize().unwrap();
        assert!(not_init(ep.initialize()), "already initialized");
        assert!(not_init(ep.configure(EndpointConfig::new("10.0.0.1:7001"))));
        ep.shutdown().unwrap();
        ep.shutdown().unwrap(); // idempotent
        assert!(not_init(ep.disconnect("10.0.0.2:7000")), "stopped");
        assert!(not_init(ep.initialize()), "not restartable");
    }

    #[test]
    fn a_failed_initialize_can_be_retried() {
        let f = MockFabric::new();
        let a = endpoint(&f, "10.0.0.1:7000");
        a.initialize().unwrap();
        // Same instance name on the same fabric: the listen is refused.
        let b = endpoint(&f, "10.0.0.1:7000");
        assert!(matches!(
            b.initialize(),
            Err(TransferEndpointError::Transport(_))
        ));
        a.shutdown().unwrap();
        b.use_transport(TransportChoice::Mock(f.clone()));
        b.initialize().unwrap();
        b.shutdown().unwrap();
    }

    #[test]
    fn transfer_methods_validate_the_peer_first() {
        let f = MockFabric::new();
        let ep = endpoint(&f, "10.0.0.1:7000");
        ep.initialize().unwrap();
        let invalid = |r: Result<(), TransferEndpointError>| {
            matches!(r, Err(TransferEndpointError::InvalidEndpoint(_)))
        };
        assert!(invalid(ep.push("10.0.0.2", &[]).map(|_| ())));
        assert!(invalid(ep.push_async("10.0.0.2", &[], Box::new(|_| {}))));
        assert!(invalid(
            ep.fetch("10.0.0.2", &[], Box::new(|_| {})).map(|_| ())
        ));
        ep.shutdown().unwrap();
    }

    #[test]
    fn two_endpoints_connect_disconnect_and_shut_down_clean() {
        let f = MockFabric::new();
        let a = endpoint(&f, "10.0.0.1:7000");
        let b = endpoint(&f, "10.0.0.2:7000");
        a.initialize().unwrap();
        b.initialize().unwrap();
        let (pa, pb) = (
            parse_peer("10.0.0.1:7000").unwrap(),
            parse_peer("10.0.0.2:7000").unwrap(),
        );
        let up = |ep: &CertusTransferEndpoint, p: &PeerId| {
            let p = p.clone();
            ep.call(|r| Command::IsUp(p, r)).unwrap()
        };

        let pb2 = pb.clone();
        a.call(|r| Command::Connect(pb2, r)).unwrap().unwrap();
        eventually("both ends up", || up(&a, &pb) && up(&b, &pa));
        assert_eq!(f.qp_count(), 2, "one connection");

        a.disconnect("10.0.0.2:7000").unwrap();
        assert!(!up(&a, &pb), "a's link is gone when disconnect returns");
        eventually("b to finish its teardown", || f.qp_count() == 0);
        assert!(!up(&b, &pa));
        a.disconnect("10.0.0.2:7000").unwrap(); // an unknown peer is not an error

        a.shutdown().unwrap();
        b.shutdown().unwrap();
        assert_eq!(f.qp_count(), 0, "every queue pair destroyed");
    }

    /// Shutdown drains what a link still holds before dropping the table:
    /// without the drain, the table's drop would find held state and the
    /// process would abort.
    #[test]
    fn shutdown_drains_held_state_before_dropping_the_table() {
        let f = MockFabric::new();
        let a = endpoint(&f, "10.0.0.1:7000");
        let b = endpoint(&f, "10.0.0.2:7000");
        a.initialize().unwrap();
        b.initialize().unwrap();
        let pb = parse_peer("10.0.0.2:7000").unwrap();
        let pb2 = pb.clone();
        a.call(|r| Command::Connect(pb2, r)).unwrap().unwrap();
        eventually("a up", || {
            let p = pb.clone();
            a.call(|r| Command::IsUp(p, r)).unwrap()
        });
        let pb3 = pb.clone();
        a.call(|r| Command::HoldRecv(pb3, r)).unwrap().unwrap();
        a.shutdown().unwrap();
        b.shutdown().unwrap();
        assert_eq!(f.qp_count(), 0);
    }

    /// Shutdown tears down live connections itself.
    #[test]
    fn shutdown_tears_down_live_connections() {
        let f = MockFabric::new();
        let a = endpoint(&f, "10.0.0.1:7000");
        let b = endpoint(&f, "10.0.0.2:7000");
        a.initialize().unwrap();
        b.initialize().unwrap();
        let pb = parse_peer("10.0.0.2:7000").unwrap();
        let pb2 = pb.clone();
        a.call(|r| Command::Connect(pb2, r)).unwrap().unwrap();
        eventually("a up", || {
            let p = pb.clone();
            a.call(|r| Command::IsUp(p, r)).unwrap()
        });
        a.shutdown().unwrap();
        eventually("b to notice", || f.qp_count() == 0);
        b.shutdown().unwrap();
    }

    #[test]
    fn stats_reach_the_endpoint_thread() {
        let f = MockFabric::new();
        let ep = endpoint(&f, "10.0.0.1:7000");
        assert_eq!(ep.stats(), TransferStats::default(), "before start");
        ep.initialize().unwrap();
        eventually("a stats round trip", || {
            ep.stats() == TransferStats::default()
        });
        ep.shutdown().unwrap();
        assert_eq!(ep.stats(), TransferStats::default(), "after stop");
    }

    /// The wrong state must survive a refused call unchanged.
    #[test]
    fn a_refused_initialize_leaves_the_state_alone() {
        let f = MockFabric::new();
        let bound = endpoint(&f, "10.0.0.9:1");
        let ep = CertusTransferEndpoint::create();
        ep.dispatch_map
            .connect(bound.dispatch_map.get().unwrap())
            .unwrap();
        ep.memory_tier
            .connect(bound.memory_tier.get().unwrap())
            .unwrap();
        assert!(ep.initialize().is_err(), "not configured");
        ep.configure(EndpointConfig::new("10.0.0.1:7000"))
            .expect("still configurable after a refused initialize");
        ep.use_transport(TransportChoice::Mock(f.clone()));
        ep.initialize().unwrap();
        assert!(ep.initialize().is_err(), "already running");
        assert_eq!(ep.stats(), TransferStats::default(), "and still running");
        ep.shutdown().unwrap();
    }

    /// `define_component!` keeps `Arc`s of the component in its own interface
    /// map, so the component is never dropped by releasing handles: `shutdown`
    /// is the only way to stop it. Pinned here so a framework change that
    /// fixes the cycle is noticed, and the doc on `shutdown` revisited.
    #[test]
    fn a_component_outlives_its_last_handle() {
        let ep = CertusTransferEndpoint::create();
        let weak = std::sync::Arc::downgrade(&ep);
        drop(ep);
        assert!(
            weak.upgrade().is_some(),
            "the framework cycle is gone: revisit shutdown docs"
        );
    }

    // --- push (T017) and its pin discipline (T022-T026) -----------------------

    /// The pool's address and rkey, as a pushing peer is told them.
    fn pool_key(ep: &CertusTransferEndpoint) -> (u64, u32) {
        let r = ep.landing_region().unwrap();
        (r.addr, r.rkey)
    }

    /// Push `items` from `src` and wait for the callback.
    fn push_and_wait(
        src: &CertusTransferEndpoint,
        to: &str,
        items: &[(CacheKey, RemoteRegion)],
    ) -> Vec<EntryOutcome> {
        let (tx, rx) = mpsc::channel();
        src.push_async(to, items, Box::new(move |o| tx.send(o).unwrap()))
            .unwrap();
        hits(
            rx.recv_timeout(Duration::from_secs(10))
                .expect("on_complete fired"),
        )
    }

    #[test]
    fn initialize_needs_the_dispatch_map_and_a_pool() {
        let ep = CertusTransferEndpoint::create();
        ep.configure(EndpointConfig::new("10.0.0.1:7000")).unwrap();
        ep.use_transport(TransportChoice::Mock(MockFabric::new()));
        match ep.initialize() {
            Err(TransferEndpointError::NotInitialized(m)) => assert!(m.contains("dispatch_map")),
            other => panic!("{other:?}"),
        }
        ep.dispatch_map
            .connect(Arc::new(LedgerMap::new()) as Arc<dyn interfaces::IDispatchMap + Send + Sync>)
            .unwrap();
        ep.memory_tier
            .connect(Arc::new(PoolTier {
                base: std::ptr::null_mut(),
                len: 0,
            })
                as Arc<dyn interfaces::IMemoryTier + Send + Sync>)
            .unwrap();
        ep.use_transport(TransportChoice::Mock(MockFabric::new()));
        match ep.initialize() {
            Err(TransferEndpointError::NotInitialized(m)) => assert!(m.contains("pool"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// The legacy path end to end over the mock: values land byte-exact in
    /// the peer's memory, outcomes come back in request order, every miss kind
    /// is a miss, and the pin ledger balances (T026).
    #[test]
    fn push_writes_hits_reports_misses_and_balances_every_pin() {
        let f = MockFabric::new();
        let mut a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        for (i, byte) in a.pool[..256].iter_mut().enumerate() {
            *byte = i as u8;
        }
        a.map.memory(10, a.pool_addr(0), 64);
        a.map.memory(11, a.pool_addr(64), 64);
        a.map.memory(12, a.pool_addr(128), 32); // wrong size: pinned, must be released
        a.map.block(13); // block tier with no block source: pinned, released, miss (D14)
        a.map.mismatch(14); // unpinned
        a.map.preload_reader(14); // someone else's pin on it
                                  // Outside the pool: the map and the tier disagree, a fault, not
                                  // residency. Pinned, released, Failed.
        a.map.memory(15, 0x9_0000, 64);
        let (land, rkey) = pool_key(&b.ep);
        let at = |off: u64| RemoteRegion {
            addr: land + off,
            rkey,
            length: 64,
        };
        let items = [
            (10, at(1024)),
            (99, at(0)), // absent
            (11, at(1088)),
            (12, at(0)),
            (13, at(0)),
            (14, at(0)),
            (15, at(0)),
        ];
        let out = push_and_wait(&a.ep, "10.0.0.2:7000", &items);
        use EntryOutcome::*;
        assert_eq!(
            out,
            [HitMemoryTier, Miss, HitMemoryTier, Miss, Miss, Miss, Failed]
        );
        assert_eq!(&b.pool[1024..1088], &a.pool[0..64], "entry 10 landed");
        assert_eq!(&b.pool[1088..1152], &a.pool[64..128], "entry 11 landed");
        assert_eq!(b.pool[0..64], [0u8; 64], "nothing for the misses");
        for key in [10, 11, 12, 13, 15] {
            assert_eq!(
                a.map.read_ref(key),
                0,
                "key {key}'s pin released exactly once"
            );
        }
        assert_eq!(a.map.read_ref(14), 1, "the other reader's pin untouched");
        assert_eq!(a.map.lookups(), 7, "one lookup per entry, no probe");
        let s = a.ep.stats();
        assert_eq!((s.requests_attempted, s.requests_completed), (1, 1));
        assert_eq!(
            (s.entries_requested, s.hit_memory_tier, s.miss, s.failed),
            (7, 2, 4, 1)
        );
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// SC-004 (T025): with send credits n, a push of n x 100 entries never
    /// holds more than n pins at once, independent of the request's length.
    #[test]
    fn pins_held_never_exceed_send_credits() {
        let f = MockFabric::new();
        let n = 3u32;
        let a = node(&f, "10.0.0.1:7000", n);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        let count = n as u64 * 100;
        for key in 0..count {
            a.map.memory(key, a.pool_addr((key % 64) as usize * 16), 16);
        }
        let (land, rkey) = pool_key(&b.ep);
        let items: Vec<_> = (0..count)
            .map(|k| {
                (
                    k,
                    RemoteRegion {
                        addr: land + (k % 64) * 16,
                        rkey,
                        length: 16,
                    },
                )
            })
            .collect();
        let out = push_and_wait(&a.ep, "10.0.0.2:7000", &items);
        assert!(out.iter().all(|o| *o == EntryOutcome::HitMemoryTier));
        assert!(
            a.map.peak_held() <= n as i64,
            "peak {} pins held with {n} credits",
            a.map.peak_held()
        );
        assert!(a.map.peak_held() >= 1);
        assert_eq!(a.map.held(), 0, "every pin released");
        assert!(a.ep.stats().send_queue_peak_in_flight <= n);
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// A push to an instance nobody listens at fails every entry — `Failed`,
    /// not `Miss`, since nothing was learned about residency — pins nothing
    /// (entries are looked up only once connected), and fires.
    #[test]
    fn push_to_an_unreachable_peer_misses_everything_and_pins_nothing() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        let region = RemoteRegion {
            addr: 0,
            rkey: 0,
            length: 8,
        };
        let out = push_and_wait(&a.ep, "10.0.0.9:7000", &[(1, region), (2, region)]);
        assert_eq!(out, [EntryOutcome::Failed, EntryOutcome::Failed]);
        assert_eq!((a.ep.stats().failed, a.ep.stats().miss), (2, 0));
        assert_eq!(a.map.lookups(), 0, "nothing looked up, so nothing pinned");
        a.ep.shutdown().unwrap();
    }

    /// A write the peer refuses (bad rkey) fails, tears the connection down,
    /// and releases every pin — the in-flight one through the drain.
    #[test]
    fn a_refused_write_tears_down_and_releases_every_pin() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 4);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        for k in 0..6 {
            a.map.memory(k, a.pool_addr(k as usize * 8), 8);
        }
        let (land, rkey) = pool_key(&b.ep);
        let items: Vec<_> = (0..6u64)
            .map(|k| {
                (
                    k,
                    RemoteRegion {
                        addr: land + k * 8,
                        rkey: if k == 2 { rkey ^ 0xdead } else { rkey },
                        length: 8,
                    },
                )
            })
            .collect();
        let out = push_and_wait(&a.ep, "10.0.0.2:7000", &items);
        assert_eq!(out.len(), 6);
        assert_eq!(
            out[..2],
            [EntryOutcome::HitMemoryTier; 2],
            "before the fault"
        );
        assert_eq!(out[2], EntryOutcome::Failed, "the refused write");
        assert!(
            out[3..].iter().all(|o| *o == EntryOutcome::Failed),
            "the rest were flushed or never sent: {out:?}"
        );
        assert_eq!(a.ep.stats().miss, 0, "a refused write is not a cold cache");
        assert_eq!(
            a.map.held(),
            0,
            "every pin released, including flushed ones"
        );
        eventually("the connection torn down", || f.qp_count() == 0);
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// Writes genuinely in flight at teardown — held there by the mock's
    /// send-pause knob — come back through the drain, and each one's pin is
    /// released there. `disconnect` is the teardown.
    #[test]
    fn in_flight_writes_release_their_pins_through_the_drain() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        let pb = PeerId::parse("10.0.0.2:7000").unwrap();
        let pb2 = pb.clone();
        a.ep.call(|r| Command::Connect(pb2, r)).unwrap().unwrap();
        eventually("connected", || {
            let p = pb.clone();
            a.ep.call(|r| Command::IsUp(p, r)).unwrap()
        });
        // Hold every send in flight from here on.
        f.pause_sends(true);
        for k in 0..4 {
            a.map.memory(k, a.pool_addr(k as usize * 8), 8);
        }
        let (land, rkey) = pool_key(&b.ep);
        let items: Vec<_> = (0..4u64)
            .map(|k| {
                (
                    k,
                    RemoteRegion {
                        addr: land + 512 + k * 8,
                        rkey,
                        length: 8,
                    },
                )
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        a.ep.push_async(
            "10.0.0.2:7000",
            &items,
            Box::new(move |o| tx.send(o).unwrap()),
        )
        .unwrap();
        eventually("all four pinned and in flight", || a.map.held() == 4);
        assert!(rx.try_recv().is_err(), "nothing has completed");

        a.ep.disconnect("10.0.0.2:7000").unwrap();
        assert_eq!(a.map.held(), 0, "the drain released every in-flight pin");
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Failed; 4],
            "flushed, not served"
        );
        assert_eq!(b.pool[512..544], [0u8; 32], "nothing landed");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// Shutdown with a push in flight (stalled on a peer that never accepts)
    /// fires the callback and leaves no pin held.
    #[test]
    fn shutdown_mid_push_fires_the_callback_and_holds_nothing() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        // A raw listener that never answers: the connect stays pending.
        let mut silent = crate::connection::mock::MockTransport::new(f.clone());
        crate::connection::transport::Transport::listen(
            &mut silent,
            &PeerId::parse("10.0.0.2:7000").unwrap(),
        )
        .unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        let (tx, rx) = mpsc::channel();
        a.ep.push_async(
            "10.0.0.2:7000",
            &[(
                1,
                RemoteRegion {
                    addr: 0,
                    rkey: 0,
                    length: 8,
                },
            )],
            Box::new(move |o| tx.send(o).unwrap()),
        )
        .unwrap();
        a.ep.shutdown().unwrap();
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Failed]
        );
        assert_eq!(a.map.held(), 0);
        drop(silent);
    }

    // --- blocking push (T018) -------------------------------------------------

    /// `push` returns the same outcomes, in the same order, as `push_async`
    /// delivers to its callback.
    #[test]
    fn blocking_push_returns_outcomes_in_order() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 2);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        a.map.memory(3, a.pool_addr(8), 8);
        let (land, rkey) = pool_key(&b.ep);
        let at = |off| RemoteRegion {
            addr: land + off,
            rkey,
            length: 8,
        };
        let out =
            a.ep.push(
                "10.0.0.2:7000",
                &[(1, at(0)), (2, at(8)), (3, at(16)), (4, at(24))],
            )
            .unwrap();
        use EntryOutcome::*;
        assert_eq!(out, [HitMemoryTier, Miss, HitMemoryTier, Miss]);
        assert_eq!(a.map.held(), 0);
        // An empty push is answered, not hung.
        assert_eq!(a.ep.push("10.0.0.2:7000", &[]).unwrap(), []);
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    #[test]
    fn blocking_push_reports_errors_and_failures() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        assert!(matches!(
            a.ep.push("10.0.0.2:7000", &[]),
            Err(TransferEndpointError::NotInitialized(_))
        ));
        a.ep.initialize().unwrap();
        let r = RemoteRegion {
            addr: 0,
            rkey: 0,
            length: 8,
        };
        assert_eq!(
            a.ep.push("10.0.0.9:7000", &[(1, r)]).unwrap(),
            [EntryOutcome::Failed],
            "nobody listening"
        );
        a.ep.shutdown().unwrap();
    }

    /// The contract forbids `push` from a completion callback. Called there it
    /// is refused, not deadlocked: the callback runs on the endpoint thread,
    /// which `push` would wait on.
    #[test]
    fn push_from_a_completion_callback_is_refused_not_deadlocked() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        let (tx, rx) = mpsc::channel();
        let inner = Arc::clone(&a.ep);
        a.ep.push_async(
            "10.0.0.9:7000",
            &[],
            Box::new(move |_| {
                let _ = tx.send(inner.push("10.0.0.9:7000", &[]).map(|_| ()));
            }),
        )
        .unwrap();
        let r = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the callback ran and push returned instead of hanging");
        match r {
            Err(TransferEndpointError::Transport(m)) => assert!(m.contains("push_async"), "{m}"),
            other => panic!("{other:?}"),
        }
        a.ep.shutdown().unwrap();
    }

    // --- landing slots (T020) -------------------------------------------------

    /// A push lands in the destination's pool, named by the pool's own rkey:
    /// the pool is registered once and is where fetched entries land.
    #[test]
    fn a_push_lands_in_the_destinations_pool() {
        let f = MockFabric::new();
        let mut a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.pool[..16].copy_from_slice(b"landing-region!!");
        a.map.memory(1, a.pool_addr(0), 16);
        let h = Landing::of(&b);
        let slot = h.region(&b.ep, 64, 16);
        assert_eq!(
            a.ep.push("10.0.0.2:7000", &[(1, slot)]).unwrap(),
            [EntryOutcome::HitMemoryTier]
        );
        assert_eq!(&h[64..80], b"landing-region!!");
        assert_eq!(h[..64], [0u8; 64], "nothing outside the slot");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// The pool's rkey survives a reconnect: registration belongs to the
    /// endpoint, not to a connection (D17).
    #[test]
    fn the_pools_rkey_survives_a_reconnect() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        let slot = Landing::of(&b).region(&b.ep, 0, 8);
        assert_eq!(
            a.ep.push("10.0.0.2:7000", &[(1, slot)]).unwrap(),
            [EntryOutcome::HitMemoryTier]
        );
        a.ep.disconnect("10.0.0.2:7000").unwrap();
        eventually("both ends torn down", || f.qp_count() == 0);
        // A new connection, the same rkey.
        assert_eq!(
            a.ep.push("10.0.0.2:7000", &[(1, slot)]).unwrap(),
            [EntryOutcome::HitMemoryTier]
        );
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// A fetch lands only in the destination's own pool: a slot outside it,
    /// by one byte at either edge, is refused before anything is sent, and
    /// so is any slot while the endpoint is not running.
    #[test]
    fn fetch_refuses_a_slot_outside_the_pool() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let drop_it: fn() -> interfaces::FetchCompletion = || Box::new(|_| {});
        let base = a.pool_addr(0);
        let len = a.pool.len() as u64;
        let slot = |addr: u64, length: u32| [(1, LocalSlot { addr, length })];
        assert!(matches!(
            a.ep.fetch("10.0.0.2:7000", &slot(base, 8), drop_it()),
            Err(TransferEndpointError::NotInitialized(_))
        ));
        a.ep.initialize().unwrap();
        for (what, addr, length) in [
            ("one byte before", base - 1, 8),
            ("one byte past the end", base + len - 7, 8),
            ("wholly outside", base + 2 * len, 8),
            ("wrapping", u64::MAX - 3, 8),
        ] {
            match a.ep.fetch("10.0.0.2:7000", &slot(addr, length), drop_it()) {
                Err(TransferEndpointError::Transport(m)) => {
                    assert!(m.contains("not inside the memory-tier pool"), "{what}: {m}")
                }
                other => panic!("{what}: {other:?}"),
            }
        }
        let s = a.ep.stats();
        assert_eq!((s.requests_attempted, f.qp_count()), (0, 0), "nothing sent");
        // The whole pool, to its last byte, is a valid slot.
        assert!(a
            .ep
            .fetch("10.0.0.9:7000", &slot(base, len as u32), drop_it())
            .is_ok());
        a.ep.shutdown().unwrap();
    }

    // --- Phase 3 checkpoint (T021) --------------------------------------------

    /// SC-002's spirit: the default build — no `rdma`, no `spdk` — serves a
    /// memory-tier entry end to end through `ITransferEndpoint` itself, as a
    /// caller holding only the interface would.
    #[test]
    fn the_default_build_serves_through_the_interface() {
        let f = MockFabric::new();
        let mut a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.pool[..4].copy_from_slice(b"KVKV");
        a.map.memory(7, a.pool_addr(0), 4);
        let source: Arc<dyn ITransferEndpoint + Send + Sync> = a.ep.clone();
        let h = Landing::of(&b);
        assert_eq!(
            source
                .push("10.0.0.2:7000", &[(7, h.region(&b.ep, 0, 4))])
                .unwrap(),
            [EntryOutcome::HitMemoryTier]
        );
        assert_eq!(&h[..4], b"KVKV");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// Invariant 1 under load: a request posted across many credit rounds,
    /// with hits and misses interleaved, still reports index i for items[i].
    #[test]
    fn outcomes_keep_request_order_across_credit_rounds() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 3);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        let (land, rkey) = pool_key(&b.ep);
        let mut expect = Vec::new();
        let items: Vec<_> = (0..50u64)
            .map(|k| {
                // Every third key absent, every fifth (otherwise) the wrong size.
                match (k % 3, k % 5) {
                    (0, _) => expect.push(EntryOutcome::Miss),
                    (_, 0) => {
                        a.map.memory(k, a.pool_addr(k as usize * 8), 4);
                        expect.push(EntryOutcome::Miss);
                    }
                    _ => {
                        a.map.memory(k, a.pool_addr(k as usize * 8), 8);
                        expect.push(EntryOutcome::HitMemoryTier);
                    }
                }
                (
                    k,
                    RemoteRegion {
                        addr: land + k * 8,
                        rkey,
                        length: 8,
                    },
                )
            })
            .collect();
        assert_eq!(a.ep.push("10.0.0.2:7000", &items).unwrap(), expect);
        assert_eq!(a.map.held(), 0);
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// FR-002: repeated pushes to one peer reuse one connection.
    #[test]
    fn repeated_pushes_reuse_one_connection() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        let (land, rkey) = pool_key(&b.ep);
        let item = [(
            1,
            RemoteRegion {
                addr: land,
                rkey,
                length: 8,
            },
        )];
        for round in 0..5 {
            assert_eq!(
                a.ep.push("10.0.0.2:7000", &item).unwrap(),
                [EntryOutcome::HitMemoryTier],
                "round {round}"
            );
            assert_eq!(f.qp_count(), 2, "round {round}: still one connection");
        }
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// Invariant 8 as an order, from the mock's event log and the pin ledger
    /// on one shared sequence: on `disconnect` with writes in flight, the
    /// queue pair is forced to ERROR **before** any of their completions is
    /// taken, every pin is released only **after** a completion, and the link
    /// is destroyed only **after** every release.
    #[test]
    fn teardown_orders_error_then_drain_then_release_then_close() {
        use crate::connection::mock::{LogEvent, Sequence};
        let f = MockFabric::new();
        let seq = Sequence::default();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        let pb = PeerId::parse("10.0.0.2:7000").unwrap();
        let pb2 = pb.clone();
        a.ep.call(|r| Command::Connect(pb2, r)).unwrap().unwrap();
        eventually("connected", || {
            let p = pb.clone();
            a.ep.call(|r| Command::IsUp(p, r)).unwrap()
        });
        // Hold every send in flight from here on.
        f.pause_sends(true);
        for k in 0..3 {
            a.map.memory(k, a.pool_addr(k as usize * 8), 8);
        }
        let (land, rkey) = pool_key(&b.ep);
        let items: Vec<_> = (0..3u64)
            .map(|k| {
                (
                    k,
                    RemoteRegion {
                        addr: land + k * 8,
                        rkey,
                        length: 8,
                    },
                )
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        a.ep.push_async(
            "10.0.0.2:7000",
            &items,
            Box::new(move |o| tx.send(o).unwrap()),
        )
        .unwrap();
        eventually("three writes in flight", || a.map.held() == 3);

        f.record(&seq);
        a.map.record(&seq);
        a.ep.disconnect("10.0.0.2:7000").unwrap();
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Failed; 3]
        );

        let log = f.log();
        let releases = a.map.releases();
        let a_link = log
            .iter()
            .find_map(|(_, e)| match e {
                LogEvent::Completed(l, _) => Some(*l),
                _ => None,
            })
            .expect("a's link completed something");
        let stamps = |want: &dyn Fn(&LogEvent) -> bool| -> Vec<u64> {
            log.iter()
                .filter(|(_, e)| want(e))
                .map(|(s, _)| *s)
                .collect()
        };
        let errored = stamps(&|e| *e == LogEvent::ToError(a_link));
        let completed = stamps(&|e| matches!(e, LogEvent::Completed(l, _) if *l == a_link));
        let closed = stamps(&|e| *e == LogEvent::Closed(a_link));
        assert_eq!(errored.len(), 1, "forced to ERROR once: {log:?}");
        // The three writes, and the link's posted receives, all flushed.
        assert!(completed.len() >= 3, "the writes drained: {log:?}");
        assert_eq!(closed.len(), 1, "{log:?}");
        assert_eq!(releases.len(), 3, "three pins released: {releases:?}");
        let (err, close) = (errored[0], closed[0]);
        assert!(
            completed.iter().all(|&c| err < c),
            "ERROR before every completion: {log:?}"
        );
        for (r, key) in &releases {
            assert!(err < *r, "pin {key} released after ERROR");
            assert!(
                completed.iter().any(|&c| c < *r),
                "pin {key} released after a completion"
            );
            assert!(
                *r < close,
                "pin {key} released before the link is destroyed"
            );
        }
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    // --- connection bring-up ---------------------------------------------------

    fn session_of(ep: &CertusTransferEndpoint, p: &str) -> Option<endpoint::SessionView> {
        let p = PeerId::parse(p).unwrap();
        ep.call(|r| Command::SessionOf(p, r)).unwrap()
    }

    fn connect(a: &CertusTransferEndpoint, to: &str) {
        let p = PeerId::parse(to).unwrap();
        let p2 = p.clone();
        a.call(|r| Command::Connect(p2, r)).unwrap().unwrap();
        eventually("the capability exchange", || {
            let p = p.clone();
            a.call(|r| Command::IsUp(p, r)).unwrap()
        });
    }

    /// Send raw bytes to `to`, as a hostile or buggy peer would. With `spend`
    /// it takes a message credit first, as a genuine request would.
    fn inject_raw(ep: &CertusTransferEndpoint, to: &str, bytes: Vec<u8>, spend: bool) {
        let peer = PeerId::parse(to).unwrap();
        ep.call(|reply| Command::Inject {
            peer,
            bytes,
            spend,
            reply,
        })
        .unwrap()
        .unwrap();
    }

    #[test]
    fn configure_refuses_queues_that_cannot_hold_the_partition() {
        let ep = CertusTransferEndpoint::create();
        let mut c = EndpointConfig::new("10.0.0.1:7000");
        c.message_credits = 0;
        assert!(matches!(
            ep.configure(c.clone()),
            Err(TransferEndpointError::Transport(_))
        ));
        c.message_credits = 40;
        c.inbound_quota = 40;
        c.max_recv_wr = 64;
        match ep.configure(c) {
            Err(TransferEndpointError::Transport(m)) => assert!(m.contains("80"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// FR-025 end to end: both endpoints exchange OP_HELLO on connect, agree
    /// version 1, adopt each other's limits — the outbound limit is the lower
    /// of our credits and the peer's quota — and return the bootstrap credit.
    #[test]
    fn connecting_runs_the_capability_exchange_at_both_ends() {
        let f = MockFabric::new();
        let mk = |name: &str, credits, quota| {
            let n = node(&f, name, 8);
            let mut c = EndpointConfig::new(name);
            c.message_credits = credits;
            c.inbound_quota = quota;
            n.ep.configure(c).unwrap();
            n.ep.use_transport(TransportChoice::Mock(f.clone()));
            n
        };
        let a = mk("10.0.0.1:7000", 6, 2);
        let b = mk("10.0.0.2:7000", 4, 3);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready too", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let sa = session_of(&a.ep, "10.0.0.2:7000").unwrap();
        let sb = session_of(&b.ep, "10.0.0.1:7000").unwrap();
        assert_eq!((sa.version, sb.version), (Some(1), Some(1)));
        assert_eq!(sa.credits_available, 3, "a's 6 clamped to b's quota 3");
        assert_eq!(sb.credits_available, 2, "b's 4 clamped to a's quota 2");
        assert_eq!(sa.outbound, (0, 1), "the bootstrap credit came back");
        assert_eq!(sa.peer_max_entries, 340);
        assert_eq!(a.ep.stats().version_downgrades, 0);
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// A data message before the exchange completes is not sent: pushes wait
    /// (they need a Ready session), and a reconnect re-learns everything.
    #[test]
    fn a_reconnect_reruns_the_exchange() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        connect(&a.ep, "10.0.0.2:7000");
        a.ep.disconnect("10.0.0.2:7000").unwrap();
        assert!(
            session_of(&a.ep, "10.0.0.2:7000").is_none(),
            "dies with the link"
        );
        eventually("b torn down", || f.qp_count() == 0);
        connect(&a.ep, "10.0.0.2:7000");
        assert_eq!(session_of(&a.ep, "10.0.0.2:7000").unwrap().version, Some(1));
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// A peer whose OP_HELLO names no version in common fails the connection
    /// at establishment, with no request ever issued (SC-013). The peer is a
    /// raw transport, since an endpoint only ever advertises a good range.
    #[test]
    fn an_incompatible_version_fails_the_connection_at_bring_up() {
        use crate::connection::transport::{Transport, TransportEvent};
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        raw.listen(&p_raw).unwrap();
        let h = Landing::of(&a);
        // The fetch connects, and waits for the exchange that then fails.
        let rx = fetch_one(&a.ep, &h, 1);
        let link = accept_raw(&mut raw, &p_raw);
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries: 340,
                    max_outstanding_requests: 4,
                    version_min: 7,
                    version_max: 9,
                },
                out,
            )
        });
        let mut evs = Vec::new();
        eventually("a tears the connection down", || {
            raw.poll_events(&mut evs);
            evs.contains(&TransportEvent::Disconnected { link })
        });
        assert!(session_of(&a.ep, "10.0.0.3:7000").is_none());
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Err(TransferEndpointError::IncompatiblePeer(m)) => {
                assert!(m.contains("7..=9") && m.contains("1..=1"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        let s = a.ep.stats();
        assert_eq!(
            (s.chunks_sent, s.version_downgrades, s.protocol_faults),
            (0, 0, 1),
            "nothing issued, and the fault counted"
        );
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// Accept the connect a table makes to a raw transport, answering with the
    /// raw end's identity, and post it some receives. Returns its link.
    fn accept_raw(
        raw: &mut crate::connection::mock::MockTransport,
        me: &PeerId,
    ) -> crate::connection::transport::LinkId {
        use crate::connection::transport::{Transport, TransportEvent};
        let request = {
            let mut evs = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                raw.poll_events(&mut evs);
                if let Some(TransportEvent::ConnectRequest { request, .. }) = evs
                    .iter()
                    .find(|e| matches!(e, TransportEvent::ConnectRequest { .. }))
                {
                    break *request;
                }
                assert!(Instant::now() < deadline, "no connect request");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let id = crate::connection::identity::encode_identity(me).unwrap();
        let caps = crate::connection::transport::QpCaps {
            max_send_wr: 16,
            max_recv_wr: 16,
        };
        let link = raw.accept(request, &id, caps).unwrap();
        // Receives for whatever the table sends (its OP_HELLO, its replies).
        let buf: &'static mut [u8] =
            Box::leak(vec![0u8; 16 * message::MESSAGE_LEN].into_boxed_slice());
        // SAFETY: leaked, so it outlives everything.
        let r: &'static crate::connection::transport::Region = Box::leak(Box::new(
            unsafe {
                raw.register(
                    buf.as_mut_ptr(),
                    buf.len(),
                    crate::connection::transport::Access::LOCAL_WRITE,
                )
            }
            .unwrap(),
        ));
        let recvs: Vec<_> = (0..16u64)
            .map(|i| crate::connection::transport::RecvRequest {
                wr_id: 1000 + i,
                buf: r
                    .sge(
                        r.addr() + i * message::MESSAGE_LEN as u64,
                        message::MESSAGE_LEN,
                    )
                    .unwrap(),
            })
            .collect();
        // SAFETY: as above.
        unsafe { raw.link(link).unwrap().post_recv(&recvs) }.unwrap();
        link
    }

    /// Send one message from a raw transport's link.
    fn send_raw(
        raw: &mut crate::connection::mock::MockTransport,
        link: crate::connection::transport::LinkId,
        encode: impl FnOnce(&mut [u8]) -> usize,
    ) {
        use crate::connection::transport::{Access, SendOp, SendRequest, Transport};
        let buf: &'static mut [u8] = Box::leak(vec![0u8; message::MESSAGE_LEN].into_boxed_slice());
        let len = encode(buf);
        // SAFETY: leaked, so it outlives the send.
        let r: &'static crate::connection::transport::Region = Box::leak(Box::new(
            unsafe { raw.register(buf.as_mut_ptr(), buf.len(), Access::LOCAL_WRITE) }.unwrap(),
        ));
        // SAFETY: as above.
        unsafe {
            raw.link(link).unwrap().post_send(&[SendRequest {
                wr_id: 1,
                op: SendOp::Send,
                local: r.sge(r.addr(), len).unwrap(),
                signaled: false,
            }])
        }
        .unwrap();
    }

    /// FR-025: no data before the peer's OP_HELLO. The peer here is a raw
    /// transport that accepts and posts receives but never says hello, so a
    /// push to it must stay unsent — not one RDMA write may reach the wire —
    /// until shutdown fails it.
    #[test]
    fn no_data_is_sent_before_the_peers_hello() {
        use crate::connection::mock::{LogEvent, Sequence};
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        crate::connection::transport::Transport::listen(&mut raw, &p_raw).unwrap();
        let (tx, rx) = mpsc::channel();
        a.ep.push_async(
            "10.0.0.3:7000",
            &[(
                1,
                RemoteRegion {
                    addr: 0,
                    rkey: 0,
                    length: 8,
                },
            )],
            Box::new(move |o| tx.send(o).unwrap()),
        )
        .unwrap();
        let seq = Sequence::default();
        f.record(&seq);
        let _link = accept_raw(&mut raw, &p_raw);
        std::thread::sleep(Duration::from_millis(50));
        assert!(rx.try_recv().is_err(), "the push is still waiting");
        assert_eq!(a.map.lookups(), 0, "not even looked up: nothing pinned");
        // a's only send so far is its OP_HELLO; a write would complete too.
        let a_sends = f
            .log()
            .iter()
            .filter(|(_, e)| matches!(e, LogEvent::Completed(..)))
            .count();
        assert!(a_sends <= 1, "only OP_HELLO went out: {:?}", f.log());
        a.ep.shutdown().unwrap();
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Failed]
        );
        drop(raw);
    }

    /// An unprocessable *reply* is never answered: it is a teardown (FR-024).
    /// Exceeding the inbound quota is the other request-side teardown;
    /// `requests_past_the_quota_tear_down` covers it.
    #[test]
    fn a_reply_nobody_asked_for_tears_down() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let mut msg = vec![0u8; message::MESSAGE_LEN];
        let n = message::encode_reply(1, 99, &[EntryOutcome::Miss], &mut msg);
        inject_raw(&a.ep, "10.0.0.2:7000", msg[..n].to_vec(), false);
        eventually("b tears down", || {
            session_of(&b.ep, "10.0.0.1:7000").is_none()
        });
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// SC-012, from the answering side: a request of unknown kind (an even
    /// opcode) is answered with OP_ERROR naming it — within a round trip,
    /// never left to a deadline — and the answering end stays connected,
    /// counting the refusal it sent. The OP_ERROR comes back for a request
    /// the injecting end never really made, so *that* end treats it as a bad
    /// reply and tears down, which is the right answer there; the next test
    /// covers an OP_ERROR for a genuine fetch.
    #[test]
    fn an_unknown_opcode_is_answered_and_the_answerer_survives() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let mut msg = vec![0u8; message::HEADER_LEN];
        msg[0] = 0x42; // no such kind, and even: a request
        inject_raw(&a.ep, "10.0.0.2:7000", msg, false);
        eventually("a receives b's OP_ERROR, and tears down", || {
            a.ep.stats().protocol_faults == 1 && session_of(&a.ep, "10.0.0.2:7000").is_none()
        });
        assert_eq!(b.ep.stats().protocol_faults, 1, "b counts the refusal");
        assert_eq!(
            b.ep.stats().deadline_expiries,
            0,
            "b answered within a round trip"
        );
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// SC-012, from the requesting side: an OP_ERROR for one of our fetches
    /// fails exactly that fetch, and the connection and a sibling fetch on it
    /// complete normally. The refusal comes from a raw peer, since an endpoint
    /// only refuses what it cannot parse.
    #[test]
    fn an_op_error_fails_one_fetch_and_spares_its_siblings() {
        use crate::connection::transport::Transport;
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        raw.listen(&p_raw).unwrap();
        let p2 = p_raw.clone();
        a.ep.call(|r| Command::Connect(p2, r)).unwrap().unwrap();
        let link = accept_raw(&mut raw, &p_raw);
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries: 340,
                    max_outstanding_requests: 4,
                    version_min: 1,
                    version_max: 1,
                },
                out,
            )
        });
        send_raw(&mut raw, link, message::encode_hello_reply);
        eventually("a ready", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.credits_available > 0)
        });
        let h = Landing::of(&a);
        let (tx, rx) = mpsc::channel();
        for which in [0, 1] {
            let tx = tx.clone();
            a.ep.fetch(
                "10.0.0.3:7000",
                &[(which, h.slot(which as usize * 8, 8))],
                Box::new(move |o| tx.send((which, o)).unwrap()),
            )
            .unwrap();
        }
        eventually("both fetches out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 2)
        });
        // a numbers its contexts from 0 per connection: refuse context 0,
        // answer context 1 with a miss.
        send_raw(&mut raw, link, |out| {
            message::encode_error(
                &message::ErrorReply {
                    context_id: 0,
                    offending_opcode: message::Kind::Fetch.opcode(),
                    reason: message::Reason::Malformed,
                    max_supported_version: 1,
                },
                out,
            )
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 1, &[EntryOutcome::Miss], out)
        });
        let mut got: Vec<_> = (0..2)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        got.sort_by_key(|(w, _)| *w);
        assert!(
            matches!(&got[0], (0, Err(TransferEndpointError::Protocol(_)))),
            "the refused fetch fails as a whole: {:?}",
            got[0]
        );
        assert_eq!(
            hits(got.remove(1).1),
            [EntryOutcome::Miss],
            "its sibling is answered"
        );
        assert!(
            session_of(&a.ep, "10.0.0.3:7000").is_some(),
            "the connection stays"
        );
        assert_eq!(a.ep.stats().protocol_faults, 1);
        let _ = raw.link(link);
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    // --- fetch ------------------------------------------------------------------

    /// SC-001: the destination names one peer and a run of keys; the values
    /// land in its slots and the outcomes come back in request order. There is
    /// no zyre receptacle at all (FR-004, structurally), and no third endpoint
    /// on the fabric hears a single message.
    #[test]
    fn a_fetch_lands_values_from_the_one_named_peer() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let mut src = node(&f, "10.0.0.2:7000", 8);
        let bystander = node(&f, "10.0.0.3:7000", 8);
        for n in [&dest, &src, &bystander] {
            n.ep.initialize().unwrap();
        }
        src.pool[..16].copy_from_slice(b"first...second..");
        src.map.memory(10, src.pool_addr(0), 8);
        src.map.memory(12, src.pool_addr(8), 8);
        let h = Landing::of(&dest);
        let items = [
            (10, h.slot(0, 8)),
            (11, h.slot(8, 8)), // the source lacks it
            (12, h.slot(16, 8)),
        ];
        let (tx, rx) = mpsc::channel();
        let ctx = dest
            .ep
            .fetch(
                "10.0.0.2:7000",
                &items,
                Box::new(move |o| tx.send(o).unwrap()),
            )
            .unwrap();
        let out = hits(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        use EntryOutcome::*;
        assert_eq!(out, [HitMemoryTier, Miss, HitMemoryTier]);
        assert_eq!(&h[0..8], b"first...");
        assert_eq!(h[8..16], [0u8; 8], "nothing for the miss");
        assert_eq!(&h[16..24], b"second..");
        assert!(ctx.0 > 0);
        assert_eq!(src.map.held(), 0, "the source released every pin");
        assert_eq!(bystander.map.lookups(), 0);
        assert!(
            session_of(&bystander.ep, "10.0.0.1:7000").is_none(),
            "never contacted"
        );
        assert!(
            session_of(&bystander.ep, "10.0.0.2:7000").is_none(),
            "never contacted"
        );
        let s = dest.ep.stats();
        assert_eq!((s.requests_attempted, s.requests_completed), (1, 1));
        assert_eq!((s.hit_memory_tier, s.miss), (2, 1));
        for n in [&dest, &src, &bystander] {
            n.ep.shutdown().unwrap();
        }
    }

    /// SC-006: a fetch for keys the peer lacks returns all Miss and causes no
    /// further traffic — no retry, no other peer asked (invariant 3, D12).
    #[test]
    fn an_all_miss_fetch_is_answered_and_nothing_follows() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        let h = Landing::of(&dest);
        let items: Vec<_> = (0..4u64).map(|k| (k, h.slot(k as usize * 8, 8))).collect();
        let (tx, rx) = mpsc::channel();
        dest.ep
            .fetch(
                "10.0.0.2:7000",
                &items,
                Box::new(move |o| tx.send(o).unwrap()),
            )
            .unwrap();
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Miss; 4]
        );
        std::thread::sleep(Duration::from_millis(20));
        let sd = session_of(&dest.ep, "10.0.0.2:7000").unwrap();
        assert_eq!(sd.outbound, (0, 1), "exactly one request ever went out");
        assert_eq!(src.map.lookups(), 4, "one lookup per key, and no more");
        assert_eq!(h[..32], [0u8; 32]);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// T034: the source replies only after every write for a hit has
    /// completed. With the source's sends held in flight, the destination's
    /// callback must not fire — a reply sent early would let it.
    #[test]
    fn the_reply_waits_for_the_writes() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        src.map.memory(1, src.pool_addr(0), 8);
        connect(&dest.ep, "10.0.0.2:7000");
        eventually("src ready", || {
            session_of(&src.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let h = Landing::of(&dest);
        // Pause, then fetch: the request itself is held until released, so
        // release only the request, by letting sends run until the source has
        // the pin — its write posted — and pausing again before it executes.
        f.pause_sends(true);
        let (tx, rx) = mpsc::channel();
        dest.ep
            .fetch(
                "10.0.0.2:7000",
                &[(1, h.slot(0, 8))],
                Box::new(move |o| tx.send(o).unwrap()),
            )
            .unwrap();
        eventually("the request is queued", || {
            session_of(&dest.ep, "10.0.0.2:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        f.step_sends_until(|| src.map.held() == 1);
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            rx.try_recv().is_err(),
            "no reply while the write is in flight"
        );
        assert_eq!(dest.ep.stats().requests_outstanding_outbound, 1);
        assert_eq!(src.ep.stats().requests_in_service_inbound, 1);
        f.pause_sends(false);
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::HitMemoryTier]
        );
        eventually("both gauges back to zero", || {
            dest.ep.stats().requests_outstanding_outbound == 0
                && src.ep.stats().requests_in_service_inbound == 0
        });
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// FR-026 / D22: one connection, both directions, separate books. Each
    /// end fetches from the other at once; each numbers its requests from 0,
    /// so context 0 is live in both directions, and the two never cross.
    #[test]
    fn the_same_context_id_live_both_ways_is_two_requests() {
        let f = MockFabric::new();
        let mut a = node(&f, "10.0.0.1:7000", 8);
        let mut b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.pool[..4].copy_from_slice(b"AAAA");
        b.pool[..4].copy_from_slice(b"BBBB");
        a.map.memory(1, a.pool_addr(0), 4);
        b.map.memory(2, b.pool_addr(0), 4);
        let ha = Landing::of(&a);
        let hb = Landing::of(&b);
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        f.pause_sends(true); // both requests out before either is served
        let (tx, rx) = mpsc::channel();
        let ta = tx.clone();
        a.ep.fetch(
            "10.0.0.2:7000",
            &[(2, ha.slot(0, 4))],
            Box::new(move |o| ta.send(("a", o)).unwrap()),
        )
        .unwrap();
        b.ep.fetch(
            "10.0.0.1:7000",
            &[(1, hb.slot(0, 4))],
            Box::new(move |o| tx.send(("b", o)).unwrap()),
        )
        .unwrap();
        eventually("both requests queued", || {
            session_of(&a.ep, "10.0.0.2:7000").is_some_and(|s| s.outbound.0 == 1)
                && session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        f.pause_sends(false);
        let mut got: Vec<_> = (0..2)
            .map(|_| {
                let (w, r) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                (w, hits(r))
            })
            .collect();
        got.sort_by_key(|(w, _)| *w);
        assert_eq!(
            got,
            [
                ("a", vec![EntryOutcome::HitMemoryTier]),
                ("b", vec![EntryOutcome::HitMemoryTier])
            ]
        );
        assert_eq!(&ha[..4], b"BBBB", "a got b's value");
        assert_eq!(&hb[..4], b"AAAA", "b got a's value");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// FR-026's quota, now reachable end to end: serving is asynchronous, so
    /// requests pile up in service. A peer sending past the quota it was
    /// granted is torn down.
    #[test]
    fn requests_past_the_quota_tear_down() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        b.map.memory(1, b.pool_addr(0), 8);
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let (land, rkey) = pool_key(&a.ep);
        let region = RemoteRegion {
            addr: land,
            rkey,
            length: 8,
        };
        // Hold b's writes in flight so its fetches stay in service, then send
        // nine against a quota of eight, bypassing a's own credits.
        f.pause_sends(true);
        let mut msg = vec![0u8; message::MESSAGE_LEN];
        for ctx in 100..109u64 {
            let n = message::encode_fetch(1, ctx, &[(1, region)], &mut msg);
            inject_raw(&a.ep, "10.0.0.2:7000", msg[..n].to_vec(), false);
        }
        std::thread::sleep(Duration::from_millis(20));
        f.pause_sends(false);
        eventually("b tears down", || {
            session_of(&b.ep, "10.0.0.1:7000").is_none()
        });
        assert_eq!(b.map.held(), 0, "every pin released through the drain");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// Message credits block, never fail (FR-015): more fetches than credits
    /// all complete.
    #[test]
    fn more_fetches_than_credits_all_complete() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        src.map.memory(1, src.pool_addr(0), 8);
        let h = Landing::of(&dest);
        let (tx, rx) = mpsc::channel();
        for i in 0..20usize {
            let tx = tx.clone();
            dest.ep
                .fetch(
                    "10.0.0.2:7000",
                    &[(1, h.slot(i * 8, 8))],
                    Box::new(move |o| tx.send(o).unwrap()),
                )
                .unwrap();
        }
        for _ in 0..20 {
            assert_eq!(
                hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
                [EntryOutcome::HitMemoryTier]
            );
        }
        let s = session_of(&dest.ep, "10.0.0.2:7000").unwrap();
        let credits = EndpointConfig::new("x:1").message_credits;
        assert!(
            s.outbound.1 <= credits,
            "never more than the {credits} credits out: {s:?}"
        );
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// A fetch in flight when its connection is torn down reports Failed —
    /// and only after the teardown, so its slots are safe to reclaim.
    #[test]
    fn a_fetch_lost_to_a_teardown_fails_after_the_barrier() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        src.map.memory(1, src.pool_addr(0), 8);
        connect(&dest.ep, "10.0.0.2:7000");
        let h = Landing::of(&dest);
        f.pause_sends(true);
        let (tx, rx) = mpsc::channel();
        dest.ep
            .fetch(
                "10.0.0.2:7000",
                &[(1, h.slot(0, 8))],
                Box::new(move |o| tx.send(o).unwrap()),
            )
            .unwrap();
        eventually("the fetch is out", || {
            session_of(&dest.ep, "10.0.0.2:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        dest.ep.disconnect("10.0.0.2:7000").unwrap();
        // disconnect returns only after the barrier, and the callback fired
        // before it returned: nothing can still write into the slot.
        assert!(
            matches!(
                rx.try_recv().unwrap(),
                Err(TransferEndpointError::Transport(_))
            ),
            "a fetch with no reply fails as a whole"
        );
        f.pause_sends(false);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// A raw peer at 10.0.0.3:7000 that a has connected to and greeted, with
    /// `max_entries` as given: (a, the raw transport, its link, a's landing
    /// half).
    fn raw_peer(
        f: &MockFabric,
        max_entries: u32,
    ) -> (
        Node,
        crate::connection::mock::MockTransport,
        crate::connection::transport::LinkId,
        Landing,
    ) {
        raw_peer_with(f, max_entries, |_| {})
    }

    /// [`raw_peer`], with a's configuration adjusted by `tweak`.
    fn raw_peer_with(
        f: &MockFabric,
        max_entries: u32,
        tweak: impl FnOnce(&mut EndpointConfig),
    ) -> (
        Node,
        crate::connection::mock::MockTransport,
        crate::connection::transport::LinkId,
        Landing,
    ) {
        use crate::connection::transport::Transport;
        let a = node_with(f, "10.0.0.1:7000", tweak);
        a.ep.initialize().unwrap();
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        raw.listen(&p_raw).unwrap();
        let p2 = p_raw.clone();
        a.ep.call(|r| Command::Connect(p2, r)).unwrap().unwrap();
        let link = accept_raw(&mut raw, &p_raw);
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries,
                    max_outstanding_requests: 4,
                    version_min: 1,
                    version_max: 1,
                },
                out,
            )
        });
        send_raw(&mut raw, link, message::encode_hello_reply);
        eventually("a ready", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.credits_available > 0)
        });
        let h = Landing::of(&a);
        (a, raw, link, h)
    }

    fn fetch_one(
        a: &CertusTransferEndpoint,
        h: &Landing,
        n: usize,
    ) -> mpsc::Receiver<interfaces::FetchResult> {
        let (tx, rx) = mpsc::channel();
        let items: Vec<_> = (0..n).map(|i| (i as u64, h.slot(i * 8, 8))).collect();
        a.fetch(
            "10.0.0.3:7000",
            &items,
            // `let _`: a test may stop listening before the fetch completes,
            // and a callback that panics on the endpoint thread aborts.
            Box::new(move |o| {
                let _ = tx.send(o);
            }),
        )
        .unwrap();
        rx
    }

    /// A reply with the wrong number of outcomes is a bad reply: torn down,
    /// and the fetch fails — never a truncated or padded answer.
    #[test]
    fn a_reply_of_the_wrong_length_tears_down() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        let rx = fetch_one(&a.ep, &h, 2);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::HitMemoryTier], out)
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Transport(_))
        ));
        eventually("a tears down", || {
            session_of(&a.ep, "10.0.0.3:7000").is_none()
        });
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// An OP_ERROR is matched by context **and** kind: one naming our
    /// fetch's context but another kind answers nothing we sent, and is a
    /// teardown, not a refusal of the fetch.
    #[test]
    fn an_op_error_naming_the_wrong_kind_tears_down() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        let rx = fetch_one(&a.ep, &h, 1);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_error(
                &message::ErrorReply {
                    context_id: 0,
                    offending_opcode: message::Kind::Hello.opcode(),
                    reason: message::Reason::Malformed,
                    max_supported_version: 1,
                },
                out,
            )
        });
        eventually("a tears down", || {
            session_of(&a.ep, "10.0.0.3:7000").is_none()
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Transport(_))
        ));
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// An unknown *odd* opcode is a reply this end cannot read: it may
    /// return a credit or not, so the credits are in doubt and the
    /// connection goes, counted — never answered.
    #[test]
    fn an_unknown_reply_tears_down_unanswered() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        let rx = fetch_one(&a.ep, &h, 1);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            out[..message::HEADER_LEN].fill(0);
            out[0] = 0x41;
            message::HEADER_LEN
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Transport(_))
        ));
        let s = a.ep.stats();
        assert_eq!(s.protocol_faults, 1, "counted");
        assert_eq!(s.deadline_expiries, 0, "not left to a deadline");
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// FR-024: a bad reply's teardown is counted as a protocol fault, so a
    /// teardown loop shows in `stats()`.
    #[test]
    fn a_bad_reply_teardown_is_counted() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        let rx = fetch_one(&a.ep, &h, 2);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::Miss], out)
        });
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        assert_eq!(a.ep.stats().protocol_faults, 1);
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// A second OP_HELLO is answered with OP_ERROR, counted, and the session
    /// already agreed stands: a fetch on it still completes.
    #[test]
    fn a_duplicate_hello_is_refused_and_the_session_stands() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries: 340,
                    max_outstanding_requests: 4,
                    version_min: 1,
                    version_max: 1,
                },
                out,
            )
        });
        eventually("a refuses it", || a.ep.stats().protocol_faults == 1);
        let rx = fetch_one(&a.ep, &h, 1);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::Miss], out)
        });
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Miss]
        );
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// FR-024: a request before the peer's OP_HELLO is answered with
    /// OP_ERROR, counted, and the connection stays: once the peer says hello,
    /// the session comes up as usual.
    #[test]
    fn a_request_before_hello_is_refused_and_the_connection_stays() {
        use crate::connection::transport::Transport;
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        a.ep.initialize().unwrap();
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        raw.listen(&p_raw).unwrap();
        let p2 = p_raw.clone();
        a.ep.call(|r| Command::Connect(p2, r)).unwrap().unwrap();
        let link = accept_raw(&mut raw, &p_raw);
        send_raw(&mut raw, link, |out| message::encode_fetch(0, 5, &[], out));
        eventually("a refuses it", || a.ep.stats().protocol_faults == 1);
        assert!(
            session_of(&a.ep, "10.0.0.3:7000").is_some(),
            "still connected"
        );
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries: 340,
                    max_outstanding_requests: 4,
                    version_min: 1,
                    version_max: 1,
                },
                out,
            )
        });
        send_raw(&mut raw, link, message::encode_hello_reply);
        eventually("a ready", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.credits_available > 0)
        });
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// FR-025, T044: a fetch is chunked against the **peer's** max_entries,
    /// not ours. Against a peer taking 2, five entries go out as three
    /// messages, each its own context id, each costing its own credit; the
    /// replies come back in reverse and are placed by each chunk's offset.
    #[test]
    fn a_fetch_is_chunked_against_the_peers_max_entries() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 2);
        let rx = fetch_one(&a.ep, &h, 5);
        eventually("three chunks are out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 3)
        });
        assert_eq!(a.ep.stats().chunks_sent, 3);
        use EntryOutcome::*;
        // Context ids are per connection, from 0: chunks 0, 1, 2. Answer the
        // last first, each with outcomes that say which chunk they answer.
        for (ctx, outcomes) in [
            (2u64, vec![HitMemoryTier]),
            (0, vec![Miss, HitMemoryTier]),
            (1, vec![Failed, Miss]),
        ] {
            assert!(
                rx.try_recv().is_err(),
                "no completion before the last chunk"
            );
            send_raw(&mut raw, link, |out| {
                message::encode_reply(1, ctx, &outcomes, out)
            });
        }
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [Miss, HitMemoryTier, Failed, Miss, HitMemoryTier],
            "in request order, whatever order the replies came in"
        );
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// A chunk's reply of the wrong length is a bad reply, even when it is
    /// the right length for the whole fetch.
    #[test]
    fn a_chunk_reply_must_match_its_chunk() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 2);
        let rx = fetch_one(&a.ep, &h, 3);
        eventually("two chunks are out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 2)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 1, &[EntryOutcome::Miss; 3], out)
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Transport(_))
        ));
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// Decision (a): one refused chunk fails the whole fetch with Protocol —
    /// but only once its sibling chunks have settled, since they may be
    /// writing into its slots. Unsent chunks are not sent.
    #[test]
    fn a_refused_chunk_fails_the_fetch_after_its_siblings_settle() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 1);
        // Peer quota 4, our credits 8: four chunks out, two more waiting.
        let rx = fetch_one(&a.ep, &h, 6);
        eventually("four chunks are out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 4)
        });
        let refuse = message::ErrorReply {
            context_id: 1,
            offending_opcode: message::Kind::Fetch.opcode(),
            reason: message::Reason::Other(9),
            max_supported_version: 1,
        };
        send_raw(&mut raw, link, |out| message::encode_error(&refuse, out));
        for ctx in [0u64, 2] {
            send_raw(&mut raw, link, |out| {
                message::encode_reply(1, ctx, &[EntryOutcome::HitMemoryTier], out)
            });
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "chunk 3 is still out"
        );
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 3, &[EntryOutcome::Miss], out)
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Protocol(m)) if m.contains("chunk 1")
        ));
        let s = session_of(&a.ep, "10.0.0.3:7000").expect("the connection stays up");
        assert_eq!(s.outbound.0, 0, "the two unsent chunks were never sent");
        assert_eq!(a.ep.stats().chunks_sent, 4);
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// SC-016 end to end: both ends fetch from each other at full quota at
    /// once, and every fetch completes. Progress alone cannot show the reply
    /// reserve is there: a receive is copied out and re-posted as soon as it
    /// is handled, so a buffer shortage only stalls a sender on RNR retry, it
    /// never wedges both ends. What the reserve guarantees is that **no send
    /// ever finds its peer without a buffer** — requests use the quota,
    /// replies the reserve — so the mock's RNR-stall count must stay zero.
    #[test]
    fn two_way_traffic_at_full_quota_makes_progress() {
        let f = MockFabric::new();
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        a.map.memory(1, a.pool_addr(0), 8);
        b.map.memory(1, b.pool_addr(0), 8);
        let n = 24; // several times the credits, in each direction
        let ha = Landing::of(&a);
        let hb = Landing::of(&b);
        connect(&a.ep, "10.0.0.2:7000");
        eventually("b ready", || {
            session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available > 0)
        });
        let (tx, rx) = mpsc::channel();
        for i in 0..n {
            for (ep, to, h) in [(&a.ep, "10.0.0.2:7000", &ha), (&b.ep, "10.0.0.1:7000", &hb)] {
                let tx = tx.clone();
                ep.fetch(
                    to,
                    &[(1, h.slot(i * 8, 8))],
                    Box::new(move |o| {
                        let _ = tx.send(o);
                    }),
                )
                .unwrap();
            }
        }
        for _ in 0..2 * n {
            assert_eq!(
                hits(
                    rx.recv_timeout(Duration::from_secs(10))
                        .expect("no deadlock")
                ),
                [EntryOutcome::HitMemoryTier]
            );
        }
        let sa = session_of(&a.ep, "10.0.0.2:7000").unwrap();
        let sb = session_of(&b.ep, "10.0.0.1:7000").unwrap();
        let quota = EndpointConfig::new("x:1").inbound_quota;
        assert!(
            sa.inbound.1 <= quota && sb.inbound.1 <= quota,
            "quota {quota} held: {sa:?} {sb:?}"
        );
        assert_eq!(
            f.rnr_stalls(),
            0,
            "no message waited for a buffer, bring-up included"
        );
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    /// FR-026: a link's receives are posted before the peer can send. With
    /// no RNR retry at all, the first OP_HELLO each way — sent the instant
    /// each end sees the link up — must find a receive, or the connection
    /// goes to ERROR. Many bring-ups, so a race lost even rarely shows; and
    /// both directions of initiation, since each end's first receives travel
    /// a different path (with the connect, with the accept).
    #[test]
    fn the_first_messages_find_receives_posted() {
        let f = MockFabric::new();
        f.rnr_retry_limit(Some(0));
        let a = node(&f, "10.0.0.1:7000", 8);
        let b = node(&f, "10.0.0.2:7000", 8);
        a.ep.initialize().unwrap();
        b.ep.initialize().unwrap();
        for round in 0..200 {
            let (from, to, name) = if round % 2 == 0 {
                (&a.ep, &b.ep, "10.0.0.2:7000")
            } else {
                (&b.ep, &a.ep, "10.0.0.1:7000")
            };
            let back = if round % 2 == 0 {
                "10.0.0.1:7000"
            } else {
                "10.0.0.2:7000"
            };
            connect(from, name);
            eventually("both ends ready", || {
                session_of(from, name).is_some_and(|s| s.credits_available > 0)
                    && session_of(to, back).is_some_and(|s| s.credits_available > 0)
            });
            from.disconnect(name).unwrap();
            eventually("both ends torn down", || f.qp_count() == 0);
        }
        assert_eq!(f.rnr_stalls(), 0, "no first message waited for a buffer");
        a.ep.shutdown().unwrap();
        b.ep.shutdown().unwrap();
    }

    // --- Phase 8: reply deadlines ------------------------------------------

    fn fast_deadline(c: &mut EndpointConfig) {
        c.fetch_deadline = Some(Duration::from_millis(50));
    }

    /// T052/T071: a peer that never replies. The fetch fails with `Deadline`,
    /// naming the peer, keys, context and queue-pair state; its sibling on the
    /// same connection is aborted with `Transport`; both callbacks fire only
    /// after the queue pair went to ERROR and was destroyed; and a write the
    /// peer attempts afterwards lands nowhere.
    #[test]
    fn an_expired_deadline_errors_the_qp_then_fails_every_fetch() {
        use crate::connection::mock::{LogEvent, Sequence};
        use crate::connection::transport::{Access, SendOp, SendRequest, Transport};
        let f = MockFabric::new();
        let seq = Sequence::default();
        let (a, mut raw, link, h) = raw_peer_with(&f, 340, fast_deadline);
        f.record(&seq);
        let stamped = |rx: mpsc::Receiver<interfaces::FetchResult>| {
            let (tx, out) = mpsc::channel();
            let seq = seq.clone();
            std::thread::spawn(move || {
                if let Ok(r) = rx.recv_timeout(Duration::from_secs(5)) {
                    let _ = tx.send((r, seq.next()));
                }
            });
            out
        };
        // The stamp is taken after the callback, so it orders after it too.
        let expired = stamped(fetch_one(&a.ep, &h, 2));
        eventually("the first fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        let (tx, rx) = mpsc::channel();
        a.ep.fetch(
            "10.0.0.3:7000",
            &[(7, h.slot(16, 8))],
            Box::new(move |o| {
                let _ = tx.send(o);
            }),
        )
        .unwrap();
        let sibling = stamped(rx);
        let (r, done) = expired.recv_timeout(Duration::from_secs(5)).unwrap();
        let Err(TransferEndpointError::Deadline(why)) = r else {
            panic!("expected Deadline, got {r:?}");
        };
        for part in [
            "10.0.0.3:7000",
            "keys 0..=1",
            "2 entries",
            "context 0",
            "queue pair Rts",
        ] {
            assert!(why.contains(part), "{part:?} missing from {why:?}");
        }
        let (r, sib_done) = sibling.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(&r, Err(TransferEndpointError::Transport(m)) if m.contains("aborted")),
            "{r:?}"
        );
        let log = f.log();
        let first =
            |want: &dyn Fn(&LogEvent) -> bool| log.iter().find(|(_, e)| want(e)).map(|(s, _)| *s);
        let errored = first(&|e| matches!(e, LogEvent::ToError(_))).expect("forced to ERROR");
        let closed = first(&|e| matches!(e, LogEvent::Closed(_))).expect("destroyed");
        assert!(
            errored < closed && closed < done && closed < sib_done,
            "{log:?}"
        );
        let s = a.ep.stats();
        assert_eq!(s.deadline_expiries, 1);
        assert_eq!(s.requests_attempted, 2);
        assert!(session_of(&a.ep, "10.0.0.3:7000").is_none(), "torn down");

        // The late write: the peer finally answers, into the slot.
        let late: &'static mut [u8] = Box::leak(b"LATE!!!!".to_vec().into_boxed_slice());
        // SAFETY: leaked, so it outlives the send.
        let r: &'static crate::connection::transport::Region = Box::leak(Box::new(
            unsafe { raw.register(late.as_mut_ptr(), late.len(), Access::LOCAL_WRITE) }.unwrap(),
        ));
        let slot = h.region(&a.ep, 0, 8);
        let l = raw.link(link).expect("the peer's end is still alive");
        {
            // SAFETY: as above.
            unsafe {
                l.post_send(&[SendRequest {
                    wr_id: 9,
                    op: SendOp::RdmaWrite {
                        remote_addr: slot.addr,
                        rkey: slot.rkey,
                    },
                    local: r.sge(r.addr(), 8).unwrap(),
                    signaled: true,
                }])
            }
            .expect("the peer can still post");
        }
        let mut done = Vec::new();
        eventually("the late write completes", || {
            raw.link(link).unwrap().poll(&mut done, 16).unwrap();
            done.iter().any(|c| c.wr_id == 9)
        });
        let c = done.iter().find(|c| c.wr_id == 9).unwrap();
        assert!(c.result.is_err(), "the late write was refused: {c:?}");
        assert_eq!(h[..8], [0u8; 8], "no write lands after expiry");
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// With no deadline configured, a silent peer is waited on: nothing
    /// expires, and a reply long after still completes the fetch.
    #[test]
    fn no_deadline_waits_for_the_reply() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer_with(&f, 340, |c| c.fetch_deadline = None);
        let rx = fetch_one(&a.ep, &h, 1);
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "still waiting"
        );
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::Miss], out)
        });
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Miss]
        );
        assert_eq!(a.ep.stats().deadline_expiries, 0);
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// The deadline runs from the send, not the `fetch` call (FR-016): a fetch
    /// held back — here until the capability exchange completes — does not
    /// expire however long it waits.
    #[test]
    fn a_fetch_not_yet_sent_does_not_expire() {
        use crate::connection::transport::Transport;
        let f = MockFabric::new();
        let a = node_with(&f, "10.0.0.1:7000", fast_deadline);
        a.ep.initialize().unwrap();
        let mut raw = crate::connection::mock::MockTransport::new(f.clone());
        let p_raw = PeerId::parse("10.0.0.3:7000").unwrap();
        raw.listen(&p_raw).unwrap();
        let p2 = p_raw.clone();
        a.ep.call(|r| Command::Connect(p2, r)).unwrap().unwrap();
        let link = accept_raw(&mut raw, &p_raw);
        send_raw(&mut raw, link, |out| {
            message::encode_hello(
                &message::Hello {
                    max_entries: 340,
                    max_outstanding_requests: 4,
                    version_min: 1,
                    version_max: 1,
                },
                out,
            )
        });
        // No OP_HELLO_REPLY yet: the bootstrap credit is still out.
        let h = Landing::of(&a);
        let rx = fetch_one(&a.ep, &h, 1);
        assert!(
            rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "unsent for 5x the deadline, and not expired"
        );
        assert_eq!(a.ep.stats().deadline_expiries, 0);
        send_raw(&mut raw, link, message::encode_hello_reply);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::Miss], out)
        });
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Miss]
        );
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// `ReportPartial` is deferred: refused at configure, never accepted and
    /// silently treated as `FailLoud`.
    #[test]
    fn report_partial_is_refused_at_configure() {
        let ep = CertusTransferEndpoint::create();
        let mut c = EndpointConfig::new("10.0.0.1:7000");
        c.deadline_action = interfaces::DeadlineAction::ReportPartial;
        assert!(matches!(
            ep.configure(c),
            Err(TransferEndpointError::Transport(m)) if m.contains("ReportPartial")
        ));
        ep.configure(EndpointConfig::new("10.0.0.1:7000")).unwrap();
    }

    /// SC-010: after an expiry, the next fetch to that peer reconnects lazily
    /// and lands in the same pool — its rkey survived.
    #[test]
    fn after_an_expiry_the_next_fetch_reconnects_and_lands_in_the_same_pool() {
        let f = MockFabric::new();
        let dest = node_with(&f, "10.0.0.1:7000", fast_deadline);
        let mut src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        src.pool[..8].copy_from_slice(b"value...");
        src.map.memory(1, src.pool_addr(0), 8);
        connect(&dest.ep, "10.0.0.2:7000");
        let h = Landing::of(&dest);
        let fetch = || {
            let (tx, rx) = mpsc::channel();
            dest.ep
                .fetch(
                    "10.0.0.2:7000",
                    &[(1, h.slot(0, 8))],
                    Box::new(move |o| {
                        let _ = tx.send(o);
                    }),
                )
                .unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap()
        };
        f.pause_sends(true);
        assert!(matches!(fetch(), Err(TransferEndpointError::Deadline(_))));
        f.pause_sends(false);
        assert_eq!(h[..8], [0u8; 8], "the flushed request served nothing");
        assert_eq!(src.map.lookups(), 0);
        assert_eq!(hits(fetch()), [EntryOutcome::HitMemoryTier]);
        assert_eq!(&h[..8], b"value...");
        assert_eq!(dest.ep.stats().deadline_expiries, 1);
        assert_eq!(src.map.held(), 0);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    // --- Phase 6: block-tier serving (T039-T042, T072, T073) -----------------

    use crate::test_doubles::DiskSource;

    /// A source node at 10.0.0.2:7000 whose block tier is `disk`, allocating
    /// `buffers` x 256 bytes from the top half of its pool, with `readers`
    /// reader threads and `credits` send credits; and a destination at
    /// 10.0.0.1:7000 with 4 KiB of landing slots. Both initialized.
    fn block_pair(
        f: &MockFabric,
        buffers: usize,
        readers: u32,
        credits: u32,
    ) -> (Node, Node, Arc<DiskSource>, Landing) {
        let dest = node(f, "10.0.0.1:7000", 8);
        let src = node_with(f, "10.0.0.2:7000", |c| {
            c.block_readers = readers;
            c.send_credits = credits;
        });
        assert!(
            buffers * 256 <= 2048,
            "buffers fit the top half of the pool"
        );
        let disk = Arc::new(DiskSource::new(src.pool_addr(2048), buffers, 256));
        src.ep
            .use_block_source(Arc::clone(&disk) as Arc<dyn bounce::BlockSource>);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        let h = Landing::of(&dest);
        (dest, src, disk, h)
    }

    fn fetch_from_src(
        dest: &Node,
        items: &[(CacheKey, LocalSlot)],
    ) -> mpsc::Receiver<interfaces::FetchResult> {
        let (tx, rx) = mpsc::channel();
        dest.ep
            .fetch(
                "10.0.0.2:7000",
                items,
                Box::new(move |o| {
                    let _ = tx.send(o);
                }),
            )
            .unwrap();
        rx
    }

    /// The independent test of US3: a block-tier entry arrives byte-exact,
    /// is reported HitBlockTier, and its buffer and pin are both returned.
    #[test]
    fn a_block_tier_entry_is_read_and_served() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 4, 2, 8);
        disk.put(0x8000, b"from the ssd!!!!");
        src.map.block_at(7, 0x8000);
        let rx = fetch_from_src(&dest, &[(7, h.slot(0, 16))]);
        let out = hits(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert_eq!(out, [EntryOutcome::HitBlockTier]);
        assert_eq!(&h[..16], b"from the ssd!!!!");
        assert_eq!(disk.reads(), 1, "read once");
        assert_eq!(disk.free_buffers(), 4, "the buffer came back");
        assert_eq!(src.map.held(), 0, "the pin came back");
        assert_eq!(src.map.lookups(), 1, "one lookup: no probe-then-pin");
        let s = src.ep.stats();
        assert_eq!(
            (
                s.block_reads_attempted,
                s.block_reads_completed,
                s.block_reads_failed
            ),
            (1, 1, 0)
        );
        assert_eq!((s.bounce_buffers_in_use, s.bounce_buffers_peak), (0, 1));
        assert_eq!(dest.ep.stats().hit_block_tier, 1);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// T039 (FR-011, D7): the pin is released when the read completes, before
    /// the write is posted; the buffer is freed only after the write completes.
    /// All three on one recorded sequence.
    #[test]
    fn the_block_pin_goes_after_the_read_and_the_lease_after_the_write() {
        use crate::connection::mock::{LogEvent, Sequence};
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 4, 2, 8);
        connect(&dest.ep, "10.0.0.2:7000");
        disk.put(0x8000, b"abcdefgh");
        src.map.block_at(7, 0x8000);
        let seq = Sequence::default();
        f.record(&seq);
        src.map.record(&seq);
        disk.record(&seq);
        let rx = fetch_from_src(&dest, &[(7, h.slot(0, 8))]);
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::HitBlockTier]
        );
        let stamps = disk.stamps();
        let read = stamps.iter().find(|s| s.1 == "read").unwrap().0;
        let freed = stamps.iter().find(|s| s.1 == "free").unwrap().0;
        let released = src.map.releases()[0].0;
        // The source's write is the first completion on the source's link
        // after the read; the mock stamps it when the reaper takes it.
        let log = f.log();
        let written = log
            .iter()
            .filter(|(st, e)| *st > read && matches!(e, LogEvent::Completed(..)))
            .map(|(st, _)| *st)
            .find(|&st| st < freed)
            .expect("a completion between the read and the free");
        assert!(read < released, "pin released after the read");
        assert!(
            released < written,
            "pin released before the write completed"
        );
        assert!(
            written < freed,
            "buffer freed only after the write completed"
        );
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// SC-011 (T073): on a request mixing tiers, every memory-tier write is
    /// posted and lands while the device read is still held.
    #[test]
    fn memory_tier_writes_do_not_wait_for_device_reads() {
        let f = MockFabric::new();
        let (dest, mut src, disk, h) = block_pair(&f, 4, 2, 8);
        src.pool[..24].copy_from_slice(b"mem-one.mem-two.mem-thr.");
        src.map.memory(1, src.pool_addr(0), 8);
        src.map.block_at(2, 0x8000);
        src.map.memory(3, src.pool_addr(8), 8);
        src.map.memory(4, src.pool_addr(16), 8);
        disk.put(0x8000, b"block-2.");
        disk.hold_reads();
        let items: Vec<_> = (1..=4u64)
            .map(|k| (k, h.slot((k as usize - 1) * 8, 8)))
            .collect();
        let rx = fetch_from_src(&dest, &items);
        // The read may start after the memory-tier writes land; wait until it
        // is genuinely held, so what follows is checked under it.
        eventually("the device read is held", || disk.reading().0 == 1);
        let base = h.as_ptr();
        eventually("the memory-tier writes land under a held read", || {
            // SAFETY: reading our own slots; the peer writes only the ones
            // checked, and a torn read just retries.
            let got = unsafe { std::slice::from_raw_parts(base, 32) };
            &got[..8] == b"mem-one." && &got[16..32] == b"mem-two.mem-thr."
        });
        assert_eq!(disk.reading().0, 1, "the device read is still held");
        assert!(rx.try_recv().is_err(), "no reply before the block write");
        assert_eq!(&h[8..16], [0u8; 8], "nothing yet for the block entry");
        disk.release_reads();
        use EntryOutcome::*;
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [HitMemoryTier, HitBlockTier, HitMemoryTier, HitMemoryTier]
        );
        assert_eq!(&h[8..16], b"block-2.");
        assert_eq!(src.map.held(), 0);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// SC-004 with readers (T072): pins held never exceed send credits plus
    /// readers, however many block-tier entries; every entry still lands.
    #[test]
    fn block_tier_pins_stay_within_credits_plus_readers() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 8, 2, 2);
        for k in 0..40u64 {
            disk.put(0x1_0000 + k * 0x100, &[k as u8; 8]);
            src.map.block_at(k, 0x1_0000 + k * 0x100);
        }
        let items: Vec<_> = (0..40u64).map(|k| (k, h.slot(k as usize * 8, 8))).collect();
        let rx = fetch_from_src(&dest, &items);
        let out = hits(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        assert!(
            out.iter().all(|o| *o == EntryOutcome::HitBlockTier),
            "{out:?}"
        );
        for k in 0..40usize {
            assert_eq!(h[k * 8..k * 8 + 8], [k as u8; 8], "entry {k}");
        }
        assert!(
            src.map.peak_held() <= 2 + 2,
            "peak pins {} > credits 2 + readers 2",
            src.map.peak_held()
        );
        assert!(disk.reading().1 <= 2, "at most two reads at once");
        assert_eq!((src.map.held(), disk.free_buffers()), (0, 8));
        let s = src.ep.stats();
        assert_eq!(s.block_reads_completed, 40);
        assert!(s.block_reads_attempted >= 40);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// A reserve with room for one buffer at a time: entries beyond it are
    /// deferred, holding no pin, and all complete as buffers come back.
    #[test]
    fn a_full_reserve_defers_and_still_completes() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 1, 4, 8);
        for k in 0..6u64 {
            disk.put(0x1_0000 + k * 0x100, &[0xa0 + k as u8; 8]);
            src.map.block_at(k, 0x1_0000 + k * 0x100);
        }
        let items: Vec<_> = (0..6u64).map(|k| (k, h.slot(k as usize * 8, 8))).collect();
        let rx = fetch_from_src(&dest, &items);
        let out = hits(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        assert_eq!(out, [EntryOutcome::HitBlockTier; 6]);
        for k in 0..6usize {
            assert_eq!(h[k * 8..k * 8 + 8], [0xa0 + k as u8; 8]);
        }
        let s = src.ep.stats();
        assert!(s.block_deferrals > 0, "something waited for the one buffer");
        assert_eq!(s.bounce_buffers_peak, 1);
        assert_eq!(s.block_reads_failed, 0, "deferral is not failure");
        assert_eq!((src.map.held(), disk.free_buffers()), (0, 1));
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// NeverFits and Io are Failed — counted, never retried — and neither
    /// leaks a pin or a buffer; the entries beside them are served.
    #[test]
    fn unservable_block_reads_fail_and_are_not_retried() {
        let f = MockFabric::new();
        // Three readers for three entries: none waits for a reader, so any
        // deferral would be a retry.
        let (dest, src, disk, h) = block_pair(&f, 4, 3, 8);
        disk.put(0x100, b"good....");
        src.map.block_at(1, 0x100);
        src.map.block_at(2, 0x200);
        disk.never_fits_at(0x200);
        src.map.block_at(3, 0x300);
        disk.fail_at(0x300);
        let items: Vec<_> = (1..=3u64).map(|k| (k, h.slot(k as usize * 8, 8))).collect();
        let out = hits(
            fetch_from_src(&dest, &items)
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
        );
        use EntryOutcome::*;
        assert_eq!(out, [HitBlockTier, Failed, Failed]);
        let s = src.ep.stats();
        assert_eq!(
            (
                s.block_reads_attempted,
                s.block_reads_completed,
                s.block_reads_failed
            ),
            (3, 1, 2)
        );
        assert_eq!(s.block_deferrals, 0, "never retried");
        assert_eq!(
            src.map.lookups(),
            3,
            "one lookup each: neither failure looked up again"
        );
        assert_eq!(
            disk.reads(),
            2,
            "NeverFits issued no read; Io was read once"
        );
        assert_eq!((src.map.held(), disk.free_buffers()), (0, 4));
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// A teardown while a read is held: the reply never comes, the read is
    /// waited for, and its pin and buffer are both returned when it does.
    #[test]
    fn a_teardown_mid_read_returns_the_pin_and_lease_when_the_read_ends() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 4, 2, 8);
        disk.put(0x100, b"late....");
        src.map.block_at(1, 0x100);
        disk.hold_reads();
        let _rx = fetch_from_src(&dest, &[(1, h.slot(0, 8))]);
        eventually("the read is out", || disk.reading().0 == 1);
        src.ep.disconnect("10.0.0.1:7000").unwrap();
        assert_eq!(src.map.held(), 1, "the read still holds its pin");
        disk.release_reads();
        eventually("pin and buffer back", || {
            src.map.held() == 0 && disk.free_buffers() == 4
        });
        assert_eq!(h[..8], [0u8; 8], "nothing written for a dead push");
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// Shutdown waits for a held read, however long, and returns everything.
    #[test]
    fn shutdown_waits_for_reads_in_flight() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 4, 2, 8);
        src.map.block_at(1, 0x100);
        disk.hold_reads();
        let _rx = fetch_from_src(&dest, &[(1, h.slot(0, 8))]);
        eventually("the read is out", || disk.reading().0 == 1);
        let disk2 = Arc::clone(&disk);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            disk2.release_reads();
        });
        let t0 = Instant::now();
        src.ep.shutdown().unwrap();
        assert!(t0.elapsed() >= Duration::from_millis(100), "it waited");
        releaser.join().unwrap();
        assert_eq!((src.map.held(), disk.free_buffers()), (0, 4));
        dest.ep.shutdown().unwrap();
    }

    /// T041 (D14): with no block source, a block-tier answer is a Miss —
    /// never HitBlockTier, never an error — and its pin is released.
    #[test]
    fn without_a_block_source_a_block_entry_is_a_miss() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        src.map.block_at(1, 0x100);
        let h = Landing::of(&dest);
        let out = hits(
            fetch_from_src(&dest, &[(1, h.slot(0, 8))])
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
        );
        assert_eq!(out, [EntryOutcome::Miss]);
        assert_eq!(src.map.held(), 0);
        assert_eq!(src.ep.stats().block_reads_attempted, 0);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// SC-005 (T042): serving block-tier entries takes nothing from the
    /// source's cache — the only memory touched is the bounce range, outside
    /// the entries the map points at — and it never promotes (the map is never
    /// asked to create or convert an entry; the double panics if it were).
    #[test]
    fn serving_the_block_tier_leaves_the_cache_alone() {
        let f = MockFabric::new();
        let (dest, mut src, disk, h) = block_pair(&f, 4, 2, 8);
        src.pool[..2048].fill(0x5a);
        src.map.memory(1, src.pool_addr(0), 8);
        disk.put(0x100, b"cold....");
        src.map.block_at(2, 0x100);
        let before = src.pool[..2048].to_vec();
        let out = hits(
            fetch_from_src(&dest, &[(1, h.slot(0, 8)), (2, h.slot(8, 8))])
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
        );
        assert_eq!(
            out,
            [EntryOutcome::HitMemoryTier, EntryOutcome::HitBlockTier]
        );
        assert_eq!(src.pool[..2048], before[..], "the cache's bytes untouched");
        assert_eq!(src.map.lookups(), 2, "one lookup each, nothing else asked");
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// The `spdk` wiring: with the `dispatcher` receptacle bound, `initialize`
    /// starts readers over it, and a block-tier entry is read through
    /// `read_to_bounce` and its buffer returned through `release_bounce`.
    #[cfg(feature = "spdk")]
    #[test]
    fn an_spdk_endpoint_reads_through_its_dispatcher_receptacle() {
        let f = MockFabric::new();
        let dest = node(&f, "10.0.0.1:7000", 8);
        let src = node(&f, "10.0.0.2:7000", 8);
        let disk = Arc::new(DiskSource::new(src.pool_addr(2048), 2, 256));
        src.ep
            .dispatcher
            .connect(
                Arc::new(crate::test_doubles::DiskDispatcher(Arc::clone(&disk)))
                    as Arc<dyn interfaces::IDispatcher + Send + Sync>,
            )
            .unwrap();
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        disk.put(0x40, b"via the receptacle!!");
        src.map.block_at(5, 0x40);
        let h = Landing::of(&dest);
        let out = hits(
            fetch_from_src(&dest, &[(5, h.slot(0, 20))])
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
        );
        assert_eq!(out, [EntryOutcome::HitBlockTier]);
        assert_eq!(&h[..20], b"via the receptacle!!");
        assert_eq!(
            (disk.reads(), disk.free_buffers(), src.map.held()),
            (1, 2, 0)
        );
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    // --- Phase 7: chunking (T043-T048) --------------------------------------

    /// SC-007, T046: a ~3.4 GiB-shaped fetch — 1,536 entries, five 340-entry
    /// messages — completes over two message credits, every entry landing
    /// byte-exact and every outcome in request order across chunk boundaries.
    /// The memory/block mix shows each chunk is served whole.
    #[test]
    fn a_large_fetch_completes_in_chunks_over_few_credits() {
        let f = MockFabric::new();
        let dest = node_with(&f, "10.0.0.1:7000", |c| c.message_credits = 2);
        let mut src = node(&f, "10.0.0.2:7000", 8);
        dest.ep.initialize().unwrap();
        src.ep.initialize().unwrap();
        // 16 distinct 2-byte values, shared round-robin by the keys.
        for v in 0..16usize {
            src.pool[v * 2..v * 2 + 2].copy_from_slice(&[v as u8, 0xee]);
        }
        const N: usize = 1536;
        for k in 0..N as u64 {
            if k % 7 == 3 {
                continue; // a miss now and then
            }
            src.map.memory(k, src.pool_addr((k as usize % 16) * 2), 2);
        }
        let h = Landing::of(&dest);
        let items: Vec<_> = (0..N).map(|i| (i as u64, h.slot(i * 2, 2))).collect();
        let rx = fetch_from_src(&dest, &items);
        let out = hits(rx.recv_timeout(Duration::from_secs(20)).unwrap());
        assert_eq!(out.len(), N);
        for (i, o) in out.iter().enumerate() {
            if i % 7 == 3 {
                assert_eq!(*o, EntryOutcome::Miss, "entry {i}");
                assert_eq!(h[i * 2..i * 2 + 2], [0, 0]);
            } else {
                assert_eq!(*o, EntryOutcome::HitMemoryTier, "entry {i}");
                assert_eq!(h[i * 2..i * 2 + 2], [(i % 16) as u8, 0xee], "entry {i}");
            }
        }
        let s = dest.ep.stats();
        assert_eq!(s.chunks_sent, 5, "ceil(1536 / 340)");
        assert_eq!((s.requests_attempted, s.requests_completed), (1, 1));
        let peak = session_of(&dest.ep, "10.0.0.2:7000").unwrap().outbound.1;
        assert!(peak <= 2, "never more than two chunks out: {peak}");
        assert_eq!(src.map.held(), 0);
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    /// Credits are waited for, never failed on: with the peer stalled, a
    /// fetch of six one-entry chunks against two credits sends exactly two,
    /// and the rest go as replies return the credits.
    #[test]
    fn chunks_wait_for_message_credits() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer_with(&f, 1, |c| c.message_credits = 2);
        let rx = fetch_one(&a.ep, &h, 6);
        eventually("two chunks are out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 2)
        });
        // Settle time for a wrong extra send to show; the check is that none does.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(a.ep.stats().chunks_sent, 2, "the rest wait for credits");
        assert!(rx.try_recv().is_err(), "waiting is not failing");
        for ctx in 0..6u64 {
            eventually("chunk is out", || a.ep.stats().chunks_sent > ctx);
            send_raw(&mut raw, link, |out| {
                message::encode_reply(1, ctx, &[EntryOutcome::Miss], out)
            });
        }
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::Miss; 6]
        );
        assert_eq!(session_of(&a.ep, "10.0.0.3:7000").unwrap().outbound.1, 2);
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// Fetches to one peer take turns a chunk at a time: a small fetch queued
    /// behind a large one goes out before the large one's later chunks.
    #[test]
    fn a_small_fetch_is_not_starved_by_a_large_one() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer_with(&f, 1, |c| c.message_credits = 1);
        let big = fetch_one(&a.ep, &h, 4); // context 0 goes out at once
        eventually("the first chunk is out", || a.ep.stats().chunks_sent == 1);
        let (tx, small) = mpsc::channel();
        a.ep.fetch(
            "10.0.0.3:7000",
            &[(9, h.slot(40, 8))],
            Box::new(move |o| {
                let _ = tx.send(o);
            }),
        )
        .unwrap();
        eventually("the small fetch is queued", || {
            a.ep.stats().requests_attempted == 2
        });
        // Freeing the one credit: the small fetch's turn comes next.
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 0, &[EntryOutcome::Miss], out)
        });
        eventually("a second message is out", || a.ep.stats().chunks_sent == 2);
        send_raw(&mut raw, link, |out| {
            message::encode_reply(1, 1, &[EntryOutcome::HitMemoryTier], out)
        });
        assert_eq!(
            hits(small.recv_timeout(Duration::from_secs(5)).unwrap()),
            [EntryOutcome::HitMemoryTier],
            "context 1 was the small fetch's"
        );
        assert!(big.try_recv().is_err(), "the large fetch is still going");
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// The deadline runs per chunk, from its own send, and names it: a stalled
    /// third chunk aborts the whole fetch, after the teardown.
    #[test]
    fn a_stalled_chunk_names_itself_and_fails_the_fetch() {
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer_with(&f, 1, fast_deadline);
        let rx = fetch_one(&a.ep, &h, 3);
        eventually("three chunks are out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 3)
        });
        for ctx in [0u64, 1] {
            send_raw(&mut raw, link, |out| {
                message::encode_reply(1, ctx, &[EntryOutcome::HitMemoryTier], out)
            });
        }
        let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let Err(TransferEndpointError::Deadline(why)) = r else {
            panic!("expected Deadline, got {r:?}");
        };
        for part in ["chunk 2 of 3", "context 2", "keys 2..=2", "1 entries"] {
            assert!(why.contains(part), "{part:?} missing from {why:?}");
        }
        assert!(
            session_of(&a.ep, "10.0.0.3:7000").is_none(),
            "torn down first"
        );
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// T045: scarcity at the source delays its reply, never splits it. A
    /// six-entry block-tier request served through a one-buffer reserve draws
    /// exactly one reply, after many deferrals.
    #[test]
    fn a_scarce_source_still_replies_once_per_request() {
        let f = MockFabric::new();
        let (dest, src, disk, h) = block_pair(&f, 1, 2, 8);
        for k in 0..6u64 {
            disk.put(0x100 * (k + 1), &[k as u8 + 1; 8]);
            src.map.block_at(k, 0x100 * (k + 1));
        }
        let items: Vec<_> = (0..6u64).map(|k| (k, h.slot(k as usize * 8, 8))).collect();
        let rx = fetch_from_src(&dest, &items);
        assert_eq!(
            hits(rx.recv_timeout(Duration::from_secs(10)).unwrap()),
            [EntryOutcome::HitBlockTier; 6]
        );
        let s = session_of(&dest.ep, "10.0.0.2:7000").unwrap();
        assert_eq!(s.outbound, (0, 1), "one request out at most, and none now");
        assert!(
            src.ep.stats().block_deferrals > 0,
            "it was genuinely scarce"
        );
        // The source's inbound books: one request, one reply.
        let s = session_of(&src.ep, "10.0.0.1:7000").unwrap();
        assert_eq!(s.inbound, (0, 1));
        dest.ep.shutdown().unwrap();
        src.ep.shutdown().unwrap();
    }

    // --- T011: fabric faults ------------------------------------------------

    /// A peer that sends more than a receive buffer holds: on the NIC only the
    /// receive fails and both queue pairs stay up, so tearing the connection
    /// down is the endpoint's own decision — it does, and loudly; a fetch
    /// outstanding on it fails after the barrier.
    #[test]
    fn an_oversized_message_is_the_endpoints_teardown() {
        use crate::connection::transport::{Access, SendOp, SendRequest, Transport};
        let f = MockFabric::new();
        let (a, mut raw, link, h) = raw_peer(&f, 340);
        let rx = fetch_one(&a.ep, &h, 1);
        eventually("the fetch is out", || {
            session_of(&a.ep, "10.0.0.3:7000").is_some_and(|s| s.outbound.0 == 1)
        });
        let big: &'static mut [u8] =
            Box::leak(vec![0u8; message::MESSAGE_LEN + 64].into_boxed_slice());
        // SAFETY: leaked, so it outlives the send.
        let r: &'static crate::connection::transport::Region = Box::leak(Box::new(
            unsafe { raw.register(big.as_mut_ptr(), big.len(), Access::LOCAL_WRITE) }.unwrap(),
        ));
        // SAFETY: as above.
        unsafe {
            raw.link(link).unwrap().post_send(&[SendRequest {
                wr_id: 2,
                op: SendOp::Send,
                local: r.sge(r.addr(), big.len()).unwrap(),
                signaled: false,
            }])
        }
        .unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(TransferEndpointError::Transport(_))
        ));
        eventually("a tore down", || {
            session_of(&a.ep, "10.0.0.3:7000").is_none()
        });
        assert_eq!(a.ep.stats().protocol_faults, 1, "and counted it");
        a.ep.shutdown().unwrap();
        drop(raw);
    }

    /// FR-026 under a finite RNR retry, made deterministic. Every fetch
    /// misses, so the source replies at once with no writes. a's sends are
    /// held while b's 8 requests reach a and a answers them: a's send queue
    /// holds 8 replies. a then asks for 8 of its own, posted behind (a has 16
    /// send credits, so all 16 are on its send queue at once). Released,
    /// all 16 messages run at b in one burst, inside one call, before b's
    /// thread can re-post a single receive. With the partition b posted 8
    /// reserve + 8 quota and every one lands; without the reserve it posted 8,
    /// the ninth finds none, and with a retry limit of zero that is
    /// `RNR_RETRY_EXC_ERR`: the connection goes to ERROR and the fetches fail
    /// loudly rather than stall. The deadlock the partition prevents, visible.
    #[test]
    fn the_receive_partition_survives_a_finite_rnr_retry() {
        fn run(no_reserve: bool) -> (usize, usize, u64) {
            let f = MockFabric::new();
            // 8 message credits and an 8-request quota on both, so b posts
            // 8 + 8 receives and the burst is 16 — not the defaults, which
            // the partition's arithmetic does not depend on. 16 send credits
            // on a, so its 8 replies and 8 requests are all on its send queue
            // at once: with 8, the requests wait in the outbox behind the
            // replies and the burst is never 16.
            let eight = |c: &mut EndpointConfig| {
                c.message_credits = 8;
                c.inbound_quota = 8;
            };
            let a = node_with(&f, "10.0.0.1:7000", |c| {
                eight(c);
                c.send_credits = 16;
            });
            let b = node_with(&f, "10.0.0.2:7000", eight);
            a.ep.initialize().unwrap();
            b.ep.initialize().unwrap();
            if no_reserve {
                for n in [&a, &b] {
                    n.ep.call(Command::NoReplyReserve).unwrap();
                }
            }
            let ha = Landing::of(&a);
            let hb = Landing::of(&b);
            connect(&a.ep, "10.0.0.2:7000");
            eventually("both ready", || {
                session_of(&b.ep, "10.0.0.1:7000").is_some_and(|s| s.credits_available == 8)
                    && session_of(&a.ep, "10.0.0.2:7000").is_some_and(|s| s.credits_available == 8)
            });
            f.rnr_retry_limit(Some(0));
            let stalls = f.rnr_stalls();
            let a_id = PeerId::parse("10.0.0.1:7000").unwrap();
            f.hold_sends_from(&a_id, true);
            let (tx, rx) = mpsc::channel();
            let ask = |ep: &CertusTransferEndpoint, to: &str, h: &Landing| {
                for i in 0..8 {
                    let tx = tx.clone();
                    ep.fetch(
                        to,
                        &[(99, h.slot(i * 8, 8))], // held by nobody
                        Box::new(move |o| {
                            let _ = tx.send(o);
                        }),
                    )
                    .unwrap();
                }
            };
            ask(&b.ep, "10.0.0.1:7000", &hb);
            eventually("a answered all 8, its replies held", || {
                f.pending_sends_from(&a_id) == 8
            });
            ask(&a.ep, "10.0.0.2:7000", &ha);
            eventually("a's 8 requests queued behind", || {
                f.pending_sends_from(&a_id) == 16
            });
            f.hold_sends_from(&a_id, false); // the burst
            let (mut ok, mut failed) = (0, 0);
            for _ in 0..16 {
                match rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("every fetch completes")
                {
                    Ok(_) => ok += 1,
                    Err(_) => failed += 1,
                }
            }
            let stalled = f.rnr_stalls() - stalls;
            a.ep.shutdown().unwrap();
            b.ep.shutdown().unwrap();
            (ok, failed, stalled)
        }
        assert_eq!(
            run(false),
            (16, 0, 0),
            "with the reserve: no RNR, all answered"
        );
        let (ok, failed, stalled) = run(true);
        assert_eq!(stalled, 1, "without it the ninth message found no buffer");
        assert!(
            failed > 0,
            "and the connection failed loudly: {ok} ok, {failed} failed"
        );
    }

    #[test]
    fn configure_refuses_zero_block_readers() {
        let ep = CertusTransferEndpoint::create();
        let mut c = EndpointConfig::new("10.0.0.1:7000");
        c.block_readers = 0;
        assert!(matches!(
            ep.configure(c),
            Err(TransferEndpointError::Transport(m)) if m.contains("block_readers")
        ));
    }
}
