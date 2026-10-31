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
``lib/shmq-dispatcher/src/wire.rs`` and the shared-memory layout mirrors
``lib/shm-queue/src/lib.rs``. Any change to either Rust file must be
mirrored here.
"""

from __future__ import annotations

import atexit
import ctypes
import errno
import mmap
import fcntl
import os
import platform
import struct
import sys
import threading
import time
from typing import Iterable, Sequence

# ── shm-queue layout constants (mirror lib/shm-queue/src/lib.rs) ──────

MAGIC_READY = 0x5148_4D53  # "SMHQ"
ABI_VERSION = 2  # 2: channels are claimed by OFD lock (lib/shm-queue module docs)
CONTROL_SIZE = 64  # one cache line per control block

# Control-block field offsets within a 64-byte control block.
OFF_SEQ = 0  # request: published last ; response: server echoes (futex word)
OFF_OPCODE = 4  # request: opcode ; response: status
OFF_LEN = 8
OFF_OWNER = 12  # request control only: non-zero while claimed (observability)

# A claim waits this long for a dead owner's in-flight request to be answered.
TAKEOVER_TIMEOUT_S = 30.0

# Every live Ring, for the after-fork hook (weak: a closed Ring is forgotten).
import weakref as _weakref  # noqa: E402

_LIVE_RINGS: "_weakref.WeakSet[Ring]" = _weakref.WeakSet()


def _close_inherited_claims() -> None:
    """In a forked child: close the claim descriptors copied from the parent.

    A claim is an OFD lock held by an open file description; a child holding a
    copy keeps the parent's channels locked after the parent dies, so they could
    never be reclaimed. The child's threads never used those channels (threads do
    not survive fork), so it has nothing to lose."""
    for ring in list(_LIVE_RINGS):
        ring._forget_claims_after_fork()


os.register_at_fork(after_in_child=_close_inherited_claims)


def _ofd_lock(fd: int, off: int) -> bool:
    """OFD write lock on byte ``off`` of ``fd``, without waiting. False if
    another open file description holds it."""
    lk = struct.pack("hhqqi", fcntl.F_WRLCK, os.SEEK_SET, off, 1, 0)
    try:
        fcntl.fcntl(fd, fcntl.F_OFD_SETLK, lk)
    except OSError as e:
        if e.errno in (errno.EAGAIN, errno.EACCES):
            return False
        raise
    return True

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

# ── wire opcodes / status (mirror lib/shmq-dispatcher/src/wire.rs) ────

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
OP_TOUCH_CHECK = 16
OP_FETCH = 17
OP_CLEAR_ALL = 18

# An UNPIN key count meaning "release every pin on this channel" (no keys
# follow); answered with the number released, u64. Mirrors wire::UNPIN_ALL.
UNPIN_ALL = 0xFFFFFFFF
OP_LOOKUP_BLOCKS = 19
OP_COPY_TO_STORE_BLOCKS = 20
OP_LOOKUP_CHUNKS = 21
OP_COPY_TO_STORE_CHUNKS = 22

# ChunkBlockBatch block-id markers (mirror wire::CHUNK_SCRATCH / CHUNK_ABSENT): a GPU
# block the load does not want (its bytes go to the scratch handle), and one the key
# does not hold (the short last key of a transfer).
CHUNK_SCRATCH = 0xFFFFFFFFFFFFFFFF
CHUNK_ABSENT = 0xFFFFFFFFFFFFFFFE

STATUS_OK = 0

# Per-key Check response states (mirror wire.rs `check_state`). Widened from a
# plain exists-bool: MISS/RESIDENT keep the old 0/1 meaning, PENDING is new.
CHECK_MISS = 0
CHECK_RESIDENT = 1
CHECK_PENDING = 2

# Fetch response status (mirror wire.rs `fetch_status`).
FETCH_OK = 0
FETCH_NO_ENDPOINT = 1
FETCH_BAD_PEER = 2
FETCH_TOO_MANY = 3

# Eviction-reason encoding in the TakeEvents response (mirror translate.rs).
REASON_DEMOTED = 0
REASON_REMOVED = 1

# HandleBatch sizing (mirror wire.rs HANDLE_ENTRY_SIZE / REGION_SIZE). Used to
# chunk oversize offload/load batches so one request never exceeds cap_req.
HANDLE_ENTRY_SIZE = 64 + 4  # cuda_ipc_handle[64] + gpu_device_id:i32
REGION_SIZE = 4 + 8 + 4  # handle_idx:u32 + offset:u64 + size:u32
_HANDLE_BATCH_FIXED = 4 + 4  # n_handles:u32 + n_entries:u32
_ENTRY_FIXED = 8 + 2  # key:u64 + nreg:u16
# BlockBatch sizing (mirror wire.rs BlockBatch): per-layer row and per-key cost.
_LAYER_ROW = struct.Struct("<IQQI")  # handle_idx:u32 + base_delta:u64 + stride:u64 + size:u32
_BLOCK_ENTRY_SIZE = 8 + 8  # key:u64 + block_id:u64

# ── futex (shared variant; SYS_futex is 202 on x86-64) ───────────────────────

_SYS_futex = 202
_FUTEX_WAIT = 0  # shared (NOT _PRIVATE)

_libc = ctypes.CDLL("libc.so.6", use_errno=True)
_libc.syscall.restype = ctypes.c_long


class _timespec(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long), ("tv_nsec", ctypes.c_long)]


class RingError(RuntimeError):
    """A transport-level or server-reported (STATUS_ERROR) failure."""


class ServerRestartedError(RingError):
    """The certus server this ring attached to was replaced by a new one at the
    same path: the ring's mapping is orphaned, and only reattaching (in practice,
    restarting the engine) recovers."""


# A waiting request probes the mailbox path for a replacement server once the
# heartbeat has stood still this long (and at most once per interval after).
QUIET_BEFORE_PROBE_S = 0.5


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


_U64_MASK = 0xFFFFFFFFFFFFFFFF


def encode_keys(keys: Sequence[int]) -> bytes:
    """`{ n:u32, [key:u64]*n }` — Check/Commit/Abort/Unpin request."""
    # One bulk pack for the whole list. An out-of-range int makes struct raise,
    # so retry with the keys masked to u64 — same bytes as a per-key mask.
    n = len(keys)
    try:
        return struct.pack(f"<I{n}Q", n, *keys)
    except struct.error:
        return struct.pack(f"<I{n}Q", n, *[k & _U64_MASK for k in keys])


def encode_promote_keys(promote: bool, keys: Sequence[int]) -> bytes:
    """`{ promote:u8, n:u32, [key:u64]*n }` — Touch/Pin request."""
    # Single bulk pack (no encode_keys + concat); same masked retry, same bytes.
    n = len(keys)
    p = 1 if promote else 0
    try:
        return struct.pack(f"<BI{n}Q", p, n, *keys)
    except struct.error:
        return struct.pack(f"<BI{n}Q", p, n, *[k & _U64_MASK for k in keys])


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


def encode_layer_table(layers, extra_handles=None):
    """The fixed head of a BlockBatch (LookupBlocks / CopyToStoreBlocks request),
    from ``n_handles`` through the layers array.

    With ``extra_handles`` (``[(handle_bytes[64], gpu_device_id)]``, for a
    ChunkBlockBatch's scratch buffer), those are listed after the layers'
    handles, are not layers, and the call returns ``(table, index of the first
    extra handle)``.

    ``layers`` is ``[(handle_bytes[64], gpu_device_id, base_delta, stride, size)]``
    in layer order. Distinct ``(handle_bytes, gpu_device_id)`` pairs are emitted
    once, as in :func:`encode_handle_batch`; the server computes region ``L`` of
    a block as ``base_delta + block_id * stride`` of layer ``L``. Built once per
    worker and prepended to every :func:`encode_block_entries` body.
    """
    layers = list(layers)
    if not layers or len(layers) > 0xFFFF:
        raise RingError(f"layer table must have 1..65535 layers, got {len(layers)}")
    table: dict[tuple[bytes, int], int] = {}
    table_rows: list[tuple[bytes, int]] = []
    rows = bytearray(struct.pack("<H", len(layers)))
    for hb, dev, base_delta, stride, size in layers:
        tkey = (hb, dev)
        idx = table.get(tkey)
        if idx is None:
            idx = len(table_rows)
            table[tkey] = idx
            table_rows.append(tkey)
        rows += _LAYER_ROW.pack(idx, base_delta, stride, size)

    first_extra = len(table_rows)
    for tkey in extra_handles or ():
        table_rows.append(tkey)
    head = bytearray(struct.pack("<I", len(table_rows)))
    for hb, dev in table_rows:
        if len(hb) != 64:
            raise RingError(f"CUDA IPC handle must be 64 bytes, got {len(hb)}")
        head += hb
        head += struct.pack("<i", dev)
    if extra_handles is None:
        return bytes(head) + bytes(rows)
    return bytes(head) + bytes(rows), first_extra


def encode_block_entries(keys: Sequence[int], block_ids: Sequence[int]) -> bytes:
    """`{ n:u32, [key:u64]*n, [block_id:u64]*n }` — the per-call tail of a
    BlockBatch. One bulk pack; keys are masked to u64 on overflow as in
    :func:`encode_keys`. Block ids must be non-negative ints (struct raises
    otherwise)."""
    n = len(keys)
    if len(block_ids) != n:
        raise RingError(f"{n} keys != {len(block_ids)} block ids")
    fmt = f"<I{n}Q{n}Q"
    try:
        return struct.pack(fmt, n, *keys, *block_ids)
    except struct.error:
        return struct.pack(fmt, n, *[k & _U64_MASK for k in keys], *block_ids)


def encode_chunk_entries(scratch_idx: int, blocks_per_key: int, keys: Sequence[int],
                         block_ids: Sequence[int]) -> bytes:
    """`{ scratch_idx:u32, blocks_per_key:u16, n:u32, [key:u64]*n,
    [block_id:u64]*(n*blocks_per_key) }`: the per-call tail of a ChunkBlockBatch,
    after the layer table. ``block_ids`` holds ``blocks_per_key`` ids per key, in
    key order; CHUNK_SCRATCH / CHUNK_ABSENT mark a skipped / absent block."""
    n = len(keys)
    if blocks_per_key < 1 or blocks_per_key > 0xFFFF:
        raise RingError(f"blocks_per_key must be 1..65535, got {blocks_per_key}")
    if len(block_ids) != n * blocks_per_key:
        raise RingError(f"{n} keys x {blocks_per_key} != {len(block_ids)} block ids")
    fmt = f"<IHI{n}Q{n * blocks_per_key}Q"
    try:
        return struct.pack(fmt, scratch_idx, blocks_per_key, n, *keys, *block_ids)
    except struct.error:
        return struct.pack(fmt, scratch_idx, blocks_per_key, n,
                           *[k & _U64_MASK for k in keys], *block_ids)


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


def encode_fetch(peer: str, entries: Sequence[tuple[int, int]]) -> bytes:
    """`{ peer_len:u16, peer:[u8], n:u32, [key:u64, size:u32]*n }` — Fetch."""
    p = peer.encode("utf-8")
    out = bytearray(struct.pack("<H", len(p)))
    out += p
    out += struct.pack("<I", len(entries))
    for key, size in entries:
        out += struct.pack("<QI", key & _U64_MASK, size & 0xFFFFFFFF)
    return bytes(out)


def decode_fetch(payload: bytes, n: int) -> tuple[int, list[bool]]:
    """`{ status:u8, [started:u8]*n }` → (status, started flags). The flags are
    present only when status is FETCH_OK; otherwise every key reads not started."""
    if not payload:
        return FETCH_NO_ENDPOINT, [False] * n
    status = payload[0]
    if status != FETCH_OK:
        return status, [False] * n
    return status, [i + 1 < len(payload) and payload[i + 1] != 0 for i in range(n)]


def decode_ok_flags(payload: bytes, n: int) -> list[bool]:
    """`[ok:u8]*n` response → list of bools (missing bytes default False)."""
    return [i < len(payload) and payload[i] != 0 for i in range(n)]


def decode_states(payload: bytes, n: int) -> list[int]:
    """`[state:u8]*n` Check response → list of raw state ints (missing bytes
    default ``CHECK_MISS``). Values are 0=miss, 1=resident, 2=pending."""
    # Bulk byte->int conversion; extra bytes are ignored, a short tail pads MISS.
    states = list(payload[:n])
    short = n - len(states)
    if short > 0:
        states.extend([CHECK_MISS] * short)
    return states


def decode_take_events(payload: bytes) -> tuple[list[tuple[int, int]], int]:
    """`{ n:u32, [key:u64, reason:u32]*n, dropped:u64 }` → (events, dropped).

    Defensive against a truncated response. The server serializes events into the
    fixed ``cap_resp`` response slot; a drain that produces more bytes than that
    slot holds is truncated by the transport while the header count ``n`` still
    claims the full number, so a blind ``n``-record read walks off the end
    (``struct.error`` — which surfaces as an engine-killing EngineDeadError). We
    decode only the records the buffer physically contains and fold any
    claimed-but-absent remainder into ``dropped``. This is safe because the
    connector's ``lookup()``/Check is authoritative and live: a missed eviction
    event only costs vLLM some stale bookkeeping, never a wrong load (a
    fully/partially-evicted block reads as a MISS at load time). ``take_events()``
    now bounds ``max_events`` so this truncation path should not trigger, but the
    guard turns any residual capacity mismatch into the already-handled lossy path
    instead of a crash."""
    buf_len = len(payload)
    if buf_len < 4:
        return [], 0
    off = 0
    (n,) = struct.unpack_from("<I", payload, off)
    off += 4
    # Records that fit after the u32 header, leaving room for the u64 trailer.
    max_fit = max(0, (buf_len - 4 - 8) // 12)
    decodable = min(n, max_fit)
    events = []
    for _ in range(decodable):
        key, reason = struct.unpack_from("<QI", payload, off)
        off += 12
        events.append((key, reason))
    if decodable < n:
        # Buffer was truncated: the trailing u64 dropped counter was overwritten
        # by records, so it can't be read. Report the events we could not decode
        # as dropped (best-effort) rather than crash the engine.
        return events, n - decodable
    if buf_len < off + 8:
        return events, 0
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
        deadline: float | None = None,
        log=None,
        claim_slot: int = 0,
        claim_slots: int = 1,
    ):
        _require_x86_64()
        self._path = path
        self._spin_iters = int(spin_iters)
        self._attempt_timeout = float(attempt_timeout)
        # Per-request deadline. A full server memory tier makes OP_RESERVE
        # backpressure on the server side; that backpressure is now bounded per
        # reserve *batch* (Translator::op_reserve), but on a persistently
        # saturated tier a legitimate request can still take longer than the
        # 30 s default. CERTUS_SHMQ_DEADLINE_S lets an operator widen it without
        # a code change so a slow-but-alive server is not misreported as dead.
        # An explicit `deadline=` argument still wins over the env var.
        if deadline is None:
            deadline = float(os.environ.get("CERTUS_SHMQ_DEADLINE_S", "30.0"))
        self._deadline = float(deadline)
        self._log = log or (lambda msg: print(f"[certus-shmq] {msg}", file=sys.stderr, flush=True))

        # Channel-claim partitioning for the TP>1 multi-process case. Under
        # MultiprocExecutor (TP=2) three processes share this one mailbox —
        # engine-core (scheduler/manager, slot 0) plus one worker per rank
        # (slots 1..W) — and ``_claim_channel``'s owner-word claim is only
        # process-locally locked (a non-atomic read-then-write on the shared
        # word), so two processes could otherwise grab the same channel. Each
        # slot scans a disjoint channel slice. claim_slots=1 (the default, and
        # TP=1's single-process UniProcExecutor) → full range = exact baseline.
        self._claim_slots = max(1, int(claim_slots))
        self._claim_slot = int(claim_slot)
        if not (0 <= self._claim_slot < self._claim_slots):
            raise ValueError(
                f"claim_slot {self._claim_slot} out of range for claim_slots={self._claim_slots}"
            )

        self._fd = -1
        self._mm = None
        self._buf = None
        self._base = 0

        self._claim_lock = threading.Lock()
        self._tls = threading.local()
        # channel -> the open file description whose OFD lock is our claim.
        self._claim_fds: dict[int, int] = {}
        # Channels taken over from dead clients.
        self.reclaimed = 0
        # Pins a previous owner left on channels this ring claimed (see _my_channel).
        self.pins_released_on_claim = 0
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
                self._fd = os.open(self._path, os.O_RDWR | os.O_CLOEXEC)
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
        # Claim locks live one byte per channel past the mapped region.
        self._lock_base = total
        _LIVE_RINGS.add(self)

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

    def server_replaced(self) -> int | None:
        """The generation of a different, ready server now at this mailbox's
        path, or ``None``: same server, or none ready there (stopped, or still
        starting). A restarted server recreates the file as a new inode, so this
        client's mapping is then orphaned and no request on it is answered."""
        try:
            fd = os.open(self._path, os.O_RDONLY | os.O_CLOEXEC)
        except OSError:
            return None
        try:
            hdr = os.pread(fd, _HEADER_SIZE, 0)
        except OSError:
            return None
        finally:
            os.close(fd)
        if len(hdr) < _HDR_GENERATION + 4:
            return None
        magic, _abi, gen = struct.unpack_from("<III", hdr, 0)
        if magic != MAGIC_READY or gen == self._generation:
            return None
        return gen

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
        """Claim a channel: one whose OFD lock no live process holds.

        The lock (one byte past the mapped region, per channel) is the claim, so
        a channel whose owner died -- however it died -- is free, and is taken
        over: the new owner continues the channel's sequence after the dead
        owner's last request has been answered. Each claim uses its own open file
        description, since locks of one description do not conflict with each
        other. The TP>1 slot partition still keeps one engine's processes in
        disjoint slices."""
        tid = (threading.get_ident() * 2654435761) & 0xFFFFFFFF or 1
        per = max(1, self._num_channels // self._claim_slots)
        lo = self._claim_slot * per
        hi = self._num_channels if self._claim_slot == self._claim_slots - 1 else lo + per
        with self._claim_lock:
            for ch in range(lo, hi):
                if ch in self._claim_fds:
                    continue  # ours: a probe from our own process would succeed
                fd = os.open(f"/proc/self/fd/{self._fd}", os.O_RDWR | os.O_CLOEXEC)
                if not _ofd_lock(fd, self._lock_base + ch):
                    os.close(fd)  # a live owner
                    continue
                owner_off = self._channel_base(ch) + OFF_OWNER
                previous = self._rd_u32(owner_off)
                self._wr_u32(owner_off, tid)
                seq = self._adopt_seq(ch, TAKEOVER_TIMEOUT_S)
                if seq is None:
                    # The dead owner's request is still unanswered: unusable
                    # until the server replies. Leave it.
                    self._wr_u32(owner_off, previous)
                    os.close(fd)
                    continue
                self._seqs[ch] = seq
                if previous:
                    self.reclaimed += 1
                    self._log(f"reclaimed shmq channel {ch} from a dead client")
                self._claim_fds[ch] = fd
                return ch
        raise RingError(
            f"no free shmq channel in slot {self._claim_slot}/{self._claim_slots} "
            f"range [{lo},{hi}) of {self._num_channels}"
        )

    def _adopt_seq(self, ch: int, timeout: float) -> int | None:
        """Wait until channel ``ch``'s last request (if any) has been answered;
        return that seq, the base to continue from. None after ``timeout``."""
        base = self._channel_base(ch)
        rc = base + self._resp_control_off
        deadline = time.monotonic() + timeout
        while True:
            req = self._rd_u32(base + OFF_SEQ)
            resp = self._rd_u32(rc + OFF_SEQ)
            if req == resp:
                return req
            if time.monotonic() > deadline:
                return None
            self._futex_wait(self._base + rc + OFF_SEQ, resp, 0.01)

    def _forget_claims_after_fork(self) -> None:
        for fd in self._claim_fds.values():
            try:
                os.close(fd)
            except OSError:
                pass
        self._claim_fds = {}
        self._tls = threading.local()

    def _my_channel(self) -> int:
        ch = getattr(self._tls, "channel", None)
        if ch is None:
            ch = self._claim_channel()
            self._tls.channel = ch
            # Pins belong to the channel lease: whatever a previous owner left
            # (a dead one's, or one in flight when it died -- the claim waited
            # for that reply) is released before this owner's first request.
            self.pins_released_on_claim += self._unpin_all_on(ch)
        return ch

    def _unpin_all_on(self, ch: int) -> int:
        """UNPIN-all on channel ``ch`` (this thread's): the server releases
        every pin recorded on it. Returns the number released."""
        status, payload = self._request_on(ch, OP_UNPIN, struct.pack("<I", UNPIN_ALL))
        if status != STATUS_OK:
            raise RingError(
                f"server error (op={OP_UNPIN}, unpin-all): {payload.decode('utf-8', 'replace')}"
            )
        return decode_u64(payload)

    def release_channel(self) -> None:
        """Release this thread's claimed channel back to the free pool.

        Channels are claimed sticky-per-thread on first use (see
        :meth:`_my_channel`) and normally held for the life of the thread. A
        caller that churns threads — a ``ThreadPoolExecutor`` recreated per
        phase, or fresh worker threads per benchmark round — must release before
        the thread exits, otherwise the claim is held until this process exits
        (only a dead process's claims are reclaimed, not a dead thread's).

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
        # Pins end with the lease. Best-effort: a dead server holds none, and
        # the next claimer of this channel releases them regardless.
        try:
            self._unpin_all_on(ch)
        except RingError:
            pass
        with self._claim_lock:
            # Word first, then the lock: a claimer that gets the lock must not
            # see our id and count a clean release as a takeover.
            self._wr_u32(self._channel_base(ch) + OFF_OWNER, 0)
            fd = self._claim_fds.pop(ch, None)
            if fd is not None:
                os.close(fd)
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
        return self._request_on(self._my_channel(), opcode, data)

    def _request_on(self, ch: int, opcode: int, data: bytes) -> tuple[int, bytes]:
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
        beat, beat_since = self.heartbeat(), start
        while True:
            cur = resp_seq.value
            if cur == seq:
                return self._read_response(rc)
            now = time.monotonic()
            b = self.heartbeat()
            if b != beat:
                beat, beat_since = b, now
            elif now - beat_since >= QUIET_BEFORE_PROBE_S:
                # The poller never sleeps, so a heartbeat that stands still
                # means a dead or wedged server. Look once per interval for a
                # server that replaced it: then no reply can ever come here.
                beat_since = now
                gen = self.server_replaced()
                if gen is not None:
                    raise ServerRestartedError(
                        f"certus server at {self._path!r} restarted (generation "
                        f"{self._generation:#x} -> {gen:#x}); this mailbox is orphaned. "
                        "Restart this engine to attach to the new server."
                    )
            if now - start > self._deadline:
                raise RingError(
                    f"shmq request deadline exceeded after {self._deadline:g}s "
                    "(server dead, or tier saturated and backpressuring longer "
                    "than the deadline — raise CERTUS_SHMQ_DEADLINE_S)"
                )
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

    def check_states(self, keys: Sequence[int]) -> list[int]:
        """Tri-state Check: per key, ``CHECK_MISS`` / ``CHECK_RESIDENT`` /
        ``CHECK_PENDING`` (a store in flight). This is the full-fidelity probe;
        ``check()`` collapses it to a bool for callers that only need existence."""
        keys = list(keys)
        if not keys:
            return []
        return decode_states(self._dispatch(OP_CHECK, encode_keys(keys)), len(keys))

    def check(self, keys: Sequence[int]) -> list[bool]:
        # Existence view over check_states: a pending key counts as present, so
        # store-dedup does not re-store a block already being written.
        return [s != CHECK_MISS for s in self.check_states(keys)]

    def touch(self, keys: Sequence[int], promote: bool = False) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        return decode_ok_flags(
            self._dispatch(OP_TOUCH, encode_promote_keys(promote, keys)), len(keys)
        )

    # Cleared the first time a server rejects TouchCheck as an unknown opcode, so
    # every later touch_states() on this ring goes straight to the fallback.
    _touch_check_supported = True

    def touch_states(self, keys: Sequence[int], promote: bool = False) -> list[int]:
        """Fused Touch + tri-state Check in one round trip: touches every key and
        returns its ``CHECK_*`` state (a failed touch reads as ``CHECK_MISS``).

        A server that predates TouchCheck answers it with ``unknown opcode``;
        this then falls back to separate Touch and Check round trips with the
        same per-key result, and stays on the fallback for this ring."""
        keys = list(keys)
        if not keys:
            return []
        data = encode_promote_keys(promote, keys)
        if self._touch_check_supported:
            status, payload = self.request(OP_TOUCH_CHECK, data)
            if status == STATUS_OK:
                return decode_states(payload, len(keys))
            msg = payload.decode("utf-8", "replace")
            if not msg.startswith(f"unknown opcode {OP_TOUCH_CHECK}"):
                raise RingError(f"server error (op={OP_TOUCH_CHECK}): {msg}")
            self._touch_check_supported = False
        touched = decode_ok_flags(self._dispatch(OP_TOUCH, data), len(keys))
        states = self.check_states(keys)
        # Same mapping as the server's fused op: PENDING wins, RESIDENT needs a
        # successful touch, anything else is a miss.
        return [
            s if s == CHECK_PENDING or (t and s == CHECK_RESIDENT) else CHECK_MISS
            for t, s in zip(touched, states)
        ]

    # Cleared the first time a server rejects Fetch as an unknown opcode: an old
    # server cannot fetch, which is the same as one with no endpoint.
    _fetch_supported = True

    def fetch(self, peer: str, entries: Sequence[tuple[int, int]]) -> tuple[int, list[bool]]:
        """Fetch ``entries`` ``[(key, size)]`` — a leading miss run, in prefix
        order — from the transfer endpoint ``peer`` ("host:port") into this
        server's memory tier. Returns ``(status, started)``. Never waits for the
        transfer: a started key reads ``CHECK_PENDING`` until it is
        ``CHECK_RESIDENT`` (fetched) or ``CHECK_MISS``."""
        entries = list(entries)
        if not entries:
            return FETCH_OK, []
        if not self._fetch_supported:
            return FETCH_NO_ENDPOINT, [False] * len(entries)
        status, payload = self.request(OP_FETCH, encode_fetch(peer, entries))
        if status != STATUS_OK:
            msg = payload.decode("utf-8", "replace")
            if msg.startswith(f"unknown opcode {OP_FETCH}"):
                self._fetch_supported = False
                return FETCH_NO_ENDPOINT, [False] * len(entries)
            raise RingError(f"server error (op={OP_FETCH}): {msg}")
        return decode_fetch(payload, len(entries))

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
        """Release pins this thread's channel took. A key pinned on another
        channel (another thread) is refused: pin and unpin on one thread."""
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

    def copy_to_store_blocks(
        self, table: bytes, keys: Sequence[int], block_ids: Sequence[int], timings=None
    ) -> list[bool]:
        """CopyToStoreBlocks: ``table`` from :func:`encode_layer_table`, one block
        id per key. Chunked by key count to fit cap_req; results in key order.
        If ``timings`` is a list, the ns spent building request bytes is appended."""
        return self._block_batch_op(OP_COPY_TO_STORE_BLOCKS, table, keys, block_ids, timings)

    def lookup_blocks(
        self, table: bytes, keys: Sequence[int], block_ids: Sequence[int], timings=None
    ) -> list[bool]:
        """LookupBlocks: ``table`` from :func:`encode_layer_table`, one block id
        per key. Chunked by key count to fit cap_req; hit flags in key order.
        If ``timings`` is a list, the ns spent building request bytes is appended."""
        return self._block_batch_op(OP_LOOKUP_BLOCKS, table, keys, block_ids, timings)

    def _block_batch_op(
        self, opcode: int, table: bytes, keys, block_ids, timings=None
    ) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        block_ids = list(block_ids)
        per_chunk = (self._cap_req - len(table) - 4) // _BLOCK_ENTRY_SIZE
        if per_chunk < 1:
            raise RingError(
                f"layer table of {len(table)} B leaves no room for a block in "
                f"cap_req {self._cap_req} B"
            )
        n = len(keys)
        results: list[bool] = []
        encode_ns = 0
        for i in range(0, n, per_chunk):
            ck = keys[i:i + per_chunk]
            if timings is not None:
                t0 = time.perf_counter_ns()
            data = table + encode_block_entries(ck, block_ids[i:i + per_chunk])
            if timings is not None:
                encode_ns += time.perf_counter_ns() - t0
            results.extend(decode_ok_flags(self._dispatch(opcode, data), len(ck)))
        if timings is not None:
            timings.append(encode_ns)
        return results

    def copy_to_store_chunks(self, table: bytes, scratch_idx: int, blocks_per_key: int,
                             keys: Sequence[int], block_ids: Sequence[int], timings=None) -> list[bool]:
        """CopyToStoreChunks: ``table`` from :func:`encode_layer_table`,
        ``blocks_per_key`` block ids per key (see :func:`encode_chunk_entries`).
        Chunked by key count to fit cap_req; results in key order."""
        return self._chunk_batch_op(OP_COPY_TO_STORE_CHUNKS, table, scratch_idx,
                                    blocks_per_key, keys, block_ids, timings)

    def lookup_chunks(self, table: bytes, scratch_idx: int, blocks_per_key: int,
                      keys: Sequence[int], block_ids: Sequence[int], timings=None) -> list[bool]:
        """LookupChunks: as :meth:`copy_to_store_chunks`; hit flags in key order."""
        return self._chunk_batch_op(OP_LOOKUP_CHUNKS, table, scratch_idx,
                                    blocks_per_key, keys, block_ids, timings)

    def _chunk_batch_op(self, opcode: int, table: bytes, scratch_idx: int, blocks_per_key: int,
                        keys, block_ids, timings=None) -> list[bool]:
        keys = list(keys)
        if not keys:
            return []
        block_ids = list(block_ids)
        per_chunk = (self._cap_req - len(table) - 10) // (8 + 8 * blocks_per_key)
        if per_chunk < 1:
            raise RingError(
                f"layer table of {len(table)} B leaves no room for a key of "
                f"{blocks_per_key} blocks in cap_req {self._cap_req} B"
            )
        results: list[bool] = []
        encode_ns = 0
        for i in range(0, len(keys), per_chunk):
            ck = keys[i:i + per_chunk]
            if timings is not None:
                t0 = time.perf_counter_ns()
            data = table + encode_chunk_entries(
                scratch_idx, blocks_per_key, ck,
                block_ids[i * blocks_per_key:(i + len(ck)) * blocks_per_key])
            if timings is not None:
                encode_ns += time.perf_counter_ns() - t0
            results.extend(decode_ok_flags(self._dispatch(opcode, data), len(ck)))
        if timings is not None:
            timings.append(encode_ns)
        return results

    def take_events(self, max_events: int = 0) -> tuple[list[tuple[int, int]], int]:
        # The response is serialized into the fixed cap_resp slot as
        # { n:u32, [key:u64, reason:u32]*n, dropped:u64 }. An unlimited drain
        # (max_events=0) can produce more events than that slot holds: the server
        # writes the true count but the transport truncates the bytes to cap_resp,
        # and the decoder then reads past the end. Bound an unlimited (or too-large)
        # request to the number of 12-byte event records that provably fit. The
        # server honors the cap (translate.rs op_take_events) and leaves the
        # remainder QUEUED for the next drain — lossless, not counted as dropped.
        # Under TP>1 each logical block is W per-rank keys, so evictions multiply
        # and routinely exceed the slot; this bound is what keeps the engine alive.
        fit = max(1, (self._cap_resp - 4 - 8) // 12)
        if max_events <= 0 or max_events > fit:
            max_events = fit
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
        """Evict the whole memory tier (entries with an SSD copy stay there).
        Returns the number of entries cleared."""
        return decode_u64(self._dispatch(OP_CLEAR_MEMORY_TIER, b""))

    def clear_all(self) -> tuple[int, int, int]:
        """Remove every entry from both tiers: the cache is empty afterwards.
        Returns ``(removed, still_pinned, fetch_pending)``: entries removed,
        entries a pin kept, and keys of fetches still in flight (they land after
        the clear)."""
        payload = self._dispatch(OP_CLEAR_ALL, b"")
        return struct.unpack_from("<QQQ", payload)

    def flush_to_ssd(self) -> int:
        """Drain pending write-through jobs. Returns the number flushed."""
        return decode_u64(self._dispatch(OP_FLUSH_TO_SSD, b""))

    def get_io_stats(self) -> dict[str, int]:
        """Cumulative SSD read/write counters (see :data:`IO_STATS_FIELDS`)."""
        return decode_io_stats(self._dispatch(OP_GET_IO_STATS, b""))

    # ── teardown ──

    def close(self) -> None:
        # Release this thread's channel first, while the mapping is still valid,
        # then every other claim this ring holds: other threads' channels go when
        # their descriptions close, as they would at process exit.
        self.release_channel()
        atexit.unregister(self._release_at_exit)  # explicit close: drop the hook
        with self._claim_lock:
            for ch, fd in self._claim_fds.items():
                if self._base:
                    self._wr_u32(self._channel_base(ch) + OFF_OWNER, 0)
                os.close(fd)
            self._claim_fds = {}
        _LIVE_RINGS.discard(self)
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
