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
//!
//! # Channel claims (ABI 2)
//!
//! A client claims a channel by taking an **open-file-description lock**
//! (`F_OFD_SETLK`) on one byte of the mailbox file, at
//! `total_size + channel` -- past the mapped region, so the layout is
//! untouched. The lock is the claim: the kernel releases it when the holding
//! process exits, however it exits, so a channel whose owner died is free to
//! the next claimer, which takes it over. The owner word (`OFF_OWNER`) is kept
//! as a non-zero marker for observability only.
//!
//! Each claim uses its own open file description (the mapping's file reopened
//! through `/proc/self/fd`): OFD locks held by one description do not conflict
//! with each other, so sharing one would let a thread "claim" a channel a
//! sibling thread holds. Locks are keyed on the inode, so they are seen across
//! PID and mount namespaces -- a vLLM pod and a certus pod share only the tmpfs
//! file. A forked child that inherits a claim's descriptor keeps the claim
//! alive after its parent dies; the Rust client opens with `O_CLOEXEC` (std's
//! default) and is not used across `fork`. The Python client closes inherited
//! claim descriptors in an after-fork hook. Even with `O_CLOEXEC`, a
//! multithreaded process that spawns a child holds a copy of every claim
//! between the fork and the exec, so a channel released in that window reads
//! as held for that moment: a claimer then takes another channel, or finds
//! none and fails as for a full mailbox. Never a leak.
//!
//! A takeover inherits the channel's sequence numbers: the new owner waits until
//! the dead owner's last request (if any) has been answered, then continues from
//! that sequence, so the server neither skips its first request nor matches it
//! against a stale reply. Every claim does this, since a channel's previous
//! owner may have been another process even after a clean release.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Magic value written to [`Header::magic`] once the server has finished
/// initialising the region. Clients spin on this before touching any channel.
pub const MAGIC_READY: u32 = 0x5148_4d53; // "SMHQ" little-endian-ish
/// Layout ABI version. Client asserts a match at attach.
///
/// 2: channels are claimed by OFD lock (see the module docs). A version-1
/// client holds no locks, so a version-2 client would take its live channels
/// for dead ones; the two must never share a mailbox.
pub const ABI_VERSION: u32 = 2;

/// How long a claim waits for a dead owner's in-flight request to be answered
/// before giving up on that channel.
pub const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(30);
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
#[derive(Debug)]
pub struct Header {
    /// `0` until ready, then [`MAGIC_READY`] (published last, with `Release`).
    pub magic: AtomicU32,
    pub abi_version: u32,
    /// Distinct for every server start ([`fresh_generation`]), never 0. A
    /// client whose server has gone quiet compares it with the generation now
    /// at the mailbox path: a different one means the server restarted and
    /// this client holds an orphaned mapping ([`Client::server_replaced`]).
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

/// A generation for a server start: distinct from the previous start's, and
/// never 0.
///
/// It cannot be "the previous value plus one": the server unlinks its mailbox
/// when it stops, so a restart finds no previous value to read. Wall-clock
/// nanoseconds folded with the PID instead -- two starts on one path are at
/// least a process lifetime apart, so a repeat needs a 1-in-2^32 collision.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "folding: the high bits are mixed in before each narrowing cast"
)]
pub fn fresh_generation() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mixed = (nanos as u64) ^ (u64::from(std::process::id()) << 32) ^ ((nanos >> 64) as u64);
    let folded = (mixed ^ (mixed >> 32)) as u32;
    if folded == 0 {
        1
    } else {
        folded
    }
}

/// How long the heartbeat must stand still before a waiting client looks for a
/// replacement server at the mailbox path, and the least time between looks.
pub const QUIET_BEFORE_PROBE: Duration = Duration::from_millis(500);

/// Tracks whether a server's heartbeat has stood still long enough to probe.
struct Quiet {
    beat: u64,
    since: std::time::Instant,
}

impl Quiet {
    fn new(beat: u64) -> Self {
        Quiet {
            beat,
            since: std::time::Instant::now(),
        }
    }

