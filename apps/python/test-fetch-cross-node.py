#!/usr/bin/env python3
"""Cross-node explicit fetch (T087): one certus fetches blocks from another.

Two ``full-transfer`` certus servers, one per node, each started by hand. Run the
``source`` role on one node and the ``dest`` role on the other:

  source: POPULATEs ``--keys`` blocks from GPU memory into its certus, block k
          filled with a byte pattern only it has, then waits.
  dest:   asks its own certus to FETCH those keys from the source's transfer
          endpoint (``--peer ip:port``, as the source logged at start), waits
          for CHECK to read every key RESIDENT (or MISS), LOOKUPs them into its
          own GPU memory, and compares every byte. It also checks that the
          first LOOKUP of a fetched block is attributed FETCHED (tier 4) and the
          second DRAM (tier 1), and runs a run of keys the source lacks, which
          must read MISS without hanging.

Usage:
  # on node5 (source)
  python apps/python/test-fetch-cross-node.py source --shm /dev/shm/certus-t087 --keys 256
  # on node2 (dest)
  python apps/python/test-fetch-cross-node.py dest --shm /dev/shm/certus-t087 \\
      --peer 10.0.0.105:7481 --keys 256
"""

import argparse
import ctypes
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from certus_shmq_helpers import connect, single_region_entries  # noqa: E402
from certus_shmq_connector import ring as R  # noqa: E402

_cuda = ctypes.CDLL("libcudart.so")
_cuda.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_cuda.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_cuda.cudaMemcpy.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
_cuda.cudaMemset.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_size_t]
H2D, D2H = 1, 2
KEY_BASE = 0x7087_0000_0000  # default --key-base


def ck(rc, what):
    if rc != 0:
        sys.exit(f"{what} failed: cuda error {rc}")


def gpu_buffer(nbytes):
    ptr = ctypes.c_void_p()
    ck(_cuda.cudaMalloc(ctypes.byref(ptr), nbytes), "cudaMalloc")
    h = (ctypes.c_ubyte * 64)()
    ck(_cuda.cudaIpcGetMemHandle(h, ptr), "cudaIpcGetMemHandle")
    return ptr, bytes(h)


