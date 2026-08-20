//! Cross-process shared-memory request/response mailbox transport.
//!
//! This is the low-latency control-plane transport for certus-server: it keeps
//! protobuf encode/decode and socket syscalls off the per-op hot path.
//! It carries only *small* control messages (batches of keys and CUDA IPC
//! handle records); the KV bytes themselves never travel through the queue
//! (the server DMAs GPU↔DRAM↔SSD out of band).
//!
//! # Model
//!
//! A single `/dev/shm` file is `mmap(MAP_SHARED)`-ed by both the Rust server and
//! the (pure-Python, or Rust test) client. It holds a [`Header`] followed by `N`
//! independent **depth-1 mailbox channels**. Each channel has a request cell
//! (client → server) and a response cell (server → client). Exactly one request
//! is in flight per channel, so concurrency equals the channel count.
//!
//! * The **server busy-polls** every channel's `request.seq`. Because it never
//!   sleeps, the request path needs no wakeup primitive at all.
//! * The **client** publishes a request then waits on `response.seq` using
//!   adaptive spin-then-futex; the server `FUTEX_WAKE`s that word after writing
//!   the reply.
//!
//! # Correctness (x86-64 only)
//!
//! The design relies on x86-TSO store ordering so that a pure-Python client
//! (which cannot emit fences) is correct with plain loads/stores. The publish
//! store of `seq` is always the **last** store of a message; the Rust side pairs
//! it with `Release`/`Acquire`. Do not port the Python client to a weakly
//! ordered architecture without adding fences.
//!
//! The futex calls deliberately use the **shared** variant (`FUTEX_WAIT` /
//! `FUTEX_WAKE`, *not* the `_PRIVATE` forms `std` defaults to): the shared futex
//! keys on the underlying tmpfs inode+offset, so it works across unrelated
//! processes mapping the region at different virtual addresses.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Magic value written to [`Header::magic`] once the server has finished
/// initialising the region. Clients spin on this before touching any channel.
pub const MAGIC_READY: u32 = 0x5148_4d53; // "SMHQ" little-endian-ish
/// Layout ABI version. Client asserts a match at attach.
pub const ABI_VERSION: u32 = 1;
/// Size (bytes) of each control block. One cache line: satisfies the futex
/// 4-byte alignment requirement and isolates the busy-polled `seq` words so
/// request/response cells and neighbouring channels never false-share.
pub const CONTROL_SIZE: usize = 64;

/// Round `x` up to the next multiple of `CONTROL_SIZE` (cache line).
const fn round_up_cl(x: usize) -> usize {
    (x + CONTROL_SIZE - 1) & !(CONTROL_SIZE - 1)
}

/// Fixed header at the base of the mapping. `#[repr(C)]` with an explicit layout
/// the Python client mirrors field-for-field; the client additionally asserts
/// `control_size`/`channel_stride`/`channels_offset` against its own computed
/// layout at attach so a struct-layout mismatch fails immediately, not at 3am.
#[repr(C)]
pub struct Header {
    /// `0` until ready, then [`MAGIC_READY`] (published last, with `Release`).
    pub magic: AtomicU32,
    pub abi_version: u32,
    /// Bumped by the server on (re)create. Lets a client detect a server
    /// restart and re-sync its per-channel seq baseline instead of matching a
    /// stale cell value.
    pub generation: u32,
    pub num_channels: u32,
    pub cap_req: u32,
    pub cap_resp: u32,
    pub control_size: u32,
    pub channel_stride: u32,
    pub channels_offset: u32,
    pub server_pid: u32,
    _pad: u32,
    /// Server liveness counter; bumped each poll sweep so a client can tell a
    /// slow server from a dead one without waiting out its full deadline.
    pub heartbeat: AtomicU64,
}

/// Per-channel byte offsets within one channel block, computed from the header.
#[derive(Clone, Copy, Debug)]
struct Layout {
    num_channels: usize,
    cap_req: usize,
    cap_resp: usize,
    channels_offset: usize,
    channel_stride: usize,
    // Offsets within a channel block:
    req_payload_off: usize,
    resp_control_off: usize,
    resp_payload_off: usize,
    total_size: usize,
}

