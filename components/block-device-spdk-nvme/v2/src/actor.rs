//! Actor handler for the SPDK NVMe block device component.
//!
//! [`BlockDeviceHandler`] implements [`ActorHandler<ControlMessage>`] and
//! processes control messages (connect/disconnect clients) on the actor's
//! main MPSC channel. On each `handle()` call it also polls all connected
//! client ingress channels for IO commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::tsc::TscClock;

use component_core::actor::ActorHandler;
use component_core::channel::ChannelError;

use interfaces::{Command, Completion, DmaBuffer, ILogger, NvmeBlockError, OpHandle};

use crate::command::{ClientSession, ControlMessage};
use crate::controller::NvmeController;
use crate::namespace;

#[cfg(feature = "telemetry")]
use crate::telemetry::TelemetryStats;

/// Entry produced by async SPDK completion callbacks.
pub(crate) struct AsyncCompletionEntry {
    /// Client that submitted the operation.
    pub client_id: u64,
    /// Operation handle assigned at submission.
    pub handle: u64,
    /// Result of the operation.
    pub result: Result<(), NvmeBlockError>,
    /// Whether this was a read (true) or write (false).
    pub is_read: bool,
    /// Telemetry: latency in nanoseconds.
    #[cfg(feature = "telemetry")]
    pub latency_ns: u64,
    /// Telemetry: bytes transferred.
    #[cfg(feature = "telemetry")]
    pub bytes: u64,
}

/// Context passed to SPDK async completion callbacks via raw pointer.
///
/// Allocated from [`ContextPool`] at submission time and returned to the
/// pool in the callback. The `completions` pointer is valid because
/// callbacks execute on the actor thread during `process_completions()`.
struct AsyncIoContext {
    client_id: u64,
    handle: u64,
    is_read: bool,
    completions: *mut Vec<AsyncCompletionEntry>,
    pool: *mut ContextPool,
    #[cfg(feature = "telemetry")]
    start: u64,
    #[cfg(feature = "telemetry")]
    bytes: u64,
    #[cfg(feature = "telemetry")]
    tsc: *const TscClock,
}

// SAFETY: AsyncIoContext is only used on the actor thread. Raw pointers
// (completions, pool) are valid for the actor thread's lifetime.
unsafe impl Send for AsyncIoContext {}

/// Fixed-capacity slab pool for [`AsyncIoContext`], eliminating per-IO
/// heap allocation. All operations are single-threaded (actor thread).
struct ContextPool {
    #[allow(clippy::vec_box)] // Box required: SPDK takes ownership via Box::into_raw.
    slots: Vec<Box<AsyncIoContext>>,
}

impl ContextPool {
    fn new(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
        }
    }

    /// Acquire a context from the pool, or allocate a new one if empty.
    fn acquire(&mut self) -> Box<AsyncIoContext> {
        self.slots.pop().unwrap_or_else(|| {
            Box::new(AsyncIoContext {
                client_id: 0,
                handle: 0,
                is_read: false,
                completions: std::ptr::null_mut(),
                pool: std::ptr::null_mut(),
                #[cfg(feature = "telemetry")]
                start: 0,
                #[cfg(feature = "telemetry")]
                bytes: 0,
                #[cfg(feature = "telemetry")]
                tsc: std::ptr::null(),
            })
        })
    }

    /// Return a context to the pool for reuse.
    fn release(&mut self, ctx: Box<AsyncIoContext>) {
        self.slots.push(ctx);
    }
}

