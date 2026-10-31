# SPDX-License-Identifier: Apache-2.0
"""Byte-for-byte parity tests for the ring.py wire encoders/decoders.

These exercise the pure request-encoders and response-decoders in isolation
(no shared memory, no server) and assert the exact little-endian framing that
``lib/shmq-dispatcher/src/{wire.rs,translate.rs}`` produce and consume.
The framing is the fragile seam between the Rust server and this Python client:
a one-byte offset drift here is silent corruption, so every op that ring.py
speaks gets a round-trip test whose expected bytes are spelled out by hand
rather than re-derived from the same encoder under test.

Ops 11–15 (Populate/Remove/ClearMemoryTier/FlushToSsd/GetIoStats) were added to
reach full IDispatcher parity when gRPC was removed; they are covered here
alongside the older ops they reuse framing from.
"""

from __future__ import annotations

import struct

import pytest

import certus_shmq_connector.ring as ring
from certus_shmq_connector.ring import (
    CHECK_MISS,
    CHECK_PENDING,
    CHECK_RESIDENT,
    IO_STATS_FIELDS,
    OP_CLEAR_MEMORY_TIER,
    OP_FLUSH_TO_SSD,
    OP_GET_IO_STATS,
    OP_POPULATE,
    OP_REMOVE,
    OP_TOUCH_CHECK,
    decode_io_stats,
    decode_ok_flags,
    decode_states,
    decode_u64,
    encode_handle_batch,
    encode_keys,
    encode_promote_keys,
)


# ── opcode numbering must match wire.rs exactly ──────────────────────────────


def test_new_opcodes_have_the_wire_values():
    # wire.rs: POPULATE=11, REMOVE=12, CLEAR_MEMORY_TIER=13, FLUSH_TO_SSD=14,
    # GET_IO_STATS=15, TOUCH_CHECK=16.
    assert (OP_POPULATE, OP_REMOVE, OP_CLEAR_MEMORY_TIER, OP_FLUSH_TO_SSD,
            OP_GET_IO_STATS, OP_TOUCH_CHECK) == (11, 12, 13, 14, 15, 16)


# ── Check: req is a key list; resp is `[state:u8]*n` (0=miss/1=resident/2=pend)─


def test_check_state_constants_match_wire_rs():
    # wire.rs check_state: MISS=0, RESIDENT=1, PENDING=2. These bytes are the
    # server's op_check response alphabet; a drift here silently remaps hits.
    assert (CHECK_MISS, CHECK_RESIDENT, CHECK_PENDING) == (0, 1, 2)


def test_decode_states_reads_one_byte_per_key_in_order():
    # server writes exactly one state byte per requested key, in key order.
    payload = bytes([CHECK_RESIDENT, CHECK_MISS, CHECK_PENDING])
    assert decode_states(payload, 3) == [1, 0, 2]


def test_decode_states_pads_short_payload_with_miss():
    # A truncated response must never invent hits: missing tail bytes read MISS,
    # so a lost/short frame degrades to "not present" rather than a false HIT.
    assert decode_states(bytes([CHECK_RESIDENT]), 3) == [1, 0, 0]
    assert decode_states(b"", 2) == [0, 0]


def test_decode_states_backward_compatible_with_bool_check():
    # The legacy exists-view is exactly `state != MISS`; pending counts as present
    # for store-dedup, resident counts as present, only miss is absent.
    payload = bytes([CHECK_MISS, CHECK_RESIDENT, CHECK_PENDING])
    states = decode_states(payload, 3)
    assert [s != CHECK_MISS for s in states] == [False, True, True]


# ── Remove: req `{ n:u32, [key:u64]*n }` (shared encode_keys) resp `[ok:u8]*n`─


def test_remove_request_is_a_key_list():
    keys = [1, 2, 0xFFFF_FFFF_FFFF_FFFF]
    blob = encode_keys(keys)
    assert blob == struct.pack("<I", 3) + struct.pack("<QQQ", *keys)


def test_remove_response_decodes_per_key_flags():
    # server writes one u8 per key, in order.
    payload = bytes([1, 0, 1])
    assert decode_ok_flags(payload, 3) == [True, False, True]


# ── ClearMemoryTier / FlushToSsd: empty req, resp `{ u64 }` ──────────────────


def test_decode_u64_reads_little_endian_first_word():
    assert decode_u64(struct.pack("<Q", 4242)) == 4242
    # Trailing bytes (if any) are ignored — only the first u64 is the count.
    assert decode_u64(struct.pack("<Q", 7) + b"\xff\xff") == 7


