//! Per-node load generator for the multi-node remote-lookup performance test.
//!
//! Runs on one node against that node's own `localhost` gRPC endpoint and is
//! orchestrated across machines by `scripts/bench-remote-lookup-multinode.sh`.
//! Like the correctness driver it replaces for perf work, this is a lab tool: it
//! needs real RDMA NICs, NVMe drives and a GPU, so it is not a `cargo test`
//! target.
//!
//! # Why this is Rust
//!
//! The Python driver behind `scripts/test-full-remote-multinode.sh` cannot
//! measure this path. It builds each object's bytes one at a time through a
//! Python generator, sends one key per gRPC round-trip, and DMAs every key into
//! a single shared GPU buffer so nothing can overlap. Against RDMA plus SPDK the
//! result would be a measurement of the interpreter.
//!
//! Three things here exist to keep the client off the critical path:
//!
//! * **One CUDA allocation for the whole process**, exported as one IPC handle,
//!   addressed per entry through `IpcHandle.offset`. The server opens the handle
//!   once (it dedupes per handle) and hands the whole batch to a single
//!   `batch_lookup`, so per-key client overhead collapses.
//! * **Many keys per RPC** (`--batch-size`) with **many RPCs in flight**
//!   (`--workers` x `--inflight`), each in-flight RPC owning a disjoint slice of
//!   that allocation so concurrent DMAs never alias.
//! * **No host/device copy on the measured path.** A lookup lands in GPU memory
//!   by DMA; the client only touches it when `--verify` is on.
//!
//! # Subcommands
//!
//! * `populate` — store a key range on this node (the holder role).
//! * `lookup` — fetch a key range and report throughput/latency (the requester
//!   role). Emits the `GetIoStats` delta so the orchestrator can prove the bytes
//!   came over the fabric rather than off local disk.
//! * `demote` — `FlushToSsd` then `ClearMemoryTier` then `Check`, moving this
//!   node's entries to the disk tier while keeping them findable.
//! * `iostats` — print this node's cumulative SSD counters.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use tonic::transport::{Channel, Endpoint};

mod pb {
    tonic::include_proto!("certus.dispatcher.v1");
}

/// The six CUDA runtime entry points this bench needs.
///
/// Declared here rather than taken from `gpu-services::cuda_ffi` on purpose: that
/// crate's `IGpuServices` impl is only complete when its `spdk` feature is on
/// (the trait's SPDK methods are `#[cfg(feature = "spdk")]` in `interfaces`), so
/// depending on it would break any `cargo build`/`cargo doc` that also builds a
/// crate enabling `interfaces/spdk`. `apps/baseline-generalized-fs` declares its
/// CUDA externs locally for the same reason.
mod cuda {
    use super::{c_char, c_int, c_void, CStr};

    pub type CudaError = c_int;
    pub const SUCCESS: CudaError = 0;
    pub const MEMCPY_HOST_TO_DEVICE: c_int = 1;
    pub const MEMCPY_DEVICE_TO_HOST: c_int = 2;

    /// Opaque 64-byte CUDA IPC memory handle.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct IpcMemHandle {
        pub reserved: [u8; 64],
    }

    extern "C" {
        pub fn cudaSetDevice(device: c_int) -> CudaError;
        pub fn cudaMalloc(devptr: *mut *mut c_void, size: usize) -> CudaError;
        pub fn cudaFree(devptr: *mut c_void) -> CudaError;
        pub fn cudaIpcGetMemHandle(handle: *mut IpcMemHandle, devptr: *mut c_void) -> CudaError;
        pub fn cudaMemcpy(
            dst: *mut c_void,
            src: *const c_void,
            count: usize,
            kind: c_int,
        ) -> CudaError;
        pub fn cudaGetErrorString(error: CudaError) -> *const c_char;
    }

    /// Human-readable text for a CUDA error code.
    pub fn error_string(err: CudaError) -> String {
        // SAFETY: cudaGetErrorString returns a static NUL-terminated string for
        // any code, including unknown ones.
        unsafe {
            let p = cudaGetErrorString(err);
            if p.is_null() {
                format!("unknown CUDA error {err}")
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }
}

use pb::dispatcher_client::DispatcherClient;

/// gRPC message ceiling. Requests carry keys and handles rather than payload, but
/// a `Check` over a large keyspace is chunked against this.
const MAX_MSG: usize = 64 * 1024 * 1024;

/// Keys per `Check`/`Remove` control call, so a large keyspace does not build one
/// oversized message.
const CONTROL_CHUNK: usize = 8192;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "remote-lookup-bench",
    about = "Per-node load generator for the multi-node remote-lookup perf test"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Store a key range on this node (holder role).
    Populate(RunArgs),
    /// Fetch a key range from peers and report throughput/latency (requester role).
    Lookup(RunArgs),
    /// Flush write-through, drain the memory tier, and confirm keys stay findable.
    Demote(DemoteArgs),
    /// Print this node's cumulative SSD read/write counters.
    Iostats(EndpointArgs),
}

