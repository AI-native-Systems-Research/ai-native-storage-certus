#!/usr/bin/env python3
"""Per-node gRPC driver for the multi-node remote-lookup RDMA cluster test.

This runs *on a single node*, against that node's own ``localhost`` gRPC
endpoint, and is orchestrated across machines by
``scripts/test-full-remote-multinode.sh`` over SSH. It is deliberately NOT a
``cargo test`` / pytest target: it needs real RDMA NICs, GPUs, and named lab
machines.

Two subcommands:

  populate   Store a batch of keys (each filled with a deterministic per-key
             pattern) into this node's cache via the gRPC Populate RPC. Run on
             the holder node.

  lookup     Look up a batch of keys via the gRPC Lookup RPC and *prove the hit
             came from a remote peer over RDMA* rather than from this node.
             Run on the requester node (which never populated the keys).

Why the lookup phase can prove remoteness without any remote-vs-local flag:
the dispatcher publishes a remote hit into the local tier exactly like a local
hit, and ``ServiceCounters`` has no remote counter. So we correlate:

  1. Check(keys)      -> every key must be absent locally beforehand.
  2. GetIoStats()     -> snapshot read_ops / read_bytes.
  3. Lookup(keys)     -> every key must be satisfied.
  4. GetIoStats()     -> read_ops / read_bytes must be UNCHANGED (the value
                         arrived via one-sided RDMA into DRAM, not from this
                         node's local SSD tier).
  5. --verify         -> the DMA'd bytes match the holder's per-key pattern.

If keys were absent locally, then satisfied, with zero local disk reads, the
data could only have come from a peer over RDMA.

The CUDA/IPC and pattern helpers are copied from ``certus-api-bench.py`` to keep
this a self-contained, single-file driver (easy to scp to a bare node).
"""

import argparse
import ctypes
import json
import os
import random
import sys

# The generated gRPC stubs live next to this script; when scp'd to a bare node
# the driver and stubs are copied together, so add our own directory to the path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc  # noqa: E402
import dispatcher_pb2  # noqa: E402
import dispatcher_pb2_grpc  # noqa: E402


# --------------------------------------------------------------------------
# CUDA + pattern helpers (mirrors apps/python/certus-api-bench.py).
# Raw cudaMalloc is required: cudaIpcGetMemHandle needs the base of an
# allocation, which PyTorch's caching allocator does not provide.
# --------------------------------------------------------------------------
_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaSetDevice.argtypes = [ctypes.c_int]
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMemcpy.restype = ctypes.c_int
_libcudart.cudaMemcpy.argtypes = [
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.c_int,
]
_CUDA_MEMCPY_H2D = 1
_CUDA_MEMCPY_D2H = 2


def parse_size(s):
    """Parse a human-readable size string (e.g. '128K', '4M', '2G') into bytes."""
    s = s.strip()
    if not s:
        raise argparse.ArgumentTypeError("empty size string")
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if suffix in multipliers:
        num_str, multiplier = s[:-1], multipliers[suffix]
    else:
        num_str, multiplier = s, 1
    try:
        value = int(num_str)
    except ValueError:
        raise argparse.ArgumentTypeError(f"invalid size number: '{num_str}'")
    if value <= 0:
        raise argparse.ArgumentTypeError(f"size must be positive, got '{s}'")
    return value * multiplier


def _make_pattern(key, block_size):
    """Deterministic per-key byte pattern (same key -> same bytes on any node)."""
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(block_size))


def _cuda_set_device(gpu_id):
    err = _libcudart.cudaSetDevice(gpu_id)
    if err != 0:
        raise RuntimeError(f"cudaSetDevice({gpu_id}) failed: {err}")


def _cuda_alloc(size):
    """Allocate GPU memory; return (device_ptr, 64-byte IPC handle bytes)."""
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def _cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


def _gpu_write(dev_ptr, data):
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    err = _libcudart.cudaMemcpy(dev_ptr, ctypes.byref(buf), len(data), _CUDA_MEMCPY_H2D)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy H2D failed: {err}")


def _gpu_read(dev_ptr, size):
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


# --------------------------------------------------------------------------
# Argument helpers
# --------------------------------------------------------------------------
def parse_keys(spec):
    """Parse a key spec: 'A-B' inclusive range, or a comma-separated list."""
    spec = spec.strip()
    if "-" in spec and "," not in spec:
        lo_s, hi_s = spec.split("-", 1)
        lo, hi = int(lo_s), int(hi_s)
        if hi < lo:
            raise argparse.ArgumentTypeError(f"empty key range: '{spec}'")
        return list(range(lo, hi + 1))
    keys = [int(p) for p in spec.split(",") if p.strip()]
    if not keys:
        raise argparse.ArgumentTypeError(f"no keys parsed from: '{spec}'")
    return keys


def make_stub(server):
    channel = grpc.insecure_channel(server)
    return dispatcher_pb2_grpc.DispatcherStub(channel)


