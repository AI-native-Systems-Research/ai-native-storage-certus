/// Per-thread IO worker for the IOPS benchmark.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rand::Rng;

use interfaces::{ClientChannels, Command, Completion, DmaBuffer, NamespaceInfo, NvmeBlockError};

use crate::config::{BenchConfig, IoMode, OpType, Pattern};
use crate::lba::{LbaGenerator, RandomLba, SequentialLba};
use crate::stats::ThreadResult;

/// A per-thread IO worker that submits async operations and drains completions.
pub struct Worker {
    config: Arc<BenchConfig>,
    channels: ClientChannels,
    ns_info: NamespaceInfo,
    sector_size: usize,
    /// read_bufs[block_size_index][slot]
    read_bufs: Vec<Vec<Arc<Mutex<DmaBuffer>>>>,
    /// write_bufs[block_size_index][slot]
    write_bufs: Vec<Vec<Arc<DmaBuffer>>>,
    /// FIFO queue of (submit_time, is_read, block_size_bytes) for in-flight ops.
    in_flight: VecDeque<(Instant, bool, usize)>,
    op_counter: Arc<AtomicU64>,
    byte_counter: Arc<AtomicU64>,
    stop_flag: Arc<AtomicBool>,
    submit_count: u64,
    lba_gen: Box<dyn LbaGenerator + Send>,
}

impl Worker {
    /// Create a new worker.
    ///
    /// `thread_index` is used for sequential LBA region partitioning.
    /// The caller must provide pre-connected `ClientChannels` and the `NamespaceInfo`
    /// for the target namespace.
    pub fn new(
        config: Arc<BenchConfig>,
        channels: ClientChannels,
        ns_info: NamespaceInfo,
        op_counter: Arc<AtomicU64>,
        byte_counter: Arc<AtomicU64>,
        stop_flag: Arc<AtomicBool>,
        thread_index: u32,
    ) -> Result<Self, NvmeBlockError> {
        let sector_size = ns_info.sector_size as usize;
        let max_blocks_per_io = (config.max_block_size() / sector_size) as u64;

        let mut read_bufs = Vec::with_capacity(config.block_sizes.len());
        let mut write_bufs = Vec::with_capacity(config.block_sizes.len());

        for &block_size in &config.block_sizes {
            let mut rbufs = Vec::with_capacity(config.queue_depth as usize);
            let mut wbufs = Vec::with_capacity(config.queue_depth as usize);
            for _ in 0..config.queue_depth {
                let rb = DmaBuffer::new(block_size, sector_size, None)
                    .map_err(NvmeBlockError::SpdkEnv)?;
                rbufs.push(Arc::new(Mutex::new(rb)));

                let wb = DmaBuffer::new(block_size, sector_size, None)
                    .map_err(NvmeBlockError::SpdkEnv)?;
                wbufs.push(Arc::new(wb));
            }
            read_bufs.push(rbufs);
            write_bufs.push(wbufs);
        }

        let lba_gen: Box<dyn LbaGenerator + Send> = match config.pattern {
            Pattern::Random => {
                Box::new(RandomLba::new(ns_info.num_sectors, max_blocks_per_io))
            }
            Pattern::Sequential => Box::new(SequentialLba::new(
                thread_index,
                config.threads,
                ns_info.num_sectors,
                max_blocks_per_io,
            )),
        };

        Ok(Self {
            config,
            channels,
            ns_info,
            sector_size,
            read_bufs,
            write_bufs,
            in_flight: VecDeque::new(),
            op_counter,
            byte_counter,
            stop_flag,
            submit_count: 0,
            lba_gen,
        })
    }

    /// Run the IO loop until the stop flag is set.
    ///
    /// Returns the collected per-thread statistics.
    pub fn run(&mut self) -> ThreadResult {
        let mut result = ThreadResult::default();
        let timeout_ms = (self.config.duration + 5) * 1000; // generous timeout

        // Fill the pipeline initially.
        while self.in_flight.len() < self.config.queue_depth as usize
            && !self.stop_flag.load(Ordering::Relaxed)
        {
            self.submit_batch(timeout_ms);
        }

        // Main IO loop.
        loop {
            if self.stop_flag.load(Ordering::Relaxed) && self.in_flight.is_empty() {
                break;
            }

            // Drain completions (non-blocking).
            self.drain_completions(&mut result);

            // Re-submit to keep pipeline full.
            if !self.stop_flag.load(Ordering::Relaxed) {
                while self.in_flight.len() < self.config.queue_depth as usize {
                    self.submit_batch(timeout_ms);
                }
            } else {
                // Draining: yield to let the actor process remaining in-flight ops.
                std::thread::yield_now();
            }
        }

        result
    }