#[derive(Args, Clone)]
struct EndpointArgs {
    /// gRPC endpoint of the server on this node.
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    server: String,
}

#[derive(Args, Clone)]
struct DemoteArgs {
    #[command(flatten)]
    ep: EndpointArgs,

    /// Keys this node is expected to hold, as an inclusive range. Supplying it
    /// turns on the before/after `Check` that proves the entries were demoted
    /// rather than destroyed — strongly recommended, since `entries_cleared`
    /// alone cannot distinguish the two.
    #[arg(long, value_parser = parse_range)]
    keys: Option<(u64, u64)>,

    /// Shard divisor for `--shard-eq`.
    #[arg(long, default_value_t = 1)]
    shard_mod: u64,

    /// Restrict the check to keys where `key % shard_mod == this`.
    #[arg(long)]
    shard_eq: Option<u64>,
}

impl DemoteArgs {
    /// Keys to check around the demote, if the caller named a range.
    fn check_keys(&self) -> Result<Option<Vec<u64>>, String> {
        let Some((lo, hi)) = self.keys else {
            return Ok(None);
        };
        if self.shard_mod == 0 {
            return Err("--shard-mod must be non-zero".into());
        }
        let keys: Vec<u64> = (lo..=hi)
            .filter(|k| match self.shard_eq {
                Some(i) => k % self.shard_mod == i,
                None => true,
            })
            .collect();
        Ok(Some(keys))
    }
}

#[derive(Args, Clone)]
struct RunArgs {
    #[command(flatten)]
    ep: EndpointArgs,

    /// Whole keyspace as an inclusive range, e.g. `1-200000`.
    #[arg(long, value_parser = parse_range)]
    keys: (u64, u64),

    /// Per-key object size, e.g. `64K`, `1M`.
    #[arg(long, default_value = "1M", value_parser = parse_size)]
    object_size: u32,

    /// Shard divisor. With `--shard-eq`/`--shard-ne`, selects this node's keys out
    /// of the whole range without any cross-node coordination.
    #[arg(long, default_value_t = 1)]
    shard_mod: u64,

    /// Keep keys where `key % shard_mod == this` (the holder's own shard).
    #[arg(long, conflicts_with = "shard_ne")]
    shard_eq: Option<u64>,

    /// Keep keys where `key % shard_mod != this` (everything a requester must get
    /// from a peer, never from itself).
    #[arg(long)]
    shard_ne: Option<u64>,

    /// Keys per RPC.
    #[arg(long, default_value_t = 64)]
    batch_size: usize,

    /// Independent gRPC connections. Each gets its own HTTP/2 channel.
    #[arg(long, default_value_t = 4)]
    workers: usize,

    /// Concurrent RPCs per worker. Total in flight is `workers * inflight`, and
    /// GPU memory is allocated for that many disjoint batch buffers.
    #[arg(long, default_value_t = 4)]
    inflight: usize,

    /// Passes over the selected key list. Between passes a `lookup` run removes
    /// the keys it fetched, because a remote hit is published into the local tier
    /// and pass 2 would otherwise be served locally.
    #[arg(long, default_value_t = 1)]
    iterations: usize,

    /// Keys fetched before timing starts, to pay cold RDMA connect costs outside
    /// the measurement. Ignored by `populate`.
    #[arg(long, default_value_t = 0)]
    warmup_keys: usize,

    /// Also remove the fetched keys after the *last* pass, not just between
    /// passes. Without it a `lookup` run leaves its keys published in the local
    /// tier, so a second invocation against the same cluster is served locally
    /// and measures nothing. With it every invocation is a genuine remote fetch,
    /// which is what lets one cluster bring-up serve a whole config sweep.
    /// The removal is outside the timed window, exactly as between passes.
    /// Ignored by `populate`.
    #[arg(long)]
    cleanup: bool,

    /// CUDA device ordinal.
    #[arg(long, default_value_t = 0)]
    gpu_device: i32,

    /// Check the key stamp in each returned object. Adds a device-to-host copy
    /// per batch, so it lowers the reported rate — use it to qualify a run, not
    /// to measure one.
    #[arg(long)]
    verify: bool,
}

