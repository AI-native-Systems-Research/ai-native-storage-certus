# Quickstart: Remote Lookup Batch Interface

## Build

```bash
cargo build -p remote-lookup
```

## Test

```bash
# Unit and doc tests
cargo test -p remote-lookup

# Lint check
cargo clippy -p remote-lookup -- -D warnings

# Documentation check
cargo doc -p remote-lookup --no-deps
```

## Usage Example

```rust
use component_core::query_interface;
use interfaces::{CacheKey, IpcHandle, IRemoteLookup};

// Create component
let comp = RemoteLookupComponent::new_default();
let rl = query_interface!(comp, IRemoteLookup).unwrap();

// Connect (placeholder — no actual network)
rl.connect("remote-node:9090").unwrap();

// Batch lookup (placeholder returns NotFound for each entry)
let mut buf = vec![0u8; 4096];
let entries: Vec<(CacheKey, IpcHandle)> = vec![
    (1, IpcHandle { address: buf.as_mut_ptr(), size: 4096 }),
    (2, IpcHandle { address: buf.as_mut_ptr(), size: 4096 }),
];
let results = rl.batch_lookup(&entries);
assert_eq!(results.len(), 2);
```

## Integration

To use `batch_lookup` from the dispatcher or other components:

1. Declare an `IRemoteLookup` receptacle in your component
2. Bind the `remote-lookup` component at wiring time
3. Call `batch_lookup` with the same `(CacheKey, IpcHandle)` slice used for local dispatch