/// SPDK NVMe command completion callback for asynchronous operations.
///
/// # Safety
///
/// `ctx` must point to a valid `Box<AsyncIoContext>` that was leaked via
/// `Box::into_raw`. `cpl` must be a valid SPDK NVMe completion entry.
/// This callback runs on the actor thread during `process_completions()`.
unsafe extern "C" fn async_completion_cb(
    ctx: *mut std::ffi::c_void,
    cpl: *const spdk_sys::spdk_nvme_cpl,
) {
    // SAFETY: ctx was created via Box::into_raw from ContextPool::acquire().
    let io_ctx = unsafe { Box::from_raw(ctx as *mut AsyncIoContext) };
    // SAFETY: cpl is a valid SPDK completion entry.
    let status = unsafe { (*cpl).__bindgen_anon_1.status };
    let sct = status.sct();
    let sc = status.sc();

    let result = if sct == 0 && sc == 0 {
        Ok(())
    } else {
        let err_msg = format!("NVMe error sct={sct} sc={sc}");
        if io_ctx.is_read {
            Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::ReadFailed(err_msg),
            ))
        } else {
            Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(err_msg),
            ))
        }
    };

    // Extract raw pointers before moving fields out of io_ctx.
    let completions_ptr = io_ctx.completions;
    let pool_ptr = io_ctx.pool;
    let client_id = io_ctx.client_id;
    let handle = io_ctx.handle;
    let is_read = io_ctx.is_read;
    #[cfg(feature = "telemetry")]
    let latency_ns = {
        // SAFETY: tsc pointer is valid — it points to the handler's TscClock
        // which outlives all in-flight IOs.
        let tsc = unsafe { &*io_ctx.tsc };
        tsc.ticks_to_ns(crate::tsc::rdtsc() - io_ctx.start)
    };
    #[cfg(feature = "telemetry")]
    let bytes = io_ctx.bytes;

    // SAFETY: completions pointer is valid — callback runs on the actor
    // thread during process_completions(), same thread that owns the Vec.
    let completions = unsafe { &mut *completions_ptr };
    completions.push(AsyncCompletionEntry {
        client_id,
        handle,
        result,
        is_read,
        #[cfg(feature = "telemetry")]
        latency_ns,
        #[cfg(feature = "telemetry")]
        bytes,
    });

    // SAFETY: pool pointer is valid — same actor thread owns the pool.
    let pool = unsafe { &mut *pool_ptr };
    pool.release(io_ctx);
}

/// Tracks an in-flight asynchronous IO operation.
#[allow(dead_code)]
pub(crate) struct PendingOp {
    /// Component-assigned operation handle.
    pub handle: u64,
    /// Timeout deadline (TSC ticks).
    pub deadline: u64,
    /// Index of the queue pair used for this operation.
    pub qpair_idx: usize,
    /// Pinned read buffer — keeps DMA memory alive until SPDK completion.
    pub read_buf: Option<Arc<Mutex<DmaBuffer>>>,
    /// Pinned write buffer — keeps DMA memory alive until SPDK completion.
    pub write_buf: Option<Arc<DmaBuffer>>,
}

// SAFETY: PendingOp is only accessed from the actor thread.
unsafe impl Send for PendingOp {}

/// Per-client state maintained by the actor, extending `ClientSession`
/// with async operation tracking.
struct ClientState {
    /// The underlying channel session.
    session: ClientSession,
    /// In-flight async operations keyed by handle.
    pending_ops: HashMap<u64, PendingOp>,
}

/// Actor handler for the NVMe block device component.
///
/// Owns the NVMe controller and manages client sessions. The actor runs
/// on a dedicated thread pinned to the NUMA node of the NVMe controller.
pub(crate) struct BlockDeviceHandler {
    /// The attached NVMe controller.
    controller: NvmeController,
    /// Connected client sessions.
    clients: Vec<ClientState>,
    /// Monotonically increasing operation handle counter.
    next_handle: u64,
    /// Buffer for async SPDK completion entries, filled during
    /// `process_completions()` and drained afterward.
    async_completions: Vec<AsyncCompletionEntry>,
    /// Reusable scratch buffer for draining completions without allocating.
    completion_scratch: Vec<AsyncCompletionEntry>,
    /// Slab pool for async IO context objects, avoiding per-IO heap allocation.
    context_pool: ContextPool,
    /// Reusable scratch buffer for timed-out operation handles.
    timeout_scratch: Vec<u64>,
    /// Telemetry stats collector (feature-gated).
    #[cfg(feature = "telemetry")]
    pub telemetry: Arc<TelemetryStats>,
    /// Last time `check_timeouts()` was called (TSC ticks), throttled to ~1ms.
    last_timeout_check: u64,
    /// Low-overhead TSC clock for hot-path timing.
    tsc: TscClock,
    /// Optional logger from the component's ILogger receptacle.
    logger: Option<Arc<dyn ILogger + Send + Sync>>,
}

/// Completion context for synchronous SPDK NVMe commands.
///
/// Shared between the caller and the SPDK completion callback via raw pointer.
struct SyncCompletionCtx {
    done: std::sync::atomic::AtomicBool,
    /// NVMe status code type (0 = success).
    sct: std::sync::atomic::AtomicU16,
    /// NVMe status code (0 = success).
    sc: std::sync::atomic::AtomicU16,
}