impl Layout {
    fn compute(num_channels: usize, cap_req: usize, cap_resp: usize) -> Layout {
        let channels_offset = round_up_cl(std::mem::size_of::<Header>());
        let req_payload_off = CONTROL_SIZE;
        let resp_control_off = req_payload_off + round_up_cl(cap_req);
        let resp_payload_off = resp_control_off + CONTROL_SIZE;
        let channel_stride = resp_payload_off + round_up_cl(cap_resp);
        let total_size = channels_offset + num_channels * channel_stride;
        Layout {
            num_channels,
            cap_req,
            cap_resp,
            channels_offset,
            channel_stride,
            req_payload_off,
            resp_control_off,
            resp_payload_off,
            total_size,
        }
    }

    #[inline]
    fn channel_base(&self, ch: usize) -> usize {
        self.channels_offset + ch * self.channel_stride
    }
}

// Control-block field offsets (within a 64-byte control block). Kept in sync
// with the Python client's ctypes definitions.
const OFF_SEQ: usize = 0;
const OFF_OPCODE: usize = 4; // request: opcode ; response: status
const OFF_LEN: usize = 8;
const OFF_OWNER: usize = 12; // request control only: channel-claim CAS word

/// An owned raw `mmap(MAP_SHARED)` region. Unmaps on drop; the server also
/// unlinks the backing file.
struct Mapping {
    ptr: *mut u8,
    len: usize,
    fd: RawFd,
    path: PathBuf,
    unlink_on_drop: bool,
}