# ── GetIoStats: empty req, resp is 6×u64 in a fixed order ────────────────────


def test_io_stats_field_order_matches_translate_rs():
    # translate.rs op_get_io_stats writes exactly this order.
    assert IO_STATS_FIELDS == (
        "read_ops",
        "read_bytes",
        "read_latency_ns_sum",
        "write_ops",
        "write_bytes",
        "write_latency_ns_sum",
    )


def test_decode_io_stats_maps_six_u64s_by_position():
    vals = (10, 20, 30, 40, 50, 60)
    payload = struct.pack("<6Q", *vals)
    stats = decode_io_stats(payload)
    assert stats == {
        "read_ops": 10,
        "read_bytes": 20,
        "read_latency_ns_sum": 30,
        "write_ops": 40,
        "write_bytes": 50,
        "write_latency_ns_sum": 60,
    }


# ── Populate: req is a single-region HandleBatch (reuses encode_handle_batch) ─


def _decode_handle_batch(blob: bytes):
    """Independent mini-parser mirroring translate.rs decode_handle_batch, so a
    round-trip is checked against a *different* implementation than the encoder."""
    off = 0
    (n_handles,) = struct.unpack_from("<I", blob, off)
    off += 4
    handles = []
    for _ in range(n_handles):
        hb = blob[off:off + 64]
        off += 64
        (dev,) = struct.unpack_from("<i", blob, off)
        off += 4
        handles.append((hb, dev))
    (n_entries,) = struct.unpack_from("<I", blob, off)
    off += 4
    entries = []
    for _ in range(n_entries):
        (key,) = struct.unpack_from("<Q", blob, off)
        off += 8
        (nreg,) = struct.unpack_from("<H", blob, off)
        off += 2
        regions = []
        for _ in range(nreg):
            idx, roff, size = struct.unpack_from("<IQI", blob, off)
            off += 16
            regions.append((idx, roff, size))
        entries.append((key, regions))
    return handles, entries, off


def test_populate_handle_batch_round_trips_with_one_region_per_key():
    h0 = bytes(range(64))
    entries = [
        (100, [(h0, 3, 0, 4096)]),
        (200, [(h0, 3, 4096, 4096)]),  # same handle -> deduped into one table row
    ]
    blob = encode_handle_batch(entries)
    handles, decoded_entries, consumed = _decode_handle_batch(blob)

    assert consumed == len(blob)  # no trailing/garbage bytes
    assert handles == [(h0, 3)]  # single deduped handle-table row
    assert decoded_entries == [
        (100, [(0, 0, 4096)]),
        (200, [(0, 4096, 4096)]),
    ]


def test_populate_handle_must_be_64_bytes():
    from certus_shmq_connector.ring import RingError

    with pytest.raises(RingError):
        encode_handle_batch([(1, [(b"\x00" * 32, 0, 0, 4096)])])


# ── bulk key codec: byte-identical to the old per-key encoders/decoder ───────

# Reference copies of the pre-bulk per-key implementations. The bulk helpers in
# ring.py must reproduce these bytes/lists exactly for every input.


def _old_encode_keys(keys):
    out = bytearray(struct.pack("<I", len(keys)))
    for k in keys:
        out += struct.pack("<Q", k & 0xFFFFFFFFFFFFFFFF)
    return bytes(out)


def _old_encode_promote_keys(promote, keys):
    return struct.pack("<B", 1 if promote else 0) + _old_encode_keys(keys)


def _old_decode_states(payload, n):
    return [payload[i] if i < len(payload) else CHECK_MISS for i in range(n)]


_CODEC_SIZES = [0, 1, 2, 63, 64, 65, 4096]


def _key_lists():
    import random

    rnd = random.Random(20261003)
    for n in _CODEC_SIZES:
        yield [rnd.getrandbits(64) for _ in range(n)]
        if n:
            # Edge values at both ends of u64, placed at the ends of the list.
            keys = [rnd.getrandbits(64) for _ in range(n)]
            keys[0] = 0
            keys[-1] = 2**64 - 1
            yield keys


@pytest.mark.parametrize("promote", [False, True])
def test_bulk_encoders_match_per_key_reference(promote):
    for keys in _key_lists():
        assert encode_keys(keys) == _old_encode_keys(keys)
        assert encode_promote_keys(promote, keys) == _old_encode_promote_keys(promote, keys)


