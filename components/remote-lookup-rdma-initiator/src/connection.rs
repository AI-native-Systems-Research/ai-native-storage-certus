//! Outbound RDMA connection table and per-connection submit/completion engine.
//!
//! The [`ConnectionTable`] keeps one entry per remote host, keyed by its
//! normalized `"ip:port"` endpoint, and each entry owns a dedicated thread that
//! drives that host's queue pair. Callers do not touch the queue pair at all:
//! [`push_async`](ConnectionTable::push_async) hands a batch to the host's thread
//! through a bounded queue and returns, and the thread posts the work requests,
//! reaps their completions, and invokes the batch's completion callback.
//!
//! # Why a thread per connection
//!
//! Verbs is asynchronous, and the throughput that matters comes from keeping many
//! writes in flight while their control plane (discovery, status round trips, local
//! lookups) proceeds in parallel. A synchronous `push` cannot do that: it must hold
//! the queue pair from the first post until the last completion, so exactly one
//! batch per peer is ever on the wire.
//!
//! The connection thread does **both** the posting and the reaping. That is
//! deliberate: an [`RdmaConn`]'s underlying queue pair is `Send` but not `Sync`, and
//! keeping every access on one thread preserves that invariant exactly, rather than
//! trading it for new synchronization around the queue pair. Submitters only enqueue.
//!
//! Because the connection is owned by exactly one thread, the transient
//! "connecting"/"disconnecting" states the old shared state machine needed are gone
//! — there is no other thread to publish them to. Recovery is likewise a phase of
//! the thread's execution rather than an observable state: batches submitted while it
//! reconnects simply wait in the queue.
//!
//! # Flow control
//!
//! Two bounds apply. The queue pair's send queue admits at most [`PUSH_WINDOW`]
//! outstanding writes, which the thread tracks as credits and tops up as completions
//! free them — that is what pipelines successive batches instead of draining the send
//! queue between them. The submit queue itself is bounded too, and a full one is
//! reported as [`PushStatus::UnableToConnect`] rather than queued: every queued batch
//! holds its submitter's resources (typically read pins, which block eviction), so an
//! unbounded queue would let one sick peer stall the local memory tier.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use component_core::channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use component_core::channel::ChannelError;
use interfaces::{ILogger, PushCompletion, PushStatus, RemoteLookupRdmaInitiatorError};

#[cfg(feature = "rdma")]
use crate::rdma;
use crate::telemetry::TelemetryCollector;

/// Errors from RDMA operations, surfaced across the transport seam.
///
/// Defined here (not in the feature-gated `rdma` module) because the seam traits
/// and the mock transport reference it even when the real rdma-core path is
/// compiled out.
#[derive(Debug, Clone)]
pub enum RdmaError {
    /// A connection could not be established.
    ConnectionFailed(String),
    /// A memory-region registration or buffer allocation failed.
    AllocationFailed(String),
    /// An RDMA write failed.
    WriteFailed(String),
    /// A send/recv operation failed.
    SendRecvFailed(String),
    /// A resource limit was hit (queue depth, CQ entries, …).
    ResourceExhausted(String),
    /// A CM/verbs event error.
    EventError(String),
}

impl fmt::Display for RdmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "RDMA connection failed: {msg}"),
            Self::AllocationFailed(msg) => write!(f, "RDMA allocation failed: {msg}"),
            Self::WriteFailed(msg) => write!(f, "RDMA write failed: {msg}"),
            Self::SendRecvFailed(msg) => write!(f, "RDMA send/recv failed: {msg}"),
            Self::ResourceExhausted(msg) => write!(f, "RDMA resource exhausted: {msg}"),
            Self::EventError(msg) => write!(f, "RDMA event error: {msg}"),
        }
    }
}

impl std::error::Error for RdmaError {}

/// One reaped work completion.
///
/// Carries no notion of which batch it belongs to: correlating a completion back to
/// its submitter is the reaper's job, via the `wr_id` chosen when posting.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The tag passed to [`RdmaConn::post_write`].
    pub wr_id: u64,
    /// `None` when the write succeeded; otherwise a diagnosable description
    /// including the completion status and queue-pair state.
    pub error: Option<String>,
}

/// How long a posted write may go uncompleted before its connection is declared
/// stuck and rebuilt.
///
/// rdma_cm owns the queue pair's RTR/RTS transition and leaves the hardware ACK
/// timeout at its (large) default, so the first write on a stale but nominally warm
/// connection would otherwise burn ~15s of retransmit before `RETRY_EXC`. This
/// software cap abandons a stuck transfer far sooner, so reconnect-and-replay
/// finishes inside the caller's operation deadline. It sits far above healthy latency
/// (a full send queue of 64 KiB writes completes in ~200us on 200G RoCE), so only a
/// genuinely stuck transfer trips it.
///
/// Measured from the moment a batch's *first* write is posted, not from submission —
/// a batch waiting its turn behind others is queued, not stuck.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Depth of each host's submit queue, in batches.
///
/// A power of two, as the underlying ring buffer requires. Sized so a caller can run
/// well ahead of the wire without the backlog of held read pins growing without
/// bound; beyond this, submissions are rejected rather than queued.
const SUBMIT_QUEUE_DEPTH: usize = 256;

/// How many submitted batches a connection thread will hold at once.
///
/// This is what makes [`SUBMIT_QUEUE_DEPTH`] mean anything. Without it the thread
/// would drain the whole channel into its own tracking map on every iteration, so the
/// channel would never be full, submissions would never be rejected, and the backlog
/// of held read pins — the thing the bound exists to limit — would grow without
/// bound anyway. Stopping the drain here is what pushes back-pressure into the
/// channel, and from there to the submitter.
///
/// Comfortably more than the send queue can hold in flight, so it never throttles a
/// healthy connection; it only bites when completions have stopped arriving.
const MAX_TRACKED_BATCHES: usize = 64;

/// Per-phase breakdown of one connect, in microseconds, for telemetry
/// attribution of cold-connect latency. All zero when the transport does not
/// measure (e.g. the mock).
#[derive(Debug, Clone, Copy, Default)]
pub struct ConnectTiming {
    /// `rdma_cm` address resolution.
    pub resolve_addr_us: u64,
    /// `rdma_cm` route resolution.
    pub resolve_route_us: u64,
    /// Connect handshake through `RDMA_CM_EVENT_ESTABLISHED`.
    pub handshake_us: u64,
    /// Pool memory-region registration (`ibv_reg_mr`).
    pub mr_reg_us: u64,
}

impl ConnectTiming {
    /// True if any phase was measured (i.e. a real, timed connect).
    fn is_measured(&self) -> bool {
        self.resolve_addr_us | self.resolve_route_us | self.handshake_us | self.mr_reg_us != 0
    }
}

/// Establishes outbound RDMA connections. A seam so the connection engine can be
/// unit-tested without RDMA hardware.
pub trait RdmaTransport: Send + Sync {
    /// Connect to `addr:port` and return a ready-to-use connection plus the
    /// per-phase timing of establishing it.
    fn connect(
        &self,
        addr: &str,
        port: u16,
    ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError>;
}

/// One RDMA write: `len` bytes at local pool address `local`, destined for the
/// remote region (`remote_addr`, `rkey`).
#[derive(Clone, Copy)]
pub struct WindowWrite {
    /// Local pool address of the value (from `IMemoryTier::peek`).
    pub local: *const u8,
    /// Number of bytes to write.
    pub len: usize,
    /// Remote destination address.
    pub remote_addr: u64,
    /// Remote key authorizing the write.
    pub rkey: u32,
}

// SAFETY: `local` addresses the memory-tier pool, which the submitter is required to
// keep valid and unevicted until its completion callback runs — see the
// `IRemoteLookupRdmaInitiator` documentation. The pointer is handed to the NIC and is
// never dereferenced in Rust, so moving the descriptor to the connection thread
// creates no aliasing and no race.
unsafe impl Send for WindowWrite {}

/// Largest number of writes that may be outstanding on one queue pair at a time,
/// bounded by its send-queue and completion-queue depths.
///
/// The connection thread treats this as a credit limit: it posts up to this many
/// writes and then posts more only as completions retire earlier ones, so successive
/// batches overlap instead of the send queue draining to empty between them.
///
/// Kept here (not only in the feature-gated `rdma` module) so the flow-control logic
/// and its tests exist in the non-`rdma` build too; the assertion below stops the two
/// definitions drifting apart.
pub const PUSH_WINDOW: usize = 128;

#[cfg(feature = "rdma")]
const _: () = assert!(PUSH_WINDOW == rdma::WINDOW);

/// A live outbound connection capable of RDMA-writing from the local
/// memory-tier pool into a remote region.
///
/// Posting and reaping are separate so a caller can keep many writes in flight. An
/// implementation may assume both are only ever called from one thread at a time.
pub trait RdmaConn: Send {
    /// Returns `false` if the queue pair has entered the error state and the
    /// connection must be rebuilt.
    fn qp_healthy(&self) -> bool;