/// SPDK NVMe command completion callback for synchronous operations.
///
/// # Safety
///
/// `ctx` must point to a valid `SyncCompletionCtx`. `cpl` must be
/// a valid SPDK NVMe completion entry.
unsafe extern "C" fn sync_completion_cb(
    ctx: *mut std::ffi::c_void,
    cpl: *const spdk_sys::spdk_nvme_cpl,
) {
    // SAFETY: ctx was created from a &SyncCompletionCtx reference.
    let completion = unsafe { &*(ctx as *const SyncCompletionCtx) };
    // SAFETY: cpl is a valid SPDK completion entry.
    let status = unsafe { (*cpl).__bindgen_anon_1.status };
    completion
        .sct
        .store(status.sct(), std::sync::atomic::Ordering::Release);
    completion
        .sc
        .store(status.sc(), std::sync::atomic::Ordering::Release);
    completion
        .done
        .store(true, std::sync::atomic::Ordering::Release);
}

impl BlockDeviceHandler {
    /// Pre-allocated context pool capacity (sum of standard qpair depths).
    const CONTEXT_POOL_CAPACITY: usize = 340;

    /// Create a new handler with the given controller.
    #[allow(dead_code)]
    pub(crate) fn new(
        controller: NvmeController,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
    ) -> Self {
        let tsc = TscClock::new();
        let now = tsc.now();
        Self {
            controller,
            clients: Vec::new(),
            next_handle: 1,
            async_completions: Vec::new(),
            completion_scratch: Vec::new(),
            context_pool: ContextPool::new(Self::CONTEXT_POOL_CAPACITY),
            timeout_scratch: Vec::new(),
            #[cfg(feature = "telemetry")]
            telemetry: Arc::new(TelemetryStats::new()),
            last_timeout_check: now,
            tsc,
            logger,
        }
    }

    /// Create a new handler with the given controller and shared telemetry.
    #[cfg(feature = "telemetry")]
    pub(crate) fn with_telemetry(
        controller: NvmeController,
        telemetry: Arc<TelemetryStats>,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
    ) -> Self {
        let tsc = TscClock::new();
        let now = tsc.now();
        Self {
            controller,
            clients: Vec::new(),
            next_handle: 1,
            async_completions: Vec::new(),
            completion_scratch: Vec::new(),
            context_pool: ContextPool::new(Self::CONTEXT_POOL_CAPACITY),
            timeout_scratch: Vec::new(),
            telemetry,
            last_timeout_check: now,
            tsc,
            logger,
        }
    }

    fn log_info(&self, msg: &str) {
        if let Some(ref log) = self.logger {
            log.info(msg);
        }
    }

    fn log_debug(&self, msg: &str) {
        if let Some(ref log) = self.logger {
            log.debug(msg);
        }
    }

