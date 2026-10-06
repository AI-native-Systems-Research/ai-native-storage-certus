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
    decode_io_stats,
    decode_ok_flags,
    decode_states,
    decode_u64,
    encode_handle_batch,
    encode_keys,
)


# ── opcode numbering must match wire.rs exactly ──────────────────────────────


def test_new_opcodes_have_the_wire_values():
    # wire.rs: POPULATE=11, REMOVE=12, CLEAR_MEMORY_TIER=13, FLUSH_TO_SSD=14,
    # GET_IO_STATS=15.
    assert (OP_POPULATE, OP_REMOVE, OP_CLEAR_MEMORY_TIER, OP_FLUSH_TO_SSD,
            OP_GET_IO_STATS) == (11, 12, 13, 14, 15)


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
