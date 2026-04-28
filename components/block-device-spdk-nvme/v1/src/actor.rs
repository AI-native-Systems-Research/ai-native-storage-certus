//! Actor handler for the SPDK NVMe block device component.
//!
//! [`BlockDeviceHandler`] implements [`ActorHandler<ControlMessage>`] and
//! processes control messages (connect/disconnect clients) on the actor's
//! main MPSC channel. On each `handle()` call it also polls all connected
//! client ingress channels for IO commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
/// Boxed and leaked into a raw pointer at submission time; reconstructed
/// in the callback. The `completions` pointer is valid because callbacks
/// execute on the actor thread during `process_completions()`.
struct AsyncIoContext {
    client_id: u64,
    handle: u64,
    is_read: bool,
    completions: *mut Vec<AsyncCompletionEntry>,
    #[cfg(feature = "telemetry")]
    start: Instant,
    #[cfg(feature = "telemetry")]
    bytes: u64,
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
    // SAFETY: ctx was created via Box::into_raw(Box::new(AsyncIoContext { .. })).
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

    // SAFETY: completions pointer is valid — callback runs on the actor
    // thread during process_completions(), same thread that owns the Vec.
    let completions = unsafe { &mut *io_ctx.completions };
    completions.push(AsyncCompletionEntry {
        client_id: io_ctx.client_id,
        handle: io_ctx.handle,
        result,
        is_read: io_ctx.is_read,
        #[cfg(feature = "telemetry")]
        latency_ns: io_ctx.start.elapsed().as_nanos() as u64,
        #[cfg(feature = "telemetry")]
        bytes: io_ctx.bytes,
    });
}

