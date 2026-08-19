# SPDX-License-Identifier: Apache-2.0
"""Pure-Python client for the certus-shmq shared-memory control transport.

This is the drop-in replacement for the gRPC ``client.py``/stub: it speaks the
same *control-plane* ops the connector needs (Check / Touch / Reserve /
CopyToStore / CommitStore / AbortStore / Pin / Unpin / Lookup / TakeEvents), but
over a lock-free ``/dev/shm`` mailbox instead of tonic/gRPC. The KV bytes never
travel through here — exactly as with gRPC, the server opens the client's CUDA
IPC handles and DMAs GPU<->DRAM<->SSD out of band.

Correctness is **x86-64 only.** The scheme leans on x86-TSO store ordering so a
Python client (which cannot emit a fence) is correct with plain loads/stores:
the request's ``seq`` word is published *last*, after the payload/opcode/len
stores, and the server pairs it with an ``Acquire`` load. ``ring.py`` refuses to
start on any other architecture — do not port it to a weakly ordered ISA without
adding fences.

The wire framing (opcode + little-endian blob) mirrors, byte-for-byte,
``components/shmq-dispatcher/src/wire.rs`` and the shared-memory layout mirrors
``components/shm-queue/src/lib.rs``. Any change to either Rust file must be
mirrored here.
"""

from __future__ import annotations

import atexit
import ctypes
import errno
import mmap
import os
import platform
import struct
import sys
import threading
import time
from typing import Iterable, Sequence

# ── shm-queue layout constants (mirror components/shm-queue/src/lib.rs) ──────

MAGIC_READY = 0x5148_4D53  # "SMHQ"
ABI_VERSION = 1
CONTROL_SIZE = 64  # one cache line per control block

# Control-block field offsets within a 64-byte control block.
OFF_SEQ = 0  # request: published last ; response: server echoes (futex word)
OFF_OPCODE = 4  # request: opcode ; response: status
OFF_LEN = 8
OFF_OWNER = 12  # request control only: channel-claim word

# Fixed header field offsets (repr(C); heartbeat is u64 → 8-aligned at 48).
_HDR_MAGIC = 0
_HDR_ABI = 4
_HDR_GENERATION = 8
_HDR_NUM_CHANNELS = 12
_HDR_CAP_REQ = 16
_HDR_CAP_RESP = 20
_HDR_CONTROL_SIZE = 24
_HDR_CHANNEL_STRIDE = 28
_HDR_CHANNELS_OFFSET = 32
_HDR_SERVER_PID = 36
# _pad at 40 ; heartbeat (u64) at 48
_HEADER_SIZE = 56

# ── wire opcodes / status (mirror components/shmq-dispatcher/src/wire.rs) ────

OP_CHECK = 1
OP_TOUCH = 2
OP_RESERVE = 3
OP_COPY_TO_STORE = 4
OP_COMMIT_STORE = 5
OP_ABORT_STORE = 6
OP_PIN = 7
OP_UNPIN = 8
OP_LOOKUP = 9
OP_TAKE_EVENTS = 10
OP_POPULATE = 11
OP_REMOVE = 12
OP_CLEAR_MEMORY_TIER = 13
OP_FLUSH_TO_SSD = 14
OP_GET_IO_STATS = 15

STATUS_OK = 0

# Eviction-reason encoding in the TakeEvents response (mirror translate.rs).
REASON_DEMOTED = 0
REASON_REMOVED = 1

# HandleBatch sizing (mirror wire.rs HANDLE_ENTRY_SIZE / REGION_SIZE). Used to
# chunk oversize offload/load batches so one request never exceeds cap_req.
HANDLE_ENTRY_SIZE = 64 + 4  # cuda_ipc_handle[64] + gpu_device_id:i32
REGION_SIZE = 4 + 8 + 4  # handle_idx:u32 + offset:u64 + size:u32
_HANDLE_BATCH_FIXED = 4 + 4  # n_handles:u32 + n_entries:u32
_ENTRY_FIXED = 8 + 2  # key:u64 + nreg:u16