fn parse_range(s: &str) -> Result<(u64, u64), String> {
    let (lo, hi) = s
        .split_once('-')
        .ok_or_else(|| format!("expected LO-HI, got '{s}'"))?;
    let lo: u64 = lo
        .trim()
        .parse()
        .map_err(|_| format!("bad low bound: {lo}"))?;
    let hi: u64 = hi
        .trim()
        .parse()
        .map_err(|_| format!("bad high bound: {hi}"))?;
    if hi < lo {
        return Err(format!("empty range: {lo}-{hi}"));
    }
    Ok((lo, hi))
}

/// Accept `4096`, `64K`, `1M`, `2G` — the same spelling `certus-server-yaml`
/// takes for `--memory-tier-size`.
fn parse_size(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let (digits, mult) = match s.as_bytes().last() {
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1024u64),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let n: u64 = digits.parse().map_err(|_| format!("invalid size: '{s}'"))?;
    let bytes = n
        .checked_mul(mult)
        .ok_or_else(|| format!("overflow: '{s}'"))?;
    u32::try_from(bytes).map_err(|_| format!("size exceeds u32: '{s}'"))
}

impl RunArgs {
    /// The keys this node handles, after shard selection.
    fn selected_keys(&self) -> Result<Vec<u64>, String> {
        if self.shard_mod == 0 {
            return Err("--shard-mod must be non-zero".into());
        }
        let (lo, hi) = self.keys;
        let keep: Box<dyn Fn(u64) -> bool> = match (self.shard_eq, self.shard_ne) {
            (Some(i), None) => Box::new(move |k: u64| k % self.shard_mod == i),
            (None, Some(i)) => Box::new(move |k: u64| k % self.shard_mod != i),
            (None, None) => Box::new(|_| true),
            (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
        };
        let keys: Vec<u64> = (lo..=hi).filter(|&k| keep(k)).collect();
        if keys.is_empty() {
            return Err(format!(
                "shard selection left no keys (range {lo}-{hi}, mod {})",
                self.shard_mod
            ));
        }
        Ok(keys)
    }

    fn slots(&self) -> usize {
        self.workers.max(1) * self.inflight.max(1)
    }
}

// ---------------------------------------------------------------------------
// GPU landing buffer
// ---------------------------------------------------------------------------

/// One CUDA allocation shared by every in-flight RPC, exported once as a CUDA IPC
/// handle. Each in-flight slot owns a disjoint `batch_size * object_size` region,
/// and each entry within a slot is addressed by `IpcHandle.offset`, so concurrent
/// server-side DMAs never alias.
struct LandingBuffer {
    base: *mut c_void,
    handle: Vec<u8>,
    slot_bytes: u64,
    object_size: u32,
    gpu_device: i32,
    total_bytes: u64,
}

// SAFETY: `base` is a device pointer used only as an opaque address — it is
// passed to the server in an IPC handle and offset arithmetic, never dereferenced
// on the host except through `cudaMemcpy`, which is thread-safe.
unsafe impl Send for LandingBuffer {}
unsafe impl Sync for LandingBuffer {}

impl LandingBuffer {
    fn new(
        slots: usize,
        batch_size: usize,
        object_size: u32,
        gpu_device: i32,
    ) -> Result<Self, String> {
        let slot_bytes = batch_size as u64 * object_size as u64;
        let total_bytes = slot_bytes * slots as u64;

        // SAFETY: plain CUDA runtime calls with valid out-pointers.
        unsafe {
            let err = cuda::cudaSetDevice(gpu_device);
            if err != cuda::SUCCESS {
                return Err(format!(
                    "cudaSetDevice({gpu_device}): {}",
                    cuda::error_string(err)
                ));
            }
            let mut base: *mut c_void = std::ptr::null_mut();
            let err = cuda::cudaMalloc(&mut base, total_bytes as usize);
            if err != cuda::SUCCESS {
                return Err(format!(
                    "cudaMalloc({total_bytes} bytes = {} MiB): {}",
                    total_bytes / (1024 * 1024),
                    cuda::error_string(err)
                ));
            }
            let mut raw = cuda::IpcMemHandle {
                reserved: [0u8; 64],
            };
            let err = cuda::cudaIpcGetMemHandle(&mut raw, base);
            if err != cuda::SUCCESS {
                cuda::cudaFree(base);
                return Err(format!("cudaIpcGetMemHandle: {}", cuda::error_string(err)));
            }
            // The proto carries the handle as opaque bytes.
            let handle = raw.reserved.to_vec();
            Ok(Self {
                base,
                handle,
                slot_bytes,
                object_size,
                gpu_device,
                total_bytes,
            })
        }
    }

    /// Offset of entry `i` within in-flight slot `slot`, relative to the
    /// allocation base — exactly what `IpcHandle.offset` expects.
    fn offset(&self, slot: usize, i: usize) -> u64 {
        slot as u64 * self.slot_bytes + i as u64 * self.object_size as u64
    }