@pytest.mark.parametrize("promote", [False, True])
def test_bulk_encoders_mask_out_of_range_ints_like_reference(promote):
    # Negative and >= 2**64 ints take the masked retry; bytes must match the
    # old per-key `k & mask` exactly.
    for n in _CODEC_SIZES[1:]:
        keys = [7] * n
        keys[0] = -1
        keys[-1] = 2**64 + 5
        keys[n // 2] = -(2**70) + 3
        assert encode_keys(keys) == _old_encode_keys(keys)
        assert encode_promote_keys(promote, keys) == _old_encode_promote_keys(promote, keys)


def test_bulk_encoders_reject_float_key_with_type_error():
    with pytest.raises(TypeError):
        _old_encode_keys([1, 2.0])
    with pytest.raises(TypeError):
        encode_keys([1, 2.0])
    with pytest.raises(TypeError):
        encode_promote_keys(False, [1, 2.0])


def test_bulk_decode_states_matches_per_key_reference():
    import random

    rnd = random.Random(7)
    for n in _CODEC_SIZES:
        for plen in sorted({0, max(0, n - 1), n // 2, n, n + 1, n + 64}):
            payload = bytes(rnd.choice((CHECK_MISS, CHECK_RESIDENT, CHECK_PENDING)) for _ in range(plen))
            got = decode_states(payload, n)
            assert got == _old_decode_states(payload, n)
            assert len(got) == n
            assert all(type(s) is int for s in got)


# ── presence: the bulk codec is what ring.py actually runs ───────────────────


def test_presence_bulk_encoders_make_one_pack_call(monkeypatch):
    import certus_shmq_connector.ring as ring

    assert ring._U64_MASK == 2**64 - 1

    class _CountingStruct:
        error = struct.error

        def __init__(self):
            self.calls = 0

        def pack(self, *args):
            self.calls += 1
            return struct.pack(*args)

    counter = _CountingStruct()
    monkeypatch.setattr(ring, "struct", counter)
    keys = list(range(1, 65))
    for encode in (encode_keys, lambda ks: encode_promote_keys(True, ks)):
        counter.calls = 0
        encode(keys)
        assert counter.calls == 1
        bad = list(keys)
        bad[10] = 2**64 + 1
        counter.calls = 0
        encode(bad)
        assert counter.calls == 2  # masked retry


def test_presence_bulk_decode_states_slices_only():
    class _SliceOnly:
        def __init__(self, data):
            self._data = data

        def __getitem__(self, idx):
            if isinstance(idx, slice):
                return self._data[idx]
            raise AssertionError("decode_states must not index per byte")

        def __len__(self):
            raise AssertionError("decode_states must not call len(payload)")

    data = bytes([CHECK_RESIDENT, CHECK_PENDING, CHECK_MISS] * 22)  # 66 bytes
    for n in (len(data) + 5, len(data), len(data) - 7):
        assert decode_states(_SliceOnly(data), n) == decode_states(data, n)


# ── TouchCheck fallback for servers that predate opcode 16 ──────────────────


class _ScriptedRing(ring.Ring):
    """A Ring with no shared memory: `request` replays scripted replies and
    records the opcodes it was asked to send."""

    def __init__(self, replies):
        self._replies = list(replies)
        self.sent = []

    def request(self, opcode, data):
        self.sent.append(opcode)
        return self._replies.pop(0)


_STATUS_ERROR = 1


def test_touch_states_uses_fused_op_on_a_current_server():
    r = _ScriptedRing([(ring.STATUS_OK, bytes([CHECK_RESIDENT, CHECK_MISS]))])
    assert r.touch_states([7, 8]) == [CHECK_RESIDENT, CHECK_MISS]
    assert r.sent == [OP_TOUCH_CHECK]


def test_touch_states_falls_back_to_touch_then_check_on_an_old_server():
    unknown = (_STATUS_ERROR, f"unknown opcode {OP_TOUCH_CHECK}".encode())
    # Keys: resident+touched, resident but touch failed, pending with touch
    # failed, miss — the fused op's mapping gives RESIDENT, MISS, PENDING, MISS.
    touch = (ring.STATUS_OK, bytes([1, 0, 0, 1]))
    check = (ring.STATUS_OK, bytes([CHECK_RESIDENT, CHECK_RESIDENT, CHECK_PENDING, CHECK_MISS]))
    r = _ScriptedRing([unknown, touch, check, touch, check])

    expected = [CHECK_RESIDENT, CHECK_MISS, CHECK_PENDING, CHECK_MISS]
    assert r.touch_states([1, 2, 3, 4]) == expected
    assert r.sent == [OP_TOUCH_CHECK, ring.OP_TOUCH, ring.OP_CHECK]

    # The rejection is remembered: the next call skips the fused op entirely.
    r.sent.clear()
    assert r.touch_states([1, 2, 3, 4]) == expected
    assert r.sent == [ring.OP_TOUCH, ring.OP_CHECK]


def test_touch_states_raises_other_server_errors():
    r = _ScriptedRing([(_STATUS_ERROR, b"duplicate key 5")])
    with pytest.raises(ring.RingError, match="duplicate key 5"):
        r.touch_states([5, 5])
    assert r._touch_check_supported


# ── Fetch (17): mirrors wire.rs and translate.rs op_fetch ───────────────────


def test_fetch_opcode_and_statuses_match_wire_rs():
    assert ring.OP_FETCH == 17
    assert (ring.FETCH_OK, ring.FETCH_NO_ENDPOINT, ring.FETCH_BAD_PEER, ring.FETCH_TOO_MANY) == (
        0, 1, 2, 3)


def test_fetch_request_layout():
    import struct

    b = ring.encode_fetch("10.0.0.5:7481", [(1, 4096), (2**64 - 1, 65536)])
    assert b[:2] == struct.pack("<H", 13) and b[2:15] == b"10.0.0.5:7481"
    assert struct.unpack_from("<I", b, 15) == (2,)
    assert struct.unpack_from("<QIQI", b, 19) == (1, 4096, 2**64 - 1, 65536)
    assert len(b) == 19 + 2 * 12


def test_fetch_response_decodes_only_when_ok():
    assert ring.decode_fetch(bytes([0, 1, 1, 0]), 3) == (0, [True, True, False])
    assert ring.decode_fetch(bytes([1]), 2) == (1, [False, False])
    assert ring.decode_fetch(b"", 2) == (ring.FETCH_NO_ENDPOINT, [False, False])


def test_fetch_on_an_old_server_is_no_endpoint_and_remembered():
    unknown = (_STATUS_ERROR, f"unknown opcode {ring.OP_FETCH}".encode())
    r = _ScriptedRing([unknown])
    assert r.fetch("h:1", [(1, 8)]) == (ring.FETCH_NO_ENDPOINT, [False])
    assert r.fetch("h:1", [(2, 8)]) == (ring.FETCH_NO_ENDPOINT, [False])
    assert r.sent == [ring.OP_FETCH], "not asked again"


# ── BlockBatch (LookupBlocks / CopyToStoreBlocks): layer table + block ids ───


def test_block_opcodes_have_the_wire_values():
    # wire.rs: LOOKUP_BLOCKS=17, COPY_TO_STORE_BLOCKS=18.
    assert (ring.OP_LOOKUP_BLOCKS, ring.OP_COPY_TO_STORE_BLOCKS) == (19, 20)


def _decode_block_batch(blob: bytes):
    """Independent mini-parser mirroring translate.rs decode_block_batch: returns
    (handles, entries expanded through the layer formula, bytes consumed)."""
    off = 0
    (n_handles,) = struct.unpack_from("<I", blob, off)
    off += 4
    handles = []
    for _ in range(n_handles):
        hb = blob[off:off + 64]
        off += 64
        (dev,) = struct.unpack_from("<i", blob, off)
        off += 4
        handles.append((hb, dev))
    (n_layers,) = struct.unpack_from("<H", blob, off)
    off += 2
    layers = []
    for _ in range(n_layers):
        layers.append(struct.unpack_from("<IQQI", blob, off))
        off += 24
    (n,) = struct.unpack_from("<I", blob, off)
    off += 4
    keys = struct.unpack_from(f"<{n}Q", blob, off)
    off += 8 * n
    block_ids = struct.unpack_from(f"<{n}Q", blob, off)
    off += 8 * n
    entries = [
        (k, [(idx, base + b * stride, size) for idx, base, stride, size in layers])
        for k, b in zip(keys, block_ids)
    ]
    return handles, entries, off


def _kv(tag: int, dev: int, stride: int, base_delta: int):
    from certus_shmq_connector.gpu import KvCacheIpc

    return KvCacheIpc(handle_bytes=bytes([tag]) * 64, gpu_device_id=dev,
                      stride_bytes=stride, base_delta=base_delta)


def _layer_rows(regions):
    # The handler's table rows: size == stride, as _regions_for_block emits.
    return [(r.handle_bytes, r.gpu_device_id, r.base_delta, r.stride_bytes, r.stride_bytes)
            for r in regions]


_BLOCK_CASES = [
    # 32 per-layer tensors, non-contiguous ids.
    ([_kv(i, 0, 65536, 512 * i) for i in range(32)], [9, 3, 40, 41, 1000]),
    # Single region (p1), reversed ids.
    ([_kv(7, 1, 2 << 20, 0)], [5, 4, 3, 2, 1, 0]),
    # Several layers sharing one handle; ids near the int32 limit.
    ([_kv(6, 0, 4096, 0), _kv(6, 0, 4096, 1024), _kv(8, 0, 4096, 64)],
     [2**31 - 1, 17, 2**31 - 2]),
]


@pytest.mark.parametrize("regions,block_ids", _BLOCK_CASES)
def test_block_batch_expands_to_the_legacy_handle_batch(regions, block_ids):
    from certus_shmq_connector.handler import _regions_for_block
    from certus_shmq_connector.ring import encode_block_entries, encode_layer_table

    keys = [1000 + i for i in range(len(block_ids))]
    blob = encode_layer_table(_layer_rows(regions)) + encode_block_entries(keys, block_ids)
    handles, entries, consumed = _decode_block_batch(blob)
    assert consumed == len(blob)

    legacy = encode_handle_batch(
        [(k, _regions_for_block(regions, b)) for k, b in zip(keys, block_ids)]
    )
    ref_handles, ref_entries, _ = _decode_handle_batch(legacy)
    assert handles == ref_handles  # same deduped table, same order
    assert entries == ref_entries


def test_layer_table_rejects_bad_handles_and_empty_tables():
    from certus_shmq_connector.ring import RingError, encode_layer_table

    with pytest.raises(RingError):
        encode_layer_table([(b"\x00" * 32, 0, 0, 4096, 4096)])
    with pytest.raises(RingError):
        encode_layer_table([])


def test_block_entries_mask_keys_and_reject_negative_block_ids():
    from certus_shmq_connector.ring import encode_block_entries

    blob = encode_block_entries([-1, 2**64 + 5], [3, 4])
    assert blob == struct.pack("<I2Q2Q", 2, 2**64 - 1, 5, 3, 4)
    with pytest.raises(struct.error):
        encode_block_entries([1], [-1])


class _BlockScriptedRing(ring.Ring):
    """A Ring with no shared memory and a fixed cap_req: `request` records each
    payload and answers one ok byte per key it carries."""

    def __init__(self, cap_req):
        self._cap_req = cap_req
        self.sent = []

    def request(self, opcode, data):
        self.sent.append((opcode, data))
        (n,) = struct.unpack_from("<I", data, self.table_len)
        return ring.STATUS_OK, bytes([1]) * n


def test_block_batch_chunks_by_key_count():
    from certus_shmq_connector.ring import encode_block_entries, encode_layer_table

    table = encode_layer_table(_layer_rows(_BLOCK_CASES[0][0]))
    per_chunk = 5
    r = _BlockScriptedRing(len(table) + 4 + 16 * per_chunk + 7)
    r.table_len = len(table)
    keys = list(range(100, 112))
    ids = list(range(50, 38, -1))
    timings = []
    assert r.lookup_blocks(table, keys, ids, timings=timings) == [True] * 12
    assert len(timings) == 1
    assert [op for op, _ in r.sent] == [ring.OP_LOOKUP_BLOCKS] * 3
    assert [d for _, d in r.sent] == [
        table + encode_block_entries(keys[i:i + 5], ids[i:i + 5]) for i in (0, 5, 10)
    ]
    assert all(len(d) <= r.cap_req for _, d in r.sent)

    r.sent.clear()
    assert r.copy_to_store_blocks(table, keys[:5], ids[:5]) == [True] * 5
    assert [op for op, _ in r.sent] == [ring.OP_COPY_TO_STORE_BLOCKS]


def test_block_batch_rejects_a_table_larger_than_cap_req():
    from certus_shmq_connector.ring import RingError, encode_layer_table

    table = encode_layer_table(_layer_rows(_BLOCK_CASES[0][0]))
    r = _BlockScriptedRing(len(table) + 4 + 15)
    with pytest.raises(RingError):
        r.lookup_blocks(table, [1], [0])


def test_reserve_refuses_a_size_past_u32():
    from certus_shmq_connector.ring import encode_reserve

    encode_reserve([(1, 0xFFFFFFFF, 0)])
    with pytest.raises(ValueError):
        encode_reserve([(1, 1 << 32, 0)])


def test_slot_size_past_u32_is_refused():
    from certus_shmq_connector.spec import _check_slot_size

    _check_slot_size(2 << 20, 2047)
    with pytest.raises(ValueError):
        _check_slot_size(2 << 20, 2048)