# ── futex (shared variant; SYS_futex is 202 on x86-64) ───────────────────────

_SYS_futex = 202
_FUTEX_WAIT = 0  # shared (NOT _PRIVATE)

_libc = ctypes.CDLL("libc.so.6", use_errno=True)
_libc.syscall.restype = ctypes.c_long


class _timespec(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long), ("tv_nsec", ctypes.c_long)]


class RingError(RuntimeError):
    """A transport-level or server-reported (STATUS_ERROR) failure."""


def _require_x86_64() -> None:
    m = platform.machine().lower()
    if m not in ("x86_64", "amd64"):
        raise RingError(
            f"certus-shmq ring.py is x86-64 only (got {platform.machine()!r}); "
            "it relies on x86-TSO store ordering and Python cannot emit a fence"
        )


def _round_up_cl(x: int) -> int:
    return (x + CONTROL_SIZE - 1) & ~(CONTROL_SIZE - 1)


# ── pure encode/decode helpers (server-independent; unit-testable) ───────────


def encode_keys(keys: Sequence[int]) -> bytes:
    """`{ n:u32, [key:u64]*n }` — Check/Commit/Abort/Unpin request."""
    out = bytearray(struct.pack("<I", len(keys)))
    for k in keys:
        out += struct.pack("<Q", k & 0xFFFFFFFFFFFFFFFF)
    return bytes(out)


def encode_promote_keys(promote: bool, keys: Sequence[int]) -> bytes:
    """`{ promote:u8, n:u32, [key:u64]*n }` — Touch/Pin request."""
    return struct.pack("<B", 1 if promote else 0) + encode_keys(keys)


def encode_reserve(entries: Sequence[tuple[int, int, int]]) -> bytes:
    """`{ n:u32, [key:u64, size:u32, session:u64]*n }` — Reserve request.

    ``entries`` is ``[(key, size_bytes, session_id)]``.
    """
    out = bytearray(struct.pack("<I", len(entries)))
    for key, size, session in entries:
        out += struct.pack(
            "<QIQ",
            key & 0xFFFFFFFFFFFFFFFF,
            size & 0xFFFFFFFF,
            session & 0xFFFFFFFFFFFFFFFF,
        )
    return bytes(out)


def _entry_cost(regions, seen_handles: set) -> tuple[int, int]:
    """(bytes this entry adds excluding new handles, count of new handle rows)."""
    new = 0
    for hb, dev, _off, _sz in regions:
        if (hb, dev) not in seen_handles:
            new += 1
    return _ENTRY_FIXED + len(regions) * REGION_SIZE, new


def encode_handle_batch(entries) -> bytes:
    """HandleBatch (CopyToStore / Lookup request).

    ``entries`` is ``[(key, regions)]`` where ``regions`` is
    ``[(handle_bytes[64], gpu_device_id:int, offset:int, size:int)]``. Distinct
    ``(handle_bytes, gpu_device_id)`` pairs are emitted once in a handle table
    and each region references its table index — the handles are identical
    across blocks of a region (only ``offset`` differs), so this is 5-6x smaller
    than inlining a handle per region per block.
    """
    table: dict[tuple[bytes, int], int] = {}
    table_rows: list[tuple[bytes, int]] = []
    body = bytearray(struct.pack("<I", len(entries)))
    for key, regions in entries:
        body += struct.pack("<Q", key & 0xFFFFFFFFFFFFFFFF)
        body += struct.pack("<H", len(regions))
        for hb, dev, off, size in regions:
            tkey = (hb, dev)
            idx = table.get(tkey)
            if idx is None:
                idx = len(table_rows)
                table[tkey] = idx
                table_rows.append(tkey)
            body += struct.pack("<IQI", idx, off & 0xFFFFFFFFFFFFFFFF, size & 0xFFFFFFFF)

    head = bytearray(struct.pack("<I", len(table_rows)))
    for hb, dev in table_rows:
        if len(hb) != 64:
            raise RingError(f"CUDA IPC handle must be 64 bytes, got {len(hb)}")
        head += hb
        head += struct.pack("<i", dev)
    return bytes(head) + bytes(body)


