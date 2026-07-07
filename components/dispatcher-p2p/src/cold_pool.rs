//! Persistent worker pool for P2P cold-path pipeline execution.
//!
//! Pre-allocates NVMe `ClientChannels` per worker at init, eliminating
//! per-batch connection setup overhead. Each worker executes the P2P
//! pipeline (SSD → BAR1 ring → D2D → client GPU).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use interfaces::{ClientChannels, DispatcherError, IBlockDevice};

use crate::p2p_ring::{P2pRing, ThreadPartition};
use crate::pipeline::{self, P2pColdJob};

/// Request submitted to a cold-pool worker.
pub struct P2pColdReadRequest {
    pub jobs: Vec<P2pColdJob>,
    pub partition: ThreadPartition,
    /// Pointer to the P2P ring. Caller guarantees it lives until result_tx is received.
    pub ring_ptr: *const P2pRing,
    pub result_tx: Sender<Vec<Result<(), DispatcherError>>>,
}

// SAFETY: P2pColdJob contains raw pointers valid for the pipeline call duration;
// ring_ptr is valid for the duration of the request (caller holds RwLock read guard).
// The caller ensures all pointers remain valid until result_tx is received.
unsafe impl Send for P2pColdReadRequest {}

struct WorkerHandle {
    sender: Sender<P2pColdReadRequest>,
    _handle: JoinHandle<()>,
}

/// Persistent pool of P2P cold-path pipeline workers.
///
/// Each worker owns a pre-connected `ClientChannels` for one drive.
/// Work is dispatched to the worker for the target drive.
pub struct P2pColdReadPool {
    workers: Vec<Vec<WorkerHandle>>,
    shutdown: Arc<AtomicBool>,
    num_drives: usize,
}

impl P2pColdReadPool {
    /// Create the pool with `queues_per_drive` workers for each drive.
    pub fn new(
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        queues_per_drive: usize,
    ) -> Result<Self, DispatcherError> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let num_drives = drives.len();
        let mut workers: Vec<Vec<WorkerHandle>> = Vec::with_capacity(num_drives);

        for (drive_idx, drive) in drives.iter().enumerate() {
            let mut drive_workers = Vec::with_capacity(queues_per_drive);

            for queue_slot in 0..queues_per_drive {
                let channels = drive.connect_client().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "cold_pool connect_client drive={drive_idx} slot={queue_slot}: {e}"
                    ))
                })?;

                let (tx, rx): (Sender<P2pColdReadRequest>, Receiver<P2pColdReadRequest>) =
                    crossbeam_channel::bounded(1);

                let shutdown_flag = Arc::clone(&shutdown);
                let drive_ref = Arc::clone(drive);

                let handle = thread::Builder::new()
                    .name(format!("p2p-cold-d{drive_idx}-q{queue_slot}"))
                    .spawn(move || {
                        Self::worker_loop(&shutdown_flag, &rx, &*drive_ref, channels);
                    })
                    .map_err(|e| {
                        DispatcherError::IoError(format!("cold_pool spawn failed: {e}"))
                    })?;

                drive_workers.push(WorkerHandle {
                    sender: tx,
                    _handle: handle,
                });
            }

            workers.push(drive_workers);
        }

        Ok(Self {
            workers,
            shutdown,
            num_drives,
        })
    }

    /// Submit a P2P cold-read pipeline job to a worker for the given drive.
    pub fn submit(
        &self,
        drive_idx: usize,
        slot: usize,
        request: P2pColdReadRequest,
    ) -> Result<(), DispatcherError> {
        let drive_workers = &self.workers[drive_idx % self.num_drives];
        let worker = &drive_workers[slot % drive_workers.len()];
        worker.sender.send(request).map_err(|_| {
            DispatcherError::IoError("cold_pool worker disconnected".into())
        })
    }

    pub fn num_drives(&self) -> usize {
        self.num_drives
    }

    pub fn queues_per_drive(&self) -> usize {
        self.workers.first().map_or(0, |w| w.len())
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    fn worker_loop(
        shutdown: &AtomicBool,
        rx: &Receiver<P2pColdReadRequest>,
        drive: &dyn IBlockDevice,
        channels: ClientChannels,
    ) {
        loop {
            let request = match rx.recv() {
                Ok(req) => req,
                Err(_) => break,
            };

            if shutdown.load(Ordering::Acquire) {
                let _ = request.result_tx.send(
                    request
                        .jobs
                        .iter()
                        .map(|_| Err(DispatcherError::IoError("pool shutting down".into())))
                        .collect(),
                );
                break;
            }

            // SAFETY: ring_ptr is valid for the duration of this request — caller holds
            // the RwLock read guard until result_tx is received.
            let ring = unsafe { &*request.ring_ptr };

            let results = unsafe {
                pipeline::pipelined_multi_object_p2p(
                    drive,
                    ring,
                    &request.partition,
                    &channels,
                    &request.jobs,
                )
            };

            let _ = request.result_tx.send(results);
        }
    }
}

impl Drop for P2pColdReadPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}