    /// Post one RDMA write tagged `wr_id`, without waiting for it to complete.
    ///
    /// `wr_id` is echoed back by [`poll_completions`](Self::poll_completions).
    /// Returning `Err` means this write was not queued; earlier ones may still be in
    /// flight, so the caller must still tear the connection down before releasing
    /// anything the NIC could be reading.
    ///
    /// # Safety
    ///
    /// `write.local` must point to `write.len` valid bytes inside the registered
    /// memory-tier pool region backing this connection, and must remain valid until
    /// this write's completion has been reaped or the connection has been destroyed —
    /// the NIC reads the buffer asynchronously.
    unsafe fn post_write(&self, write: &WindowWrite, wr_id: u64) -> Result<(), RdmaError>;

    /// Reap up to `max` ready completions without blocking, returning however many
    /// were available — possibly none.
    ///
    /// A failed write is reported as a [`Completion`] carrying an error, not as `Err`;
    /// `Err` means the completion queue itself could not be polled. Note that one
    /// genuinely failing write drives the queue pair into the error state, which
    /// flushes every other outstanding write with a flush error, so a burst of failed
    /// completions is expected and only one of them names the original cause.
    fn poll_completions(&self, max: usize) -> Result<Vec<Completion>, RdmaError>;
}

/// Connection state, owned exclusively by one connection thread.
enum ConnState {
    /// No live connection (initial state, or after a failed connect / teardown).
    Disconnected,
    /// A healthy, established connection ready for writes.
    Connected(Box<dyn RdmaConn>),
}

/// A per-item action decided by the caller after the local memory-tier lookup.
pub enum ItemPlan {
    /// A terminal status decided before any RDMA write is attempted
    /// ([`PushStatus::KeyNotFound`] or [`PushStatus::SizeMismatch`]).
    Done(PushStatus),
    /// A write to perform once the host is connected.
    Write {
        /// Local pool address of the value (from `IMemoryTier::peek`).
        local: *const u8,
        /// Number of bytes to write.
        len: usize,
        /// Remote destination address.
        remote_addr: u64,
        /// Remote key authorizing the write.
        rkey: u32,
    },
}

/// One submitted batch, tracked by the connection thread until every write in it
/// has completed.
struct Batch {
    /// Monotonic identity, used to correlate completions back to this batch.
    seq: u64,
    /// The writes to perform, in submission order. Retained for the whole batch
    /// lifetime because recovery replays them.
    writes: Vec<WindowWrite>,
    /// For each write, the index in `statuses` its outcome belongs at.
    slots: Vec<usize>,
    /// Statuses in caller order. Items resolved before RDMA already hold their final
    /// status; writes start pessimistic and are upgraded as completions arrive.
    statuses: Vec<PushStatus>,
    /// Bytes attributable to each caller-order item, for telemetry.
    item_bytes: Vec<u64>,
    /// Index of the next write in `writes` to post.
    posted: usize,
    /// Writes posted and not yet completed.
    outstanding: usize,
    /// When the first write was posted, for the stall deadline.
    first_post: Option<Instant>,
    /// When the batch was submitted, for push-latency telemetry.
    submitted: Instant,
    /// Invoked exactly once, when the batch reaches a terminal outcome.
    on_complete: Option<PushCompletion>,
}

impl Batch {
    /// Split `resolved` into pre-decided statuses and the writes to perform.
    fn new(seq: u64, resolved: Vec<ItemPlan>, on_complete: PushCompletion) -> Self {
        let mut statuses = Vec::with_capacity(resolved.len());
        let mut item_bytes = Vec::with_capacity(resolved.len());
        let mut writes = Vec::new();
        let mut slots = Vec::new();

        for item in resolved {
            match item {
                ItemPlan::Done(status) => {
                    statuses.push(status);
                    item_bytes.push(0);
                }
                ItemPlan::Write {
                    local,
                    len,
                    remote_addr,
                    rkey,
                } => {
                    slots.push(statuses.len());
                    statuses.push(PushStatus::UnableToConnect);
                    item_bytes.push(len as u64);
                    writes.push(WindowWrite {
                        local,
                        len,
                        remote_addr,
                        rkey,
                    });
                }
            }
        }

        Self {
            seq,
            writes,
            slots,
            statuses,
            item_bytes,
            posted: 0,
            outstanding: 0,
            first_post: None,
            submitted: Instant::now(),
            on_complete: Some(on_complete),
        }
    }

    /// True once every write has been posted and completed.
    fn is_finished(&self) -> bool {
        self.posted == self.writes.len() && self.outstanding == 0
    }

    /// Reset for a replay after the connection was rebuilt. Statuses for writes go
    /// back to pessimistic; already-final `Done` items are untouched.
    fn rewind(&mut self) {
        self.posted = 0;
        self.outstanding = 0;
        self.first_post = None;
        for &slot in &self.slots {
            self.statuses[slot] = PushStatus::UnableToConnect;
        }
    }

    /// Mark every write with `status` — used when the connection is unrecoverable.
    fn set_all_writes(&mut self, status: PushStatus) {
        for &slot in &self.slots {
            self.statuses[slot] = status;
        }
    }

    /// Fire the completion callback, recording per-item and per-push telemetry.
    ///
    /// Idempotent: the callback is taken, so a second call does nothing.
    fn finish(&mut self, telemetry: &TelemetryCollector) {
        let Some(callback) = self.on_complete.take() else {
            return;
        };
        for (status, bytes) in self.statuses.iter().zip(&self.item_bytes) {
            let attributed = if *status == PushStatus::Success {
                *bytes
            } else {
                0
            };
            telemetry.record_item(*status, attributed);
        }
        telemetry.record_push(self.submitted.elapsed().as_micros() as u64);
        callback(std::mem::take(&mut self.statuses));
    }
}

impl Drop for Batch {
    /// A batch dropped without having been finished still owes its submitter a
    /// callback — the submitter may be holding read pins that only the callback (or
    /// its drop) releases. Report the writes as unable-to-connect rather than
    /// silently discarding the batch.
    fn drop(&mut self) {
        if let Some(callback) = self.on_complete.take() {
            self.set_all_writes(PushStatus::UnableToConnect);
            callback(std::mem::take(&mut self.statuses));
        }
    }
}

/// Work handed to a connection thread.
enum ConnCmd {
    /// Post a batch of writes and report its per-item outcome.
    Push(Batch),
    /// Establish the connection now, so a later push skips the cold connect.
    Warm,
    /// Stop: fail everything still owed a callback, tear the connection down, exit.
    Shutdown,
}

/// One remote host's submit queue and the thread serving it.
struct HostSlot {
    /// Bounded submit queue into the connection thread.
    tx: MpscSender<ConnCmd>,
    /// Set to ask the thread to stop.
    ///
    /// Out of band rather than a queued command, because the queue can be full
    /// exactly when teardown matters most — a peer whose completions have stopped is
    /// both the reason the queue backed up and the reason we are tearing down. A
    /// `Shutdown` message behind 256 pushes would not be seen for a long time.
    stopping: Arc<AtomicBool>,
    /// Taken at teardown so the thread can be joined exactly once.
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl HostSlot {
    /// Signal the thread to stop and wait for it, so that by the time this returns
    /// the queue pair is destroyed and the NIC is no longer reading pool memory.
    ///
    /// Joining is the point: it is what makes teardown a barrier the caller can rely
    /// on before reclaiming anything the NIC could have been reading.
    ///
    /// This can still take as long as one in-flight `connect` attempt, since that call
    /// is blocking and rdma_cm owns its timeout; the flag is checked before starting a
    /// connect but cannot interrupt one already under way.
    fn shutdown_and_join(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Best-effort nudge so a parked thread wakes at once rather than after its
        // poll interval. The flag, not this message, is what stops it.
        let _ = self.tx.try_send(ConnCmd::Shutdown);
        if let Some(handle) = self.thread.lock().expect("thread lock poisoned").take() {
            let _ = handle.join();
        }
    }
}

/// A table of outbound RDMA connections keyed by `"ip:port"` endpoint.
pub struct ConnectionTable {
    transport: Arc<dyn RdmaTransport>,
    slots: Mutex<HashMap<String, Arc<HostSlot>>>,
    telemetry: Arc<TelemetryCollector>,
    logger: Arc<dyn ILogger + Send + Sync>,
    /// Source of batch identities, shared across hosts so a `seq` is unique
    /// process-wide and stays meaningful in logs.
    next_seq: Mutex<u64>,
}

impl ConnectionTable {
    /// Create a table that establishes connections via `transport`, records metrics
    /// into `telemetry` (a no-op unless the `telemetry` feature is on), and logs
    /// through `logger`.
    ///
    /// The logger is owned rather than passed per call because the work is done on
    /// per-connection threads that outlive any one call.
    pub fn new(
        transport: Arc<dyn RdmaTransport>,
        telemetry: Arc<TelemetryCollector>,
        logger: Arc<dyn ILogger + Send + Sync>,
    ) -> Self {
        Self {
            transport,
            slots: Mutex::new(HashMap::new()),
            telemetry,
            logger,
            next_seq: Mutex::new(0),
        }
    }