    fn ipc(&self, slot: usize, i: usize) -> pb::IpcHandle {
        pb::IpcHandle {
            cuda_ipc_handle: self.handle.clone(),
            size: self.object_size,
            gpu_device_id: self.gpu_device,
            offset: self.offset(slot, i),
        }
    }

    /// Copy `bytes` into slot `slot` (host to device). Populate only.
    fn write_slot(&self, slot: usize, bytes: &[u8]) -> Result<(), String> {
        // SAFETY: dst is within the allocation (slot < slots, len <= slot_bytes)
        // and src is a valid host slice for `bytes.len()`.
        unsafe {
            let dst =
                (self.base as usize + (slot as u64 * self.slot_bytes) as usize) as *mut c_void;
            let err = cuda::cudaMemcpy(
                dst,
                bytes.as_ptr() as *const c_void,
                bytes.len(),
                cuda::MEMCPY_HOST_TO_DEVICE,
            );
            if err != cuda::SUCCESS {
                return Err(format!("cudaMemcpy H2D: {}", cuda::error_string(err)));
            }
        }
        Ok(())
    }

    /// Copy slot `slot` back to `out` (device to host). Verify only.
    fn read_slot(&self, slot: usize, out: &mut [u8]) -> Result<(), String> {
        // SAFETY: src is within the allocation and `out` is a valid host slice.
        unsafe {
            let src =
                (self.base as usize + (slot as u64 * self.slot_bytes) as usize) as *const c_void;
            let err = cuda::cudaMemcpy(
                out.as_mut_ptr() as *mut c_void,
                src,
                out.len(),
                cuda::MEMCPY_DEVICE_TO_HOST,
            );
            if err != cuda::SUCCESS {
                return Err(format!("cudaMemcpy D2H: {}", cuda::error_string(err)));
            }
        }
        Ok(())
    }
}

impl Drop for LandingBuffer {
    fn drop(&mut self) {
        // SAFETY: `base` came from cudaMalloc and is freed exactly once.
        unsafe {
            cuda::cudaFree(self.base);
        }
    }
}

// ---------------------------------------------------------------------------
// Payload stamping
// ---------------------------------------------------------------------------

/// Constant fill for object bodies. Content is uniform on purpose: generating
/// distinct bytes per key is what made the Python driver unusable at scale.
const FILL: u8 = 0xAB;

/// Write the key into the first and last 8 bytes of `obj`.
///
/// A uniform fill alone cannot catch the system returning some *other* key's
/// value, since every object would look identical. Stamping the key at both ends
/// makes a mixup detectable in O(1) per object rather than O(size).
fn stamp(obj: &mut [u8], key: u64) {
    let n = obj.len();
    debug_assert!(n >= 16, "object_size must be at least 16 bytes to stamp");
    obj[..8].copy_from_slice(&key.to_le_bytes());
    obj[n - 8..].copy_from_slice(&key.to_le_bytes());
}

/// Whether `obj` carries `key`'s stamp at both ends.
fn stamp_ok(obj: &[u8], key: u64) -> bool {
    let n = obj.len();
    if n < 16 {
        return false;
    }
    obj[..8] == key.to_le_bytes() && obj[n - 8..] == key.to_le_bytes()
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Percentiles over per-RPC latencies, in microseconds.
struct Percentiles {
    p50: f64,
    p99: f64,
    p999: f64,
    max: f64,
}

fn percentiles(mut ns: Vec<u64>) -> Percentiles {
    if ns.is_empty() {
        return Percentiles {
            p50: 0.0,
            p99: 0.0,
            p999: 0.0,
            max: 0.0,
        };
    }
    ns.sort_unstable();
    let at = |q: f64| -> f64 {
        // Nearest-rank; clamped so q=1.0 lands on the last sample.
        let idx = ((ns.len() as f64 * q).ceil() as usize).clamp(1, ns.len()) - 1;
        ns[idx] as f64 / 1000.0
    };
    Percentiles {
        p50: at(0.50),
        p99: at(0.99),
        p999: at(0.999),
        max: ns[ns.len() - 1] as f64 / 1000.0,
    }
}

// ---------------------------------------------------------------------------
// Client helpers
// ---------------------------------------------------------------------------

type Client = DispatcherClient<Channel>;

async fn connect(server: &str) -> Result<Client, String> {
    let ep = Endpoint::from_shared(server.to_string())
        .map_err(|e| format!("bad endpoint '{server}': {e}"))?
        // Nagle off: batches are latency-sensitive and already large.
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(10));
    let channel = ep
        .connect()
        .await
        .map_err(|e| format!("connect to {server} failed: {e}"))?;
    Ok(DispatcherClient::new(channel)
        .max_decoding_message_size(MAX_MSG)
        .max_encoding_message_size(MAX_MSG))
}

async fn io_stats(client: &mut Client) -> Result<pb::IoStatsResponse, String> {
    client
        .get_io_stats(pb::GetIoStatsRequest {})
        .await
        .map(|r| r.into_inner())
        .map_err(|e| format!("GetIoStats: {}", e.message()))
}

/// `Remove` every key, chunked. Used between `lookup` passes: a remote hit is
/// published into the local tier, so without this pass 2 would be a local hit and
/// would measure nothing.
async fn remove_all(client: &mut Client, keys: &[u64]) -> Result<(), String> {
    for chunk in keys.chunks(CONTROL_CHUNK) {
        client
            .remove(pb::BatchRemoveRequest {
                keys: chunk.to_vec(),
            })
            .await
            .map_err(|e| format!("Remove: {}", e.message()))?;
    }
    Ok(())
}

/// How many of `keys` this node can currently find (either tier).
async fn count_present(client: &mut Client, keys: &[u64]) -> Result<usize, String> {
    let mut present = 0usize;
    for chunk in keys.chunks(CONTROL_CHUNK) {
        let resp = client
            .check(pb::BatchCheckRequest {
                keys: chunk.to_vec(),
            })
            .await
            .map_err(|e| format!("Check: {}", e.message()))?;
        present += resp
            .into_inner()
            .results
            .iter()
            .filter(|r| r.exists)
            .count();
    }
    Ok(present)
}

// ---------------------------------------------------------------------------
// Batch execution
// ---------------------------------------------------------------------------

/// Outcome of one worker task's share of the work.
#[derive(Default)]
struct Tally {
    ok: usize,
    failed: usize,
    verify_failures: usize,
    latencies_ns: Vec<u64>,
    first_error: Option<String>,
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.ok += other.ok;
        self.failed += other.failed;
        self.verify_failures += other.verify_failures;
        self.latencies_ns.extend(other.latencies_ns);
        if self.first_error.is_none() {
            self.first_error = other.first_error;
        }
    }
}

