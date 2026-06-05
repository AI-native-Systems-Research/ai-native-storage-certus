//! Actor handler for the file-backed block device component.
//!
//! [`FilesysHandler`] implements [`ActorHandler<ControlMessage>`] and
//! processes control messages (connect/disconnect clients, shutdown).
//! On each `on_idle()` call it polls all connected client ingress channels
//! for IO commands and harvests io_uring completions.

use std::collections::HashMap;
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
    #[cfg(feature = "telemetry")]
    start_ns: u64,
    #[cfg(feature = "telemetry")]
    bytes: u64,
}

/// The actor handler for file-backed IO.
pub struct FilesysHandler {
    fd: OwnedFd,
    config: DeviceConfig,
    ring: Option<IoUring>,
    clients: HashMap<u64, ClientSession>,
    inflight: HashMap<u64, InflightOp>,
    next_handle: u64,
    logger: Option<Arc<dyn ILogger + Send + Sync>>,
    shutdown_requested: bool,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<TelemetryStats>,
}

impl FilesysHandler {
    fn try_create_ring(logger: &Option<Arc<dyn ILogger + Send + Sync>>) -> Option<IoUring> {
        match IoUring::new(crate::DEFAULT_RING_DEPTH) {
            Ok(ring) => Some(ring),
            Err(e) => {
                if let Some(ref log) = logger {
                    log.warn(&format!(
                        "io_uring unavailable ({e}), falling back to sync IO for async commands"
                    ));
                }
                None
            }
        }
    }

