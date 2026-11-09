# GPU Handle Test (Server + Client)

End-to-end demo of Python-to-Rust GPU IPC handle handoff over a Unix domain socket.

## Prerequisites

- NVIDIA GPU with compute capability >= 7.0 (Volta+)
- CUDA toolkit installed (`/usr/local/cuda`)
- Python 3 with CuPy (`pip install cupy-cuda12x`)

## Build the Rust server

```bash
cargo build -p gpu-handle-test-server --features gpu
```

## Run

Terminal 1 (Rust server):

```bash
cargo run -p gpu-handle-test-server --features gpu -- /tmp/gpu-ipc.sock
```

Terminal 2 (Python client):

```bash
python3 apps/gpu-handle-test-client/client.py /tmp/gpu-ipc.sock
```

## Protocol

1. Client allocates GPU memory via CuPy
2. Client obtains CUDA IPC handle for the allocation
3. Client encodes: base64(handle_bytes[64] || size_le_u64[8]) = 72 bytes raw → 96 chars base64
4. Client sends: 4-byte LE length prefix + base64 payload over Unix socket
5. Server deserializes IPC handle, verifies device memory, pins, creates DMA buffer
6. Server sends: `ACK\n` on success or `NACK: <error>\n` on failure

## Expected Output

Server:

```
Listening on /tmp/gpu-ipc.sock
Connection accepted
IPC handle deserialized: 4194304 bytes
GPU memory verified: device type, contiguous
GPU memory pinned for DMA
DMA buffer created: 4194304 bytes
Sent ACK
```

Client:

```
Allocated 4 MiB on GPU
IPC handle exported
Sent payload (96 bytes base64)
Server response: ACK
```