def chunk_handle_batches(entries, cap_req: int, log=None):
    """Split ``entries`` into chunks whose encoded HandleBatch fits in cap_req.

    Handles are deduped *within* a chunk, so the running size is computed
    incrementally as handles are first referenced. Chunk order preserves entry
    order, so a caller can reassemble per-key results by concatenation. A single
    entry larger than cap_req cannot be split (a block is atomic) — it is sent in
    its own chunk and ``log`` (if given) is warned.
    """
    chunks = []
    cur = []
    seen: set = set()
    cur_bytes = _HANDLE_BATCH_FIXED
    for key, regions in entries:
        add_body, new_h = _entry_cost(regions, seen)
        add = add_body + new_h * HANDLE_ENTRY_SIZE
        if cur and cur_bytes + add > cap_req:
            chunks.append(cur)
            cur = []
            seen = set()
            cur_bytes = _HANDLE_BATCH_FIXED
            add_body, new_h = _entry_cost(regions, seen)
            add = add_body + new_h * HANDLE_ENTRY_SIZE
        cur.append((key, regions))
        for hb, dev, _off, _sz in regions:
            seen.add((hb, dev))
        cur_bytes += add
        if len(cur) == 1 and cur_bytes > cap_req and log is not None:
            log(
                f"single block encodes to {cur_bytes} B > cap_req {cap_req} B "
                f"(key={key}, {len(regions)} regions); sending anyway"
            )
    if cur:
        chunks.append(cur)
    return chunks


def decode_ok_flags(payload: bytes, n: int) -> list[bool]:
    """`[ok:u8]*n` response → list of bools (missing bytes default False)."""
    return [i < len(payload) and payload[i] != 0 for i in range(n)]


def decode_take_events(payload: bytes) -> tuple[list[tuple[int, int]], int]:
    """`{ n:u32, [key:u64, reason:u32]*n, dropped:u64 }` → (events, dropped)."""
    off = 0
    (n,) = struct.unpack_from("<I", payload, off)
    off += 4
    events = []
    for _ in range(n):
        key, reason = struct.unpack_from("<QI", payload, off)
        off += 12
        events.append((key, reason))
    (dropped,) = struct.unpack_from("<Q", payload, off)
    return events, dropped


# Field order of the GetIoStats response (mirror translate.rs op_get_io_stats).
IO_STATS_FIELDS = (
    "read_ops",
    "read_bytes",
    "read_latency_ns_sum",
    "write_ops",
    "write_bytes",
    "write_latency_ns_sum",
)


def decode_u64(payload: bytes) -> int:
    """`{ u64 }` response → int. ClearMemoryTier / FlushToSsd reply shape."""
    (val,) = struct.unpack_from("<Q", payload, 0)
    return val


def decode_io_stats(payload: bytes) -> dict[str, int]:
    """`{ 6×u64 }` response → dict keyed by :data:`IO_STATS_FIELDS`.

    Order matches the gRPC ``IoStatsResponse`` (no histogram buckets):
    ``read_ops, read_bytes, read_latency_ns_sum, write_ops, write_bytes,
    write_latency_ns_sum``.
    """
    vals = struct.unpack_from("<6Q", payload, 0)
    return dict(zip(IO_STATS_FIELDS, vals))


# ── the ring client ──────────────────────────────────────────────────────────


