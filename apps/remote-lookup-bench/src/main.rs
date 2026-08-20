//! Per-node load generator for the multi-node remote-lookup performance test.
//!
//! Runs on one node against that node's own local `/dev/shm` shmq mailbox and is
//! orchestrated across machines by `scripts/bench-remote-lookup-multinode.sh`.
//! Like the correctness driver it replaces for perf work, this is a lab tool: it
//! needs real RDMA NICs, NVMe drives and a GPU, so it is not a `cargo test`
//! target.
//!
//! # Why this is Rust
//!
//! The Python driver behind `scripts/test-full-remote-multinode.sh` cannot
//! measure this path. It builds each object's bytes one at a time through a
//! Python generator, sends one key per round-trip, and DMAs every key into a
//! single shared GPU buffer so nothing can overlap. Against RDMA plus SPDK the
//! result would be a measurement of the interpreter.
//!
//! Three things here exist to keep the client off the critical path:
//!
//! * **One CUDA allocation for the whole process**, exported as one IPC handle,
//!   addressed per entry through a per-region `offset`. The server opens the
//!   handle once (it dedupes per handle) and hands the whole batch to a single
//!   `batch_lookup`, so per-key client overhead collapses.
//! * **Many keys per request** (`--batch-size`) with **many requests in flight**
//!   (`--workers` x `--inflight` lanes), each lane owning its own shmq channel
//!   and a disjoint slice of that allocation so concurrent DMAs never alias.
//! * **No host/device copy on the measured path.** A lookup lands in GPU memory
//!   by DMA; the client only touches it when `--verify` is on.
//!
//! # Transport
//!
//! The control plane is the `/dev/shm` shmq mailbox (`shm-queue` +
//! `shmq-dispatcher`), not gRPC. The client is blocking (spin-then-futex), so a
//! lane is a plain OS thread holding one claimed channel: total lanes in flight
//! must be `<=` the server's `--channels`.
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

use shm_queue::Client;
use shmq_dispatcher::wire::{self, op};

/// The six CUDA runtime entry points this bench needs.
///
/// Declared here rather than taken from `gpu-services::cuda_ffi` on purpose: that
/// crate's `IGpuServices` impl is only complete when its `spdk` feature is on
/// (the trait's SPDK methods are `#[cfg(feature = "spdk")]` in `interfaces`), so
/// depending on it would break any `cargo build`/`cargo doc` that also builds a
/// crate enabling `interfaces/spdk`.
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

/// Keys per `Check`/`Remove` control call, so a large keyspace does not build one
/// oversized message. Clamped down further if the server's `cap_req` is small.
const CONTROL_CHUNK: usize = 8192;

/// shmq `request` tuning: busy-spin iterations before parking, per-park timeout,
/// and the overall hard deadline after which the server is treated as dead. The
/// deadline is generous because a cold lookup pays RDMA-connect and SSD-read
/// latency; it is a liveness backstop, not a per-op budget.
const SPIN_ITERS: u32 = 2000;
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(100);
const REQUEST_DEADLINE: Duration = Duration::from_secs(120);

/// How long `attach` waits for the server to publish the mailbox ready flag.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Path to the server's `/dev/shm` shmq mailbox on this node.
    #[arg(long = "shm-path", default_value = "/dev/shm/certus-shmq")]
    shm_path: String,
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

    /// Keys per request.
    #[arg(long, default_value_t = 64)]
    batch_size: usize,

    /// Parallelism groups. Kept alongside `--inflight` for CLI compatibility with
    /// the orchestration scripts; total lanes in flight is `workers * inflight`.
    #[arg(long, default_value_t = 4)]
    workers: usize,

    /// Concurrent requests per worker. Total lanes in flight is
    /// `workers * inflight`; each lane is one OS thread holding one shmq channel
    /// and a disjoint batch buffer, so this total must not exceed the server's
    /// `--channels`. GPU memory is allocated for that many disjoint buffers.
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