    /// Submit a batch of IO operations.
    ///
    /// When `batch_size == 1`, sends a single command directly (no wrapping).
    /// When `batch_size > 1`, groups commands into `Command::BatchSubmit`.
    /// The actual count may be less than `batch_size` if the pipeline is nearly full.
    fn submit_batch(&mut self, timeout_ms: u64) {
        let remaining = self.config.queue_depth as usize - self.in_flight.len();
        let count = (self.config.batch_size as usize).min(remaining);
        if count == 0 {
            return;
        }

        if count == 1 && self.config.batch_size == 1 {
            let cmd = self.build_command(timeout_ms);
            if self.channels.command_tx.send(cmd).is_ok() {
                self.submit_count += 1;
            }
        } else {
            let mut ops = Vec::with_capacity(count);
            for _ in 0..count {
                ops.push(self.build_command(timeout_ms));
                self.submit_count += 1;
            }
            let batch = Command::BatchSubmit { ops };
            if self.channels.command_tx.send(batch).is_err() {
                // If the send fails, remove the in-flight entries we just pushed.
                for _ in 0..count {
                    self.in_flight.pop_back();
                    self.submit_count -= 1;
                }
            }
        }
    }

    /// Build a single IO command and record its in-flight entry.
    fn build_command(&mut self, timeout_ms: u64) -> Command {
        let bs_idx = if self.config.block_sizes.len() == 1 {
            0
        } else {
            rand::thread_rng().gen_range(0..self.config.block_sizes.len())
        };
        let block_size = self.config.block_sizes[bs_idx];
        let blocks_per_io = (block_size / self.sector_size) as u64;

        let lba = self.lba_gen.next_lba(blocks_per_io);
        let slot = self.submit_count as usize % self.config.queue_depth as usize;
        let is_read = self.choose_is_read();

        self.in_flight
            .push_back((Instant::now(), is_read, block_size));

        match (self.config.io_mode, is_read) {
            (IoMode::Async, true) => Command::ReadAsync {
                ns_id: self.ns_info.ns_id,
                lba,
                buf: Arc::clone(&self.read_bufs[bs_idx][slot]),
                timeout_ms,
                tag: 0,
            },
            (IoMode::Async, false) => Command::WriteAsync {
                ns_id: self.ns_info.ns_id,
                lba,
                buf: Arc::clone(&self.write_bufs[bs_idx][slot]),
                timeout_ms,
                tag: 0,
            },
            (IoMode::Sync, true) => Command::ReadSync {
                ns_id: self.ns_info.ns_id,
                lba,
                buf: Arc::clone(&self.read_bufs[bs_idx][slot]),
            },
            (IoMode::Sync, false) => Command::WriteSync {
                ns_id: self.ns_info.ns_id,
                lba,
                buf: Arc::clone(&self.write_bufs[bs_idx][slot]),
            },
        }
    }

    /// Choose whether the next op should be a read based on the configured OpType.
    fn choose_is_read(&self) -> bool {
        match self.config.op {
            OpType::Read => true,
            OpType::Write => false,
            OpType::ReadWrite => rand::random::<bool>(),
        }
    }

    /// Drain all available completions from the callback channel.
    fn drain_completions(&mut self, result: &mut ThreadResult) {
        while let Ok(completion) = self.channels.completion_rx.try_recv() {
            self.handle_completion(completion, result);
        }
    }

    /// Process a single completion message.
    fn handle_completion(&mut self, completion: Completion, result: &mut ThreadResult) {
        match completion {
            Completion::ReadDone { result: r, .. } => {
                if let Some((start, _, block_size)) = self.in_flight.pop_front() {
                    let latency_ns = start.elapsed().as_nanos() as u64;
                    result.latencies_ns.push(latency_ns);
                    if r.is_ok() {
                        result.read_ops += 1;
                        result.total_bytes += block_size as u64;
                        self.op_counter.fetch_add(1, Ordering::Relaxed);
                        self.byte_counter
                            .fetch_add(block_size as u64, Ordering::Relaxed);
                    } else {
                        result.errors += 1;
                    }
                }
            }
            Completion::WriteDone { result: r, .. } => {
                if let Some((start, _, block_size)) = self.in_flight.pop_front() {
                    let latency_ns = start.elapsed().as_nanos() as u64;
                    result.latencies_ns.push(latency_ns);
                    if r.is_ok() {
                        result.write_ops += 1;
                        result.total_bytes += block_size as u64;
                        self.op_counter.fetch_add(1, Ordering::Relaxed);
                        self.byte_counter
                            .fetch_add(block_size as u64, Ordering::Relaxed);
                    } else {
                        result.errors += 1;
                    }
                }
            }
            Completion::Timeout { .. } => {
                self.in_flight.pop_front();
                result.errors += 1;
            }
            Completion::Error { .. } => {
                self.in_flight.pop_front();
                result.errors += 1;
            }
            _ => {} // Ignore other completions
        }
    }
}