    #[cfg(not(feature = "telemetry"))]
    pub fn new(
        fd: OwnedFd,
        config: DeviceConfig,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
    ) -> Self {
        let ring = Self::try_create_ring(&logger);
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
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
        telemetry: Arc<TelemetryStats>,
    ) -> Self {
        let ring = Self::try_create_ring(&logger);
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
            } => {
                self.handle_read_async(client_id, ns_id, lba, buf, timeout_ms);
            }
            Command::WriteAsync {
                ns_id,
                lba,
                buf,
                timeout_ms,
            } => {
                self.handle_write_async(client_id, ns_id, lba, buf, timeout_ms);
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
            Command::NsCreate { .. }
            | Command::NsDelete { .. }
            | Command::NsFormat { .. }
            | Command::ControllerReset => {
                self.send_completion(
                    client_id,
                    Completion::Error {
                        handle: None,
                        error: NvmeBlockError::NotSupported(
                            "operation not supported on file-backed device".into(),
                        ),
                    },
                );
            }
        }
    }

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
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = self.fd.as_raw_fd();

        let result = {
            let mut guard = buf.lock().expect("DmaBuffer lock poisoned");
            let slice = guard.as_mut_slice();
            // SAFETY: fd is valid, slice is valid for buf_len bytes.
            let ret = unsafe {
                libc::pread(
                    fd,
                    slice.as_mut_ptr() as *mut libc::c_void,
                    buf_len,
                    offset as i64,
                )
            };
            if ret < 0 {
                Err(NvmeBlockError::BlockDevice(
                    interfaces::BlockDeviceError::ReadFailed(
                        std::io::Error::last_os_error().to_string(),
                    ),
                ))
            } else {
                Ok(())
            }
        };

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry.record_op(0, buf_len as u64);
        }

        self.send_completion(client_id, Completion::ReadDone { handle, result });
    }

    fn handle_write_sync(&mut self, client_id: u64, ns_id: u32, lba: u64, buf: Arc<DmaBuffer>) {
        let handle = self.next_op_handle();
        let buf_len = buf.len();
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::WriteDone {
                    handle,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);
        let fd = self.fd.as_raw_fd();
        let slice = buf.as_slice();

        // SAFETY: fd is valid, slice is valid for buf_len bytes.
        let ret = unsafe {
            libc::pwrite(
                fd,
                slice.as_ptr() as *const libc::c_void,
                buf_len,
                offset as i64,
            )
        };

        let result = if ret < 0 {
            Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(
                    std::io::Error::last_os_error().to_string(),
                ),
            ))
        } else {
            // fdatasync for durability
            // SAFETY: fd is valid.
            let sync_ret = unsafe { libc::fdatasync(fd) };
            if sync_ret < 0 {
                Err(NvmeBlockError::BlockDevice(
                    interfaces::BlockDeviceError::WriteFailed(format!(
                        "fdatasync failed: {}",
                        std::io::Error::last_os_error()
                    )),
                ))
            } else {
                Ok(())
            }
        };

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry.record_op(0, buf_len as u64);
        }

        self.send_completion(client_id, Completion::WriteDone { handle, result });
    }

    fn handle_read_async(
        &mut self,
        client_id: u64,
        ns_id: u32,
        lba: u64,
        buf: Arc<std::sync::Mutex<DmaBuffer>>,
        _timeout_ms: u64,
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
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);

        if let Some(ref mut ring) = self.ring {
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
                if ring.submission().push(&read_sqe).is_err() {
                    self.send_completion(
                        client_id,
                        Completion::ReadDone {
                            handle,
                            result: Err(NvmeBlockError::NotInitialized(
                                "io_uring submission queue full".into(),
                            )),
                        },
                    );
                    return;
                }
            }

            let _ = ring.submit();

            let deadline = if _timeout_ms > 0 {
                Some(Instant::now() + std::time::Duration::from_millis(_timeout_ms))
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
                    start_ns: Instant::now().elapsed().as_nanos() as u64,
                    #[cfg(feature = "telemetry")]
                    bytes: buf_len as u64,
                },
            );
        } else {
            // Fallback: sync pread
            let fd = self.fd.as_raw_fd();
            let result = {
                let mut guard = buf.lock().expect("DmaBuffer lock poisoned");
                let slice = guard.as_mut_slice();
                // SAFETY: fd is valid, slice is valid for buf_len bytes.
                let ret = unsafe {
                    libc::pread(
                        fd,
                        slice.as_mut_ptr() as *mut libc::c_void,
                        buf_len,
                        offset as i64,
                    )
                };
                if ret < 0 {
                    Err(NvmeBlockError::BlockDevice(
                        interfaces::BlockDeviceError::ReadFailed(
                            std::io::Error::last_os_error().to_string(),
                        ),
                    ))
                } else {
                    Ok(())
                }
            };

            #[cfg(feature = "telemetry")]
            if result.is_ok() {
                self.telemetry.record_op(0, buf_len as u64);
            }

            self.send_completion(client_id, Completion::ReadDone { handle, result });
        }
    }

    fn handle_write_async(
        &mut self,
        client_id: u64,
        ns_id: u32,
        lba: u64,
        buf: Arc<DmaBuffer>,
        _timeout_ms: u64,
    ) {
        let handle = self.next_op_handle();
        let buf_len = buf.len();
        let num_blocks_needed = (buf_len as u64) / self.config.block_size() as u64;

        if let Err(e) = self.validate_lba(ns_id, lba, num_blocks_needed) {
            self.send_completion(
                client_id,
                Completion::WriteDone {
                    handle,
                    result: Err(e),
                },
            );
            return;
        }

        let offset = self.offset_for_lba(lba);

        if let Some(ref mut ring) = self.ring {
            let fd = types::Fd(self.fd.as_raw_fd());
            let ptr = buf.as_slice().as_ptr();

            let write_sqe = opcode::Write::new(fd, ptr, buf_len as u32)
                .offset(offset)
                .build()
                .user_data(handle.0)
                .flags(io_uring::squeue::Flags::IO_LINK);

            let fsync_sqe = opcode::Fsync::new(fd)
                .flags(io_uring::types::FsyncFlags::DATASYNC)
                .build()
                .user_data(handle.0 | (1 << 63));

            // SAFETY: SQEs are valid and fd is valid.
            unsafe {
                let mut sq = ring.submission();
                if sq.push(&write_sqe).is_err() {
                    drop(sq);
                    self.send_completion(
                        client_id,
                        Completion::WriteDone {
                            handle,
                            result: Err(NvmeBlockError::NotInitialized(
                                "io_uring submission queue full".into(),
                            )),
                        },
                    );
                    return;
                }
                if sq.push(&fsync_sqe).is_err() {
                    if let Some(ref log) = self.logger {
                        log.warn("failed to push fsync SQE, write may not be durable");
                    }
                }
            }

            let _ = ring.submit();

            let deadline = if _timeout_ms > 0 {
                Some(Instant::now() + std::time::Duration::from_millis(_timeout_ms))
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
                    start_ns: Instant::now().elapsed().as_nanos() as u64,
                    #[cfg(feature = "telemetry")]
                    bytes: buf_len as u64,
                },
            );
        } else {
            // Fallback: sync pwrite + fdatasync
            let fd = self.fd.as_raw_fd();
            let slice = buf.as_slice();

            // SAFETY: fd is valid, slice is valid for buf_len bytes.
            let ret = unsafe {
                libc::pwrite(
                    fd,
                    slice.as_ptr() as *const libc::c_void,
                    buf_len,
                    offset as i64,
                )
            };

            let result = if ret < 0 {
                Err(NvmeBlockError::BlockDevice(
                    interfaces::BlockDeviceError::WriteFailed(
                        std::io::Error::last_os_error().to_string(),
                    ),
                ))
            } else {
                // SAFETY: fd is valid.
                let sync_ret = unsafe { libc::fdatasync(fd) };
                if sync_ret < 0 {
                    Err(NvmeBlockError::BlockDevice(
                        interfaces::BlockDeviceError::WriteFailed(format!(
                            "fdatasync failed: {}",
                            std::io::Error::last_os_error()
                        )),
                    ))
                } else {
                    Ok(())
                }
            };

            #[cfg(feature = "telemetry")]
            if result.is_ok() {
                self.telemetry.record_op(0, buf_len as u64);
            }

            self.send_completion(client_id, Completion::WriteDone { handle, result });
        }
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
        let fd = self.fd.as_raw_fd();

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

        // SAFETY: fd is valid, zeros_ptr is aligned and valid for total_bytes.
        let ret = unsafe { libc::pwrite(fd, zeros_ptr, total_bytes, offset as i64) };

        // SAFETY: zeros_ptr was allocated via posix_memalign.
        unsafe { libc::free(zeros_ptr) };

        let result = if ret < 0 {
            Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(
                    std::io::Error::last_os_error().to_string(),
                ),
            ))
        } else {
            // SAFETY: fd is valid.
            let sync_ret = unsafe { libc::fdatasync(fd) };
            if sync_ret < 0 {
                Err(NvmeBlockError::BlockDevice(
                    interfaces::BlockDeviceError::WriteFailed(format!(
                        "fdatasync failed: {}",
                        std::io::Error::last_os_error()
                    )),
                ))
            } else {
                Ok(())
            }
        };

        #[cfg(feature = "telemetry")]
        if result.is_ok() {
            self.telemetry.record_op(0, total_bytes as u64);
        }

        self.send_completion(client_id, Completion::WriteZerosDone { handle, result });
    }

    fn handle_abort(&mut self, client_id: u64, target: OpHandle) {
        if let Some(ref mut ring) = self.ring {
            let cancel_sqe = opcode::AsyncCancel::new(target.0).build().user_data(0);

            // SAFETY: SQE is valid.
            unsafe {
                let _ = ring.submission().push(&cancel_sqe);
            }
            let _ = ring.submit();
        }

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

    fn harvest_completions(&mut self) {
        let ring = match self.ring.as_mut() {
            Some(r) => r,
            None => return,
        };

        let mut completed: Vec<(u64, i32)> = Vec::new();

        {
            let cq = ring.completion();
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
                        result,
                    }
                } else {
                    Completion::WriteDone {
                        handle: op.handle,
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
            if let Some(ref mut ring) = self.ring {
                for (key, _, _) in timed_out {
                    let cancel_sqe = opcode::AsyncCancel::new(key).build().user_data(0);
                    // SAFETY: SQE is valid.
                    unsafe {
                        let _ = ring.submission().push(&cancel_sqe);
                    }
                }
                let _ = ring.submit();
            }
        }
    }

    fn send_completion(&self, client_id: u64, completion: Completion) {
        if let Some(session) = self.clients.get(&client_id) {
            if session.callback_tx.send(completion).is_err() {
                if let Some(ref log) = self.logger {
                    log.warn(&format!("client {client_id} callback channel disconnected"));
                }
            }
        }
    }

    fn poll_clients(&mut self) {
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

impl ActorHandler<ControlMessage> for FilesysHandler {
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

// SAFETY: FilesysHandler is only used on the actor thread.
// OwnedFd, IoUring, and the client channels are all Send.
unsafe impl Send for FilesysHandler {}