    /// Allocate a new unique operation handle.
    #[allow(dead_code)]
    fn alloc_handle(&mut self) -> u64 {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    /// Poll all client ingress channels for IO commands.
    ///
    /// Also detects disconnected clients (sender dropped) and removes
    /// them silently per FR-019. Returns `true` if any commands or
    /// completions were processed.
    fn poll_clients(&mut self) -> bool {
        let mut did_work = false;
        let mut i = 0;
        while i < self.clients.len() {
            let mut disconnected = false;
            loop {
                let client = &mut self.clients[i];
                match client.session.ingress_rx.try_recv() {
                    Ok(cmd) => {
                        did_work = true;
                        if matches!(cmd, Command::ControllerReset) {
                            let client_id = self.clients[i].session.id;
                            self.handle_controller_reset(client_id);
                            continue;
                        }
                        let ClientState {
                            session,
                            pending_ops,
                        } = &mut self.clients[i];
                        Self::dispatch_command(
                            &mut self.controller,
                            session,
                            pending_ops,
                            &mut self.next_handle,
                            #[cfg(feature = "telemetry")]
                            &self.telemetry,
                            &mut self.async_completions,
                            &mut self.context_pool,
                            &self.tsc,
                            None,
                            cmd,
                        );
                    }
                    Err(ChannelError::Empty) => break,
                    Err(ChannelError::Closed) => {
                        disconnected = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
            if disconnected {
                self.log_debug(&format!(
                    "client {} disconnected (channel closed)",
                    self.clients[i].session.id
                ));
                self.clients.swap_remove(i);
            } else {
                i += 1;
            }
        }

        // Process SPDK qpair completions for async operations.
        for qp_idx in 0..self.controller.qpairs.len() {
            if let Some(qp) = self.controller.qpairs.get_mut(qp_idx) {
                // SAFETY: queue pair pointer is valid while controller is alive.
                let n = unsafe { qp.process_completions(0) };
                if n > 0 {
                    did_work = true;
                }
            }
        }

        // Swap completions into scratch buffer to avoid per-poll allocation.
        std::mem::swap(&mut self.completion_scratch, &mut self.async_completions);
        for entry in self.completion_scratch.drain(..) {
            if let Some(client) = self
                .clients
                .iter_mut()
                .find(|c| c.session.id == entry.client_id)
            {
                // If the handle was already removed (aborted or timed out),
                // silently discard the completion.
                if client.pending_ops.remove(&entry.handle).is_none() {
                    continue;
                }

                #[cfg(feature = "telemetry")]
                if entry.result.is_ok() {
                    self.telemetry.record(entry.latency_ns, entry.bytes);
                }

                let completion = if entry.is_read {
                    Completion::ReadDone {
                        handle: OpHandle(entry.handle),
                        result: entry.result,
                    }
                } else {
                    Completion::WriteDone {
                        handle: OpHandle(entry.handle),
                        result: entry.result,
                    }
                };
                let _ = client.session.callback_tx.send(completion);
            }
        }

        did_work
    }

    /// Check for timed-out async operations across all clients.
    fn check_timeouts(&mut self) {
        let now = self.tsc.now();
        for client in &mut self.clients {
            self.timeout_scratch.clear();
            for (&handle, op) in &client.pending_ops {
                if now >= op.deadline {
                    self.timeout_scratch.push(handle);
                }
            }
            for &handle in &self.timeout_scratch {
                client.pending_ops.remove(&handle);
                let _ = client.session.callback_tx.send(Completion::Timeout {
                    handle: OpHandle(handle),
                });
            }
        }
    }

    /// Handle a controller reset, cancelling ALL clients' pending ops.
    fn handle_controller_reset(&mut self, requesting_client_id: u64) {
        self.log_info(&format!(
            "controller reset requested by client {requesting_client_id}"
        ));
        for client in &mut self.clients {
            for (&h, _) in client.pending_ops.iter() {
                let _ = client.session.callback_tx.send(Completion::Error {
                    handle: Some(OpHandle(h)),
                    error: NvmeBlockError::Aborted("cancelled due to controller reset".into()),
                });
            }
            client.pending_ops.clear();
        }

        // SAFETY: controller pointer is valid while actor is running.
        let rc = unsafe { spdk_sys::spdk_nvme_ctrlr_reset(self.controller.as_ptr()) };
        let result = if rc == 0 {
            self.controller.refresh_namespaces();
            Ok(())
        } else {
            Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::ProbeFailure(format!(
                    "spdk_nvme_ctrlr_reset failed with rc={rc}"
                )),
            ))
        };

        if let Some(client) = self
            .clients
            .iter()
            .find(|c| c.session.id == requesting_client_id)
        {
            let _ = client
                .session
                .callback_tx
                .send(Completion::ResetDone { result });
        }
    }

    /// Dispatch a single command from a client.
    ///
    /// `qp_idx_override` allows a parent (e.g. `BatchSubmit`) to force all
    /// sub-commands onto a specific queue pair instead of selecting per-op.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_command(
        controller: &mut NvmeController,
        session: &mut ClientSession,
        pending_ops: &mut HashMap<u64, PendingOp>,
        next_handle: &mut u64,
        #[cfg(feature = "telemetry")] telemetry: &TelemetryStats,
        async_completions: &mut Vec<AsyncCompletionEntry>,
        context_pool: &mut ContextPool,
        tsc: &TscClock,
        qp_idx_override: Option<usize>,
        cmd: Command,
    ) {
        match cmd {
            Command::ReadSync { ns_id, lba, buf } => {
                let handle = *next_handle;
                *next_handle += 1;

                #[cfg(feature = "telemetry")]
                let bytes = {
                    let guard = buf.lock();
                    guard.map(|g| g.len() as u64).unwrap_or(0)
                };

                #[cfg(feature = "telemetry")]
                let start = tsc.now();

                let result = Self::do_sync_read(controller, ns_id, lba, &buf);

                #[cfg(feature = "telemetry")]
                if result.is_ok() {
                    telemetry.record(tsc.ticks_to_ns(tsc.now() - start), bytes);
                }

                let _ = session.callback_tx.send(Completion::ReadDone {
                    handle: OpHandle(handle),
                    result,
                });
            }
            Command::WriteSync { ns_id, lba, buf } => {
                let handle = *next_handle;
                *next_handle += 1;

                #[cfg(feature = "telemetry")]
                let bytes = buf.len() as u64;

                #[cfg(feature = "telemetry")]
                let start = tsc.now();

                let result = Self::do_sync_write(controller, ns_id, lba, &buf);

                #[cfg(feature = "telemetry")]
                if result.is_ok() {
                    telemetry.record(tsc.ticks_to_ns(tsc.now() - start), bytes);
                }

                let _ = session.callback_tx.send(Completion::WriteDone {
                    handle: OpHandle(handle),
                    result,
                });
            }
            Command::ReadAsync {
                ns_id,
                lba,
                buf,
                timeout_ms,
            } => {
                let handle = *next_handle;
                *next_handle += 1;

                // Validate and extract buffer pointer in a single lock.
                let validation = Self::validate_async_read(controller, ns_id, lba, &buf);
                if let Err(e) = validation {
                    let _ = session.callback_tx.send(Completion::ReadDone {
                        handle: OpHandle(handle),
                        result: Err(e),
                    });
                    return;
                }
                #[allow(unused_variables)]
                let (ns_ptr, num_blocks, buf_ptr, buf_len) = validation.unwrap();

                let now = tsc.now();
                let qp_idx = qp_idx_override
                    .unwrap_or_else(|| controller.qpairs.select_index(pending_ops.len() + 1));
                pending_ops.insert(
                    handle,
                    PendingOp {
                        handle,
                        deadline: tsc.deadline_from_ms(now, timeout_ms),
                        qpair_idx: qp_idx,
                        read_buf: Some(buf.clone()),
                        write_buf: None,
                    },
                );

                // Submit async SPDK read — buf_ptr was extracted during validation.
                let mut ctx = context_pool.acquire();
                ctx.client_id = session.id;
                ctx.handle = handle;
                ctx.is_read = true;
                ctx.completions = async_completions as *mut Vec<AsyncCompletionEntry>;
                ctx.pool = context_pool as *mut ContextPool;
                #[cfg(feature = "telemetry")]
                {
                    ctx.start = now;
                    ctx.bytes = buf_len;
                    ctx.tsc = tsc as *const TscClock;
                }

                let qp = controller
                    .qpairs
                    .get_mut(qp_idx)
                    .expect("qpair index valid");
                let ctx_raw = Box::into_raw(ctx);
                let rc = unsafe {
                    spdk_sys::spdk_nvme_ns_cmd_read(
                        ns_ptr,
                        qp.as_ptr(),
                        buf_ptr,
                        lba,
                        num_blocks,
                        Some(async_completion_cb),
                        ctx_raw as *mut std::ffi::c_void,
                        0,
                    )
                };

                if rc != 0 {
                    // SAFETY: submission failed, SPDK did not take ownership.
                    let ctx = unsafe { Box::from_raw(ctx_raw) };
                    context_pool.release(ctx);
                    pending_ops.remove(&handle);
                    let _ = session.callback_tx.send(Completion::ReadDone {
                        handle: OpHandle(handle),
                        result: Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::ReadFailed(format!(
                                "async spdk_nvme_ns_cmd_read submit failed with rc={rc}"
                            )),
                        )),
                    });
                } else {
                    qp.submit();
                }
            }
            Command::WriteAsync {
                ns_id,
                lba,
                buf,
                timeout_ms,
            } => {
                let handle = *next_handle;
                *next_handle += 1;

                // Validate before async submission.
                let validation = Self::validate_async_write(controller, ns_id, lba, &buf);
                if let Err(e) = validation {
                    let _ = session.callback_tx.send(Completion::WriteDone {
                        handle: OpHandle(handle),
                        result: Err(e),
                    });
                    return;
                }
                let (ns_ptr, num_blocks) = validation.unwrap();

                let now = tsc.now();
                let qp_idx = qp_idx_override
                    .unwrap_or_else(|| controller.qpairs.select_index(pending_ops.len() + 1));
                pending_ops.insert(
                    handle,
                    PendingOp {
                        handle,
                        deadline: tsc.deadline_from_ms(now, timeout_ms),
                        qpair_idx: qp_idx,
                        read_buf: None,
                        write_buf: Some(buf.clone()),
                    },
                );

                // Submit async SPDK write.
                let mut ctx = context_pool.acquire();
                ctx.client_id = session.id;
                ctx.handle = handle;
                ctx.is_read = false;
                ctx.completions = async_completions as *mut Vec<AsyncCompletionEntry>;
                ctx.pool = context_pool as *mut ContextPool;
                #[cfg(feature = "telemetry")]
                {
                    ctx.start = now;
                    ctx.bytes = buf.len() as u64;
                    ctx.tsc = tsc as *const TscClock;
                }

                let qp = controller
                    .qpairs
                    .get_mut(qp_idx)
                    .expect("qpair index valid");
                let ctx_raw = Box::into_raw(ctx);
                let rc = unsafe {
                    spdk_sys::spdk_nvme_ns_cmd_write(
                        ns_ptr,
                        qp.as_ptr(),
                        buf.as_ptr() as *mut _,
                        lba,
                        num_blocks,
                        Some(async_completion_cb),
                        ctx_raw as *mut std::ffi::c_void,
                        0,
                    )
                };

                if rc != 0 {
                    // SAFETY: submission failed, SPDK did not take ownership.
                    let ctx = unsafe { Box::from_raw(ctx_raw) };
                    context_pool.release(ctx);
                    pending_ops.remove(&handle);
                    let _ = session.callback_tx.send(Completion::WriteDone {
                        handle: OpHandle(handle),
                        result: Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::WriteFailed(format!(
                                "async spdk_nvme_ns_cmd_write submit failed with rc={rc}"
                            )),
                        )),
                    });
                } else {
                    qp.submit();
                }
            }
            Command::WriteZeros {
                ns_id,
                lba,
                num_blocks,
            } => {
                let handle = *next_handle;
                *next_handle += 1;

                let result = Self::do_write_zeros(controller, ns_id, lba, num_blocks);

                let _ = session.callback_tx.send(Completion::WriteZerosDone {
                    handle: OpHandle(handle),
                    result,
                });
            }
            Command::BatchSubmit { ops } => {
                let batch_size = ops.len();
                let batch_qp_idx = controller.qpairs.select_index(batch_size);

                for op in ops {
                    Self::dispatch_command(
                        controller,
                        session,
                        pending_ops,
                        next_handle,
                        #[cfg(feature = "telemetry")]
                        telemetry,
                        async_completions,
                        context_pool,
                        tsc,
                        Some(batch_qp_idx),
                        op,
                    );
                }
            }
            Command::AbortOp { handle } => {
                let h = handle.0;
                // Remove from pending if present; ack regardless.
                let _ = pending_ops.remove(&h);
                let _ = session.callback_tx.send(Completion::AbortAck { handle });
            }
            Command::NsProbe => {
                let namespaces = namespace::to_namespace_info_list(&controller.namespaces);
                let _ = session
                    .callback_tx
                    .send(Completion::NsProbeResult { namespaces });
            }
            Command::NsCreate { size_sectors, sector_size } => {
                // SAFETY: controller pointer is valid while actor is running.
                let result = unsafe { namespace::create(controller.as_ptr(), size_sectors, sector_size) };
                match result {
                    Ok(ns_id) => {
                        controller.refresh_namespaces();
                        let _ = session.callback_tx.send(Completion::NsCreated { ns_id });
                    }
                    Err(e) => {
                        let _ = session.callback_tx.send(Completion::Error {
                            handle: None,
                            error: e,
                        });
                    }
                }
            }
            Command::NsFormat { ns_id } => {
                // SAFETY: controller pointer is valid while actor is running.
                let result = unsafe { namespace::format(controller.as_ptr(), ns_id) };
                match result {
                    Ok(()) => {
                        let _ = session.callback_tx.send(Completion::NsFormatted { ns_id });
                    }
                    Err(e) => {
                        let _ = session.callback_tx.send(Completion::Error {
                            handle: None,
                            error: e,
                        });
                    }
                }
            }
            Command::NsDelete { ns_id } => {
                // SAFETY: controller pointer is valid while actor is running.
                let result = unsafe { namespace::delete(controller.as_ptr(), ns_id) };
                match result {
                    Ok(()) => {
                        controller.refresh_namespaces();
                        let _ = session.callback_tx.send(Completion::NsDeleted { ns_id });
                    }
                    Err(e) => {
                        let _ = session.callback_tx.send(Completion::Error {
                            handle: None,
                            error: e,
                        });
                    }
                }
            }
            Command::ControllerReset => {
                unreachable!("ControllerReset is intercepted in poll_clients");
            }
        }
    }

    /// Poll a queue pair until the synchronous completion context is signaled.
    ///
    /// Returns an error if the NVMe status indicates a failure.
    fn poll_sync_completion(
        qp: &mut crate::qpair::QueuePair,
        ctx: &SyncCompletionCtx,
        op_name: &str,
    ) -> Result<(), NvmeBlockError> {
        while !ctx.done.load(std::sync::atomic::Ordering::Acquire) {
            // SAFETY: queue pair pointer is valid.
            unsafe {
                qp.process_completions(0);
            }
        }
        let sct = ctx.sct.load(std::sync::atomic::Ordering::Acquire);
        let sc = ctx.sc.load(std::sync::atomic::Ordering::Acquire);
        if sct != 0 || sc != 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(format!(
                    "{op_name}: NVMe error sct={sct} sc={sc}"
                )),
            ));
        }
        Ok(())
    }

    /// Execute a synchronous read via SPDK.
    fn do_sync_read(
        controller: &mut NvmeController,
        ns_id: u32,
        lba: u64,
        buf: &Arc<std::sync::Mutex<interfaces::DmaBuffer>>,
    ) -> Result<(), NvmeBlockError> {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        let buf_guard = buf
            .lock()
            .map_err(|_| NvmeBlockError::ClientDisconnected("buffer lock poisoned".into()))?;
        let num_blocks = buf_guard.len() as u64 / ns.sector_size as u64;
        namespace::validate_lba_range(ns, lba, num_blocks)?;

        // Capture pointers before mutable borrow of qpairs.
        let ctrlr_ptr = controller.as_ptr();

        // SAFETY: All pointers are valid; SPDK environment is initialized.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };

        let completion = SyncCompletionCtx {
            done: std::sync::atomic::AtomicBool::new(false),
            sct: std::sync::atomic::AtomicU16::new(0),
            sc: std::sync::atomic::AtomicU16::new(0),
        };

        let qp = controller.qpairs.select_qpair(1);

        let rc = unsafe {
            spdk_sys::spdk_nvme_ns_cmd_read(
                ns_ptr,
                qp.as_ptr(),
                buf_guard.as_ptr(),
                lba,
                num_blocks as u32,
                Some(sync_completion_cb),
                &completion as *const SyncCompletionCtx as *mut std::ffi::c_void,
                0,
            )
        };

        if rc != 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::ReadFailed(format!(
                    "spdk_nvme_ns_cmd_read failed with rc={rc}"
                )),
            ));
        }

        Self::poll_sync_completion(qp, &completion, "read")
    }

    /// Execute a synchronous write via SPDK.
    fn do_sync_write(
        controller: &mut NvmeController,
        ns_id: u32,
        lba: u64,
        buf: &Arc<interfaces::DmaBuffer>,
    ) -> Result<(), NvmeBlockError> {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        let num_blocks = buf.len() as u64 / ns.sector_size as u64;
        namespace::validate_lba_range(ns, lba, num_blocks)?;

        // Capture pointers before mutable borrow of qpairs.
        let ctrlr_ptr = controller.as_ptr();

        // SAFETY: All pointers are valid; SPDK environment is initialized.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };

        let completion = SyncCompletionCtx {
            done: std::sync::atomic::AtomicBool::new(false),
            sct: std::sync::atomic::AtomicU16::new(0),
            sc: std::sync::atomic::AtomicU16::new(0),
        };

        let qp = controller.qpairs.select_qpair(1);

        let rc = unsafe {
            spdk_sys::spdk_nvme_ns_cmd_write(
                ns_ptr,
                qp.as_ptr(),
                buf.as_ptr() as *mut _,
                lba,
                num_blocks as u32,
                Some(sync_completion_cb),
                &completion as *const SyncCompletionCtx as *mut std::ffi::c_void,
                0,
            )
        };

        if rc != 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(format!(
                    "spdk_nvme_ns_cmd_write failed with rc={rc}"
                )),
            ));
        }

        Self::poll_sync_completion(qp, &completion, "write")
    }

    /// Execute a write-zeros command via `spdk_nvme_ns_cmd_write_zeroes`.
    fn do_write_zeros(
        controller: &mut NvmeController,
        ns_id: u32,
        lba: u64,
        num_blocks: u32,
    ) -> Result<(), NvmeBlockError> {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        namespace::validate_lba_range(ns, lba, num_blocks as u64)?;

        let ctrlr_ptr = controller.as_ptr();
        // SAFETY: ctrlr_ptr is valid; SPDK environment is initialized.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };

        let completion = SyncCompletionCtx {
            done: std::sync::atomic::AtomicBool::new(false),
            sct: std::sync::atomic::AtomicU16::new(0),
            sc: std::sync::atomic::AtomicU16::new(0),
        };

        let qp = controller.qpairs.select_qpair(1);

        let rc = unsafe {
            spdk_sys::spdk_nvme_ns_cmd_write_zeroes(
                ns_ptr,
                qp.as_ptr(),
                lba,
                num_blocks,
                Some(sync_completion_cb),
                &completion as *const SyncCompletionCtx as *mut std::ffi::c_void,
                0,
            )
        };

        if rc != 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(format!(
                    "spdk_nvme_ns_cmd_write_zeroes failed with rc={rc}"
                )),
            ));
        }

        Self::poll_sync_completion(qp, &completion, "write_zeros")
    }

    /// Validate an async read request and return the namespace pointer, block
    /// count, buffer pointer, and buffer length. Holds the Mutex lock just once
    /// and extracts all needed values before releasing.
    fn validate_async_read(
        controller: &NvmeController,
        ns_id: u32,
        lba: u64,
        buf: &Arc<std::sync::Mutex<interfaces::DmaBuffer>>,
    ) -> Result<(*mut spdk_sys::spdk_nvme_ns, u32, *mut std::ffi::c_void, u64), NvmeBlockError>
    {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        let buf_guard = buf
            .lock()
            .map_err(|_| NvmeBlockError::ClientDisconnected("buffer lock poisoned".into()))?;
        let num_blocks = buf_guard.len() as u64 / ns.sector_size as u64;
        namespace::validate_lba_range(ns, lba, num_blocks)?;

        let buf_ptr = buf_guard.as_ptr();
        let buf_len = buf_guard.len() as u64;

        let ctrlr_ptr = controller.as_ptr();
        // SAFETY: ctrlr_ptr is valid.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };
        Ok((ns_ptr, num_blocks as u32, buf_ptr, buf_len))
    }

    /// Validate an async write request and return the namespace pointer and block count.
    fn validate_async_write(
        controller: &NvmeController,
        ns_id: u32,
        lba: u64,
        buf: &Arc<interfaces::DmaBuffer>,
    ) -> Result<(*mut spdk_sys::spdk_nvme_ns, u32), NvmeBlockError> {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        let num_blocks = buf.len() as u64 / ns.sector_size as u64;
        namespace::validate_lba_range(ns, lba, num_blocks)?;

        let ctrlr_ptr = controller.as_ptr();
        // SAFETY: ctrlr_ptr is valid.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };
        Ok((ns_ptr, num_blocks as u32))
    }

    /// Get a reference to the controller.
    #[allow(dead_code)]
    pub(crate) fn controller(&self) -> &NvmeController {
        &self.controller
    }

    /// Get a mutable reference to the controller.
    #[allow(dead_code)]
    pub(crate) fn controller_mut(&mut self) -> &mut NvmeController {
        &mut self.controller
    }
}