class Ring:
    """Attached shared-memory control-plane client.

    Thread model: each calling thread claims a channel on first use and keeps it
    (sticky), matching the ~5 long-lived connector threads. One request is in
    flight per channel, so concurrency equals the channel count.
    """

    def __init__(
        self,
        path: str,
        *,
        ready_timeout: float = 30.0,
        spin_iters: int = 512,
        attempt_timeout: float = 0.05,
        deadline: float = 30.0,
        log=None,
    ):
        _require_x86_64()
        self._path = path
        self._spin_iters = int(spin_iters)
        self._attempt_timeout = float(attempt_timeout)
        self._deadline = float(deadline)
        self._log = log or (lambda msg: print(f"[certus-shmq] {msg}", file=sys.stderr, flush=True))

        self._fd = -1
        self._mm = None
        self._buf = None
        self._base = 0

        self._claim_lock = threading.Lock()
        self._tls = threading.local()
        # Per-channel client seq counters (each channel is single-threaded).
        self._seqs: list[int] = []

        self._attach(ready_timeout)

        # Release the attaching thread's channel at interpreter exit, so scripts
        # that exit (or sys.exit) without an explicit close() do not leave a
        # stale owner word in the persistent server's shared segment. Idempotent
        # with close(); a no-op once the segment is unmapped. Worker threads that
        # outlive their channel use still release explicitly (see run_pipeline).
        atexit.register(self._release_at_exit)

    def _release_at_exit(self) -> None:
        # Swallow errors: at interpreter shutdown the segment may already be gone.
        try:
            self.release_channel()
        except Exception:
            pass

    # ── attach / geometry ──

    def _attach(self, ready_timeout: float) -> None:
        deadline = time.monotonic() + ready_timeout
        # Open the file (it may not exist yet if the server is still starting).
        while True:
            try:
                self._fd = os.open(self._path, os.O_RDWR)
                break
            except OSError as e:
                if e.errno != errno.ENOENT or time.monotonic() > deadline:
                    raise RingError(f"cannot open shmq file {self._path!r}: {e}") from e
                time.sleep(0.01)

        size = os.fstat(self._fd).st_size
        if size < _HEADER_SIZE:
            # Server may have created the file but not yet set its length.
            while size < _HEADER_SIZE and time.monotonic() < deadline:
                time.sleep(0.01)
                size = os.fstat(self._fd).st_size
            if size < _HEADER_SIZE:
                raise RingError(f"shmq file {self._path!r} too small ({size} B)")

        self._mm = mmap.mmap(self._fd, size, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE)
        self._buf = (ctypes.c_char * size).from_buffer(self._mm)
        self._base = ctypes.addressof(self._buf)

        # Spin (bounded) on READY before touching any channel.
        while self._rd_u32(_HDR_MAGIC) != MAGIC_READY:
            if time.monotonic() > deadline:
                raise RingError(f"shmq server at {self._path!r} not ready (magic never set)")
            time.sleep(0.001)

        abi = self._rd_u32(_HDR_ABI)
        if abi != ABI_VERSION:
            raise RingError(f"shmq ABI mismatch: server {abi} != client {ABI_VERSION}")

        self._num_channels = self._rd_u32(_HDR_NUM_CHANNELS)
        self._cap_req = self._rd_u32(_HDR_CAP_REQ)
        self._cap_resp = self._rd_u32(_HDR_CAP_RESP)
        self._generation = self._rd_u32(_HDR_GENERATION)

        # Recompute the layout and assert the server's advertised geometry
        # matches ours — a repr/padding mismatch fails here, not at 3am.
        channels_offset = _round_up_cl(_HEADER_SIZE)
        req_payload_off = CONTROL_SIZE
        resp_control_off = req_payload_off + _round_up_cl(self._cap_req)
        resp_payload_off = resp_control_off + CONTROL_SIZE
        channel_stride = resp_payload_off + _round_up_cl(self._cap_resp)

        hdr_control_size = self._rd_u32(_HDR_CONTROL_SIZE)
        hdr_channel_stride = self._rd_u32(_HDR_CHANNEL_STRIDE)
        hdr_channels_offset = self._rd_u32(_HDR_CHANNELS_OFFSET)
        if (
            hdr_control_size != CONTROL_SIZE
            or hdr_channel_stride != channel_stride
            or hdr_channels_offset != channels_offset
        ):
            raise RingError(
                "shmq layout mismatch between server and client "
                f"(control_size {hdr_control_size}/{CONTROL_SIZE}, "
                f"channel_stride {hdr_channel_stride}/{channel_stride}, "
                f"channels_offset {hdr_channels_offset}/{channels_offset})"
            )

        total = channels_offset + self._num_channels * channel_stride
        if size < total:
            raise RingError(f"shmq file too small: {size} < computed {total}")

        self._channels_offset = channels_offset
        self._req_payload_off = req_payload_off
        self._resp_control_off = resp_control_off
        self._resp_payload_off = resp_payload_off
        self._channel_stride = channel_stride
        self._seqs = [0] * self._num_channels

    @property
    def channel_count(self) -> int:
        return self._num_channels

    @property
    def cap_req(self) -> int:
        return self._cap_req

    @property
    def cap_resp(self) -> int:
        return self._cap_resp

    @property
    def generation(self) -> int:
        return self._generation

    def heartbeat(self) -> int:
        """Current server liveness counter (u64 at header offset 48)."""
        return ctypes.c_uint64.from_address(self._base + 48).value

    # ── raw word access ──

    def _rd_u32(self, off: int) -> int:
        return ctypes.c_uint32.from_address(self._base + off).value

    def _wr_u32(self, off: int, val: int) -> None:
        ctypes.c_uint32.from_address(self._base + off).value = val & 0xFFFFFFFF

    def _channel_base(self, ch: int) -> int:
        return self._channels_offset + ch * self._channel_stride

    # ── channel claim (process-local; sticky per thread) ──

    def _claim_channel(self) -> int:
        # A non-zero owner marks the channel taken. Only this process's threads
        # claim (the server never touches the owner word), so a Python-lock-
        # guarded read-then-write is a correct claim among them.
        tid = (threading.get_ident() * 2654435761) & 0xFFFFFFFF or 1
        with self._claim_lock:
            for ch in range(self._num_channels):
                owner_off = self._channel_base(ch) + OFF_OWNER
                if self._rd_u32(owner_off) == 0:
                    self._wr_u32(owner_off, tid)
                    return ch
        raise RingError(f"no free shmq channel (all {self._num_channels} in use)")

    def _my_channel(self) -> int:
        ch = getattr(self._tls, "channel", None)
        if ch is None:
            ch = self._claim_channel()
            self._tls.channel = ch
        return ch

    def release_channel(self) -> None:
        """Release this thread's claimed channel back to the free pool.

        Channels are claimed sticky-per-thread on first use (see
        :meth:`_my_channel`) and normally held for the life of the thread. A
        caller that churns threads — a ``ThreadPoolExecutor`` recreated per
        phase, or fresh worker threads per benchmark round — must release before
        the thread exits, otherwise the owner word stays set and the channel is
        leaked (a later ``_claim_channel`` scan will never find it free).

        Safe to call with no channel held (no-op). Releasing is safe even though
        seq counters are *not* reset: ``self._seqs`` is indexed by channel, not
        by thread, so a later claimer of the same channel simply continues the
        monotonic sequence. Do not call with a request in flight on the channel.
        """
        ch = getattr(self._tls, "channel", None)
        if ch is None:
            return
        # If the segment is already unmapped (post-close), the owner word is
        # gone with it — just forget the claim rather than dereference address 0.
        if not self._base:
            self._tls.channel = None
            return
        with self._claim_lock:
            self._wr_u32(self._channel_base(ch) + OFF_OWNER, 0)
        self._tls.channel = None

    def _next_seq(self, ch: int) -> int:
        s = (self._seqs[ch] + 1) & 0xFFFFFFFF
        if s == 0:  # never publish 0 (the empty/initial sentinel)
            s = 1
        self._seqs[ch] = s
        return s

    def _futex_wait(self, addr: int, expected: int, timeout: float) -> None:
        ts = _timespec(int(timeout), int((timeout - int(timeout)) * 1_000_000_000))
        _libc.syscall(
            ctypes.c_long(_SYS_futex),
            ctypes.c_void_p(addr),
            ctypes.c_int(_FUTEX_WAIT),  # shared (NOT _PRIVATE)
            ctypes.c_uint(expected & 0xFFFFFFFF),
            ctypes.byref(ts),
            None,
            ctypes.c_int(0),
        )

    # ── one round-trip ──

    def request(self, opcode: int, data: bytes) -> tuple[int, bytes]:
        """Publish one request on this thread's channel and block (adaptive
        spin-then-futex) for the reply. Returns ``(status, payload)``."""
        if len(data) > self._cap_req:
            raise RingError(f"request payload {len(data)} B exceeds cap_req {self._cap_req} B")
        ch = self._my_channel()
        base = self._channel_base(ch)
        rc = base + self._resp_control_off
        seq = self._next_seq(ch)

        # Publish: payload, opcode, len, then seq LAST. x86-TSO keeps these
        # stores ordered, so the server's Acquire load of seq observes them all.
        if data:
            ctypes.memmove(self._base + base + self._req_payload_off, data, len(data))
        self._wr_u32(base + OFF_OPCODE, opcode)
        self._wr_u32(base + OFF_LEN, len(data))
        self._wr_u32(base + OFF_SEQ, seq)

        resp_seq_addr = self._base + rc + OFF_SEQ
        resp_seq = ctypes.c_uint32.from_address(resp_seq_addr)

        # Bounded busy-spin first (a busy-polling server usually replies sub-µs,
        # so this covers the common case without a futex syscall). A fast reply
        # exits early, so the cap only bites when the reply is genuinely delayed
        # (e.g. a cold SSD lookup taking ms) — exactly when we want to stop
        # spinning and park, releasing the GIL for the other connector threads.
        # Hence a small budget, not a large one.
        for _ in range(self._spin_iters):
            if resp_seq.value == seq:
                return self._read_response(rc)

        start = time.monotonic()
        while True:
            cur = resp_seq.value
            if cur == seq:
                return self._read_response(rc)
            if time.monotonic() - start > self._deadline:
                raise RingError("shmq request deadline exceeded (server dead?)")
            # Val-guarded park: the kernel returns EAGAIN immediately if seq
            # already moved off `cur`, closing the check-then-wait race.
            self._futex_wait(resp_seq_addr, cur, self._attempt_timeout)

    def _read_response(self, rc: int) -> tuple[int, bytes]:
        status = self._rd_u32(rc + OFF_OPCODE)
        ln = self._rd_u32(rc + OFF_LEN)
        if ln > self._cap_resp:
            ln = self._cap_resp
        base = rc - self._resp_control_off
        payload = ctypes.string_at(self._base + base + self._resp_payload_off, ln)
        return status, payload

    def _dispatch(self, opcode: int, data: bytes) -> bytes:
        status, payload = self.request(opcode, data)
        if status != STATUS_OK:
            raise RingError(
                f"server error (op={opcode}): {payload.decode('utf-8', 'replace')}"
            )
        return payload

    # ── typed ops (mirror the gRPC connector's call-sites) ──

    def check(self, keys: Sequence[int]) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(self._dispatch(OP_CHECK, encode_keys(keys)), len(keys))

    def touch(self, keys: Sequence[int], promote: bool = False) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(
            self._dispatch(OP_TOUCH, encode_promote_keys(promote, keys)), len(keys)
        )

    def reserve(self, entries: Sequence[tuple[int, int, int]]) -> list[bool]:
        entries = list(entries)
        if not entries:
            return []
        return decode_ok_flags(
            self._dispatch(OP_RESERVE, encode_reserve(entries)), len(entries)
        )

    def commit_store(self, keys: Sequence[int]) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(self._dispatch(OP_COMMIT_STORE, encode_keys(keys)), len(keys))

    def abort_store(self, keys: Sequence[int]) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(self._dispatch(OP_ABORT_STORE, encode_keys(keys)), len(keys))

    def pin(self, keys: Sequence[int], promote: bool = False) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(
            self._dispatch(OP_PIN, encode_promote_keys(promote, keys)), len(keys)
        )

    def unpin(self, keys: Sequence[int]) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(self._dispatch(OP_UNPIN, encode_keys(keys)), len(keys))

    def copy_to_store(self, entries) -> list[bool]:
        """CopyToStore, chunked to fit cap_req. Results are in entry order."""
        return self._handle_batch_op(OP_COPY_TO_STORE, entries)

    def lookup(self, entries) -> list[bool]:
        """Lookup, chunked to fit cap_req. Results (hit flags) are in entry order."""
        return self._handle_batch_op(OP_LOOKUP, entries)

    def _handle_batch_op(self, opcode: int, entries) -> list[bool]:
        entries = list(entries)
        if not entries:
            return []
        chunks = chunk_handle_batches(entries, self._cap_req, log=self._log)
        if len(chunks) > 1:
            self._log(f"op={opcode} batch of {len(entries)} split into {len(chunks)} chunks")
        results: list[bool] = []
        for chunk in chunks:
            payload = self._dispatch(opcode, encode_handle_batch(chunk))
            results.extend(decode_ok_flags(payload, len(chunk)))
        return results

    def take_events(self, max_events: int = 0) -> tuple[list[tuple[int, int]], int]:
        payload = self._dispatch(OP_TAKE_EVENTS, struct.pack("<I", max_events & 0xFFFFFFFF))
        return decode_take_events(payload)

    def populate(self, entries) -> list[bool]:
        """Populate cache entries by DMA from GPU, chunked to fit cap_req.

        ``entries`` is ``[(key, regions)]`` in the same shape ``copy_to_store``
        takes, but every entry must carry **exactly one** region — ``populate``
        takes a single CUDA IPC handle per key (the server rejects ``nreg != 1``,
        returning a False flag for that entry). Results are in entry order.
        """
        entries = list(entries)
        for key, regions in entries:
            if len(regions) != 1:
                raise RingError(
                    f"populate requires exactly one region per entry "
                    f"(key={key} has {len(regions)}); use copy_to_store for multi-region"
                )
        return self._handle_batch_op(OP_POPULATE, entries)

    def remove(self, keys: Sequence[int]) -> list[bool]:
        """Remove entries entirely. Returns per-key success flags in order."""
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(self._dispatch(OP_REMOVE, encode_keys(keys)), len(keys))

    def clear_memory_tier(self) -> int:
        """Evict the whole memory tier. Returns the number of entries cleared."""
        return decode_u64(self._dispatch(OP_CLEAR_MEMORY_TIER, b""))

    def flush_to_ssd(self) -> int:
        """Drain pending write-through jobs. Returns the number flushed."""
        return decode_u64(self._dispatch(OP_FLUSH_TO_SSD, b""))

    def get_io_stats(self) -> dict[str, int]:
        """Cumulative SSD read/write counters (see :data:`IO_STATS_FIELDS`)."""
        return decode_io_stats(self._dispatch(OP_GET_IO_STATS, b""))

    # ── teardown ──

    def close(self) -> None:
        # Release this thread's channel first, while the mapping is still valid.
        # The owner word lives in the shared /dev/shm segment, so a client that
        # exits without releasing leaves the channel marked owned on a persistent
        # server — starving later client processes. Worker threads that outlive
        # their channel use should call release_channel() directly; close() (run
        # by the thread that attached) covers the common single-threaded scripts.
        self.release_channel()
        atexit.unregister(self._release_at_exit)  # explicit close: drop the hook
        # Drop the ctypes view before unmapping (mmap.close() errors if exported
        # pointers still reference the buffer).
        self._buf = None
        self._base = 0
        if self._mm is not None:
            try:
                self._mm.close()
            except (BufferError, ValueError):
                pass
            self._mm = None
        if self._fd >= 0:
            os.close(self._fd)
            self._fd = -1

    def __enter__(self) -> "Ring":
        return self

    def __exit__(self, *exc) -> None:
        self.close()
