# Quickstart: gRPC Dispatcher Server

## Prerequisites

- SPDK environment configured (hugepages, IOMMU, NVMe devices unbound from kernel)
- Rust stable >= 1.75
- Python >= 3.8 with pip
- protobuf compiler (`protoc`) installed

## Build the Server

```bash
# From repository root
cargo build -p certus-server
```

## Run the Server

```bash
# Minimal (one device, default port 50051)
./target/debug/certus-server \
  --device-pci 0000:02:00.0

# Multiple devices, custom port
./target/debug/certus-server \
  --device-pci 0000:02:00.0 \
  --device-pci 0000:03:00.0 \
  --listen 0.0.0.0:50052

# With TLS
./target/debug/certus-server \
  --device-pci 0000:02:00.0 \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem
```

## Install Python Client Dependencies

```bash
cd apps/certus-server/python-client
pip install -r requirements.txt
```

## Run the Test Client

```bash
# Against local server on default port
python test_client.py

# Custom server address
python test_client.py --server localhost:50052
```

## Expected Output

```
Testing certus-server gRPC dispatcher...
[PASS] Batch populate: 10 entries
[PASS] Batch check: all 10 exist
[PASS] Batch lookup: 10 entries retrieved
[PASS] Batch remove: 10 entries removed
[PASS] Check after remove: 0 exist
[PASS] Duplicate key rejection
[PASS] Non-existent key handling
All tests passed.
```

## Shutdown

Send SIGTERM or Ctrl+C to the server process. It will drain any active request and exit cleanly.