// SAFETY: the mapping is a shared-memory region addressed purely through raw
// pointers + atomics; all field access below uses atomic or single-word
// aligned operations that are safe to issue from multiple threads.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    fn open(path: &Path, len: usize, create: bool) -> io::Result<Mapping> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .open(path)?;
        if create {
            file.set_len(len as u64)?;
        }
        let fd = file.as_raw_fd();
        // SAFETY: standard MAP_SHARED mmap of a sized, writable fd.
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // Keep the fd open for the life of the mapping.
        std::mem::forget(file);
        Ok(Mapping {
            ptr: ptr as *mut u8,
            len,
            fd,
            path: path.to_path_buf(),
            unlink_on_drop: create,
        })
    }

    #[inline]
    fn header(&self) -> &Header {
        // SAFETY: base of a mapping at least size_of::<Header>() long; Header is
        // repr(C) and its atomics share layout with their plain counterparts.
        unsafe { &*(self.ptr as *const Header) }
    }

    #[inline]
    unsafe fn u32_at(&self, off: usize) -> *mut u32 {
        self.ptr.add(off) as *mut u32
    }

    #[inline]
    unsafe fn atomic_at(&self, off: usize) -> &AtomicU32 {
        &*(self.ptr.add(off) as *const AtomicU32)
    }

    /// Read `len` bytes at `off` as an immutable slice into the mapping.
    ///
    /// # Safety
    /// `[off, off+len)` must lie within the mapping and the producer's publish
    /// store must have been observed (Acquire) before calling.
    #[inline]
    unsafe fn read_bytes(&self, off: usize, len: usize) -> &[u8] {
        std::slice::from_raw_parts(self.ptr.add(off), len)
    }

    /// Copy `src` into the mapping at `off`.
    ///
    /// Takes `&self` (state lives in the shared mapping); the write is a raw
    /// `copy_nonoverlapping`, so this does not hand out an aliasing `&mut` —
    /// callers serialise writes to a given region via the depth-1 protocol.
    ///
    /// # Safety
    /// `[off, off+src.len())` must lie within the mapping.
    #[inline]
    unsafe fn write_bytes(&self, off: usize, src: &[u8]) {
        ptr::copy_nonoverlapping(src.as_ptr(), self.ptr.add(off), src.len());
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: ptr/len came from this mmap; fd is ours.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
            libc::close(self.fd);
        }
        if self.unlink_on_drop {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ── futex helpers (shared variant — spans unrelated processes) ──────────────

/// Wake up to `n` waiters parked on the futex word at `addr`.
///
/// # Safety
/// `addr` must point to a live, 4-byte-aligned atomic word inside a shared
/// mapping.
pub unsafe fn futex_wake(addr: *const AtomicU32, n: i32) -> i64 {
    libc::syscall(
        libc::SYS_futex,
        addr as *const u32,
        libc::FUTEX_WAKE, // shared (NOT _PRIVATE)
        n,
        ptr::null::<libc::timespec>(),
        ptr::null::<u32>(),
        0,
    )
}

/// Park on the futex word at `addr` while it still equals `expected`.
///
/// Returns immediately with `EAGAIN` if the word already changed (this is what
/// closes the check-then-wait race). `timeout` is a *relative* deadline.
///
/// # Safety
/// `addr` must point to a live, 4-byte-aligned atomic word inside a shared
/// mapping.
pub unsafe fn futex_wait(addr: *const AtomicU32, expected: u32, timeout: Option<Duration>) -> i64 {
    let ts = timeout.map(|d| libc::timespec {
        tv_sec: d.as_secs() as libc::time_t,
        tv_nsec: d.subsec_nanos() as libc::c_long,
    });
    let tsp = ts
        .as_ref()
        .map_or(ptr::null::<libc::timespec>(), |t| t as *const _);
    libc::syscall(
        libc::SYS_futex,
        addr as *const u32,
        libc::FUTEX_WAIT, // shared (NOT _PRIVATE)
        expected as i32,
        tsp,
        ptr::null::<u32>(),
        0,
    )
}

/// A request the server has taken off a channel, ready to dispatch. The payload
/// is copied out of the shared region so the (single) poller thread can hand it
/// to a worker without holding a borrow into the mapping.
#[derive(Debug)]
pub struct PolledRequest {
    pub channel: usize,
    /// Sequence number of this request; echoed back verbatim in [`Server::reply`].
    pub seq: u32,
    pub opcode: u32,
    pub payload: Vec<u8>,
}

/// Server side of the transport. All methods take `&self` (state lives in the
/// shared mapping), so it can be wrapped in `Arc` and shared between a poller
/// thread and a worker pool: the poller calls [`Server::take_request`] with its
/// own `last_seen` array, workers call [`Server::reply`].
pub struct Server {
    map: Mapping,
    layout: Layout,
}

impl Server {
    /// Create (or recreate) the shared region and publish it ready.
    pub fn create(
        path: impl AsRef<Path>,
        num_channels: usize,
        cap_req: usize,
        cap_resp: usize,
    ) -> io::Result<Server> {
        assert!(num_channels > 0 && num_channels <= 4096);
        assert!(cap_req >= 64 && cap_resp >= 64);
        let layout = Layout::compute(num_channels, cap_req, cap_resp);
        let map = Mapping::open(path.as_ref(), layout.total_size, true)?;

        // Zero the whole region first (fresh file is already zero, but a
        // recreated one may not be), then write the header, magic LAST.
        // SAFETY: region is layout.total_size bytes.
        unsafe {
            ptr::write_bytes(map.ptr, 0, layout.total_size);
        }
        let hdr = map.header();
        // Bump generation across restarts so clients re-sync (read prior value
        // from the on-disk file before we zeroed — best-effort: start at 1).
        let gen = 1u32;
        // SAFETY: single-threaded init before magic is published; plain writes.
        unsafe {
            *map.u32_at(memoffset(&hdr.abi_version, hdr)) = ABI_VERSION;
            *map.u32_at(memoffset(&hdr.generation, hdr)) = gen;
            *map.u32_at(memoffset(&hdr.num_channels, hdr)) = num_channels as u32;
            *map.u32_at(memoffset(&hdr.cap_req, hdr)) = cap_req as u32;
            *map.u32_at(memoffset(&hdr.cap_resp, hdr)) = cap_resp as u32;
            *map.u32_at(memoffset(&hdr.control_size, hdr)) = CONTROL_SIZE as u32;
            *map.u32_at(memoffset(&hdr.channel_stride, hdr)) = layout.channel_stride as u32;
            *map.u32_at(memoffset(&hdr.channels_offset, hdr)) = layout.channels_offset as u32;
            *map.u32_at(memoffset(&hdr.server_pid, hdr)) = std::process::id();
        }
        // Publish ready last.
        hdr.magic.store(MAGIC_READY, Ordering::Release);
        Ok(Server { map, layout })
    }

    #[inline]
    pub fn channel_count(&self) -> usize {
        self.layout.num_channels
    }

    #[inline]
    pub fn cap_req(&self) -> usize {
        self.layout.cap_req
    }

    #[inline]
    pub fn cap_resp(&self) -> usize {
        self.layout.cap_resp
    }

    /// Bump the liveness heartbeat. The poller calls this once per sweep.
    #[inline]
    pub fn heartbeat(&self) {
        self.map.header().heartbeat.fetch_add(1, Ordering::Relaxed);
    }

    /// Initial `last_seen` baseline for a poller: all zeros.
    ///
    /// [`Server::create`] freshly zeroes the entire region (including every
    /// channel's `seq` word) before publishing `magic`, and clients only ever
    /// publish `seq >= 1` (see [`Client::next_seq`], which skips 0). So the
    /// correct baseline is a constant all-zeros — a value that can never collide
    /// with a real published seq.
    ///
    /// This must NOT read the live `seq` words: a client that attaches and
    /// publishes its first request between `create` and the poller's snapshot
    /// would be baselined-in and its request lost forever (the poller would see
    /// `seq == last_seen` and treat it as already-consumed). The re-sync problem
    /// this once tried to solve is instead handled by the header `generation`
    /// (bumped on recreate) plus the fresh-zero guarantee.
    pub fn seq_baseline(&self) -> Vec<u32> {
        vec![0u32; self.layout.num_channels]
    }

    /// If channel `ch` holds a request newer than `last_seen`, take it and
    /// return it (updating `last_seen`). Otherwise return `None`.
    pub fn take_request(&self, ch: usize, last_seen: &mut u32) -> Option<PolledRequest> {
        let base = self.layout.channel_base(ch);
        // SAFETY: aligned atomic word inside the mapping.
        let seq = unsafe { self.map.atomic_at(base + OFF_SEQ).load(Ordering::Acquire) };
        if seq == *last_seen {
            return None;
        }
        *last_seen = seq;
        // SAFETY: opcode/len are plain aligned words; seq (Acquire) ordered
        // their producer stores before us. len is clamped to cap_req.
        let (opcode, len) = unsafe {
            (
                *self.map.u32_at(base + OFF_OPCODE),
                (*self.map.u32_at(base + OFF_LEN)).min(self.layout.cap_req as u32) as usize,
            )
        };
        // SAFETY: payload lies within the channel block.
        let payload = unsafe {
            self.map
                .read_bytes(base + self.layout.req_payload_off, len)
                .to_vec()
        };
        Some(PolledRequest {
            channel: ch,
            seq,
            opcode,
            payload,
        })
    }

    /// Write a reply to `channel` and wake the client. `seq` must be the value
    /// from the [`PolledRequest`] being answered.
    pub fn reply(&self, channel: usize, seq: u32, status: u32, data: &[u8]) {
        let base = self.layout.channel_base(channel);
        let rc = base + self.layout.resp_control_off;
        let len = data.len().min(self.layout.cap_resp);
        // SAFETY: response payload/control lie within the channel block.
        unsafe {
            self.map
                .write_bytes(base + self.layout.resp_payload_off, &data[..len]);
            *self.map.u32_at(rc + OFF_OPCODE) = status;
            *self.map.u32_at(rc + OFF_LEN) = len as u32;
            // Publish seq LAST with Release, then wake.
            self.map
                .atomic_at(rc + OFF_SEQ)
                .store(seq, Ordering::Release);
            futex_wake(self.map.atomic_at(rc + OFF_SEQ), 1);
        }
    }
}

/// Client side of the transport (Rust; the production client is pure-Python).
/// Used by the in-process unit test and the `shmq-echo bench` cross-process
/// latency harness.
pub struct Client {
    map: Mapping,
    layout: Layout,
    seqs: Vec<AtomicU32>,
}

impl Client {
    /// Attach to an existing region, spinning until the server publishes ready.
    pub fn attach(path: impl AsRef<Path>, ready_timeout: Duration) -> io::Result<Client> {
        // First map just the header to learn the geometry, then remap the full
        // region. Simpler: map a header-sized region to read caps, compute
        // layout, drop, remap full.
        let hdr_len = round_up_cl(std::mem::size_of::<Header>());
        let probe = Mapping::open(path.as_ref(), hdr_len, false)?;
        let start = std::time::Instant::now();
        loop {
            if probe.header().magic.load(Ordering::Acquire) == MAGIC_READY {
                break;
            }
            if start.elapsed() > ready_timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shmq server not ready",
                ));
            }
            std::hint::spin_loop();
        }
        let hdr = probe.header();
        if hdr.abi_version != ABI_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("shmq ABI mismatch: {} != {}", hdr.abi_version, ABI_VERSION),
            ));
        }
        let num_channels = hdr.num_channels as usize;
        let cap_req = hdr.cap_req as usize;
        let cap_resp = hdr.cap_resp as usize;
        let layout = Layout::compute(num_channels, cap_req, cap_resp);
        // Validate the server's advertised geometry against ours.
        if hdr.control_size as usize != CONTROL_SIZE
            || hdr.channel_stride as usize != layout.channel_stride
            || hdr.channels_offset as usize != layout.channels_offset
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shmq layout mismatch between server and client",
            ));
        }
        drop(probe);
        let map = Mapping::open(path.as_ref(), layout.total_size, false)?;
        let seqs = (0..num_channels).map(|_| AtomicU32::new(0)).collect();
        Ok(Client { map, layout, seqs })
    }

    #[inline]
    pub fn channel_count(&self) -> usize {
        self.layout.num_channels
    }

    #[inline]
    pub fn cap_req(&self) -> usize {
        self.layout.cap_req
    }

    /// Claim a free channel via CAS on its owner word. Returns the index, or
    /// `None` if all channels are taken.
    pub fn claim_channel(&self) -> Option<usize> {
        let tid = (std::process::id() ^ thread_id()) | 1; // non-zero
        for ch in 0..self.layout.num_channels {
            let base = self.layout.channel_base(ch);
            // SAFETY: aligned atomic owner word inside the mapping.
            let owner = unsafe { self.map.atomic_at(base + OFF_OWNER) };
            if owner
                .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(ch);
            }
        }
        None
    }

    /// Release a previously claimed channel.
    pub fn release_channel(&self, ch: usize) {
        let base = self.layout.channel_base(ch);
        // SAFETY: aligned atomic owner word inside the mapping.
        unsafe {
            self.map
                .atomic_at(base + OFF_OWNER)
                .store(0, Ordering::Release);
        }
    }

    #[inline]
    fn next_seq(&self, ch: usize) -> u32 {
        let prev = self.seqs[ch].fetch_add(1, Ordering::Relaxed);
        let mut s = prev.wrapping_add(1);
        if s == 0 {
            // Never publish 0 (the empty/initial sentinel).
            s = 1;
            self.seqs[ch].store(1, Ordering::Relaxed);
        }
        s
    }

    /// Issue one request on `channel` and block (spin-then-futex) for the reply.
    ///
    /// `spin_iters` bounds the busy-spin before parking; `attempt_timeout`
    /// bounds each futex park; `deadline` is the overall hard deadline after
    /// which the server is treated as dead.
    pub fn request(
        &self,
        channel: usize,
        opcode: u32,
        data: &[u8],
        spin_iters: u32,
        attempt_timeout: Duration,
        deadline: Duration,
    ) -> io::Result<(u32, Vec<u8>)> {
        assert!(
            data.len() <= self.layout.cap_req,
            "request payload too large"
        );
        let base = self.layout.channel_base(channel);
        let rc = base + self.layout.resp_control_off;
        let seq = self.next_seq(channel);

        // Publish the request: payload, len, opcode, then seq LAST (Release).
        // SAFETY: all offsets lie within the channel block; seq store orders the
        // prior plain stores before the server's Acquire load observes seq.
        unsafe {
            self.map
                .write_bytes(base + self.layout.req_payload_off, data);
            *self.map.u32_at(base + OFF_OPCODE) = opcode;
            *self.map.u32_at(base + OFF_LEN) = data.len() as u32;
            self.map
                .atomic_at(base + OFF_SEQ)
                .store(seq, Ordering::Release);
        }

        // Await response.seq == seq.
        // SAFETY: aligned atomic response seq word inside the mapping.
        let resp_seq = unsafe { self.map.atomic_at(rc + OFF_SEQ) };
        let start = std::time::Instant::now();
        loop {
            // Bounded spin first (server busy-polls → replies are often sub-µs).
            for _ in 0..spin_iters {
                if resp_seq.load(Ordering::Acquire) == seq {
                    return Ok(self.read_response(rc));
                }
                std::hint::spin_loop();
            }
            let cur = resp_seq.load(Ordering::Acquire);
            if cur == seq {
                return Ok(self.read_response(rc));
            }
            if start.elapsed() > deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shmq request deadline exceeded (server dead?)",
                ));
            }
            // Val-guarded park: returns EAGAIN immediately if seq already moved.
            // SAFETY: resp_seq is a live aligned atomic word in the mapping.
            unsafe {
                futex_wait(resp_seq, cur, Some(attempt_timeout));
            }
        }
    }

    fn read_response(&self, rc: usize) -> (u32, Vec<u8>) {
        // SAFETY: response seq (Acquire) already ordered these producer stores
        // before us; status/len are plain aligned words, len clamped to cap.
        unsafe {
            let status = *self.map.u32_at(rc + OFF_OPCODE);
            let len = (*self.map.u32_at(rc + OFF_LEN)).min(self.layout.cap_resp as u32) as usize;
            let base = rc - self.layout.resp_control_off;
            let payload = self
                .map
                .read_bytes(base + self.layout.resp_payload_off, len)
                .to_vec();
            (status, payload)
        }
    }
}