    /// Look up `endpoint`'s slot, spawning its connection thread on first use.
    fn slot(&self, addr: &str, port: u16) -> Arc<HostSlot> {
        let key = format!("{addr}:{port}");
        let mut slots = self.slots.lock().expect("slots lock poisoned");
        if let Some(slot) = slots.get(&key) {
            return Arc::clone(slot);
        }

        let channel = MpscChannel::<ConnCmd>::new(SUBMIT_QUEUE_DEPTH);
        // A freshly created channel has no receiver bound and at least one sender
        // available, so neither endpoint can fail here.
        let tx = channel.sender().expect("fresh channel rejected a sender");
        let rx = channel
            .receiver()
            .expect("fresh channel rejected a receiver");

        let stopping = Arc::new(AtomicBool::new(false));
        let mut worker = ConnWorker {
            addr: addr.to_string(),
            port,
            transport: Arc::clone(&self.transport),
            telemetry: Arc::clone(&self.telemetry),
            logger: Arc::clone(&self.logger),
            stopping: Arc::clone(&stopping),
            state: ConnState::Disconnected,
            batches: HashMap::new(),
            to_post: VecDeque::new(),
            wr_slots: vec![None; PUSH_WINDOW],
            free_wr_slots: (0..PUSH_WINDOW).rev().collect(),
            recoveries: 0,
        };
        let thread = thread::Builder::new()
            .name(format!("rl-rdma-{addr}:{port}"))
            .spawn(move || {
                // `channel` is moved in so its shared state outlives the endpoints.
                let _channel = channel;
                worker.run(rx);
            })
            .expect("failed to spawn RDMA connection thread");

        let slot = Arc::new(HostSlot {
            tx,
            stopping,
            thread: Mutex::new(Some(thread)),
        });
        slots.insert(key, Arc::clone(&slot));
        slot
    }

    /// Take the next batch identity.
    fn take_seq(&self) -> u64 {
        let mut next = self.next_seq.lock().expect("next_seq lock poisoned");
        let seq = *next;
        *next = next.wrapping_add(1);
        seq
    }

    /// Queue a batch of planned writes for `endpoint` and return without waiting.
    ///
    /// `on_complete` is invoked exactly once with one [`PushStatus`] per item in
    /// `resolved`, in order. Items that arrived already decided
    /// ([`ItemPlan::Done`]) keep their status and never reach the wire.
    ///
    /// If the host's submit queue is full the batch is not queued: every write is
    /// reported as [`PushStatus::UnableToConnect`] and `on_complete` runs
    /// synchronously, on the calling thread, before this returns.
    ///
    /// # Errors
    ///
    /// [`RemoteLookupRdmaInitiatorError::InvalidEndpoint`] if `endpoint` is not a
    /// valid `"ip:port"`. The callback is dropped rather than invoked, which for a
    /// callback that releases resources on drop is equivalent.
    pub fn push_async(
        &self,
        endpoint: &str,
        resolved: Vec<ItemPlan>,
        on_complete: PushCompletion,
    ) -> Result<(), RemoteLookupRdmaInitiatorError> {
        let (addr, port) = parse_endpoint(endpoint)?;
        let batch = Batch::new(self.take_seq(), resolved, on_complete);
        let slot = self.slot(&addr, port);

        if let Err(e) = slot.tx.try_send(ConnCmd::Push(batch)) {
            // `Batch::drop` owes the callback its statuses, so the rejected batch
            // reports itself; nothing is silently dropped.
            self.logger.warn(&format!(
                "remote-lookup-rdma-initiator: submit queue to {addr}:{port} \
                 unavailable ({e:?}); reporting the batch as unable-to-connect"
            ));
        }
        Ok(())
    }

    /// Push planned writes to `endpoint`, blocking until every write has completed.
    ///
    /// A convenience wrapper over [`push_async`](Self::push_async); see the interface
    /// documentation for why callers on a latency-sensitive path should prefer the
    /// asynchronous form.
    pub fn push(
        &self,
        endpoint: &str,
        resolved: Vec<ItemPlan>,
    ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError> {
        let items = resolved.len();
        let (tx, rx) = std::sync::mpsc::channel();
        self.push_async(
            endpoint,
            resolved,
            Box::new(move |statuses| {
                let _ = tx.send(statuses);
            }),
        )?;
        // The callback is guaranteed to run exactly once for an accepted batch, so
        // this only fails if the connection thread died without honoring it.
        Ok(rx.recv().unwrap_or_else(|_| {
            self.logger.error(
                "remote-lookup-rdma-initiator: connection thread dropped a batch \
                 without reporting it",
            );
            vec![PushStatus::UnableToConnect; items]
        }))
    }

    /// Proactively establish (warm) the connection to `endpoint` without writing.
    ///
    /// Returns as soon as the request is queued; the connection thread performs the
    /// connect. A failed warm caches nothing, so a later push retries it. Errors only
    /// on an unparseable endpoint.
    pub fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError> {
        let (addr, port) = parse_endpoint(endpoint)?;
        let slot = self.slot(&addr, port);
        // Best-effort: a full queue means real work is already pending, which warms
        // the connection anyway.
        let _ = slot.tx.try_send(ConnCmd::Warm);
        Ok(())
    }

    /// Tear down the connection to a single host, if present. Idempotent.
    ///
    /// Blocks until the host's thread has exited, so on return its queue pair is
    /// destroyed and every batch it held has reported its outcome.
    pub fn disconnect(&self, endpoint: &str) {
        let (addr, port) = match parse_endpoint(endpoint) {
            Ok(hp) => hp,
            // Nothing could have been stored under an unparseable endpoint.
            Err(_) => return,
        };
        let key = format!("{addr}:{port}");
        let slot = self.slots.lock().expect("slots lock poisoned").remove(&key);
        if let Some(slot) = slot {
            slot.shutdown_and_join();
        }
    }

    /// Tear down all connections, blocking until every thread has exited.
    pub fn disconnect_all(&self) {
        let drained: Vec<Arc<HostSlot>> = {
            let mut slots = self.slots.lock().expect("slots lock poisoned");
            slots.drain().map(|(_, slot)| slot).collect()
        };
        for slot in drained {
            slot.shutdown_and_join();
        }
    }
}

impl Drop for ConnectionTable {
    /// Joining every connection thread here is what guarantees no NIC is still
    /// reading pool memory once the table is gone.
    fn drop(&mut self) {
        self.disconnect_all();
    }
}

/// Which batch and which of its writes a posted `wr_id` refers to.
#[derive(Clone, Copy)]
struct PostedRef {
    seq: u64,
    write_idx: usize,
}

/// The per-connection engine: owns the queue pair, posts writes, reaps completions.
struct ConnWorker {
    addr: String,
    port: u16,
    transport: Arc<dyn RdmaTransport>,
    telemetry: Arc<TelemetryCollector>,
    logger: Arc<dyn ILogger + Send + Sync>,
    /// Set by [`HostSlot::shutdown_and_join`] to ask this thread to stop.
    stopping: Arc<AtomicBool>,
    state: ConnState,
    /// Every accepted batch that has not yet reported, keyed by `seq`.
    batches: HashMap<u64, Batch>,
    /// Batches with writes still to post, in submission order.
    to_post: VecDeque<u64>,
    /// `wr_id` slab. A slot is free exactly when its completion has been reaped, and
    /// there are as many slots as the send queue is deep, so an outstanding `wr_id`
    /// is always unique. Slot reuse cannot be confused with a stale completion
    /// because recovery destroys the completion queue along with the queue pair.
    wr_slots: Vec<Option<PostedRef>>,
    /// Indices into `wr_slots` that are currently free.
    free_wr_slots: Vec<usize>,
    /// Recovery attempts since the last fully successful batch, to bound replay.
    recoveries: u32,
}

impl ConnWorker {
    /// Drive submissions and completions until told to stop.
    fn run(&mut self, rx: MpscReceiver<ConnCmd>) {
        self.logger.debug(&format!(
            "remote-lookup-rdma-initiator: connection thread for {}:{} started",
            self.addr, self.port
        ));
        // Let submitters unpark us so a queued batch is picked up promptly.
        rx.register_for_unpark();

        loop {
            if self.stopping.load(Ordering::SeqCst) {
                self.shutdown();
                return;
            }
            let mut progress = false;

            // 1. Accept queued work, but only up to the backlog cap — beyond that,
            //    leave it in the channel so the bound reaches the submitter.
            while self.batches.len() < MAX_TRACKED_BATCHES {
                match rx.try_recv() {
                    Ok(ConnCmd::Push(batch)) => {
                        self.accept(batch);
                        progress = true;
                    }
                    Ok(ConnCmd::Warm) => {
                        self.ensure_connected();
                        progress = true;
                    }
                    Ok(ConnCmd::Shutdown) => {
                        self.shutdown();
                        return;
                    }
                    Err(ChannelError::Empty) => break,
                    // The table dropped the sender: same as Shutdown.
                    Err(_) => {
                        self.shutdown();
                        return;
                    }
                }
            }

            // 2. Fill the send queue.
            if self.post_ready() {
                progress = true;
            }

            // 3. Retire completions.
            if self.reap() {
                progress = true;
            }

            // 4. Abandon a transfer the NIC has stopped making progress on.
            self.check_stalled();

            // 5. Spin while writes are outstanding — that is when latency matters
            //    and the completion queue is worth polling hot. Park only when there
            //    is nothing in flight and nothing waiting, so an idle peer costs no
            //    CPU.
            if !progress && self.batches.is_empty() {
                thread::park_timeout(Duration::from_millis(1));
            }
        }
    }

