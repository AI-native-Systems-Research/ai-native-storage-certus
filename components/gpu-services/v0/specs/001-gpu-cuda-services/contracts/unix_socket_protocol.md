# Protocol Contract: Python-Rust Unix Domain Socket IPC

**Transport**: Stream-oriented Unix domain socket
**Path**: `/tmp/gpu-services-ipc.sock` (configurable)
**Direction**: Python client → Rust server (request/response)

## Message Format

### Request (Client → Server)

```
┌─────────────────────────────────────────────┐
│ Header: 4 bytes                             │
│   payload_length: u32 (little-endian)       │
├─────────────────────────────────────────────┤
│ Body: payload_length bytes                  │
│   base64-encoded string (ASCII)             │
│   Decodes to exactly 72 bytes:              │
│   ├── [0..64]:  cudaIpcMemHandle_t (opaque) │
│   └── [64..72]: buffer_size (u64 LE)        │
└─────────────────────────────────────────────┘
```

### Response (Server → Client)

```
┌─────────────────────────────────────────────┐
│ 1 byte: status                              │
│   0x01 = success (handle accepted)          │
│   0x00 = error                              │
├─────────────────────────────────────────────┤
│ If error (status == 0x00):                  │
│   4 bytes: error_message_length (u32 LE)    │
│   N bytes: UTF-8 error message              │
└─────────────────────────────────────────────┘
```

## Sequence Diagram

```
Python Client                    Rust Server
     |                                |
     |---- connect() --------------->|
     |                                |
     |---- [len][base64 payload] --->|
     |                                |-- decode base64
     |                                |-- open IPC handle
     |                                |-- verify memory
     |                                |-- create DMA buffer
     |                                |-- perform DMA operation
     |<--- [0x01] (ACK) -------------|
     |                                |
     |---- close() ---------------->|
```

## Error Conditions

| Condition | Server Response |
|-----------|----------------|
| Invalid payload length (0 or > 1024) | 0x00 + "invalid payload length" |
| Base64 decode failure | 0x00 + "invalid base64 encoding" |
| Decoded size != 72 bytes | 0x00 + "payload must be exactly 72 bytes" |
| CUDA IPC open fails | 0x00 + "cudaIpcOpenMemHandle: {error}" |
| Memory verification fails | 0x00 + "memory verification failed: {reason}" |

## Python Client Reference

```python
import socket
import base64
import struct
import cupy as cp

# Allocate GPU memory
gpu_buf = cp.zeros(4096, dtype=cp.uint8)
ipc_handle = cp.cuda.runtime.ipcGetMemHandle(gpu_buf.data.ptr)

# Serialize: 64-byte handle + 8-byte LE size
payload = base64.b64encode(ipc_handle + struct.pack('<Q', 4096))

# Send via Unix socket
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/tmp/gpu-services-ipc.sock')
sock.sendall(struct.pack('<I', len(payload)) + payload)

# Read response
status = sock.recv(1)
assert status == b'\x01', "Server error"
sock.close()
```
