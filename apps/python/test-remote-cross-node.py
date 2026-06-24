#!/usr/bin/env python3
"""Cross-node RDMA test: populates cache locally, runs lookups from a remote node.

Automates the full workflow:
1. Starts certus-server-yaml with full-remote profile on this node
2. Populates cache objects via gRPC (local, requires GPU)
3. SSHes to the remote node and runs the test-client for RDMA lookups
4. Reports throughput and optionally verifies CRC32 integrity
5. Shuts down the server cleanly

Prerequisites:
  - RDMA-capable NICs on both nodes (connected fabric)
  - SSH key-based auth to the remote node (no password prompt)
  - certus-server-yaml built: CERTUS_PROFILE=full-remote cargo build -p certus-server-yaml --release --features rdma
  - test-client built: cargo build -p remote-request-handler --release
  - CUDA GPU on this node (for populate)

Usage:
    python test-remote-cross-node.py --remote-node 10.0.0.101
    python test-remote-cross-node.py --remote-node 10.0.0.101 --verify --object-size 4M --batch-size 64
"""

import argparse
import ctypes
import os
import signal
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import dispatcher_pb2
import dispatcher_pb2_grpc

# --- CUDA helpers ---

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMemset.restype = ctypes.c_int
_libcudart.cudaMemset.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]


def parse_size(s):
    s = s.strip().upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if s[-1] in multipliers:
        return int(s[:-1]) * multipliers[s[-1]]
    return int(s)


def find_repo_root():
    d = os.path.dirname(os.path.abspath(__file__))
    while d != "/":
        if os.path.isfile(os.path.join(d, "Cargo.toml")) and os.path.isdir(
            os.path.join(d, "components")
        ):
            return d
        d = os.path.dirname(d)
    return None


