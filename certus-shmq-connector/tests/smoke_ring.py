#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Standalone smoke test for the certus-shmq ring.py client.

Three checks, in increasing hardware requirements:

  1. --framing   Pure encode/decode round-trips + chunking. No server, no
                 hardware. Always run.
  2. --echo      Transport round-trip against the `shmq-echo` Rust harness (it
                 echoes each payload back with status = opcode). Proves the full
                 mmap/header/publish/spin-then-futex/response path across two
                 processes — no GPU/SPDK. Auto-spawns the harness if given
                 --echo-bin, else expects one already serving at --path.
  3. --server    Check/Reserve a few keys against a live `certus-server`
                 (needs SPDK + GPU; run on an RDMA node). Expects it serving at
                 --path.

Usage:
  python smoke_ring.py                          # framing only
  python smoke_ring.py --echo --echo-bin target/debug/shmq-echo
  python smoke_ring.py --echo --path /dev/shm/certus-shmq-echo   # external server
  python smoke_ring.py --server --path /dev/shm/certus-shmq
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import threading
import time

# Make `certus_shmq_connector` importable when run from the repo tree.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from certus_shmq_connector import ring  # noqa: E402
from certus_shmq_connector.ring import Ring, RingError  # noqa: E402


def _ok(msg: str) -> None:
    print(f"  \033[32mPASS\033[0m {msg}")


def _fail(msg: str) -> None:
    print(f"  \033[31mFAIL\033[0m {msg}")


# ── 1. framing self-test (no server) ─────────────────────────────────────────


def test_framing() -> bool:
    print("[framing] encode/decode round-trips")
    passed = True

    # encode_keys / decode_ok_flags shapes.
    b = ring.encode_keys([1, 2, 0xDEADBEEF])
    assert b[:4] == (3).to_bytes(4, "little"), "n prefix"
    assert len(b) == 4 + 3 * 8
    _ok("encode_keys length + prefix")

    b = ring.encode_promote_keys(True, [7, 8])
    assert b[0] == 1 and b[1:5] == (2).to_bytes(4, "little")
    _ok("encode_promote_keys promote byte")

    b = ring.encode_reserve([(5, 4096, 99), (6, 8192, 0)])
    # n:u32 + 2 * (u64 + u32 + u64) = 4 + 2*20
    assert len(b) == 4 + 2 * 20, f"reserve len {len(b)}"
    _ok("encode_reserve packed size (no padding)")

    # decode_ok_flags: short payload defaults missing to False.
    assert ring.decode_ok_flags(b"\x01\x00", 3) == [True, False, False]
    _ok("decode_ok_flags handles short payload")

    # decode_take_events round-trip against a hand-built blob.
    import struct

    blob = struct.pack("<I", 2)
    blob += struct.pack("<QI", 111, ring.REASON_REMOVED)
    blob += struct.pack("<QI", 222, ring.REASON_DEMOTED)
    blob += struct.pack("<Q", 9)
    events, dropped = ring.decode_take_events(blob)
    assert events == [(111, 1), (222, 0)] and dropped == 9, (events, dropped)
    _ok("decode_take_events round-trip")

    # HandleBatch: two blocks sharing the SAME two handles → table has 2 rows.
    h0 = b"\x11" * 64
    h1 = b"\x22" * 64
    entries = [
        (100, [(h0, 0, 0, 4096), (h1, 0, 0, 2048)]),
        (101, [(h0, 0, 4096, 4096), (h1, 0, 2048, 2048)]),
    ]
    enc = ring.encode_handle_batch(entries)
    n_handles = int.from_bytes(enc[:4], "little")
    assert n_handles == 2, f"dedup failed: {n_handles} handle rows"
    # size = 4 + 2*(64+4) + 4 + 2*(8 + 2 + 2*16)
    expected = 4 + 2 * ring.HANDLE_ENTRY_SIZE + 4 + 2 * (8 + 2 + 2 * ring.REGION_SIZE)
    assert len(enc) == expected, f"handle batch size {len(enc)} != {expected}"
    _ok("encode_handle_batch dedups shared handles")

    # Chunking: a tiny cap forces one entry per chunk, order preserved.
    chunks = ring.chunk_handle_batches(entries, cap_req=120, log=None)
    flat = [k for chunk in chunks for (k, _) in chunk]
    assert flat == [100, 101], f"chunk order broke: {flat}"
    assert len(chunks) >= 2, f"expected split, got {len(chunks)} chunk(s)"
    _ok(f"chunk_handle_batches split into {len(chunks)}, order preserved")

    # A generous cap keeps everything in one chunk.
    assert len(ring.chunk_handle_batches(entries, cap_req=1 << 20)) == 1
    _ok("chunk_handle_batches single chunk when it fits")

    print(f"[framing] {'OK' if passed else 'FAILED'}\n")
    return passed


# ── 2. echo transport test (no hardware) ──────────────────────────────────────


