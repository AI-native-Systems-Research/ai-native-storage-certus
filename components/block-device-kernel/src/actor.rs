//! Actor handler for the kernel block device component.
//!
//! [`KernelHandler`] implements [`ActorHandler<ControlMessage>`] and
//! processes control messages (connect/disconnect clients, shutdown).
//! All IO is routed through io_uring — there is no pread/pwrite fallback.

use std::collections::{HashMap, VecDeque};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Instant;

use component_core::actor::ActorHandler;
use component_core::channel::{Receiver, Sender};

use interfaces::{
    Command, Completion, DmaBuffer, ILogger, NamespaceInfo, NvmeBlockError, OpHandle,
};
use io_uring::{opcode, types, IoUring};

use crate::config::DeviceConfig;

#[cfg(feature = "telemetry")]
use crate::telemetry::TelemetryStats;

/// Per-client channel state held by the actor.
pub struct ClientSession {
    pub id: u64,
    pub ingress_rx: Receiver<Command>,
    pub callback_tx: Sender<Completion>,
    /// Completions that couldn't be delivered because the client's callback
    /// ring was full. Retried by [`Self::flush_pending`] each poll cycle.
    ///
    /// This makes completion delivery non-blocking: the single-threaded actor
    /// must never block sending to one client, or a slow/stalled client would
    /// head-of-line-block completion delivery to every other client on the
    /// drive (a whole-drive deadlock). Bounded in practice by the client's
    /// outstanding operations.
    pub pending: VecDeque<Completion>,
}

impl ClientSession {
    /// Create a session with an empty backlog.
    pub fn new(
        id: u64,
        ingress_rx: Receiver<Command>,
        callback_tx: Sender<Completion>,
    ) -> Self {
        Self {
            id,
            ingress_rx,
            callback_tx,
            pending: VecDeque::new(),
        }
    }

    /// Deliver a completion without ever blocking the actor. Fast path is a
    /// single `try_send`; on a full ring (or an existing backlog) the completion
    /// is buffered in FIFO order and retried by [`Self::flush_pending`].
    fn deliver(&mut self, completion: Completion) {
        if self.pending.is_empty() && self.callback_tx.try_send(completion.clone()).is_ok() {
            return;
        }
        self.pending.push_back(completion);
    }

    /// Retry delivering buffered completions, oldest first, stopping at the
    /// first that still can't be sent (ring full) to preserve ordering.
    /// Returns true if any were delivered.
    fn flush_pending(&mut self) -> bool {
        let mut delivered = false;
        while let Some(front) = self.pending.front() {
            if self.callback_tx.try_send(front.clone()).is_ok() {
                self.pending.pop_front();
                delivered = true;
            } else {
                break;
            }
        }
        delivered
    }
}

/// Control messages sent to the actor's MPSC channel.
pub enum ControlMessage {
    ConnectClient { session: ClientSession },
    DisconnectClient { client_id: u64 },
    Shutdown,
}

// SAFETY: ClientSession contains channel endpoints that are Send.
unsafe impl Send for ControlMessage {}

/// Tracking state for an in-flight async io_uring operation.
struct InflightOp {
    handle: OpHandle,
    client_id: u64,
    deadline: Option<Instant>,
    is_read: bool,
    /// Submission timestamp; elapsed time at completion is the op latency.
    #[cfg(feature = "telemetry")]
    start: Instant,
    #[cfg(feature = "telemetry")]
    bytes: u64,
}

/// The actor handler for kernel block device IO via io_uring.
pub struct KernelHandler {
    fd: OwnedFd,
    config: DeviceConfig,
    ring: IoUring,
    clients: HashMap<u64, ClientSession>,
    inflight: HashMap<u64, InflightOp>,
    next_handle: u64,
    logger: Option<Arc<dyn ILogger + Send + Sync>>,
    shutdown_requested: bool,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<TelemetryStats>,
}