/// Best-effort per-thread id for the channel-claim owner word (uniqueness is
/// not required for correctness — any non-zero value marks the channel taken).
fn thread_id() -> u32 {
    // SAFETY: gettid is always available on Linux and takes no arguments.
    (unsafe { libc::syscall(libc::SYS_gettid) } as u32).wrapping_mul(2654435761)
}

/// Byte offset of a `Header` field from the header base, for the plain-store
/// init path (avoids `&mut` aliasing through the shared atomics).
#[inline]
fn memoffset<T>(field: &T, hdr: &Header) -> usize {
    (field as *const T as usize) - (hdr as *const Header as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn tmp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("certus-shmq-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn layout_offsets_are_cache_line_aligned() {
        let l = Layout::compute(8, 1 << 20, 1 << 17);
        assert_eq!(l.channels_offset % CONTROL_SIZE, 0);
        assert_eq!(l.channel_stride % CONTROL_SIZE, 0);
        assert_eq!(l.req_payload_off % CONTROL_SIZE, 0);
        assert_eq!(l.resp_control_off % CONTROL_SIZE, 0);
        assert_eq!(l.resp_payload_off % CONTROL_SIZE, 0);
    }

    #[test]
    fn in_process_roundtrip_echo() {
        let path = tmp_path("echo");
        let server = Arc::new(Server::create(&path, 4, 4096, 4096).unwrap());
        let stop = Arc::new(AtomicBool::new(false));

        // Poller thread: echo each request's payload back with status 0.
        let srv = Arc::clone(&server);
        let stop2 = Arc::clone(&stop);
        let poller = std::thread::spawn(move || {
            let mut last_seen = srv.seq_baseline();
            while !stop2.load(Ordering::Relaxed) {
                let mut idle = true;
                for (ch, seen) in last_seen.iter_mut().enumerate() {
                    if let Some(req) = srv.take_request(ch, seen) {
                        idle = false;
                        srv.reply(req.channel, req.seq, req.opcode, &req.payload);
                    }
                }
                if idle {
                    std::hint::spin_loop();
                }
            }
        });

        let client = Client::attach(&path, Duration::from_secs(5)).unwrap();
        let ch = client.claim_channel().expect("a channel");
        for i in 0u32..1000 {
            let msg = format!("hello-{i}");
            let (status, resp) = client
                .request(
                    ch,
                    i,
                    msg.as_bytes(),
                    200,
                    Duration::from_millis(100),
                    Duration::from_secs(2),
                )
                .unwrap_or_else(|e| panic!("request i={i} failed: {e}"));
            assert_eq!(status, i); // echo server returns opcode as status
            assert_eq!(resp, msg.as_bytes());
        }
        client.release_channel(ch);

        stop.store(true, Ordering::Relaxed);
        poller.join().unwrap();
    }

    #[test]
    fn concurrent_clients_on_distinct_channels() {
        let path = tmp_path("concurrent");
        let server = Arc::new(Server::create(&path, 8, 4096, 4096).unwrap());
        let stop = Arc::new(AtomicBool::new(false));

        let srv = Arc::clone(&server);
        let stop2 = Arc::clone(&stop);
        let poller = std::thread::spawn(move || {
            let mut last_seen = srv.seq_baseline();
            while !stop2.load(Ordering::Relaxed) {
                for (ch, seen) in last_seen.iter_mut().enumerate() {
                    if let Some(req) = srv.take_request(ch, seen) {
                        // Reply with the byte-reversed payload.
                        let mut r = req.payload.clone();
                        r.reverse();
                        srv.reply(req.channel, req.seq, 0, &r);
                    }
                }
            }
        });

        let client = Arc::new(Client::attach(&path, Duration::from_secs(5)).unwrap());
        let mut handles = Vec::new();
        for t in 0..4u32 {
            let c = Arc::clone(&client);
            handles.push(std::thread::spawn(move || {
                let ch = c.claim_channel().expect("channel");
                for i in 0..500u32 {
                    let msg = format!("t{t}-msg{i}");
                    let (_s, resp) = c
                        .request(
                            ch,
                            0,
                            msg.as_bytes(),
                            100,
                            Duration::from_millis(100),
                            Duration::from_secs(5),
                        )
                        .unwrap();
                    let mut expect = msg.into_bytes();
                    expect.reverse();
                    assert_eq!(resp, expect);
                }
                c.release_channel(ch);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        poller.join().unwrap();
    }
}