# --------------------------------------------------------------------------
# populate
# --------------------------------------------------------------------------
def cmd_populate(args):
    keys = parse_keys(args.keys)
    size = args.object_size
    _cuda_set_device(args.gpu_device)
    stub = make_stub(args.server)

    dev_ptr, handle = _cuda_alloc(size)
    ipc = dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=handle, size=size, gpu_device_id=args.gpu_device
    )
    failures = []
    try:
        for key in keys:
            _gpu_write(dev_ptr, _make_pattern(key, size))
            resp = stub.Populate(
                dispatcher_pb2.BatchPopulateRequest(
                    entries=[dispatcher_pb2.PopulateEntry(key=key, ipc_handle=ipc)]
                )
            )
            r = resp.results[0]
            if not r.success:
                failures.append((key, r.error_message))
    finally:
        _cuda_free(dev_ptr)

    if failures:
        for key, msg in failures[:10]:
            print(f"  populate FAILED key={key}: {msg}", file=sys.stderr)
        print(
            f"populate: {len(keys) - len(failures)}/{len(keys)} stored, "
            f"{len(failures)} failed",
            file=sys.stderr,
        )
        return 1
    print(f"populate: {len(keys)}/{len(keys)} keys stored on {args.server} "
          f"({size} bytes each)")
    return 0


# --------------------------------------------------------------------------
# lookup (+ remoteness proof)
# --------------------------------------------------------------------------
def cmd_lookup(args):
    keys = parse_keys(args.keys)
    size = args.object_size
    _cuda_set_device(args.gpu_device)
    stub = make_stub(args.server)

    # 1. Keys must be absent locally before the lookup.
    check = stub.Check(dispatcher_pb2.BatchCheckRequest(keys=keys))
    present_before = [r.key for r in check.results if r.exists]

    # 2. Snapshot local SSD read counters.
    io_before = stub.GetIoStats(dispatcher_pb2.GetIoStatsRequest())

    # 3. Look up each key, DMA'ing the value into a local GPU landing buffer.
    dev_ptr, handle = _cuda_alloc(size)
    ipc = dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=handle, size=size, gpu_device_id=args.gpu_device
    )
    satisfied, missing, verify_failures = [], [], []
    try:
        for key in keys:
            resp = stub.Lookup(
                dispatcher_pb2.BatchLookupRequest(
                    entries=[dispatcher_pb2.LookupEntry(key=key, ipc_handle=ipc)]
                )
            )
            r = resp.results[0]
            if not r.success:
                missing.append((key, r.error_message))
                continue
            satisfied.append(key)
            if args.verify:
                got = _gpu_read(dev_ptr, size)
                if got != _make_pattern(key, size):
                    verify_failures.append(key)
    finally:
        _cuda_free(dev_ptr)

    # 4. Local SSD reads must not have moved (value came over RDMA into DRAM).
    io_after = stub.GetIoStats(dispatcher_pb2.GetIoStatsRequest())
    read_ops_delta = io_after.read_ops - io_before.read_ops
    read_bytes_delta = io_after.read_bytes - io_before.read_bytes

    remote_confirmed = (
        len(satisfied) == len(keys)
        and not present_before
        and read_ops_delta == 0
        and read_bytes_delta == 0
        and (not args.verify or not verify_failures)
    )

    verdict = {
        "server": args.server,
        "total": len(keys),
        "satisfied": len(satisfied),
        "missing": len(missing),
        "present_locally_before": len(present_before),
        "read_ops_delta": read_ops_delta,
        "read_bytes_delta": read_bytes_delta,
        "verify": bool(args.verify),
        "verify_failures": len(verify_failures),
        "remote_confirmed": remote_confirmed,
    }
    print(json.dumps(verdict))

    if not remote_confirmed:
        if present_before:
            print(f"  NOT REMOTE: {len(present_before)} key(s) already local "
                  f"before lookup", file=sys.stderr)
        if missing:
            for key, msg in missing[:10]:
                print(f"  MISS key={key}: {msg}", file=sys.stderr)
        if read_ops_delta or read_bytes_delta:
            print(f"  NOT REMOTE: local SSD reads moved "
                  f"(+{read_ops_delta} ops, +{read_bytes_delta} bytes) — value "
                  f"may have come from local disk", file=sys.stderr)
        if args.verify and verify_failures:
            print(f"  CORRUPT: {len(verify_failures)} key(s) had mismatched data",
                  file=sys.stderr)
        return 1
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    def add_common(sp):
        sp.add_argument("--server", default="localhost:50051",
                        help="gRPC endpoint (default localhost:50051)")
        sp.add_argument("--keys", required=True,
                        help="key spec: 'A-B' inclusive range or comma list")
        sp.add_argument("--object-size", type=parse_size, default=parse_size("4M"),
                        help="per-key object size (e.g. 64K, 4M; default 4M)")
        sp.add_argument("--gpu-device", type=int, default=0,
                        help="CUDA device ordinal (default 0)")

    sp_pop = sub.add_parser("populate", help="store keys on this node")
    add_common(sp_pop)
    sp_pop.set_defaults(func=cmd_populate)

    sp_look = sub.add_parser("lookup", help="look up keys and prove remoteness")
    add_common(sp_look)
    sp_look.add_argument("--verify", action="store_true",
                         help="check DMA'd bytes match the holder's pattern")
    sp_look.set_defaults(func=cmd_lookup)

    args = p.parse_args()
    try:
        sys.exit(args.func(args))
    except grpc.RpcError as e:
        print(f"gRPC error against {args.server}: {e.code()}: {e.details()}",
              file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