impl KernelHandler {
    #[cfg(not(feature = "telemetry"))]
    pub fn new(
        fd: OwnedFd,
        config: DeviceConfig,
        ring: IoUring,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
    ) -> Self {
        Self {
            fd,
            config,
            ring,
            clients: HashMap::new(),
            inflight: HashMap::new(),
            next_handle: 1,
            logger,
            shutdown_requested: false,
        }
    }

    #[cfg(feature = "telemetry")]
    pub fn with_telemetry(
        fd: OwnedFd,
        config: DeviceConfig,
        ring: IoUring,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
        telemetry: Arc<TelemetryStats>,
    ) -> Self {
        Self {
            fd,
            config,
            ring,
            clients: HashMap::new(),
            inflight: HashMap::new(),
            next_handle: 1,
            logger,
            shutdown_requested: false,
            telemetry,
        }
    }

    fn next_op_handle(&mut self) -> OpHandle {
        let h = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1);
        OpHandle(h)
    }

    fn validate_lba(&self, ns_id: u32, lba: u64, num_blocks: u64) -> Result<(), NvmeBlockError> {
        if ns_id != 1 {
            return Err(NvmeBlockError::InvalidNamespace(format!(
                "ns_id {ns_id} invalid; only ns_id=1 supported"
            )));
        }
        let end = lba.checked_add(num_blocks).ok_or_else(|| {
            NvmeBlockError::LbaOutOfRange(format!("lba {lba} + count {num_blocks} overflows"))
        })?;
        if end > self.config.num_blocks() {
            return Err(NvmeBlockError::LbaOutOfRange(format!(
                "lba range [{lba}..{end}) exceeds device size {} blocks",
                self.config.num_blocks()
            )));
        }
        Ok(())
    }

    fn offset_for_lba(&self, lba: u64) -> u64 {
        lba * self.config.block_size() as u64
    }

    fn process_command(&mut self, client_id: u64, cmd: Command) {
        match cmd {
            Command::ReadSync { ns_id, lba, buf } => {
                self.handle_read_sync(client_id, ns_id, lba, buf);
            }
            Command::WriteSync { ns_id, lba, buf } => {
                self.handle_write_sync(client_id, ns_id, lba, buf);
            }
            Command::ReadAsync {
                ns_id,
                lba,
                buf,
                timeout_ms,
                tag,
            } => {
                self.handle_read_async(client_id, ns_id, lba, buf, timeout_ms, tag);
            }
            Command::WriteAsync {
                ns_id,
                lba,
                buf,
                timeout_ms,
                tag,
            } => {
                self.handle_write_async(client_id, ns_id, lba, buf, timeout_ms, tag);
            }
            Command::WriteZeros {
                ns_id,
                lba,
                num_blocks,
            } => {
                self.handle_write_zeros(client_id, ns_id, lba, num_blocks);
            }
            Command::BatchSubmit { ops } => {
                for op in ops {
                    self.process_command(client_id, op);
                }
            }
            Command::AbortOp { handle } => {
                self.handle_abort(client_id, handle);
            }
            Command::NsProbe => {
                self.handle_ns_probe(client_id);
            }
            Command::FlushSync { ns_id } => {
                // The device is opened O_DIRECT|O_DSYNC, so every write is
                // already forced to non-volatile media on completion — there
                // is no volatile write cache to drain. An explicit flush is
                // therefore a validated no-op returning success.
                let handle = self.next_op_handle();
                let result = if ns_id == 1 {
                    Ok(())
                } else {
                    Err(NvmeBlockError::InvalidNamespace(format!(
                        "ns_id {ns_id} invalid; only ns_id=1 supported"
                    )))
                };
                self.send_completion(client_id, Completion::FlushDone { handle, result });
            }
            Command::NsCreate { .. }
            | Command::NsDelete { .. }
            | Command::NsFormat { .. }
            | Command::ControllerReset => {
                self.send_completion(
                    client_id,
                    Completion::Error {
                        handle: None,
                        error: NvmeBlockError::NotSupported(
                            "operation not supported on kernel block device".into(),
                        ),
                    },
                );
            }
        }
    }

    /// Submit a read via io_uring and block until the CQE arrives.
    fn handle_read_sync(
        &mut self,
        client_id: u64,
        ns_id: u32,
        lba: u64,
        buf: Arc<std::sync::Mutex<DmaBuffer>>,
    ) {
        let handle = self.next_op_handle();

        let buf_len = {
            let guard = buf.lock().expect("DmaBuffer lock poisoned");
            guard.len()
        };
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::ReadDone {
                    handle,
                    tag: 0,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = types::Fd(self.fd.as_raw_fd());
        let ptr = {
            let mut guard = buf.lock().expect("DmaBuffer lock poisoned");
            guard.as_mut_slice().as_mut_ptr()
        };

        let read_sqe = opcode::Read::new(fd, ptr, buf_len as u32)
            .offset(offset)
            .build()
            .user_data(handle.0);

        // SAFETY: SQE is valid and fd is valid for the lifetime of the ring.
        unsafe {
            if self.ring.submission().push(&read_sqe).is_err() {
                self.send_completion(
                    client_id,
                    Completion::ReadDone {
                        handle,
                        tag: 0,
                        result: Err(NvmeBlockError::NotInitialized(
                            "io_uring submission queue full".into(),
                        )),
                    },
                );
                return;
            }
        }

        #[cfg(feature = "telemetry")]
        let start = Instant::now();

        self.ring.submit_and_wait(1).ok();

        let result = self.wait_for_cqe(handle.0);

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry
                .record_op(start.elapsed().as_nanos() as u64, buf_len as u64);
        }

        let result = result.map_err(|msg| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::ReadFailed(msg))
        });

        self.send_completion(client_id, Completion::ReadDone { handle, tag: 0, result });
    }

    /// Submit a write via io_uring and block until completion.
    /// Durability is guaranteed by O_DSYNC on the fd — no separate fsync needed.
    fn handle_write_sync(&mut self, client_id: u64, ns_id: u32, lba: u64, buf: Arc<DmaBuffer>) {
        let handle = self.next_op_handle();
        let buf_len = buf.len();
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::WriteDone {
                    handle,
                    tag: 0,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = types::Fd(self.fd.as_raw_fd());
        let ptr = buf.as_slice().as_ptr();

        let write_sqe = opcode::Write::new(fd, ptr, buf_len as u32)
            .offset(offset)
            .build()
            .user_data(handle.0);

        // SAFETY: SQE is valid and fd is valid.
        unsafe {
            if self.ring.submission().push(&write_sqe).is_err() {
                self.send_completion(
                    client_id,
                    Completion::WriteDone {
                        handle,
                        tag: 0,
                        result: Err(NvmeBlockError::NotInitialized(
                            "io_uring submission queue full".into(),
                        )),
                    },
                );
                return;
            }
        }

        #[cfg(feature = "telemetry")]
        let start = Instant::now();

        self.ring.submit_and_wait(1).ok();

        let result = self.wait_for_cqe(handle.0);

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry
                .record_op(start.elapsed().as_nanos() as u64, buf_len as u64);
        }

        let result = result.map_err(|msg| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::WriteFailed(msg))
        });

        self.send_completion(client_id, Completion::WriteDone { handle, tag: 0, result });
    }

    fn handle_read_async(
        &mut self,
        client_id: u64,
        ns_id: u32,
        lba: u64,
        buf: Arc<std::sync::Mutex<DmaBuffer>>,
        timeout_ms: u64,
        _tag: u64,
    ) {
        let handle = self.next_op_handle();
        let buf_len = {
            let guard = buf.lock().expect("DmaBuffer lock poisoned");
            guard.len()
        };
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::ReadDone {
                    handle,
                    tag: 0,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = types::Fd(self.fd.as_raw_fd());
        let ptr = {
            let mut guard = buf.lock().expect("DmaBuffer lock poisoned");
            guard.as_mut_slice().as_mut_ptr()
        };

        let read_sqe = opcode::Read::new(fd, ptr, buf_len as u32)
            .offset(offset)
            .build()
            .user_data(handle.0);

        // SAFETY: SQE is valid and fd is valid for the lifetime of the ring.
        unsafe {
            if self.ring.submission().push(&read_sqe).is_err() {
                self.send_completion(
                    client_id,
                    Completion::ReadDone {
                        handle,
                        tag: 0,
                        result: Err(NvmeBlockError::NotInitialized(
                            "io_uring submission queue full".into(),
                        )),
                    },
                );
                return;
            }
        }

        let _ = self.ring.submit();

        let deadline = if timeout_ms > 0 {
            Some(Instant::now() + std::time::Duration::from_millis(timeout_ms))
        } else {
            None
        };

        self.inflight.insert(
            handle.0,
            InflightOp {
                handle,
                client_id,
                deadline,
                is_read: true,
                #[cfg(feature = "telemetry")]
                start: Instant::now(),
                #[cfg(feature = "telemetry")]
                bytes: buf_len as u64,
            },
        );
    }

    fn handle_write_async(
        &mut self,
        client_id: u64,
        ns_id: u32,
        lba: u64,
        buf: Arc<DmaBuffer>,
        timeout_ms: u64,
        _tag: u64,
    ) {
        let handle = self.next_op_handle();
        let buf_len = buf.len();
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::WriteDone {
                    handle,
                    tag: 0,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = types::Fd(self.fd.as_raw_fd());
        let ptr = buf.as_slice().as_ptr();

        let write_sqe = opcode::Write::new(fd, ptr, buf_len as u32)
            .offset(offset)
            .build()
            .user_data(handle.0);

        // SAFETY: SQE is valid and fd is valid. O_DSYNC on the fd guarantees durability.
        unsafe {
            if self.ring.submission().push(&write_sqe).is_err() {
                self.send_completion(
                    client_id,
                    Completion::WriteDone {
                        handle,
                        tag: 0,
                        result: Err(NvmeBlockError::NotInitialized(
                            "io_uring submission queue full".into(),
                        )),
                    },
                );
                return;
            }
        }

        let _ = self.ring.submit();

        let deadline = if timeout_ms > 0 {
            Some(Instant::now() + std::time::Duration::from_millis(timeout_ms))
        } else {
            None
        };

        self.inflight.insert(
            handle.0,
            InflightOp {
                handle,
                client_id,
                deadline,
                is_read: false,
                #[cfg(feature = "telemetry")]
                start: Instant::now(),
                #[cfg(feature = "telemetry")]
                bytes: buf_len as u64,
            },
        );
    }

    fn handle_write_zeros(&mut self, client_id: u64, ns_id: u32, lba: u64, num_blocks: u32) {
        let handle = self.next_op_handle();

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks as u64) {
            self.send_completion(
                client_id,
                Completion::WriteZerosDone {
                    handle,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let total_bytes = num_blocks as usize * self.config.block_size() as usize;
        let fd = types::Fd(self.fd.as_raw_fd());

        // SAFETY: posix_memalign returns a 512-byte-aligned pointer suitable for O_DIRECT.
        let zeros_ptr = unsafe {
            let mut ptr: *mut libc::c_void = std::ptr::null_mut();
            let ret = libc::posix_memalign(&mut ptr, 512, total_bytes);
            if ret != 0 {
                self.send_completion(
                    client_id,
                    Completion::WriteZerosDone {
                        handle,
                        result: Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::WriteFailed(
                                "posix_memalign failed".into(),
                            ),
                        )),
                    },
                );
                return;
            }
            std::ptr::write_bytes(ptr as *mut u8, 0, total_bytes);
            ptr
        };

        let write_sqe =
            opcode::Write::new(fd, zeros_ptr as *const u8, total_bytes as u32)
                .offset(offset)
                .build()
                .user_data(handle.0);

        // SAFETY: SQE is valid, fd + zeros_ptr are valid. O_DSYNC guarantees durability.
        unsafe {
            if self.ring.submission().push(&write_sqe).is_err() {
                libc::free(zeros_ptr);
                self.send_completion(
                    client_id,
                    Completion::WriteZerosDone {
                        handle,
                        result: Err(NvmeBlockError::NotInitialized(
                            "io_uring submission queue full".into(),
                        )),
                    },
                );
                return;
            }
        }

        #[cfg(feature = "telemetry")]
        let start = Instant::now();

        self.ring.submit_and_wait(1).ok();

        let result = self.wait_for_cqe(handle.0);

        // SAFETY: zeros_ptr was allocated via posix_memalign.
        unsafe { libc::free(zeros_ptr) };

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry
                .record_op(start.elapsed().as_nanos() as u64, total_bytes as u64);
        }

        let result = result.map_err(|msg| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::WriteFailed(msg))
        });

        self.send_completion(client_id, Completion::WriteZerosDone { handle, result });
    }

    fn handle_abort(&mut self, client_id: u64, target: OpHandle) {
        let cancel_sqe = opcode::AsyncCancel::new(target.0).build().user_data(0);

        // SAFETY: SQE is valid.
        unsafe {
            let _ = self.ring.submission().push(&cancel_sqe);
        }
        let _ = self.ring.submit();

        self.inflight.remove(&target.0);
        self.send_completion(client_id, Completion::AbortAck { handle: target });
    }

    fn handle_ns_probe(&mut self, client_id: u64) {
        let info = NamespaceInfo {
            ns_id: 1,
            num_sectors: self.config.num_blocks(),
            sector_size: self.config.block_size(),
        };
        self.send_completion(
            client_id,
            Completion::NsProbeResult {
                namespaces: vec![info],
            },
        );
    }

    /// Wait for a specific CQE by user_data key, ignoring fsync CQEs.
    /// Returns Ok(()) on success or Err(message) on io_uring error.
    fn wait_for_cqe(&mut self, key: u64) -> Result<(), String> {
        loop {
            let mut target_result: Option<Result<(), String>> = None;
            let mut other_cqes: Vec<(u64, i32)> = Vec::new();

            {
                let cq = self.ring.completion();
                for cqe in cq {
                    let user_data = cqe.user_data();
                    if user_data & (1 << 63) != 0 {
                        continue;
                    }
                    if user_data == key {
                        let res = cqe.result();
                        target_result = Some(if res < 0 {
                            Err(format!("io_uring error: {}", -res))
                        } else {
                            Ok(())
                        });
                    } else {
                        other_cqes.push((user_data, cqe.result()));
                    }
                }
            }

            // Process any other CQEs that arrived
            for (user_data, res) in other_cqes {
                if let Some(op) = self.inflight.remove(&user_data) {
                    let result = if res < 0 {
                        let err_msg = format!("io_uring error: {}", -res);
                        if op.is_read {
                            Err(NvmeBlockError::BlockDevice(
                                interfaces::BlockDeviceError::ReadFailed(err_msg),
                            ))
                        } else {
                            Err(NvmeBlockError::BlockDevice(
                                interfaces::BlockDeviceError::WriteFailed(err_msg),
                            ))
                        }
                    } else {
                        #[cfg(feature = "telemetry")]
                        self.telemetry
                            .record_op(op.start.elapsed().as_nanos() as u64, op.bytes);
                        Ok(())
                    };

                    let completion = if op.is_read {
                        Completion::ReadDone {
                            handle: op.handle,
                            tag: 0,
                            result,
                        }
                    } else {
                        Completion::WriteDone {
                            handle: op.handle,
                            tag: 0,
                            result,
                        }
                    };
                    self.send_completion(op.client_id, completion);
                }
            }

            if let Some(result) = target_result {
                return result;
            }

            self.ring.submit_and_wait(1).ok();
        }
    }

    fn harvest_completions(&mut self) {
        let mut completed: Vec<(u64, i32)> = Vec::new();

        {
            let cq = self.ring.completion();
            for cqe in cq {
                let user_data = cqe.user_data();
                if user_data & (1 << 63) != 0 {
                    continue;
                }
                completed.push((user_data, cqe.result()));
            }
        }

        for (key, res) in completed {
            if let Some(op) = self.inflight.remove(&key) {
                let result = if res < 0 {
                    let err_msg = format!("io_uring error: {}", -res);
                    if op.is_read {
                        Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::ReadFailed(err_msg),
                        ))
                    } else {
                        Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::WriteFailed(err_msg),
                        ))
                    }
                } else {
                    #[cfg(feature = "telemetry")]
                    self.telemetry.record_op(0, op.bytes);
                    Ok(())
                };

                let completion = if op.is_read {
                    Completion::ReadDone {
                        handle: op.handle,
                        tag: 0,
                        result,
                    }
                } else {
                    Completion::WriteDone {
                        handle: op.handle,
                        tag: 0,
                        result,
                    }
                };

                self.send_completion(op.client_id, completion);
            }
        }
    }

    fn check_timeouts(&mut self) {
        if self.inflight.is_empty() {
            return;
        }

        let now = Instant::now();
        let timed_out: Vec<(u64, OpHandle, u64)> = self
            .inflight
            .iter()
            .filter_map(|(&key, op)| {
                if let Some(deadline) = op.deadline {
                    if now >= deadline {
                        return Some((key, op.handle, op.client_id));
                    }
                }
                None
            })
            .collect();

        let had_timeouts = !timed_out.is_empty();

        for (key, handle, client_id) in &timed_out {
            self.inflight.remove(key);
            self.send_completion(*client_id, Completion::Timeout { handle: *handle });
        }

        if had_timeouts {
            for (key, _, _) in timed_out {
                let cancel_sqe = opcode::AsyncCancel::new(key).build().user_data(0);
                // SAFETY: SQE is valid.
                unsafe {
                    let _ = self.ring.submission().push(&cancel_sqe);
                }
            }
            let _ = self.ring.submit();
        }
    }

    /// Deliver a completion to a client without blocking the actor thread.
    ///
    /// A blocking send here would head-of-line-block the single-threaded actor
    /// on one slow/stalled client, freezing completion delivery for every other
    /// client on the drive (a whole-drive deadlock). Delivery is therefore
    /// non-blocking: [`ClientSession::deliver`] buffers on a full ring and
    /// [`Self::poll_clients`] drains the backlog as ring space frees.
    fn send_completion(&mut self, client_id: u64, completion: Completion) {
        if let Some(session) = self.clients.get_mut(&client_id) {
            session.deliver(completion);
        }
    }

    fn poll_clients(&mut self) {
        // First, retry any completions buffered when a client's callback ring
        // was full. Non-blocking delivery (see ClientSession::deliver) means a
        // slow client can't head-of-line-block the actor for the others; its
        // completions drain here as space frees.
        for session in self.clients.values_mut() {
            session.flush_pending();
        }

        let client_ids: Vec<u64> = self.clients.keys().copied().collect();
        for client_id in client_ids {
            loop {
                let cmd = if let Some(session) = self.clients.get(&client_id) {
                    session.ingress_rx.try_recv().ok()
                } else {
                    None
                };
                match cmd {
                    Some(c) => self.process_command(client_id, c),
                    None => break,
                }
            }
        }
    }
}

impl ActorHandler<ControlMessage> for KernelHandler {
    fn handle(&mut self, msg: ControlMessage) {
        match msg {
            ControlMessage::ConnectClient { session } => {
                if let Some(ref log) = self.logger {
                    log.debug(&format!("actor: connecting client {}", session.id));
                }
                self.clients.insert(session.id, session);
            }
            ControlMessage::DisconnectClient { client_id } => {
                if let Some(ref log) = self.logger {
                    log.debug(&format!("actor: disconnecting client {client_id}"));
                }
                self.clients.remove(&client_id);
            }
            ControlMessage::Shutdown => {
                self.shutdown_requested = true;
            }
        }
    }

    fn on_idle(&mut self) -> bool {
        if self.shutdown_requested {
            return false;
        }

        self.poll_clients();
        self.harvest_completions();
        self.check_timeouts();

        !self.clients.is_empty() || !self.inflight.is_empty()
    }
}

// SAFETY: KernelHandler is only used on the actor thread.
// OwnedFd, IoUring, and the client channels are all Send.
unsafe impl Send for KernelHandler {}
