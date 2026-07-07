//! Persistent worker pool for cold-path SSD→GPU pipeline execution.
//!
//! Pre-allocates NVMe `ClientChannels` and CUDA streams per worker at init,
//! eliminating per-batch connection setup overhead in the hot path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use interfaces::{
    ClientChannels, DispatcherError, GpuStream, IBlockDevice, IGpuServices,
};

use crate::metrics::PipelineMetrics;
use crate::pipeline::{self, ColdReadJob};

/// Request submitted to a cold-pool worker.
pub struct ColdReadRequest {
    pub jobs: Vec<ColdReadJob>,
    pub chunk_size: usize,
    pub queue_depth: usize,
    pub metrics: Option<Arc<dyn PipelineMetrics>>,
    pub result_tx: Sender<Vec<Result<(), DispatcherError>>>,
}

// SAFETY: ColdReadJob contains raw pointers that are valid for the duration
// of the pipeline call; the caller ensures they remain valid until result_tx
// is received.
unsafe impl Send for ColdReadRequest {}

struct WorkerHandle {
    sender: Sender<ColdReadRequest>,
    _handle: JoinHandle<()>,
}

/// Persistent pool of cold-path pipeline workers.
///
/// Each worker owns pre-connected `ClientChannels` and CUDA streams for one
/// drive slot. Work is dispatched round-robin across workers for the target drive.
pub struct ColdReadPool {
    workers: Vec<Vec<WorkerHandle>>,
    shutdown: Arc<AtomicBool>,
    num_drives: usize,
}

impl ColdReadPool {
    /// Create the pool with `queues_per_drive` workers for each drive.
    pub fn new(
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
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

                let stream_a = gpu.create_stream().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "cold_pool create_stream drive={drive_idx} slot={queue_slot}: {e}"
                    ))
                })?;
                let stream_b = gpu.create_stream().map_err(|e| {
                    let _ = gpu.destroy_stream(stream_a);
                    DispatcherError::IoError(format!(
                        "cold_pool create_stream drive={drive_idx} slot={queue_slot}: {e}"
                    ))
                })?;
                let streams = [stream_a, stream_b];

                let (tx, rx): (Sender<ColdReadRequest>, Receiver<ColdReadRequest>) =
                    crossbeam_channel::bounded(1);

                let shutdown_flag = Arc::clone(&shutdown);
                let drive_ref = Arc::clone(drive);
                let gpu_ref = Arc::clone(gpu);

                let handle = thread::Builder::new()
                    .name(format!("cold-pool-d{drive_idx}-q{queue_slot}"))
                    .spawn(move || {
                        Self::worker_loop(
                            &shutdown_flag,
                            &rx,
                            &*drive_ref,
                            &*gpu_ref,
                            &streams,
                            channels,
                        );
                        let _ = gpu_ref.destroy_stream(streams[0]);
                        let _ = gpu_ref.destroy_stream(streams[1]);
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

    /// Submit a cold-read pipeline job to a worker for the given drive.
    ///
    /// `slot` selects which worker (0..queues_per_drive) handles this chunk.
    /// Blocks if the selected worker is busy (bounded(1) channel).
    pub fn submit(
        &self,
        drive_idx: usize,
        slot: usize,
        request: ColdReadRequest,
    ) -> Result<(), DispatcherError> {
        let drive_workers = &self.workers[drive_idx % self.num_drives];
        let worker = &drive_workers[slot % drive_workers.len()];
        worker.sender.send(request).map_err(|_| {
            DispatcherError::IoError("cold_pool worker disconnected".into())
        })
    }

    /// Number of drives in the pool.
    pub fn num_drives(&self) -> usize {
        self.num_drives
    }

    /// Number of workers per drive.
    pub fn queues_per_drive(&self) -> usize {
        self.workers.first().map_or(0, |w| w.len())
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        // Drop all senders to unblock workers waiting on recv.
        // Workers will see channel disconnect and exit.
    }

    fn worker_loop(
        shutdown: &AtomicBool,
        rx: &Receiver<ColdReadRequest>,
        drive: &dyn IBlockDevice,
        gpu: &dyn IGpuServices,
        streams: &[GpuStream; 2],
        channels: ClientChannels,
    ) {
        loop {
            let request = match rx.recv() {
                Ok(req) => req,
                Err(_) => break, // channel closed
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

            let results = unsafe {
                pipeline::pipelined_multi_object_zero_copy(
                    drive,
                    gpu,
                    streams,
                    &channels,
                    &request.jobs,
                    request.chunk_size,
                    request.queue_depth,
                    request.metrics.as_deref(),
                )
            };

            let _ = request.result_tx.send(results);
        }
    }
}

impl Drop for ColdReadPool {
    fn drop(&mut self) {
        self.shutdown();
        // Workers exit when they see the channel closed (senders dropped
        // when WorkerHandle is dropped) or the shutdown flag. JoinHandles
        // are joined on drop via _handle field.
    }
}