/// Tracks an in-flight asynchronous IO operation.
#[allow(dead_code)]
pub(crate) struct PendingOp {
    /// Component-assigned operation handle.
    pub handle: u64,
    /// Timeout deadline.
    pub deadline: Instant,
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
    /// Telemetry stats collector (feature-gated).
    #[cfg(feature = "telemetry")]
    pub telemetry: Arc<TelemetryStats>,
    /// Last time `check_timeouts()` was called, used to throttle to ~1ms.
    last_timeout_check: Instant,
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
    /// Create a new handler with the given controller.
    pub(crate) fn new(
        controller: NvmeController,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
    ) -> Self {
        Self {
            controller,
            clients: Vec::new(),
            next_handle: 1,
            async_completions: Vec::new(),
            #[cfg(feature = "telemetry")]
            telemetry: Arc::new(TelemetryStats::new()),
            last_timeout_check: Instant::now(),
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
        Self {
            controller,
            clients: Vec::new(),
            next_handle: 1,
            async_completions: Vec::new(),
            telemetry,
            last_timeout_check: Instant::now(),
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

        // Drain async completion entries and route to clients.
        let entries: Vec<AsyncCompletionEntry> = self.async_completions.drain(..).collect();
        for entry in entries {
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
        let now = Instant::now();
        for client in &mut self.clients {
            let mut timed_out = Vec::new();
            for (&handle, op) in &client.pending_ops {
                if now >= op.deadline {
                    timed_out.push(handle);
                }
            }
            for handle in timed_out {
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
    fn dispatch_command(
        controller: &mut NvmeController,
        session: &mut ClientSession,
        pending_ops: &mut HashMap<u64, PendingOp>,
        next_handle: &mut u64,
        #[cfg(feature = "telemetry")] telemetry: &TelemetryStats,
        async_completions: &mut Vec<AsyncCompletionEntry>,
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
                let start = Instant::now();

                let result = Self::do_sync_read(controller, ns_id, lba, &buf);

                #[cfg(feature = "telemetry")]
                if result.is_ok() {
                    telemetry.record(start.elapsed().as_nanos() as u64, bytes);
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
                let start = Instant::now();

                let result = Self::do_sync_write(controller, ns_id, lba, &buf);

                #[cfg(feature = "telemetry")]
                if result.is_ok() {
                    telemetry.record(start.elapsed().as_nanos() as u64, bytes);
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

                // Validate before async submission.
                let validation = Self::validate_async_read(controller, ns_id, lba, &buf);
                if let Err(e) = validation {
                    let _ = session.callback_tx.send(Completion::ReadDone {
                        handle: OpHandle(handle),
                        result: Err(e),
                    });
                    return;
                }
                let (ns_ptr, num_blocks) = validation.unwrap();

                // Select queue pair sized for this client's concurrency level.
                let qp_idx = controller.qpairs.select_index(pending_ops.len() + 1);
                pending_ops.insert(
                    handle,
                    PendingOp {
                        handle,
                        deadline: Instant::now() + std::time::Duration::from_millis(timeout_ms),
                        qpair_idx: qp_idx,
                        read_buf: Some(buf.clone()),
                        write_buf: None,
                    },
                );

                // Submit async SPDK read.
                let buf_guard = buf.lock().expect("buffer lock poisoned");
                let ctx = Box::new(AsyncIoContext {
                    client_id: session.id,
                    handle,
                    is_read: true,
                    completions: async_completions as *mut Vec<AsyncCompletionEntry>,
                    #[cfg(feature = "telemetry")]
                    start: Instant::now(),
                    #[cfg(feature = "telemetry")]
                    bytes: buf_guard.len() as u64,
                });

                let qp = controller
                    .qpairs
                    .get_mut(qp_idx)
                    .expect("qpair index valid");
                let rc = unsafe {
                    spdk_sys::spdk_nvme_ns_cmd_read(
                        ns_ptr,
                        qp.as_ptr(),
                        buf_guard.as_ptr(),
                        lba,
                        num_blocks,
                        Some(async_completion_cb),
                        Box::into_raw(ctx) as *mut std::ffi::c_void,
                        0,
                    )
                };

                if rc != 0 {
                    // Submission failed — remove pending op and report immediately.
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

                // Select queue pair sized for this client's concurrency level.
                let qp_idx = controller.qpairs.select_index(pending_ops.len() + 1);
                pending_ops.insert(
                    handle,
                    PendingOp {
                        handle,
                        deadline: Instant::now() + std::time::Duration::from_millis(timeout_ms),
                        qpair_idx: qp_idx,
                        read_buf: None,
                        write_buf: Some(buf.clone()),
                    },
                );

                // Submit async SPDK write.
                let ctx = Box::new(AsyncIoContext {
                    client_id: session.id,
                    handle,
                    is_read: false,
                    completions: async_completions as *mut Vec<AsyncCompletionEntry>,
                    #[cfg(feature = "telemetry")]
                    start: Instant::now(),
                    #[cfg(feature = "telemetry")]
                    bytes: buf.len() as u64,
                });

                let qp = controller
                    .qpairs
                    .get_mut(qp_idx)
                    .expect("qpair index valid");
                let rc = unsafe {
                    spdk_sys::spdk_nvme_ns_cmd_write(
                        ns_ptr,
                        qp.as_ptr(),
                        buf.as_ptr() as *mut _,
                        lba,
                        num_blocks,
                        Some(async_completion_cb),
                        Box::into_raw(ctx) as *mut std::ffi::c_void,
                        0,
                    )
                };

                if rc != 0 {
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
                let _qp_idx = controller.qpairs.select_index(batch_size);

                for op in ops {
                    Self::dispatch_command(
                        controller,
                        session,
                        pending_ops,
                        next_handle,
                        #[cfg(feature = "telemetry")]
                        telemetry,
                        async_completions,
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
            Command::NsCreate { size_sectors } => {
                // SAFETY: controller pointer is valid while actor is running.
                let result = unsafe {
                    namespace::create(controller.as_ptr(), size_sectors, &controller.namespaces)
                };
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
            Command::NsFormat { ns_id, lbaf } => {
                // SAFETY: controller pointer is valid while actor is running.
                let result = unsafe { namespace::format(controller.as_ptr(), ns_id, lbaf) };
                match result {
                    Ok(()) => {
                        // SAFETY: reset ensures the host re-reads identify data
                        // so the new LBA format is visible.
                        unsafe { spdk_sys::spdk_nvme_ctrlr_reset(controller.as_ptr()) };
                        controller.refresh_namespaces();
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

    /// Validate an async read request and return the namespace pointer and block count.
    fn validate_async_read(
        controller: &NvmeController,
        ns_id: u32,
        lba: u64,
        buf: &Arc<std::sync::Mutex<interfaces::DmaBuffer>>,
    ) -> Result<(*mut spdk_sys::spdk_nvme_ns, u32), NvmeBlockError> {
        let ns = namespace::validate_ns_id(&controller.namespaces, ns_id)?;
        let buf_guard = buf
            .lock()
            .map_err(|_| NvmeBlockError::ClientDisconnected("buffer lock poisoned".into()))?;
        let num_blocks = buf_guard.len() as u64 / ns.sector_size as u64;
        namespace::validate_lba_range(ns, lba, num_blocks)?;

        let ctrlr_ptr = controller.as_ptr();
        // SAFETY: ctrlr_ptr is valid.
        let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };
        Ok((ns_ptr, num_blocks as u32))
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
        // Throttle timeout checks to ~1ms — check_timeouts() allocates a Vec
        // and calls Instant::now() for every pending op, which is too expensive
        // to run on every poll iteration (millions/sec).
        let now = Instant::now();
        if now.duration_since(self.last_timeout_check).as_millis() >= 1 {
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
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        for qp_idx in 0..self.controller.qpairs.len() {
            if let Some(qp) = self.controller.qpairs.get_mut(qp_idx) {
                while qp.in_flight() > 0 && Instant::now() < deadline {
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
        let op = PendingOp {
            handle: 42,
            deadline: Instant::now() + std::time::Duration::from_secs(5),
            qpair_idx: 0,
            read_buf: None,
            write_buf: None,
        };
        assert_eq!(op.handle, 42);
        assert_eq!(op.qpair_idx, 0);
    }
}