/// What a worker does with each batch.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Populate,
    Lookup,
}

/// Invariants shared by every batch in one phase.
#[derive(Clone, Copy)]
struct Job {
    mode: Mode,
    object_size: u32,
    verify: bool,
    /// Gates latency collection, so warmup batches take the exact same path
    /// without polluting the percentiles.
    record: bool,
}

/// Run `batches` through `client`, using in-flight slot `slot` of `buf`.
async fn run_batches(
    mut client: Client,
    buf: Arc<LandingBuffer>,
    slot: usize,
    batches: Vec<Vec<u64>>,
    job: Job,
) -> Tally {
    let Job {
        mode,
        object_size,
        verify,
        record,
    } = job;
    let mut tally = Tally::default();
    // Host staging for populate (filled once, re-stamped per batch) and for
    // verify read-back. Neither exists on the measured lookup path.
    let mut host: Vec<u8> = if mode == Mode::Populate || verify {
        vec![FILL; buf.slot_bytes as usize]
    } else {
        Vec::new()
    };

    for batch in batches {
        let osz = object_size as usize;

        if mode == Mode::Populate {
            for (i, &key) in batch.iter().enumerate() {
                stamp(&mut host[i * osz..(i + 1) * osz], key);
            }
            let used = batch.len() * osz;
            if let Err(e) = buf.write_slot(slot, &host[..used]) {
                tally.failed += batch.len();
                tally.first_error.get_or_insert(e);
                continue;
            }
        }

        let start = Instant::now();
        let result = match mode {
            Mode::Populate => {
                let entries = batch
                    .iter()
                    .enumerate()
                    .map(|(i, &key)| pb::PopulateEntry {
                        key,
                        ipc_handle: Some(buf.ipc(slot, i)),
                    })
                    .collect();
                client
                    .populate(pb::BatchPopulateRequest { entries })
                    .await
                    .map(|r| r.into_inner().results)
            }
            Mode::Lookup => {
                let entries = batch
                    .iter()
                    .enumerate()
                    .map(|(i, &key)| pb::LookupEntry {
                        key,
                        ipc_handle: Some(buf.ipc(slot, i)),
                        // Single-region bench: leave the multi-region list empty
                        // so the server falls back to `ipc_handle`.
                        ipc_handles: Vec::new(),
                    })
                    .collect();
                client
                    .lookup(pb::BatchLookupRequest { entries })
                    .await
                    .map(|r| r.into_inner().results)
            }
        };
        let elapsed = start.elapsed();

        match result {
            Ok(results) => {
                if record {
                    tally.latencies_ns.push(elapsed.as_nanos() as u64);
                }
                for r in &results {
                    if r.success {
                        tally.ok += 1;
                    } else {
                        tally.failed += 1;
                        // "already exists" on populate means a prior run's keys
                        // survived; surface it rather than silently counting a
                        // failure the operator cannot explain.
                        tally
                            .first_error
                            .get_or_insert_with(|| format!("key {}: {}", r.key, r.error_message));
                    }
                }
                if verify && mode == Mode::Lookup {
                    let used = batch.len() * osz;
                    if let Err(e) = buf.read_slot(slot, &mut host[..used]) {
                        tally.first_error.get_or_insert(e);
                    } else {
                        for (i, &key) in batch.iter().enumerate() {
                            let hit = results.get(i).map(|r| r.success).unwrap_or(false);
                            if hit && !stamp_ok(&host[i * osz..(i + 1) * osz], key) {
                                tally.verify_failures += 1;
                            }
                        }
                    }
                }
            }
            Err(status) => {
                tally.failed += batch.len();
                tally
                    .first_error
                    .get_or_insert_with(|| format!("RPC failed: {}", status.message()));
            }
        }
    }
    tally
}