def test_echo(path: str, echo_bin: str | None) -> bool:
    print(f"[echo] transport round-trip via shmq-echo at {path}")
    proc = None
    if echo_bin:
        # Fresh region each run.
        try:
            os.remove(path)
        except FileNotFoundError:
            pass
        proc = subprocess.Popen(
            [echo_bin, "serve", path, "8", "1048576", "131072"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        print(f"  spawned {echo_bin} (pid {proc.pid})")

    try:
        r = Ring(path, ready_timeout=10.0)
        print(
            f"  attached: channels={r.channel_count} cap_req={r.cap_req} "
            f"cap_resp={r.cap_resp} generation={r.generation}"
        )

        # Echo returns payload verbatim, status = opcode.
        for op, payload in [(1, b""), (42, b"hello"), (7, b"\x00\x01\x02" * 100)]:
            status, resp = r.request(op, payload)
            assert status == op, f"echo status {status} != opcode {op}"
            assert resp == payload, f"echo payload mismatch (op={op}, {len(resp)}B)"
        _ok("single-thread echo (empty / small / medium payloads)")

        # A payload near cap_resp exercises the 128 KiB response path.
        big = bytes((i & 0xFF) for i in range(120_000))
        status, resp = r.request(3, big)
        assert resp == big, f"large echo mismatch ({len(resp)} vs {len(big)})"
        _ok("large-payload echo (~120 KiB)")

        # Concurrent threads on distinct channels.
        errors: list[str] = []

        def worker(t: int) -> None:
            try:
                for i in range(500):
                    msg = f"t{t}-m{i}".encode()
                    _s, resp = r.request(0, msg)
                    if resp != msg:
                        errors.append(f"t{t} i{i}: {resp!r} != {msg!r}")
                        return
            except Exception as e:  # noqa: BLE001
                errors.append(f"t{t}: {e}")

        threads = [threading.Thread(target=worker, args=(t,)) for t in range(4)]
        t0 = time.monotonic()
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        if errors:
            for e in errors[:5]:
                _fail(e)
            return False
        dt = time.monotonic() - t0
        _ok(f"4 threads x 500 round-trips concurrent ({2000/dt:.0f} req/s aggregate)")

        # Bare latency sample (single thread, warm).
        for _ in range(1000):
            r.request(0, b"warm")
        t0 = time.monotonic()
        n = 20_000
        for _ in range(n):
            r.request(0, b"x" * 32)
        dt = time.monotonic() - t0
        print(f"  latency: {n} round-trips (32B), mean {dt/n*1e6:.2f} µs, {n/dt:.0f} req/s")

        r.close()
        print("[echo] OK\n")
        return True
    finally:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
            try:
                os.remove(path)
            except FileNotFoundError:
                pass


# ── 3. live-server op test (needs hardware) ───────────────────────────────────


def test_server(path: str) -> bool:
    print(f"[server] Check/Reserve against certus-server at {path}")
    r = Ring(path, ready_timeout=10.0)
    print(
        f"  attached: channels={r.channel_count} cap_req={r.cap_req} "
        f"cap_resp={r.cap_resp} generation={r.generation}"
    )

    keys = [0xA000 + i for i in range(4)]
    exists = r.check(keys)
    assert len(exists) == len(keys)
    _ok(f"Check {keys} -> {exists}")

    reserved = r.reserve([(k, 4096, 0) for k in keys])
    assert len(reserved) == len(keys)
    _ok(f"Reserve -> {reserved}")

    # Keys we just reserved should now report present under Check.
    exists2 = r.check(keys)
    _ok(f"Check after Reserve -> {exists2}")

    # Roll the reservations back so we leave no pending state.
    aborted = r.abort_store(keys)
    _ok(f"AbortStore -> {aborted}")

    events, dropped = r.take_events(0)
    _ok(f"TakeEvents -> {len(events)} events, dropped={dropped}")

    r.close()
    print("[server] OK\n")
    return True


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--framing", action="store_true", help="run framing self-test")
    ap.add_argument("--echo", action="store_true", help="run echo transport test")
    ap.add_argument("--server", action="store_true", help="run live-server op test")
    ap.add_argument("--path", default=None, help="shm file (default: echo/server mode default)")
    ap.add_argument("--echo-bin", default=None, help="path to shmq-echo binary to auto-spawn")
    args = ap.parse_args()

    # Default: framing only (the one check that needs nothing).
    if not (args.framing or args.echo or args.server):
        args.framing = True

    ok = True
    try:
        if args.framing:
            ok &= test_framing()
        if args.echo:
            ok &= test_echo(args.path or "/dev/shm/certus-shmq-echo", args.echo_bin)
        if args.server:
            ok &= test_server(args.path or "/dev/shm/certus-shmq")
    except (RingError, AssertionError) as e:
        _fail(str(e))
        ok = False

    print("ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