impl ActorHandler<ControlMessage> for BlockDeviceHandler {
    fn handle(&mut self, msg: ControlMessage) {
        match msg {
            ControlMessage::ConnectClient { session } => {
                self.log_debug(&format!("client {} connected", session.id));
                self.clients.push(ClientState {
                    session,
                    pending_ops: HashMap::new(),
                });
            }
            ControlMessage::DisconnectClient { client_id } => {
                self.log_debug(&format!("client {client_id} disconnected"));
                if let Some(pos) = self.clients.iter().position(|c| c.session.id == client_id) {
                    self.clients.swap_remove(pos);
                }
            }
        }

        // After processing the control message, poll all clients.
        self.poll_clients();

        // Check for timed-out operations.
        self.check_timeouts();
    }

    fn on_idle(&mut self) -> bool {
        let did_work = self.poll_clients();
        let now = self.tsc.now();
        if now >= self.tsc.deadline_from_ms(self.last_timeout_check, 1) {
            self.check_timeouts();
            self.last_timeout_check = now;
        }
        did_work
    }

    fn on_start(&mut self) {
        self.log_info("actor started on NUMA-local core");
    }

    fn on_stop(&mut self) {
        self.log_info(&format!(
            "actor shutting down, {} clients connected",
            self.clients.len()
        ));

        // Drain all in-flight SPDK operations so completion callbacks don't
        // fire after the handler (and its async_completions Vec) is dropped.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for qp_idx in 0..self.controller.qpairs.len() {
            if let Some(qp) = self.controller.qpairs.get_mut(qp_idx) {
                while qp.in_flight() > 0 && std::time::Instant::now() < deadline {
                    unsafe {
                        qp.process_completions(0);
                    }
                }
                if qp.in_flight() > 0 {
                    eprintln!(
                        "warning: qpair {} still has {} in-flight ops after drain timeout",
                        qp_idx,
                        qp.in_flight()
                    );
                }
            }
        }
        self.async_completions.clear();

        for client in &self.clients {
            for &h in client.pending_ops.keys() {
                let _ = client.session.callback_tx.send(Completion::Error {
                    handle: Some(OpHandle(h)),
                    error: NvmeBlockError::Aborted("actor shutting down".into()),
                });
            }
        }
        self.clients.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_op_fields() {
        let tsc = TscClock::new();
        let op = PendingOp {
            handle: 42,
            deadline: tsc.deadline_from_ms(tsc.now(), 5000),
            qpair_idx: 0,
            read_buf: None,
            write_buf: None,
        };
        assert_eq!(op.handle, 42);
        assert_eq!(op.qpair_idx, 0);
    }
}