    /// True once per [`QUIET_BEFORE_PROBE`] of an unmoving heartbeat.
    fn stood_still(&mut self, beat: u64) -> bool {
        if beat != self.beat {
            *self = Quiet::new(beat);
            return false;
        }
        if self.since.elapsed() < QUIET_BEFORE_PROBE {
            return false;
        }
        self.since = std::time::Instant::now();
        true
    }
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
        unsafe { self.ptr.add(off) as *mut u32 }
    }

    #[inline]
    unsafe fn atomic_at(&self, off: usize) -> &AtomicU32 {
        unsafe { &*(self.ptr.add(off) as *const AtomicU32) }
    }

    /// Read `len` bytes at `off` as an immutable slice into the mapping.
    ///
    /// # Safety
    /// `[off, off+len)` must lie within the mapping and the producer's publish
    /// store must have been observed (Acquire) before calling.
    #[inline]
    unsafe fn read_bytes(&self, off: usize, len: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.add(off), len) }
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
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), self.ptr.add(off), src.len());
        }
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
    unsafe {
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
    unsafe {
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

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("num_channels", &self.layout.num_channels)
            .finish_non_exhaustive()
    }
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
        let gen = fresh_generation();
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
    /// publish `seq >= 1` (see `Client::next_seq`, which skips 0). So the
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
    /// One open file description per claimed channel; dropping it releases
    /// the claim's lock.
    claims: Mutex<Vec<Option<File>>>,
    reclaimed: AtomicU64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("num_channels", &self.layout.num_channels)
            .finish_non_exhaustive()
    }
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
        let claims = Mutex::new((0..num_channels).map(|_| None).collect());
        Ok(Client {
            map,
            layout,
            seqs,
            claims,
            reclaimed: AtomicU64::new(0),
        })
    }

    #[inline]
    pub fn channel_count(&self) -> usize {
        self.layout.num_channels
    }

    #[inline]
    pub fn cap_req(&self) -> usize {
        self.layout.cap_req
    }

    /// Claim a free channel: one whose lock no live process holds. Returns the
    /// index, or `None` if every channel is held by a live client.
    ///
    /// A channel taken over from a dead client is counted in
    /// [`Client::reclaimed`].
    ///
    /// # Panics
    ///
    /// If the mailbox file cannot be reopened, or the kernel refuses an OFD
    /// lock for a reason other than a conflict: both mean claims cannot work at
    /// all, which must not be mistaken for "all channels busy".
    #[expect(
        clippy::expect_used,
        reason = "a mailbox that cannot be reopened or locked cannot be claimed at all; \
                  reporting it as 'no channel free' would hide the cause"
    )]
    pub fn claim_channel(&self) -> Option<usize> {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for ch in 0..self.layout.num_channels {
            if claims[ch].is_some() {
                continue; // ours already: a probe from our own process would succeed
            }
            let file = self
                .reopen()
                .expect("shmq: reopen mailbox for a channel lock");
            let locked = ofd_lock(&file, self.lock_offset(ch))
                .unwrap_or_else(|e| panic!("shmq: OFD lock on channel {ch}: {e}"));
            if !locked {
                continue; // a live owner, or a concurrent claimer
            }
            let base = self.layout.channel_base(ch);
            // SAFETY: aligned atomic owner word inside the mapping.
            let owner = unsafe { self.map.atomic_at(base + OFF_OWNER) };
            let previous = owner.swap((std::process::id() ^ thread_id()) | 1, Ordering::AcqRel);
            let Some(seq) = self.adopt_seq(ch, TAKEOVER_TIMEOUT) else {
                // The dead owner's request is still unanswered: leave the
                // channel, it is unusable until the server replies.
                owner.store(previous, Ordering::Release);
                continue;
            };
            self.seqs[ch].store(seq, Ordering::Relaxed);
            if previous != 0 {
                self.reclaimed.fetch_add(1, Ordering::Relaxed);
            }
            claims[ch] = Some(file);
            return Some(ch);
        }
        None
    }

    /// Channels this client has taken over from dead clients.
    #[must_use]
    pub fn reclaimed(&self) -> u64 {
        self.reclaimed.load(Ordering::Relaxed)
    }

    /// The server generation this client attached to.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.map.header().generation
    }

    /// Whether the mailbox path now holds a different server than the one this
    /// client mapped: a server that restarted recreated the file as a new
    /// inode, so this mapping is orphaned and no request on it will ever be
    /// answered. `Some(gen)` names the new server's generation; `None` means
    /// the path holds the same server, or no ready one (stopped, or still
    /// starting) -- a quiet server that may yet answer.
    #[must_use]
    pub fn server_replaced(&self) -> Option<u32> {
        let hdr_len = round_up_cl(std::mem::size_of::<Header>());
        let now = Mapping::open(&self.map.path, hdr_len, false).ok()?;
        let hdr = now.header();
        if hdr.magic.load(Ordering::Acquire) != MAGIC_READY {
            return None;
        }
        (hdr.generation != self.generation()).then_some(hdr.generation)
    }

    /// Release a previously claimed channel. The owner word is cleared before
    /// the lock is dropped, so a claimer that gets the lock never sees a stale
    /// owner and miscounts a clean release as a takeover.
    pub fn release_channel(&self, ch: usize) {
        let base = self.layout.channel_base(ch);
        // SAFETY: aligned atomic owner word inside the mapping.
        unsafe {
            self.map
                .atomic_at(base + OFF_OWNER)
                .store(0, Ordering::Release);
        }
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        claims[ch] = None; // closes the description: the lock goes with it
    }

    /// Byte of the mailbox file whose lock is channel `ch`'s claim.
    #[expect(
        clippy::expect_used,
        reason = "a mailbox is far below i64::MAX bytes; Layout bounds it"
    )]
    fn lock_offset(&self, ch: usize) -> i64 {
        i64::try_from(self.layout.total_size + ch).expect("mailbox offset fits i64")
    }

    /// A new open file description of the mapped file (same inode, even if the
    /// path has since been replaced by a restarted server).
    fn reopen(&self) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/proc/self/fd/{}", self.map.fd))
    }

    /// Wait until channel `ch`'s last request (if any) has been answered, and
    /// return that sequence number: the base the new owner continues from.
    /// `None` if it is still unanswered after `timeout`.
    fn adopt_seq(&self, ch: usize, timeout: Duration) -> Option<u32> {
        let base = self.layout.channel_base(ch);
        let rc = base + self.layout.resp_control_off;
        let start = std::time::Instant::now();
        loop {
            // SAFETY: aligned atomic seq words inside the mapping.
            let (req, resp) = unsafe {
                (
                    self.map.atomic_at(base + OFF_SEQ).load(Ordering::Acquire),
                    self.map.atomic_at(rc + OFF_SEQ).load(Ordering::Acquire),
                )
            };
            if req == resp {
                return Some(req);
            }
            if start.elapsed() > timeout {
                return None;
            }
            // SAFETY: live aligned atomic word in the mapping.
            unsafe {
                futex_wait(
                    self.map.atomic_at(rc + OFF_SEQ),
                    resp,
                    Some(Duration::from_millis(10)),
                );
            }
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
        let mut quiet = Quiet::new(self.map.header().heartbeat.load(Ordering::Relaxed));
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
            if let Some(gen) = self.replaced_after_quiet(&mut quiet) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!(
                        "shmq server restarted (generation {:#x} -> {gen:#x}); this client's \
                         mailbox is orphaned and must reattach",
                        self.generation()
                    ),
                ));
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

    /// Called while waiting for a reply: once the heartbeat has stood still for
    /// [`QUIET_BEFORE_PROBE`], look at the mailbox path (at most once per that
    /// interval) for a server that replaced ours. A live server bumps the
    /// heartbeat every few thousand poll sweeps, so a busy one never triggers
    /// the probe.
    fn replaced_after_quiet(&self, quiet: &mut Quiet) -> Option<u32> {
        let beat = self.map.header().heartbeat.load(Ordering::Relaxed);
        if quiet.stood_still(beat) {
            self.server_replaced()
        } else {
            None
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

/// Take an OFD write lock on byte `off` of `file`, without waiting. `Ok(false)`
/// if another open file description holds it.
fn ofd_lock(file: &File, off: i64) -> io::Result<bool> {
    // SAFETY: an all-zero flock is a valid value; the fields set below are the
    // whole request (l_pid must be 0 for OFD locks).
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "F_WRLCK and SEEK_SET are small constants; struct flock declares them short"
    )]
    {
        fl.l_type = libc::F_WRLCK as libc::c_short;
        fl.l_whence = libc::SEEK_SET as libc::c_short;
    }
    fl.l_start = off;
    fl.l_len = 1;
    // SAFETY: fcntl on a valid descriptor with a pointer to a live flock.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &fl) };
    if rc == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EAGAIN | libc::EACCES) => Ok(false),
        _ => Err(e),
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