    /// Take ownership of a submitted batch.
    fn accept(&mut self, batch: Batch) {
        let seq = batch.seq;
        if batch.writes.is_empty() {
            // Nothing to send; report immediately rather than tracking it.
            let mut batch = batch;
            batch.finish(&self.telemetry);
            return;
        }
        self.batches.insert(seq, batch);
        self.to_post.push_back(seq);
    }

    /// Post as many queued writes as send-queue credits allow.
    ///
    /// Returns whether anything was posted. Batches are posted in submission order,
    /// and a batch too large for the remaining credits is posted partially and
    /// resumed as completions free them.
    fn post_ready(&mut self) -> bool {
        if self.to_post.is_empty() {
            return false;
        }
        if !self.ensure_connected() {
            // Unreachable host: nothing queued can be served.
            self.fail_all(PushStatus::UnableToConnect);
            return true;
        }

        let mut posted_any = false;
        while let Some(&seq) = self.to_post.front() {
            if self.free_wr_slots.is_empty() {
                break; // Send queue full; resume when completions land.
            }
            let Some(batch) = self.batches.get_mut(&seq) else {
                // Already reported and removed; drop its posting entry.
                self.to_post.pop_front();
                continue;
            };

            let mut failure = None;
            while batch.posted < batch.writes.len() {
                let Some(wr_slot) = self.free_wr_slots.pop() else {
                    break;
                };
                let write_idx = batch.posted;
                let write = batch.writes[write_idx];

                // SAFETY: `local`/`len` came from the submitter's memory-tier peek,
                // which the interface requires stay valid and unevicted until this
                // batch's callback runs. The write is not reported complete — and so
                // the callback cannot run — until its completion is reaped or the
                // connection is destroyed, which quiesces the NIC.
                let result = match &self.state {
                    ConnState::Connected(conn) => unsafe {
                        conn.post_write(&write, wr_slot as u64)
                    },
                    // ensure_connected returned true above.
                    ConnState::Disconnected => {
                        unreachable!("ensure_connected guarantees Connected")
                    }
                };

                match result {
                    Ok(()) => {
                        self.wr_slots[wr_slot] = Some(PostedRef { seq, write_idx });
                        batch.posted += 1;
                        batch.outstanding += 1;
                        batch.first_post.get_or_insert_with(Instant::now);
                        posted_any = true;
                    }
                    Err(e) => {
                        // The slot was never used; give it back before recovering.
                        self.free_wr_slots.push(wr_slot);
                        failure = Some(e);
                        break;
                    }
                }
            }

            if let Some(e) = failure {
                self.logger.warn(&format!(
                    "remote-lookup-rdma-initiator: posting to {}:{} failed ({e}); \
                     rebuilding the connection",
                    self.addr, self.port
                ));
                self.recover();
                return true;
            }

            if batch.posted == batch.writes.len() {
                self.to_post.pop_front();
                let n = batch.writes.len();
                self.logger.debug(&format!(
                    "remote-lookup-rdma-initiator: batch {seq} fully posted to {}:{} \
                     ({n} writes)",
                    self.addr, self.port
                ));
            } else {
                break; // Out of credits mid-batch.
            }
        }
        posted_any
    }

    /// Reap whatever the completion queue has ready, retiring writes and reporting
    /// batches that finish. Returns whether any completion was processed.
    fn reap(&mut self) -> bool {
        let outstanding = PUSH_WINDOW - self.free_wr_slots.len();
        if outstanding == 0 {
            return false;
        }

        let completions = match &self.state {
            ConnState::Connected(conn) => conn.poll_completions(outstanding),
            ConnState::Disconnected => return false,
        };
        let completions = match completions {
            Ok(c) => c,
            Err(e) => {
                self.logger.warn(&format!(
                    "remote-lookup-rdma-initiator: polling {}:{} failed ({e}); \
                     rebuilding the connection",
                    self.addr, self.port
                ));
                self.recover();
                return true;
            }
        };
        if completions.is_empty() {
            return false;
        }

        let mut first_error = None;
        for completion in &completions {
            let wr_slot = completion.wr_id as usize;
            let Some(posted) = self.wr_slots.get(wr_slot).copied().flatten() else {
                // Not a tag we issued (or already retired) — nothing to attribute.
                continue;
            };
            self.wr_slots[wr_slot] = None;
            self.free_wr_slots.push(wr_slot);

            if let Some(detail) = &completion.error {
                if first_error.is_none() {
                    first_error = Some(detail.clone());
                }
                continue;
            }

            if let Some(batch) = self.batches.get_mut(&posted.seq) {
                batch.outstanding -= 1;
                batch.statuses[batch.slots[posted.write_idx]] = PushStatus::Success;
            }
        }

        if let Some(detail) = first_error {
            self.logger.warn(&format!(
                "remote-lookup-rdma-initiator: write to {}:{} failed ({detail}); \
                 rebuilding the connection and replaying",
                self.addr, self.port
            ));
            self.recover();
            return true;
        }

        // Report every batch whose writes have all landed.
        let finished: Vec<u64> = self
            .batches
            .iter()
            .filter(|(_, b)| b.is_finished())
            .map(|(seq, _)| *seq)
            .collect();
        for seq in finished {
            if let Some(mut batch) = self.batches.remove(&seq) {
                batch.finish(&self.telemetry);
            }
            // A batch completed cleanly, so earlier trouble is behind us.
            self.recoveries = 0;
        }
        true
    }

    /// Rebuild the connection and replay everything that was in flight.
    ///
    /// Ordering is the point. The queue pair is destroyed **first**: outstanding
    /// writes are discarded rather than delivered as completions, so there is nothing
    /// to wait for, and destroying the queue pair is what synchronously guarantees the
    /// NIC has stopped reading pool memory. Only then is it safe to let any batch
    /// report — which is what releases the submitter's pins.
    ///
    /// Replay is whole-batch and idempotent: the same bytes go to the same remote
    /// addresses, the submitter's pins keep the source unchanged, and the remote
    /// landing buffers stay reserved and unpublished until status is reported. Per-write
    /// blame is not attempted because a queue-pair error flushes every outstanding
    /// write, making it unknowable.
    fn recover(&mut self) {
        // Destroying the connection destroys the queue pair: the quiesce barrier.
        self.state = ConnState::Disconnected;
        for slot in self.wr_slots.iter_mut() {
            *slot = None;
        }
        self.free_wr_slots = (0..PUSH_WINDOW).rev().collect();

        self.recoveries += 1;
        if self.recoveries > 1 {
            self.logger.warn(&format!(
                "remote-lookup-rdma-initiator: {}:{} failed again after a rebuild; \
                 failing {} batch(es)",
                self.addr,
                self.port,
                self.batches.len()
            ));
            self.fail_all(PushStatus::UnableToConnect);
            // Let a future batch start from a clean slate rather than inheriting
            // this episode's budget.
            self.recoveries = 0;
            return;
        }

        self.telemetry.record_reconnect();

        // Rewind every batch and re-queue it in submission order.
        let mut seqs: Vec<u64> = self.batches.keys().copied().collect();
        seqs.sort_unstable();
        self.to_post.clear();
        for seq in seqs {
            if let Some(batch) = self.batches.get_mut(&seq) {
                batch.rewind();
                self.to_post.push_back(seq);
            }
        }
    }