/// One CUDA allocation shared by every in-flight lane, exported once as a CUDA
/// IPC handle. Each lane owns a disjoint `batch_size * object_size` region, and
/// each entry within a lane is addressed by its `offset`, so concurrent
/// server-side DMAs never alias.
struct LandingBuffer {
    base: *mut c_void,
    /// The 64-byte CUDA IPC handle, exactly as the wire handle table carries it.
    handle: [u8; 64],
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
            Ok(Self {
                base,
                handle: raw.reserved,
                slot_bytes,
                object_size,
                gpu_device,
                total_bytes,
            })
        }
    }

    /// Offset of entry `i` within in-flight slot `slot`, relative to the
    /// allocation base — exactly what the wire region reference expects.
    fn offset(&self, slot: usize, i: usize) -> u64 {
        slot as u64 * self.slot_bytes + i as u64 * self.object_size as u64
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
// Wire encoding (mirrors shmq-dispatcher::wire / translate)
// ---------------------------------------------------------------------------

/// Encode a `{ n:u32, [key:u64]*n }` key-list request (Check / Remove).
fn encode_keys(keys: &[u64]) -> Vec<u8> {
    let mut w = wire::Writer::with_capacity(4 + keys.len() * 8);
    w.u32(keys.len() as u32);
    for &k in keys {
        w.u64(k);
    }
    w.into_bytes()
}

/// Encode a `HandleBatch` request (Populate / Lookup) for one lane's batch.
///
/// Every entry references the process's single CUDA allocation, so the handle
/// table holds exactly one handle (`n_handles == 1`) and each entry is a single
/// region (`nreg == 1`) pointing at that handle with its own offset.
fn encode_handle_batch(buf: &LandingBuffer, slot: usize, batch: &[u64]) -> Vec<u8> {
    // 4 (n_handles) + 68 (handle) + 4 (n_entries) header, then 26 bytes/entry.
    let mut w = wire::Writer::with_capacity(76 + batch.len() * 26);
    w.u32(1); // n_handles
    w.buf.extend_from_slice(&buf.handle); // 64-byte CUDA IPC handle
    w.u32(buf.gpu_device as u32); // gpu_device_id (i32 reinterpreted)
    w.u32(batch.len() as u32); // n_entries
    for (i, &key) in batch.iter().enumerate() {
        w.u64(key);
        w.buf.extend_from_slice(&1u16.to_le_bytes()); // nreg == 1
        w.u32(0); // handle_idx
        w.u64(buf.offset(slot, i)); // offset
        w.u32(buf.object_size); // size
    }
    w.into_bytes()
}

// ---------------------------------------------------------------------------
// shmq client helpers
// ---------------------------------------------------------------------------

/// Claim a channel, run `f` on it, and release it. Control-plane calls (Check /
/// Remove / GetIoStats / Flush / Clear) run when no lane threads are active, so
/// there is always a free channel as long as the server has at least one.
fn with_channel<T>(
    client: &Client,
    f: impl FnOnce(usize) -> Result<T, String>,
) -> Result<T, String> {
    let ch = client
        .claim_channel()
        .ok_or_else(|| "no free shmq channel (server --channels too low?)".to_string())?;
    let r = f(ch);
    client.release_channel(ch);
    r
}

/// Issue one request on `channel` and map a non-OK transport status to an error.
fn call(client: &Client, channel: usize, opcode: u32, data: &[u8]) -> Result<Vec<u8>, String> {
    let (status, resp) = client
        .request(
            channel,
            opcode,
            data,
            SPIN_ITERS,
            ATTEMPT_TIMEOUT,
            REQUEST_DEADLINE,
        )
        .map_err(|e| format!("shmq request (op {opcode}) failed: {e}"))?;
    if status != wire::STATUS_OK {
        return Err(format!(
            "op {opcode} server error: {}",
            String::from_utf8_lossy(&resp)
        ));
    }
    Ok(resp)
}

/// Largest key-list chunk that fits the server's request capacity.
fn control_chunk(client: &Client) -> usize {
    let cap = client.cap_req();
    if cap <= 4 {
        return 1;
    }
    ((cap - 4) / 8).clamp(1, CONTROL_CHUNK)
}

/// Cumulative SSD read/write counters (the GetIoStats response, 6×u64).
#[derive(Clone, Copy, Default)]
struct IoStats {
    read_ops: u64,
    read_bytes: u64,
    read_latency_ns_sum: u64,
    write_ops: u64,
    write_bytes: u64,
    write_latency_ns_sum: u64,
}

fn io_stats(client: &Client, ch: usize) -> Result<IoStats, String> {
    let resp = call(client, ch, op::GET_IO_STATS, &[])?;
    let mut r = wire::Reader::new(&resp);
    let mut next = || r.u64().map_err(|e| format!("GetIoStats decode: {e}"));
    Ok(IoStats {
        read_ops: next()?,
        read_bytes: next()?,
        read_latency_ns_sum: next()?,
        write_ops: next()?,
        write_bytes: next()?,
        write_latency_ns_sum: next()?,
    })
}

/// `Remove` every key, chunked. Used between `lookup` passes: a remote hit is
/// published into the local tier, so without this pass 2 would be a local hit and
/// would measure nothing. Per-key results are best-effort (mirrors the gRPC bench,
/// which only checked the call succeeded).
fn remove_all(client: &Client, ch: usize, keys: &[u64]) -> Result<(), String> {
    let chunk = control_chunk(client);
    for c in keys.chunks(chunk) {
        call(client, ch, op::REMOVE, &encode_keys(c))?;
    }
    Ok(())
}

/// How many of `keys` this node can currently find (either tier).
fn count_present(client: &Client, ch: usize, keys: &[u64]) -> Result<usize, String> {
    let chunk = control_chunk(client);
    let mut present = 0usize;
    for c in keys.chunks(chunk) {
        let resp = call(client, ch, op::CHECK, &encode_keys(c))?;
        present += resp.iter().filter(|&&b| b != 0).count();
    }
    Ok(present)
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

/// Percentiles over per-request latencies, in microseconds.
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
// Batch execution
// ---------------------------------------------------------------------------

/// Outcome of one lane thread's share of the work.
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

/// What a lane does with each batch.
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

/// Run `batches` through `client` on channel `ch`, using in-flight slot `slot`
/// of `buf`. Synchronous: the caller runs one of these per OS thread.
fn run_batches(
    client: &Client,
    buf: &LandingBuffer,
    ch: usize,
    slot: usize,
    batches: &[Vec<u64>],
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

        let opcode = match mode {
            Mode::Populate => op::POPULATE,
            Mode::Lookup => op::LOOKUP,
        };
        let req = encode_handle_batch(buf, slot, batch);

        let start = Instant::now();
        let result = call(client, ch, opcode, &req);
        let elapsed = start.elapsed();

        match result {
            Ok(resp) => {
                if record {
                    tally.latencies_ns.push(elapsed.as_nanos() as u64);
                }
                // Response is `[ok:u8]*n`, one flag per entry in request order.
                for (i, &key) in batch.iter().enumerate() {
                    let ok = resp.get(i).map(|&b| b != 0).unwrap_or(false);
                    if ok {
                        tally.ok += 1;
                    } else {
                        tally.failed += 1;
                        tally
                            .first_error
                            .get_or_insert_with(|| format!("key {key}: op returned not-ok"));
                    }
                }
                if verify && mode == Mode::Lookup {
                    let used = batch.len() * osz;
                    if let Err(e) = buf.read_slot(slot, &mut host[..used]) {
                        tally.first_error.get_or_insert(e);
                    } else {
                        for (i, &key) in batch.iter().enumerate() {
                            let hit = resp.get(i).map(|&b| b != 0).unwrap_or(false);
                            if hit && !stamp_ok(&host[i * osz..(i + 1) * osz], key) {
                                tally.verify_failures += 1;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tally.failed += batch.len();
                tally.first_error.get_or_insert(e);
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

fn run_phase(
    client: &Client,
    buf: &Arc<LandingBuffer>,
    keys: &[u64],
    args: &RunArgs,
    mode: Mode,
    record: bool,
) -> Result<(Tally, Duration), String> {
    let slots = args.slots();
    if slots > client.channel_count() {
        return Err(format!(
            "--workers×--inflight = {slots} lanes exceeds the server's {} channels; \
             lower them or raise the server --channels",
            client.channel_count()
        ));
    }
    let lanes = deal(keys, args.batch_size, slots);

    let job = Job {
        mode,
        object_size: args.object_size,
        verify: args.verify,
        record,
    };

    // One OS thread per non-empty lane; each claims its own shmq channel and
    // blocks on it (spin-then-futex). scope() joins them all before returning.
    let start = Instant::now();
    let tally = std::thread::scope(|scope| -> Result<Tally, String> {
        let mut handles = Vec::with_capacity(lanes.len());
        for (slot, batches) in lanes.iter().enumerate() {
            if batches.is_empty() {
                continue;
            }
            let buf = buf.as_ref();
            handles.push(scope.spawn(move || -> Result<Tally, String> {
                let ch = client
                    .claim_channel()
                    .ok_or_else(|| format!("lane {slot}: no free shmq channel"))?;
                let t = run_batches(client, buf, ch, slot, batches, job);
                client.release_channel(ch);
                Ok(t)
            }));
        }
        let mut tally = Tally::default();
        for h in handles {
            match h.join() {
                Ok(Ok(part)) => tally.merge(part),
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("lane thread panicked".to_string()),
            }
        }
        Ok(tally)
    })?;
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
    io_before: Option<&IoStats>,
    io_after: Option<&IoStats>,
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

fn cmd_run(args: RunArgs, mode: Mode) -> Result<(), String> {
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

    let client = Client::attach(&args.ep.shm_path, ATTACH_TIMEOUT)
        .map_err(|e| format!("attach shmq mailbox {}: {e}", args.ep.shm_path))?;

    if args.slots() > client.channel_count() {
        return Err(format!(
            "--workers×--inflight = {} lanes exceeds the server's {} channels; \
             lower them or raise the server --channels",
            args.slots(),
            client.channel_count()
        ));
    }

    // Warmup (lookup only): pay cold RDMA connect costs before timing. A cold
    // connect can dominate a short run, which is what made the first multi-node
    // runs look like failures.
    if mode == Mode::Lookup && args.warmup_keys > 0 {
        let n = args.warmup_keys.min(keys.len());
        run_phase(&client, &buf, &keys[..n], &args, mode, false)?;
        with_channel(&client, |ch| remove_all(&client, ch, &keys[..n]))?;
    }

    let io_before = if mode == Mode::Lookup {
        Some(with_channel(&client, |ch| io_stats(&client, ch))?)
    } else {
        None
    };

    let mut total = Tally::default();
    let mut elapsed = Duration::ZERO;
    for iter in 0..args.iterations.max(1) {
        let (t, d) = run_phase(&client, &buf, &keys, &args, mode, true)?;
        total.merge(t);
        elapsed += d;
        // A remote hit is published into this node's local tier, so a second pass
        // over the same keys would be served locally. Drop them to keep every
        // pass a genuine remote fetch. `--cleanup` extends that to the final pass
        // so the *next invocation* starts from the same clean state.
        if mode == Mode::Lookup && (args.cleanup || iter + 1 < args.iterations.max(1)) {
            with_channel(&client, |ch| remove_all(&client, ch, &keys))?;
        }
    }

    let io_after = if mode == Mode::Lookup {
        Some(with_channel(&client, |ch| io_stats(&client, ch))?)
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
fn cmd_demote(args: DemoteArgs) -> Result<(), String> {
    let keys = args.check_keys()?;
    let client = Client::attach(&args.ep.shm_path, ATTACH_TIMEOUT)
        .map_err(|e| format!("attach shmq mailbox {}: {e}", args.ep.shm_path))?;

    with_channel(&client, |ch| {
        let before = match &keys {
            Some(k) => count_present(&client, ch, k)?,
            None => 0,
        };

        let flushed = {
            let resp = call(&client, ch, op::FLUSH_TO_SSD, &[])?;
            wire::Reader::new(&resp)
                .u64()
                .map_err(|e| format!("FlushToSsd decode: {e}"))?
        };

        let cleared = {
            let resp = call(&client, ch, op::CLEAR_MEMORY_TIER, &[])?;
            wire::Reader::new(&resp)
                .u64()
                .map_err(|e| format!("ClearMemoryTier decode: {e}"))?
        };

        let after = match &keys {
            Some(k) => count_present(&client, ch, k)?,
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
    })
}

fn cmd_iostats(ep: EndpointArgs) -> Result<(), String> {
    let client = Client::attach(&ep.shm_path, ATTACH_TIMEOUT)
        .map_err(|e| format!("attach shmq mailbox {}: {e}", ep.shm_path))?;
    let s = with_channel(&client, |ch| io_stats(&client, ch))?;
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

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Command::Populate(a) => cmd_run(a, Mode::Populate),
        Command::Lookup(a) => cmd_run(a, Mode::Lookup),
        Command::Demote(a) => cmd_demote(a),
        Command::Iostats(a) => cmd_iostats(a),
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
                shm_path: "/dev/shm/certus-shmq".into(),
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

    /// The HandleBatch encoding must reproduce the layout the server decodes
    /// (`shmq_dispatcher::wire::decode_handle_batch`): one handle, one region per
    /// entry, offsets from the landing buffer.
    #[test]
    fn handle_batch_encodes_the_wire_layout() {
        // A fake landing buffer: we only touch the fields the encoder reads.
        let buf = LandingBuffer {
            base: std::ptr::null_mut(),
            handle: [0x11u8; 64],
            slot_bytes: 4096 * 2,
            object_size: 4096,
            gpu_device: -1,
            total_bytes: 4096 * 2,
        };
        let batch = [7u64, 42];
        let bytes = encode_handle_batch(&buf, 1, &batch);

        let mut r = wire::Reader::new(&bytes);
        assert_eq!(r.u32().unwrap(), 1, "n_handles");
        assert_eq!(r.handle().unwrap(), [0x11u8; 64]);
        assert_eq!(r.i32().unwrap(), -1, "gpu_device_id");
        assert_eq!(r.u32().unwrap(), 2, "n_entries");
        // entry 0
        assert_eq!(r.u64().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 1, "nreg");
        assert_eq!(r.u32().unwrap(), 0, "handle_idx");
        assert_eq!(r.u64().unwrap(), buf.offset(1, 0));
        assert_eq!(r.u32().unwrap(), 4096, "size");
        // entry 1
        assert_eq!(r.u64().unwrap(), 42);
        assert_eq!(r.u16().unwrap(), 1);
        assert_eq!(r.u32().unwrap(), 0);
        assert_eq!(r.u64().unwrap(), buf.offset(1, 1));
        assert_eq!(r.u32().unwrap(), 4096);
    }

    /// Key-list requests round-trip through the shared wire reader the server uses.
    #[test]
    fn key_list_encodes_the_wire_layout() {
        let keys = [3u64, 9, 27];
        let bytes = encode_keys(&keys);
        let mut r = wire::Reader::new(&bytes);
        assert_eq!(r.u32().unwrap() as usize, keys.len());
        for k in &keys {
            assert_eq!(r.u64().unwrap(), *k);
        }
    }
}