/// Split `keys` into `batch_size` chunks, then deal the chunks round-robin to
/// `slots` in-flight lanes.
fn deal(keys: &[u64], batch_size: usize, slots: usize) -> Vec<Vec<Vec<u64>>> {
    let mut lanes: Vec<Vec<Vec<u64>>> = vec![Vec::new(); slots];
    for (n, chunk) in keys.chunks(batch_size).enumerate() {
        lanes[n % slots].push(chunk.to_vec());
    }
    lanes
}

async fn run_phase(
    server: &str,
    buf: &Arc<LandingBuffer>,
    keys: &[u64],
    args: &RunArgs,
    mode: Mode,
    record: bool,
) -> Result<(Tally, Duration), String> {
    let slots = args.slots();
    let lanes = deal(keys, args.batch_size, slots);

    // One channel per worker; the `inflight` lanes of a worker multiplex over it.
    let mut channels = Vec::with_capacity(args.workers.max(1));
    for _ in 0..args.workers.max(1) {
        channels.push(connect(server).await?);
    }

    let job = Job {
        mode,
        object_size: args.object_size,
        verify: args.verify,
        record,
    };

    let mut tasks = Vec::with_capacity(slots);
    let start = Instant::now();
    for (slot, batches) in lanes.into_iter().enumerate() {
        if batches.is_empty() {
            continue;
        }
        let client = channels[slot % channels.len()].clone();
        let buf = Arc::clone(buf);
        tasks.push(tokio::spawn(async move {
            run_batches(client, buf, slot, batches, job).await
        }));
    }

    let mut tally = Tally::default();
    for t in tasks {
        match t.await {
            Ok(part) => tally.merge(part),
            Err(e) => return Err(format!("worker task panicked: {e}")),
        }
    }
    Ok((tally, start.elapsed()))
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

#[allow(clippy::too_many_arguments)]
fn emit_run_json(
    role: &str,
    args: &RunArgs,
    keys_selected: usize,
    tally: &Tally,
    elapsed: Duration,
    io_before: Option<&pb::IoStatsResponse>,
    io_after: Option<&pb::IoStatsResponse>,
    buf: &LandingBuffer,
) {
    let secs = elapsed.as_secs_f64();
    let bytes = tally.ok as u64 * args.object_size as u64;
    let gbps = if secs > 0.0 {
        bytes as f64 / secs / 1e9
    } else {
        0.0
    };
    let keys_per_s = if secs > 0.0 {
        tally.ok as f64 / secs
    } else {
        0.0
    };
    let p = percentiles(tally.latencies_ns.clone());

    let (read_ops_delta, read_bytes_delta) = match (io_before, io_after) {
        (Some(b), Some(a)) => (
            a.read_ops.saturating_sub(b.read_ops),
            a.read_bytes.saturating_sub(b.read_bytes),
        ),
        _ => (0, 0),
    };

    print!(
        concat!(
            "{{\"role\":\"{}\",",
            "\"keys_selected\":{},\"object_size\":{},\"batch_size\":{},",
            "\"workers\":{},\"inflight\":{},\"iterations\":{},\"warmup_keys\":{},",
            "\"gpu_buffer_mib\":{},",
            "\"elapsed_s\":{:.4},\"rpcs\":{},",
            "\"keys_ok\":{},\"keys_failed\":{},",
            "\"bytes\":{},\"gbps\":{:.4},\"keys_per_s\":{:.1},",
            "\"rpc_latency_us\":{{\"p50\":{:.1},\"p99\":{:.1},\"p999\":{:.1},\"max\":{:.1}}},",
            "\"local_read_ops_delta\":{},\"local_read_bytes_delta\":{},",
            "\"verify\":{},\"verify_failures\":{}"
        ),
        esc(role),
        keys_selected,
        args.object_size,
        args.batch_size,
        args.workers,
        args.inflight,
        args.iterations,
        args.warmup_keys,
        buf.total_bytes / (1024 * 1024),
        secs,
        tally.latencies_ns.len(),
        tally.ok,
        tally.failed,
        bytes,
        gbps,
        keys_per_s,
        p.p50,
        p.p99,
        p.p999,
        p.max,
        read_ops_delta,
        read_bytes_delta,
        args.verify,
        tally.verify_failures,
    );
    if let Some(e) = &tally.first_error {
        print!(",\"first_error\":\"{}\"", esc(e));
    }
    println!("}}");
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

async fn cmd_run(args: RunArgs, mode: Mode) -> Result<(), String> {
    if args.object_size < 16 {
        return Err("--object-size must be at least 16 bytes (key stamp)".into());
    }
    let keys = args.selected_keys()?;
    let buf = Arc::new(LandingBuffer::new(
        args.slots(),
        args.batch_size,
        args.object_size,
        args.gpu_device,
    )?);

    let mut ctl = connect(&args.ep.server).await?;

    // Warmup (lookup only): pay cold RDMA connect costs before timing. A cold
    // connect can dominate a short run, which is what made the first multi-node
    // runs look like failures.
    if mode == Mode::Lookup && args.warmup_keys > 0 {
        let n = args.warmup_keys.min(keys.len());
        let (_, _) = run_phase(&args.ep.server, &buf, &keys[..n], &args, mode, false).await?;
        remove_all(&mut ctl, &keys[..n]).await?;
    }

    let io_before = if mode == Mode::Lookup {
        Some(io_stats(&mut ctl).await?)
    } else {
        None
    };

    let mut total = Tally::default();
    let mut elapsed = Duration::ZERO;
    for iter in 0..args.iterations.max(1) {
        let (t, d) = run_phase(&args.ep.server, &buf, &keys, &args, mode, true).await?;
        total.merge(t);
        elapsed += d;
        // A remote hit is published into this node's local tier, so a second pass
        // over the same keys would be served locally. Drop them to keep every
        // pass a genuine remote fetch. `--cleanup` extends that to the final pass
        // so the *next invocation* starts from the same clean state.
        if mode == Mode::Lookup && (args.cleanup || iter + 1 < args.iterations.max(1)) {
            remove_all(&mut ctl, &keys).await?;
        }
    }

    let io_after = if mode == Mode::Lookup {
        Some(io_stats(&mut ctl).await?)
    } else {
        None
    };

    let role = match mode {
        Mode::Populate => "populate",
        Mode::Lookup => "lookup",
    };
    emit_run_json(
        role,
        &args,
        keys.len(),
        &total,
        elapsed,
        io_before.as_ref(),
        io_after.as_ref(),
        &buf,
    );

    if total.failed > 0 || total.verify_failures > 0 {
        return Err(format!(
            "{} failed key(s), {} verify failure(s)",
            total.failed, total.verify_failures
        ));
    }
    Ok(())
}

/// Move this node's entries to the disk tier while keeping them findable.
///
/// The order matters and is not optional. `clear_memory_tier` demotes a key that
/// already has an SSD copy (dispatch-map entry retained, pointing at the block)
/// but *force-removes from both tiers* a key whose write-through has not landed
/// yet. `FlushToSsd` first is what guarantees every key takes the demote branch.
/// `entries_cleared` cannot distinguish the two branches, so the `Check`
/// afterwards is the only proof nothing was destroyed.
async fn cmd_demote(args: DemoteArgs) -> Result<(), String> {
    let keys = args.check_keys()?;
    let mut client = connect(&args.ep.server).await?;

    let before = match &keys {
        Some(k) => count_present(&mut client, k).await?,
        None => 0,
    };

    let flushed = client
        .flush_to_ssd(pb::FlushToSsdRequest {})
        .await
        .map_err(|e| format!("FlushToSsd: {}", e.message()))?
        .into_inner()
        .jobs_flushed;

    let cleared = client
        .clear_memory_tier(pb::ClearMemoryTierRequest {})
        .await
        .map_err(|e| format!("ClearMemoryTier: {}", e.message()))?
        .into_inner()
        .entries_cleared;

    let after = match &keys {
        Some(k) => count_present(&mut client, k).await?,
        None => 0,
    };

    println!(
        "{{\"role\":\"demote\",\"jobs_flushed\":{flushed},\"entries_cleared\":{cleared},\
         \"checked\":{},\"present_before\":{before},\"present_after\":{after}}}",
        keys.is_some()
    );

    if keys.is_some() && after < before {
        return Err(format!(
            "demote lost {} key(s) ({before} findable before, {after} after) — \
             entries were force-removed instead of demoted",
            before - after
        ));
    }
    Ok(())
}

async fn cmd_iostats(ep: EndpointArgs) -> Result<(), String> {
    let mut client = connect(&ep.server).await?;
    let s = io_stats(&mut client).await?;
    println!(
        "{{\"role\":\"iostats\",\"read_ops\":{},\"read_bytes\":{},\
         \"read_latency_ns_sum\":{},\"write_ops\":{},\"write_bytes\":{},\
         \"write_latency_ns_sum\":{}}}",
        s.read_ops,
        s.read_bytes,
        s.read_latency_ns_sum,
        s.write_ops,
        s.write_bytes,
        s.write_latency_ns_sum
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Command::Populate(a) => cmd_run(a, Mode::Populate).await,
        Command::Lookup(a) => cmd_run(a, Mode::Lookup).await,
        Command::Demote(a) => cmd_demote(a).await,
        Command::Iostats(a) => cmd_iostats(a).await,
    };
    if let Err(e) = result {
        eprintln!("remote-lookup-bench: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parses_inclusively() {
        assert_eq!(parse_range("1-16").unwrap(), (1, 16));
        assert!(parse_range("16-1").is_err());
        assert!(parse_range("nope").is_err());
    }

    #[test]
    fn size_suffixes_parse() {
        assert_eq!(parse_size("4096").unwrap(), 4096);
        assert_eq!(parse_size("64K").unwrap(), 65536);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert!(parse_size("8G").is_err(), "exceeds u32");
    }

    fn args(shard_mod: u64, eq: Option<u64>, ne: Option<u64>) -> RunArgs {
        RunArgs {
            ep: EndpointArgs {
                server: "http://127.0.0.1:50051".into(),
            },
            keys: (1, 12),
            object_size: 4096,
            shard_mod,
            shard_eq: eq,
            shard_ne: ne,
            batch_size: 4,
            workers: 1,
            inflight: 1,
            iterations: 1,
            warmup_keys: 0,
            cleanup: false,
            gpu_device: 0,
            verify: false,
        }
    }

    /// Holder and requester selections must partition the keyspace exactly: every
    /// key belongs to one node, and a requester never asks for a key it holds.
    #[test]
    fn shard_selections_partition_the_keyspace() {
        let held = args(3, Some(1), None).selected_keys().unwrap();
        let wanted = args(3, None, Some(1)).selected_keys().unwrap();

        assert_eq!(held, vec![1, 4, 7, 10]);
        assert!(wanted.iter().all(|k| k % 3 != 1));
        assert!(
            held.iter().all(|k| !wanted.contains(k)),
            "a requester must never request its own shard"
        );
        assert_eq!(held.len() + wanted.len(), 12);
    }

    #[test]
    fn no_shard_flags_selects_everything() {
        assert_eq!(args(1, None, None).selected_keys().unwrap().len(), 12);
    }

    #[test]
    fn empty_shard_selection_is_an_error() {
        // mod 1 means every key is in shard 0, so `!= 0` selects nothing.
        assert!(args(1, None, Some(0)).selected_keys().is_err());
    }

    /// Every in-flight lane must get disjoint offsets, or concurrent DMAs alias.
    #[test]
    fn deal_covers_every_key_once() {
        let keys: Vec<u64> = (1..=10).collect();
        let lanes = deal(&keys, 3, 2);
        let mut seen: Vec<u64> = lanes.iter().flatten().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, keys);
    }

    #[test]
    fn stamp_roundtrips_and_detects_mixups() {
        let mut obj = vec![FILL; 4096];
        stamp(&mut obj, 42);
        assert!(stamp_ok(&obj, 42));
        assert!(!stamp_ok(&obj, 43), "another key's value must not verify");
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let p = percentiles((1..=1000).map(|n| n * 1000).collect());
        assert_eq!(p.p50, 500.0);
        assert_eq!(p.max, 1000.0);
        assert!(p.p99 >= p.p50 && p.p999 >= p.p99);
    }

    #[test]
    fn percentiles_of_empty_are_zero() {
        let p = percentiles(vec![]);
        assert_eq!(p.p50, 0.0);
        assert_eq!(p.max, 0.0);
    }
}