    /// Abandon a connection whose posted writes have stopped completing.
    fn check_stalled(&mut self) {
        let stalled = self.batches.values().any(|b| {
            b.outstanding > 0 && b.first_post.is_some_and(|t| t.elapsed() > STALL_TIMEOUT)
        });
        if stalled {
            self.logger.warn(&format!(
                "remote-lookup-rdma-initiator: no completion from {}:{} within {}s; \
                 rebuilding the connection",
                self.addr,
                self.port,
                STALL_TIMEOUT.as_secs()
            ));
            self.recover();
        }
    }

    /// Report every tracked batch with `status` for its writes and forget them.
    fn fail_all(&mut self, status: PushStatus) {
        self.to_post.clear();
        for (_, mut batch) in self.batches.drain() {
            batch.set_all_writes(status);
            batch.finish(&self.telemetry);
        }
    }

    /// Establish the connection if it is not already healthy. Returns whether a
    /// usable connection exists.
    fn ensure_connected(&mut self) -> bool {
        if matches!(&self.state, ConnState::Connected(conn) if conn.qp_healthy()) {
            return true;
        }

        // Drop any stale/error-state connection before reconnecting.
        self.state = ConnState::Disconnected;
        if self.stopping.load(Ordering::SeqCst) {
            // Tearing down: do not start a connect that teardown would then have to
            // wait out. The caller reports its batches as unable-to-connect.
            return false;
        }
        match self.transport.connect(&self.addr, self.port) {
            Ok((conn, timing)) => {
                self.state = ConnState::Connected(conn);
                self.telemetry.record_connection_established();
                if timing.is_measured() {
                    self.telemetry.record_connect_phases(
                        timing.resolve_addr_us,
                        timing.resolve_route_us,
                        timing.handshake_us,
                        timing.mr_reg_us,
                    );
                    self.logger.info(&format!(
                        "remote-lookup-rdma-initiator: connected {}:{} in \
                         resolve_addr={}us route={}us handshake={}us mr_reg={}us",
                        self.addr,
                        self.port,
                        timing.resolve_addr_us,
                        timing.resolve_route_us,
                        timing.handshake_us,
                        timing.mr_reg_us,
                    ));
                }
                true
            }
            Err(e) => {
                self.logger.warn(&format!(
                    "remote-lookup-rdma-initiator: connect to {}:{} failed: {e}",
                    self.addr, self.port
                ));
                self.telemetry.record_connection_failed();
                false
            }
        }
    }

    /// Tear down: quiesce the NIC, then report everything still owed a callback.
    fn shutdown(&mut self) {
        let had_connection = matches!(self.state, ConnState::Connected(_));
        // Destroy the queue pair before reporting anything, so no submitter can
        // reclaim memory the NIC might still be reading.
        self.state = ConnState::Disconnected;
        if had_connection {
            self.telemetry.record_disconnect();
        }
        self.fail_all(PushStatus::UnableToConnect);
        self.logger.debug(&format!(
            "remote-lookup-rdma-initiator: connection thread for {}:{} stopped",
            self.addr, self.port
        ));
    }
}

/// Parse an `"ip:port"` endpoint into its host and port parts.
pub fn parse_endpoint(endpoint: &str) -> Result<(String, u16), RemoteLookupRdmaInitiatorError> {
    let invalid = || {
        RemoteLookupRdmaInitiatorError::InvalidEndpoint(format!(
            "expected \"ip:port\", got {endpoint:?}"
        ))
    };
    let (host, port) = endpoint.rsplit_once(':').ok_or_else(invalid)?;
    if host.is_empty() {
        return Err(invalid());
    }
    let port: u16 = port.parse().map_err(|_| invalid())?;
    if port == 0 {
        return Err(invalid());
    }
    Ok((host.to_string(), port))
}

// --- Real RDMA transport (links rdma-core via crate::rdma; `rdma` feature) ---

/// Production transport: connects via `rdma_cm` and registers the memory-tier
/// pool as an RDMA memory region on each new connection.
#[cfg(feature = "rdma")]
pub struct RealTransport {
    /// Memory-tier pool base address, stored as `usize` so the transport is
    /// `Send + Sync` (the pointer is only ever handed back to `ibv_reg_mr`).
    pool_base: usize,
    pool_size: usize,
    /// This node's zyre PeerId bytes, stamped into every connect `private_data`
    /// so the remote responder can correlate the QP. Empty ⇒ unstamped.
    local_peer_id: Vec<u8>,
}

#[cfg(feature = "rdma")]
impl RealTransport {
    /// Create a transport that registers the pool at `pool_base`/`pool_size` and
    /// stamps `local_peer_id` into each connection's `private_data`.
    pub fn new(pool_base: *mut u8, pool_size: usize, local_peer_id: Vec<u8>) -> Self {
        Self {
            pool_base: pool_base as usize,
            pool_size,
            local_peer_id,
        }
    }
}

#[cfg(feature = "rdma")]
impl RdmaTransport for RealTransport {
    fn connect(
        &self,
        addr: &str,
        port: u16,
    ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
        let (conn, cm) = rdma::client_connect(addr, port, &self.local_peer_id)
            .map_err(|e| RdmaError::ConnectionFailed(format!("{e:#}")))?;
        let mr_start = Instant::now();
        let pool_mr = conn
            .register_existing_mr(self.pool_base as *const u8, self.pool_size)
            .map_err(|e| RdmaError::AllocationFailed(format!("{e:#}")))?;
        let timing = ConnectTiming {
            resolve_addr_us: cm.resolve_addr_us,
            resolve_route_us: cm.resolve_route_us,
            handshake_us: cm.handshake_us,
            mr_reg_us: mr_start.elapsed().as_micros() as u64,
        };
        Ok((Box::new(RealConn { conn, pool_mr }), timing))
    }
}

/// A live `rdma_cm` connection plus its registered memory-tier pool region.
#[cfg(feature = "rdma")]
struct RealConn {
    conn: rdma::RdmaConnection,
    pool_mr: rdma::MemoryRegion,
}

#[cfg(feature = "rdma")]
impl RdmaConn for RealConn {
    fn qp_healthy(&self) -> bool {
        self.conn.is_qp_healthy()
    }

    unsafe fn post_write(&self, write: &WindowWrite, wr_id: u64) -> Result<(), RdmaError> {
        // `{e:#}` renders the full anyhow source chain (e.g. the underlying
        // "ibv_post_send (RDMA_WRITE) failed: …"), not just the top context.
        self.conn
            .post_write_from_pool(
                &self.pool_mr,
                write.local,
                write.len,
                write.remote_addr,
                write.rkey,
                wr_id,
            )
            .map_err(|e| RdmaError::WriteFailed(format!("{e:#}")))
    }