def pattern(k, size):
    """Block k's bytes: distinct per key and per offset."""
    seed = (k * 2654435761) & 0xFF
    row = bytes((seed + i) & 0xFF for i in range(256))
    return (row * (size // 256 + 1))[:size]


def entries_for(keys, handle, size):
    return single_region_entries(
        [(k, handle, size, i * size) for i, k in enumerate(keys)], 0
    )


def source(a):
    ring = connect(a.shm)
    keys = [a.key_base + i for i in range(a.keys)]
    ptr, handle = gpu_buffer(a.keys * a.size)
    host = b"".join(pattern(k, a.size) for k in keys)
    buf = ctypes.create_string_buffer(host, len(host))
    ck(_cuda.cudaMemcpy(ptr, buf, len(host), H2D), "cudaMemcpy H2D")
    ok = ring.populate(entries_for(keys, handle, a.size))
    print(f"source: populated {sum(ok)}/{len(keys)} blocks of {a.size} B")
    if not all(ok):
        sys.exit("source: populate failed")
    print("source: serving; Ctrl-C to stop")
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass


def wait_settled(ring, keys, timeout):
    t0 = time.monotonic()
    while True:
        states = ring.check_states(keys)
        if all(s != R.CHECK_PENDING for s in states):
            return states, time.monotonic() - t0
        if time.monotonic() - t0 > timeout:
            sys.exit(f"dest: still pending after {timeout}s: "
                     f"{sum(s == R.CHECK_PENDING for s in states)} keys")
        time.sleep(0.001)


def dest(a):
    ring = connect(a.shm)
    keys = [a.key_base + i for i in range(a.keys)]
    before = ring.check_states(keys)
    assert all(s == R.CHECK_MISS for s in before), "dest must start without the keys"

    t0 = time.monotonic()
    status, started = ring.fetch(a.peer, [(k, a.size) for k in keys])
    issued = time.monotonic() - t0
    assert status == R.FETCH_OK, f"fetch refused: status {status}"
    assert all(started), f"{started.count(False)} keys not started"
    states, waited = wait_settled(ring, keys, a.timeout)
    resident = sum(s == R.CHECK_RESIDENT for s in states)
    total = time.monotonic() - t0
    gib = a.keys * a.size / 2**30
    print(f"dest: FETCH returned in {issued*1e3:.2f} ms; {resident}/{a.keys} resident "
          f"after {total*1e3:.1f} ms ({gib/total:.2f} GiB/s incl. the CHECK polling)")
    assert resident == a.keys, f"only {resident} of {a.keys} fetched"

    # Into our own GPU, and compare. Raw LOOKUP replies, so the tier byte of
    # each key's FIRST serve is visible (ring.lookup collapses it to a bool).
    ptr, handle = gpu_buffer(a.keys * a.size)
    ck(_cuda.cudaMemset(ptr, 0, a.keys * a.size), "cudaMemset")
    tiers = []
    for i in range(0, a.keys, 64):
        chunk = entries_for(keys[i:i + 64], handle, a.size)
        for j, e in enumerate(chunk):  # offsets into the whole buffer
            k, [(h, dev, _, size)] = e
            chunk[j] = (k, [(h, dev, (i + j) * a.size, size)])
        tiers += list(ring._dispatch(R.OP_LOOKUP, R.encode_handle_batch(chunk))[:len(chunk)])
    host = ctypes.create_string_buffer(a.keys * a.size)
    ck(_cuda.cudaMemcpy(host, ptr, a.keys * a.size, D2H), "cudaMemcpy D2H")
    bad = [k for i, k in enumerate(keys)
           if host.raw[i * a.size:(i + 1) * a.size] != pattern(k, a.size)]
    print(f"dest: LOOKUP served {sum(1 for t in tiers if t)}/{a.keys}; "
          f"{a.keys - len(bad)}/{a.keys} byte-exact")
    assert not bad, f"{len(bad)} blocks differ, first key {bad[0]:#x}"
    assert all(t == 4 for t in tiers), f"first serves not all FETCHED (4): {sorted(set(tiers))}"
    print("dest: every first serve attributed FETCHED (tier 4)")
    raw = ring._dispatch(R.OP_LOOKUP, R.encode_handle_batch(entries_for(keys[:4], handle, a.size)))
    assert list(raw[:4]) == [1] * 4, list(raw[:4])
    print("dest: a second serve is DRAM (tier 1): attributed once")

    # A run the source lacks: started, then MISS, quickly.
    absent = [a.key_base + 0x1000_0000 + i for i in range(8)]
    status, started = ring.fetch(a.peer, [(k, a.size) for k in absent])
    states, waited = wait_settled(ring, absent, a.timeout)
    assert all(s == R.CHECK_MISS for s in states), states
    print(f"dest: {len(absent)} keys the source lacks read MISS after {waited*1e3:.1f} ms")
    print("dest: PASS")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("role", choices=["source", "dest"])
    p.add_argument("--shm", required=True)
    p.add_argument("--peer", help="dest: the source's transfer endpoint, ip:port")
    p.add_argument("--keys", type=int, default=256)
    p.add_argument("--size", type=lambda s: int(s, 0), default=64 * 1024)
    p.add_argument("--timeout", type=float, default=30.0)
    p.add_argument("--key-base", type=lambda s: int(s, 0), default=KEY_BASE,
                   help="first key; a fresh base reruns against servers already used")
    a = p.parse_args()
    if a.role == "source":
        source(a)
    else:
        if not a.peer:
            p.error("dest needs --peer")
        dest(a)


if __name__ == "__main__":
    main()