def main():
    parser = argparse.ArgumentParser(
        description="Cross-node RDMA test: populate locally, lookup from remote"
    )
    parser.add_argument(
        "--remote-node",
        required=True,
        help="Remote node IP for running test-client (e.g., 10.0.0.101)",
    )
    parser.add_argument(
        "--local-addr",
        default=None,
        help="Local RDMA address (default: auto-detect from hostname)",
    )
    parser.add_argument(
        "--rdma-port",
        type=int,
        default=18515,
        help="RDMA handler port [default: 18515]",
    )
    parser.add_argument(
        "--grpc-port",
        type=int,
        default=50051,
        help="gRPC port [default: 50051]",
    )
    parser.add_argument(
        "--object-size",
        default="4M",
        help="Object size per cache entry [default: 4M]",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=64,
        help="Entries per RDMA lookup batch [default: 64]",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=1,
        help="Number of RDMA lookup iterations [default: 1]",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="Verify data integrity via CRC32 on remote node",
    )
    parser.add_argument(
        "--fill-byte",
        type=lambda x: int(x, 0),
        default=0xAB,
        help="Fill byte for populate data [default: 0xAB]",
    )
    parser.add_argument(
        "--server-binary",
        default=None,
        help="Path to certus-server-yaml binary [default: auto-detect]",
    )
    parser.add_argument(
        "--test-client-path",
        default=None,
        help="Path to test-client on remote node [default: auto-detect]",
    )
    parser.add_argument(
        "--no-start-server",
        action="store_true",
        help="Skip starting server (assume already running)",
    )
    parser.add_argument(
        "--gpu-device",
        type=int,
        default=0,
        help="CUDA device ordinal [default: 0]",
    )

    args = parser.parse_args()
    object_size = parse_size(args.object_size)
    repo_root = find_repo_root()

    if not repo_root:
        print("ERROR: Cannot find repo root (Cargo.toml + components/)")
        sys.exit(1)

    # Auto-detect paths
    server_bin = args.server_binary or os.path.join(
        repo_root, "target/release/certus-server-yaml"
    )
    local_test_client = os.path.join(repo_root, "target/release/test-client")
    remote_test_client = args.test_client_path or "/home/dwaddington/certus/target/release/test-client"

    # Auto-detect local RDMA address
    local_addr = args.local_addr
    if not local_addr:
        import socket
        local_addr = socket.gethostbyname(socket.gethostname())
        # Fallback: try to get the IP on the RDMA interface
        try:
            result = subprocess.run(
                ["ip", "-4", "addr", "show"],
                capture_output=True, text=True
            )
            for line in result.stdout.splitlines():
                if "10.0.0." in line and "inet" in line:
                    local_addr = line.strip().split()[1].split("/")[0]
                    break
        except Exception:
            pass

    num_objects = args.batch_size * args.iterations

    print("=" * 70)
    print("Cross-Node RDMA Test")
    print("=" * 70)
    print(f"  Local node:     {local_addr}")
    print(f"  Remote node:    {args.remote_node}")
    print(f"  RDMA port:      {args.rdma_port}")
    print(f"  Object size:    {object_size // (1024*1024)} MiB")
    print(f"  Batch size:     {args.batch_size}")
    print(f"  Iterations:     {args.iterations}")
    print(f"  Total objects:  {num_objects}")
    print(f"  Verify CRC:     {args.verify}")
    print(f"  Fill byte:      0x{args.fill_byte:02X}")
    print()

    server_proc = None

    try:
        # --- Phase 1: Start server ---
        if not args.no_start_server:
            print("-" * 70)
            print("Phase 1: Starting certus-server-yaml")
            print("-" * 70)

            if not os.path.isfile(server_bin):
                print(f"  ERROR: Server binary not found: {server_bin}")
                print("  Build with: CERTUS_PROFILE=full-remote cargo build -p certus-server-yaml --release --features rdma")
                sys.exit(1)

            server_proc = subprocess.Popen(
                [server_bin, "--device-path", "/dev/null",
                 "--rdma-port", str(args.rdma_port), "--format"],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            print(f"  Started server (PID {server_proc.pid})")
            print("  Waiting for initialization...", end="", flush=True)
            time.sleep(5)
            if server_proc.poll() is not None:
                output = server_proc.stdout.read().decode()
                print(f"\n  ERROR: Server exited early:\n{output}")
                sys.exit(1)
            print(" ready")
            print()
        else:
            print("  (skipping server start — using existing instance)")
            print()

        # --- Phase 2: Populate cache ---
        print("-" * 70)
        print("Phase 2: Populating cache via gRPC")
        print("-" * 70)

        channel = grpc.insecure_channel(
            f"localhost:{args.grpc_port}",
            options=[
                ("grpc.max_send_message_length", 256 * 1024 * 1024),
                ("grpc.max_receive_message_length", 256 * 1024 * 1024),
            ],
        )
        stub = dispatcher_pb2_grpc.DispatcherStub(channel)

        # Clear memory tier
        try:
            resp = stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
            if resp.entries_cleared > 0:
                print(f"  Cleared {resp.entries_cleared} existing entries")
        except grpc.RpcError as e:
            print(f"  ERROR: gRPC connection failed: {e.details()}")
            sys.exit(1)

        # Allocate GPU buffer
        err = _libcudart.cudaSetDevice(args.gpu_device)
        if err != 0:
            print(f"  ERROR: cudaSetDevice({args.gpu_device}) failed: {err}")
            sys.exit(1)

        dev_ptr = ctypes.c_void_p()
        _libcudart.cudaMalloc(ctypes.byref(dev_ptr), object_size)
        _libcudart.cudaMemset(dev_ptr, args.fill_byte, object_size)
        handle_buf = (ctypes.c_ubyte * 64)()
        _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
        ipc = dispatcher_pb2.IpcHandle(
            cuda_ipc_handle=bytes(handle_buf),
            size=object_size,
            gpu_device_id=args.gpu_device,
        )

        t0 = time.time()
        ok = 0
        for key in range(1, num_objects + 1):
            resp = stub.Populate(
                dispatcher_pb2.BatchPopulateRequest(
                    entries=[dispatcher_pb2.PopulateEntry(key=key, ipc_handle=ipc)]
                )
            )
            if resp.results[0].success or "already exists" in resp.results[0].error_message:
                ok += 1
        elapsed = time.time() - t0
        _libcudart.cudaFree(dev_ptr)

        populate_gbs = ok * object_size / (1024**3) / elapsed if elapsed > 0 else 0
        print(f"  Populated: {ok}/{num_objects} objects in {elapsed:.2f}s ({populate_gbs:.3f} GB/s)")
        print()

        if ok == 0:
            print("  ERROR: No objects populated")
            sys.exit(1)

        # --- Phase 3: Copy test-client to remote and run ---
        print("-" * 70)
        print(f"Phase 3: RDMA lookups from {args.remote_node}")
        print("-" * 70)

        # Copy binary to remote
        print("  Copying test-client to remote node...", end="", flush=True)
        scp_result = subprocess.run(
            ["scp", "-o", "StrictHostKeyChecking=no", local_test_client,
             f"{args.remote_node}:{remote_test_client}"],
            capture_output=True, text=True, timeout=30,
        )
        if scp_result.returncode != 0:
            print(f" FAILED\n  {scp_result.stderr}")
            sys.exit(1)
        print(" done")

        # Run test-client on remote
        cmd = [
            "ssh", "-o", "StrictHostKeyChecking=no", args.remote_node,
            remote_test_client,
            "--addr", local_addr,
            "--port", str(args.rdma_port),
            "--batch-size", str(args.batch_size),
            "--iterations", str(args.iterations),
            "--result-buf-size", str(object_size),
        ]
        if args.verify:
            cmd.extend(["--verify", "--expected-fill", str(args.fill_byte)])

        print(f"  Running: test-client --addr {local_addr} --port {args.rdma_port} "
              f"--batch-size {args.batch_size} --iterations {args.iterations}"
              f"{' --verify' if args.verify else ''}")
        print()

        result = subprocess.run(cmd, capture_output=True, text=True, timeout=300)

        if result.returncode != 0:
            print(f"  ERROR: test-client failed:\n{result.stderr or result.stdout}")
            sys.exit(1)

        # Print test-client output (indented)
        for line in result.stdout.splitlines():
            print(f"  {line}")
        print()

        # --- Summary ---
        print("=" * 70)
        print("Summary")
        print("=" * 70)
        print(f"  Nodes:           {local_addr} (server) → {args.remote_node} (client)")
        print(f"  Objects:         {ok} x {object_size // (1024*1024)} MiB")
        print(f"  Populate:        {populate_gbs:.3f} GB/s")

        # Extract throughput from test-client output
        for line in result.stdout.splitlines():
            if "throughput:" in line.lower():
                print(f"  RDMA lookup:     {line.strip().split('throughput:')[1].strip()}")
                break
        if args.verify:
            if "STATUS: PASS" in result.stdout:
                print("  Integrity:       PASS (CRC32)")
            elif "STATUS: FAIL" in result.stdout:
                print("  Integrity:       FAIL (CRC32)")
        print()

    finally:
        # --- Cleanup: stop server ---
        if server_proc and server_proc.poll() is None:
            print("Shutting down server...", end="", flush=True)
            server_proc.send_signal(signal.SIGTERM)
            try:
                server_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server_proc.kill()
                server_proc.wait()
            print(" done")


if __name__ == "__main__":
    main()