    fn poll_completions(&self, max: usize) -> Result<Vec<Completion>, RdmaError> {
        self.conn
            .poll_completions(max)
            .map_err(|e| RdmaError::WriteFailed(format!("{e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// A logger that discards everything (tests don't assert on log output).
    struct NullLogger;
    impl ILogger for NullLogger {
        fn info(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn error(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    /// What a mock transport's connections did, shared across every connection it
    /// hands out so a test can see across a reconnect.
    #[derive(Default)]
    struct MockLog {
        /// Remote addresses in the order they were posted. A replay therefore shows
        /// as the same run of addresses appearing twice.
        posted: Vec<u64>,
        /// High-water mark of writes posted but not yet reaped. This is the
        /// pipelining measure: 1 would mean we serialize on every completion.
        max_outstanding: usize,
    }

    type SharedLog = Arc<Mutex<MockLog>>;

    struct MockConn {
        healthy: AtomicBool,
        /// If set, the next poll that would return completions fails them all and
        /// drives the queue pair unhealthy — as a real queue-pair error does.
        fail_next_completions: AtomicBool,
        /// Posted `wr_id`s awaiting completion.
        pending: Mutex<VecDeque<u64>>,
        log: SharedLog,
    }

    impl RdmaConn for MockConn {
        fn qp_healthy(&self) -> bool {
            self.healthy.load(Ordering::SeqCst)
        }

        unsafe fn post_write(&self, write: &WindowWrite, wr_id: u64) -> Result<(), RdmaError> {
            let mut pending = self.pending.lock().unwrap();
            pending.push_back(wr_id);
            let mut log = self.log.lock().unwrap();
            log.posted.push(write.remote_addr);
            log.max_outstanding = log.max_outstanding.max(pending.len());
            Ok(())
        }

        fn poll_completions(&self, max: usize) -> Result<Vec<Completion>, RdmaError> {
            let mut pending = self.pending.lock().unwrap();
            let take = max.min(pending.len());
            if take == 0 {
                return Ok(Vec::new());
            }
            let failed = self.fail_next_completions.swap(false, Ordering::SeqCst);
            if failed {
                self.healthy.store(false, Ordering::SeqCst);
            }
            Ok(pending
                .drain(..take)
                .map(|wr_id| Completion {
                    wr_id,
                    error: failed.then(|| "mock completion failure".to_string()),
                })
                .collect())
        }
    }

    #[derive(Default)]
    struct MockTransport {
        /// Shared so tests can assert how many connects happened.
        connect_attempts: Arc<AtomicUsize>,
        /// Number of initial connect attempts that should fail.
        fail_first_n_connects: usize,
        /// The first successfully-connected conn fails its first completions.
        fail_first_completions: AtomicBool,
        log: SharedLog,
    }

    impl RdmaTransport for MockTransport {
        fn connect(
            &self,
            _addr: &str,
            _port: u16,
        ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
            let n = self.connect_attempts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first_n_connects {
                return Err(RdmaError::ConnectionFailed("mock connect failure".into()));
            }
            let conn = MockConn {
                healthy: AtomicBool::new(true),
                fail_next_completions: AtomicBool::new(
                    self.fail_first_completions.swap(false, Ordering::SeqCst),
                ),
                pending: Mutex::new(VecDeque::new()),
                log: Arc::clone(&self.log),
            };
            // The mock does not measure phases; timing is left zeroed.
            Ok((Box::new(conn), ConnectTiming::default()))
        }
    }

    fn write_plan(remote_addr: u64, rkey: u32, len: usize) -> ItemPlan {
        ItemPlan::Write {
            local: remote_addr as *const u8, // arbitrary; mock ignores it
            len,
            remote_addr,
            rkey,
        }
    }

    /// Build a table with a fresh (no-op unless `telemetry` feature) collector.
    fn table_with(transport: MockTransport) -> ConnectionTable {
        ConnectionTable::new(
            Arc::new(transport),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        )
    }

    /// Build a table plus a handle on what its connections did.
    fn table_with_log(transport: MockTransport) -> (ConnectionTable, SharedLog) {
        let log = Arc::clone(&transport.log);
        (table_with(transport), log)
    }

    #[test]
    fn parse_endpoint_valid() {
        assert_eq!(
            parse_endpoint("192.168.1.10:9090").unwrap(),
            ("192.168.1.10".to_string(), 9090)
        );
    }

    #[test]
    fn parse_endpoint_rejects_bad_input() {
        for bad in ["no-port", "host:0", "host:99999", ":9090", "host:abc"] {
            assert!(
                parse_endpoint(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn statuses_mapped_in_order() {
        let table = table_with(MockTransport::default());
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x1000, 7, 256),
            ItemPlan::Done(PushStatus::SizeMismatch),
        ];
        let out = table.push("10.0.0.1:5000", resolved).unwrap();
        assert_eq!(
            out,
            vec![
                PushStatus::KeyNotFound,
                PushStatus::Success,
                PushStatus::SizeMismatch,
            ]
        );
    }

    #[test]
    fn connect_failure_yields_unable_to_connect_for_writes_only() {
        let transport = MockTransport {
            fail_first_n_connects: usize::MAX, // always fail
            ..Default::default()
        };
        let table = table_with(transport);
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x2000, 1, 64),
            write_plan(0x3000, 2, 64),
        ];
        let out = table.push("10.0.0.2:5000", resolved).unwrap();
        assert_eq!(
            out,
            vec![
                PushStatus::KeyNotFound,
                PushStatus::UnableToConnect,
                PushStatus::UnableToConnect,
            ]
        );
    }

    /// The point of posting without waiting: a batch of writes must all be in flight
    /// at once, not serialized one completion at a time. A 64 KiB write is ~2.6 µs of
    /// wire time but ~28 µs of post/poll overhead, so a max-outstanding of 1 would cap
    /// a flow near 9% of line rate however much work the caller offers.
    #[test]
    fn a_whole_batch_is_in_flight_at_once() {
        let (table, log) = table_with_log(MockTransport::default());
        let addrs: Vec<u64> = (1..=64).map(|i| i * 0x1000).collect();
        let resolved: Vec<ItemPlan> = addrs.iter().map(|&a| write_plan(a, 9, 65536)).collect();

        let out = table.push("10.0.0.9:5000", resolved).unwrap();

        assert_eq!(out, vec![PushStatus::Success; 64]);
        let seen = log.lock().unwrap();
        assert_eq!(seen.posted, addrs, "every write, in order");
        assert_eq!(
            seen.max_outstanding, 64,
            "all 64 writes should be outstanding together, not reaped one by one"
        );
    }

    /// A batch deeper than the send queue is posted in full, resuming as completions
    /// free credits — and never exceeds the queue depth.
    #[test]
    fn a_batch_deeper_than_the_send_queue_still_completes() {
        let (table, log) = table_with_log(MockTransport::default());
        let total = PUSH_WINDOW + 72;
        let addrs: Vec<u64> = (0..total).map(|i| (i as u64 + 1) * 0x100).collect();
        let resolved: Vec<ItemPlan> = addrs.iter().map(|&a| write_plan(a, 9, 4096)).collect();

        let out = table.push("10.0.0.10:5000", resolved).unwrap();

        assert_eq!(out, vec![PushStatus::Success; total]);
        let seen = log.lock().unwrap();
        assert_eq!(seen.posted, addrs, "every write, in order");
        assert!(
            seen.max_outstanding <= PUSH_WINDOW,
            "credits must cap in-flight writes at the send-queue depth, saw {}",
            seen.max_outstanding
        );
        assert_eq!(
            seen.max_outstanding, PUSH_WINDOW,
            "and should fill it rather than trickling"
        );
    }

    /// On a failed completion every outstanding write is replayed after one
    /// reconnect, rather than the failure being blamed on individual writes. A
    /// queue-pair error flushes every outstanding request, so per-write blame is not
    /// knowable — and replay is safe because the remote landing buffers stay reserved
    /// and unpublished until status is reported.
    #[test]
    fn lost_writes_are_replayed_in_full_after_one_reconnect() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (table, log) = table_with_log(MockTransport {
            connect_attempts: Arc::clone(&attempts),
            fail_first_completions: AtomicBool::new(true),
            ..Default::default()
        });
        let addrs: Vec<u64> = (1..=8).map(|i| i * 0x2000).collect();
        let resolved: Vec<ItemPlan> = addrs.iter().map(|&a| write_plan(a, 5, 1024)).collect();

        let out = table.push("10.0.0.11:5000", resolved).unwrap();

        assert_eq!(out, vec![PushStatus::Success; 8]);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "one connect, one repair"
        );
        let seen = log.lock().unwrap();
        let mut expected = addrs.clone();
        expected.extend_from_slice(&addrs);
        assert_eq!(
            seen.posted, expected,
            "the replay must repost exactly what was lost"
        );
    }

    /// The one-repair budget: writes that fail again after the reconnect fail their
    /// keys rather than looping forever.
    #[test]
    fn failing_again_after_the_reconnect_gives_up() {
        // Every connection this transport hands out fails its first completions, so
        // the replay fails too.
        struct AlwaysFailing {
            attempts: Arc<AtomicUsize>,
            log: SharedLog,
        }
        impl RdmaTransport for AlwaysFailing {
            fn connect(
                &self,
                _addr: &str,
                _port: u16,
            ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                Ok((
                    Box::new(MockConn {
                        healthy: AtomicBool::new(true),
                        fail_next_completions: AtomicBool::new(true),
                        pending: Mutex::new(VecDeque::new()),
                        log: Arc::clone(&self.log),
                    }),
                    ConnectTiming::default(),
                ))
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let log: SharedLog = Arc::default();
        let table = ConnectionTable::new(
            Arc::new(AlwaysFailing {
                attempts: Arc::clone(&attempts),
                log: Arc::clone(&log),
            }),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        );

        let out = table
            .push(
                "10.0.0.12:5000",
                vec![write_plan(0x1000, 1, 512), write_plan(0x2000, 1, 512)],
            )
            .unwrap();

        assert_eq!(
            out,
            vec![PushStatus::UnableToConnect; 2],
            "writes lost twice fail their keys"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "no third attempt");
        assert_eq!(
            log.lock().unwrap().posted.len(),
            4,
            "the attempt plus one replay"
        );
    }

    /// Items resolved before any RDMA (absent key, size mismatch) keep their status
    /// and must never be posted.
    #[test]
    fn done_items_are_never_posted() {
        let (table, log) = table_with_log(MockTransport::default());
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0xC000, 2, 64),
            ItemPlan::Done(PushStatus::SizeMismatch),
            write_plan(0xD000, 2, 64),
        ];

        let out = table.push("10.0.0.13:5000", resolved).unwrap();

        assert_eq!(
            out,
            vec![
                PushStatus::KeyNotFound,
                PushStatus::Success,
                PushStatus::SizeMismatch,
                PushStatus::Success,
            ]
        );
        assert_eq!(
            log.lock().unwrap().posted,
            vec![0xC000, 0xD000],
            "only the writes"
        );
    }

    /// A batch with nothing to write reports immediately and never connects.
    #[test]
    fn a_batch_with_no_writes_reports_without_connecting() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let table = table_with(MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        });

        let out = table
            .push(
                "10.0.0.14:5000",
                vec![ItemPlan::Done(PushStatus::KeyNotFound)],
            )
            .unwrap();

        assert_eq!(out, vec![PushStatus::KeyNotFound]);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "nothing to send, so nothing to connect for"
        );
    }

    /// The completion callback must fire exactly once — never twice, never not at
    /// all. Everything the submitter owns (read pins, above all) hangs off it.
    #[test]
    fn the_callback_fires_exactly_once() {
        let table = table_with(MockTransport::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let (tx, rx) = mpsc::channel();

        table
            .push_async(
                "10.0.0.15:5000",
                vec![write_plan(0x1000, 1, 64), write_plan(0x2000, 1, 64)],
                Box::new(move |statuses| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = tx.send(statuses);
                }),
            )
            .unwrap();

        let statuses = rx.recv().expect("callback should report");
        assert_eq!(statuses, vec![PushStatus::Success; 2]);
        // The sender is dropped inside the callback, so a second invocation would
        // both bump the counter and be visible as an extra message.
        assert!(rx.recv().is_err(), "callback must not fire twice");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A full submit queue is reported as unable-to-connect rather than queued.
    /// Queuing without bound would pile up held read pins behind a sick peer, and a
    /// pinned entry cannot be evicted — one unreachable peer would stall the local
    /// memory tier.
    #[test]
    fn a_full_submit_queue_fails_fast() {
        /// Stalls in `connect` until released, so nothing drains the queue and the
        /// backlog fills — the state a peer that has gone away puts us in.
        struct Wedged {
            release: Arc<AtomicBool>,
        }
        impl RdmaTransport for Wedged {
            fn connect(
                &self,
                _addr: &str,
                _port: u16,
            ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
                while !self.release.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(RdmaError::ConnectionFailed("released".into()))
            }
        }
        let release = Arc::new(AtomicBool::new(false));
        let table = ConnectionTable::new(
            Arc::new(Wedged {
                release: Arc::clone(&release),
            }),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        );

        // Submit well past what the channel and the thread's backlog can hold. Every
        // batch reports exactly once either way, so counting reports tells us the
        // rejected ones were not swallowed.
        let reports = Arc::new(Mutex::new(Vec::new()));
        let submissions = SUBMIT_QUEUE_DEPTH + MAX_TRACKED_BATCHES + 64;
        for _ in 0..submissions {
            let sink = Arc::clone(&reports);
            table
                .push_async(
                    "10.0.0.16:5000",
                    vec![write_plan(0x1000, 1, 64)],
                    Box::new(move |statuses| sink.lock().unwrap().push(statuses)),
                )
                .unwrap();
        }

        {
            let rejected = reports.lock().unwrap();
            assert!(
                !rejected.is_empty(),
                "submissions past the queue depth must be rejected, not queued: \
                 without that, the backlog of held read pins grows without bound"
            );
            assert!(
                rejected
                    .iter()
                    .all(|s| s == &vec![PushStatus::UnableToConnect]),
                "a rejected batch reports unable-to-connect for its writes"
            );
        }

        // Let the wedged connect return so teardown can join the thread.
        release.store(true, Ordering::SeqCst);
        drop(table);

        // Nothing may be left unreported: the rejected batches plus every batch the
        // thread was holding.
        assert_eq!(
            reports.lock().unwrap().len(),
            submissions,
            "every submitted batch must report exactly once"
        );
    }

    /// Teardown must report every batch it is still holding. A submitter that never
    /// hears back never releases its read pins, and a leaked pin makes its entry
    /// permanently unevictable.
    #[test]
    fn teardown_reports_batches_it_still_holds() {
        // Connects succeed, but completions never arrive, so the batch stays in
        // flight until teardown.
        struct NeverCompletes;
        struct SilentConn;
        impl RdmaConn for SilentConn {
            fn qp_healthy(&self) -> bool {
                true
            }
            unsafe fn post_write(
                &self,
                _write: &WindowWrite,
                _wr_id: u64,
            ) -> Result<(), RdmaError> {
                Ok(())
            }
            fn poll_completions(&self, _max: usize) -> Result<Vec<Completion>, RdmaError> {
                Ok(Vec::new())
            }
        }
        impl RdmaTransport for NeverCompletes {
            fn connect(
                &self,
                _addr: &str,
                _port: u16,
            ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
                Ok((Box::new(SilentConn), ConnectTiming::default()))
            }
        }

        let table = ConnectionTable::new(
            Arc::new(NeverCompletes),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        );
        let (tx, rx) = mpsc::channel();
        table
            .push_async(
                "10.0.0.17:5000",
                vec![write_plan(0x1000, 1, 64)],
                Box::new(move |statuses| {
                    let _ = tx.send(statuses);
                }),
            )
            .unwrap();

        // Nothing has completed, so nothing has been reported yet.
        assert!(rx.try_recv().is_err());

        table.disconnect("10.0.0.17:5000");

        assert_eq!(
            rx.recv().expect("teardown must report the held batch"),
            vec![PushStatus::UnableToConnect],
        );
    }

    /// A connection whose writes stop completing is abandoned and rebuilt, rather
    /// than waiting out the hardware's multi-second retransmit budget.
    #[test]
    fn a_stalled_transfer_is_abandoned_and_the_connection_rebuilt() {
        /// Stalls the first connection's completions forever; later ones work.
        struct StallOnce {
            attempts: Arc<AtomicUsize>,
            log: SharedLog,
        }
        struct StallingConn;
        impl RdmaConn for StallingConn {
            fn qp_healthy(&self) -> bool {
                true
            }
            unsafe fn post_write(
                &self,
                _write: &WindowWrite,
                _wr_id: u64,
            ) -> Result<(), RdmaError> {
                Ok(())
            }
            fn poll_completions(&self, _max: usize) -> Result<Vec<Completion>, RdmaError> {
                Ok(Vec::new())
            }
        }
        impl RdmaTransport for StallOnce {
            fn connect(
                &self,
                _addr: &str,
                _port: u16,
            ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
                let n = self.attempts.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Ok((Box::new(StallingConn), ConnectTiming::default()));
                }
                Ok((
                    Box::new(MockConn {
                        healthy: AtomicBool::new(true),
                        fail_next_completions: AtomicBool::new(false),
                        pending: Mutex::new(VecDeque::new()),
                        log: Arc::clone(&self.log),
                    }),
                    ConnectTiming::default(),
                ))
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let table = ConnectionTable::new(
            Arc::new(StallOnce {
                attempts: Arc::clone(&attempts),
                log: Arc::default(),
            }),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        );

        // The stall detector fires after STALL_TIMEOUT, then the replay lands on a
        // healthy connection.
        let out = table
            .push("10.0.0.18:5000", vec![write_plan(0x1000, 1, 64)])
            .unwrap();

        assert_eq!(
            out,
            vec![PushStatus::Success],
            "the replay on a fresh connection should succeed"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "the stalled connection is abandoned and rebuilt exactly once"
        );
    }

    /// Successive batches to one peer overlap on the wire. This is the property the
    /// whole asynchronous design exists for: with a synchronous push, batch N+1
    /// cannot start until batch N's last completion is reaped, so max-outstanding
    /// could never exceed a single batch.
    #[test]
    fn successive_batches_overlap_on_the_wire() {
        /// Holds completions back until released, so several batches accumulate.
        struct Gated {
            conn_log: SharedLog,
            release: Arc<AtomicBool>,
        }
        struct GatedConn {
            pending: Mutex<VecDeque<u64>>,
            log: SharedLog,
            release: Arc<AtomicBool>,
        }
        impl RdmaConn for GatedConn {
            fn qp_healthy(&self) -> bool {
                true
            }
            unsafe fn post_write(&self, write: &WindowWrite, wr_id: u64) -> Result<(), RdmaError> {
                let mut pending = self.pending.lock().unwrap();
                pending.push_back(wr_id);
                let mut log = self.log.lock().unwrap();
                log.posted.push(write.remote_addr);
                log.max_outstanding = log.max_outstanding.max(pending.len());
                Ok(())
            }
            fn poll_completions(&self, max: usize) -> Result<Vec<Completion>, RdmaError> {
                if !self.release.load(Ordering::SeqCst) {
                    return Ok(Vec::new());
                }
                let mut pending = self.pending.lock().unwrap();
                let take = max.min(pending.len());
                Ok(pending
                    .drain(..take)
                    .map(|wr_id| Completion { wr_id, error: None })
                    .collect())
            }
        }
        impl RdmaTransport for Gated {
            fn connect(
                &self,
                _addr: &str,
                _port: u16,
            ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
                Ok((
                    Box::new(GatedConn {
                        pending: Mutex::new(VecDeque::new()),
                        log: Arc::clone(&self.conn_log),
                        release: Arc::clone(&self.release),
                    }),
                    ConnectTiming::default(),
                ))
            }
        }

        let log: SharedLog = Arc::default();
        let release = Arc::new(AtomicBool::new(false));
        let table = ConnectionTable::new(
            Arc::new(Gated {
                conn_log: Arc::clone(&log),
                release: Arc::clone(&release),
            }),
            Arc::new(TelemetryCollector::new()),
            Arc::new(NullLogger),
        );

        // Four batches of 8, submitted without waiting for any of them.
        let batches = 4;
        let per_batch = 8;
        let done = Arc::new(AtomicUsize::new(0));
        for b in 0..batches {
            let counter = Arc::clone(&done);
            let resolved: Vec<ItemPlan> = (0..per_batch)
                .map(|i| write_plan(((b * per_batch + i) as u64 + 1) * 0x1000, 1, 64))
                .collect();
            table
                .push_async(
                    "10.0.0.19:5000",
                    resolved,
                    Box::new(move |_| {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }),
                )
                .unwrap();
        }

        // Wait until every write has been posted while completions are still gated.
        let deadline = Instant::now() + Duration::from_secs(5);
        while log.lock().unwrap().posted.len() < batches * per_batch {
            assert!(Instant::now() < deadline, "writes were never all posted");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            log.lock().unwrap().max_outstanding > per_batch,
            "more than one batch must be in flight at once, saw {}",
            log.lock().unwrap().max_outstanding
        );

        release.store(true, Ordering::SeqCst);
        while done.load(Ordering::SeqCst) < batches {
            assert!(Instant::now() < deadline, "batches never completed");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn disconnect_forces_fresh_connection() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);

        let out1 = table
            .push("10.0.0.4:5000", vec![write_plan(1, 1, 8)])
            .unwrap();
        assert_eq!(out1, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        table.disconnect("10.0.0.4:5000");

        let out2 = table
            .push("10.0.0.4:5000", vec![write_plan(2, 2, 8)])
            .unwrap();
        assert_eq!(out2, vec![PushStatus::Success]);
        // Disconnect dropped the slot, so a second connect was required.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reused_connection_does_not_reconnect() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);
        // First push connects; second push to the same endpoint reuses it.
        table
            .push("10.0.0.5:5000", vec![write_plan(1, 1, 8)])
            .unwrap();
        let out = table
            .push("10.0.0.5:5000", vec![write_plan(2, 2, 8)])
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        // A single connect served both pushes.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_endpoint_is_method_error() {
        let table = table_with(MockTransport::default());
        let err = table.push("garbage", vec![]).unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::InvalidEndpoint(_)
        ));
    }

    /// A rejected submission drops the callback rather than invoking it. The
    /// interface documents that as equivalent for a callback that releases on drop,
    /// which is what makes an unparseable endpoint safe.
    #[test]
    fn invalid_endpoint_drops_the_callback() {
        let table = table_with(MockTransport::default());
        let dropped = Arc::new(AtomicBool::new(false));

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let flag = DropFlag(Arc::clone(&dropped));

        let err = table.push_async(
            "garbage",
            vec![write_plan(1, 1, 8)],
            Box::new(move |_| {
                // Captured so the guard's lifetime is the callback's.
                let _ = &flag;
            }),
        );
        assert!(err.is_err());
        assert!(
            dropped.load(Ordering::SeqCst),
            "the callback (and whatever it owns) must be released"
        );
    }

    #[test]
    fn warm_connect_establishes_and_push_reuses() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);
        // Warming is queued to the connection thread, so wait for it to land.
        table.connect("10.0.0.5:5000").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "warm never connected");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        // ...so a subsequent push to the same endpoint reuses it (no reconnect).
        let out = table
            .push("10.0.0.5:5000", vec![write_plan(1, 1, 8)])
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn warm_connect_failure_is_ok_and_caches_nothing() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            fail_first_n_connects: 1,
            ..Default::default()
        };
        let table = table_with(transport);
        // A failed warm still returns Ok (never surfaces a transient failure)...
        table.connect("10.0.0.7:5000").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "warm never attempted a connect");
            thread::sleep(Duration::from_millis(5));
        }
        // ...and caches nothing, so the next push retries the connect and wins.
        let out = table
            .push("10.0.0.7:5000", vec![write_plan(1, 1, 8)])
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn warm_connect_invalid_endpoint_is_method_error() {
        let table = table_with(MockTransport::default());
        let err = table.connect("garbage").unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::InvalidEndpoint(_)
        ));
    }

    // --- Telemetry wiring (only meaningful with the `telemetry` feature) ---

    #[cfg(feature = "telemetry")]
    fn table_with_telemetry(
        transport: MockTransport,
    ) -> (ConnectionTable, Arc<TelemetryCollector>) {
        let telemetry = Arc::new(TelemetryCollector::new());
        let table = ConnectionTable::new(
            Arc::new(transport),
            Arc::clone(&telemetry),
            Arc::new(NullLogger),
        );
        (table, telemetry)
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_push_outcomes() {
        let (table, tm) = table_with_telemetry(MockTransport::default());
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x1000, 7, 256),
            ItemPlan::Done(PushStatus::SizeMismatch),
        ];
        table.push("10.0.0.9:5000", resolved).unwrap();
        assert_eq!(tm.items_success(), 1);
        assert_eq!(tm.items_key_not_found(), 1);
        assert_eq!(tm.items_size_mismatch(), 1);
        assert_eq!(tm.bytes_written(), 256);
        assert_eq!(tm.connections_established(), 1);
        assert_eq!(tm.pushes(), 1);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_reconnect() {
        let transport = MockTransport {
            fail_first_completions: AtomicBool::new(true),
            ..Default::default()
        };
        let (table, tm) = table_with_telemetry(transport);
        table
            .push("10.0.0.10:5000", vec![write_plan(1, 1, 64)])
            .unwrap();
        assert_eq!(tm.reconnects(), 1);
        // Initial connect + one repair.
        assert_eq!(tm.connections_established(), 2);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_connection_failure() {
        let transport = MockTransport {
            fail_first_n_connects: usize::MAX,
            ..Default::default()
        };
        let (table, tm) = table_with_telemetry(transport);
        table
            .push("10.0.0.11:5000", vec![write_plan(1, 1, 64)])
            .unwrap();
        assert!(tm.connection_failures() >= 1);
        assert_eq!(tm.items_unable_to_connect(), 1);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_disconnect() {
        let (table, tm) = table_with_telemetry(MockTransport::default());
        table
            .push("10.0.0.12:5000", vec![write_plan(1, 1, 8)])
            .unwrap();
        table.disconnect("10.0.0.12:5000");
        assert_eq!(tm.disconnects(), 1);
    }
}
