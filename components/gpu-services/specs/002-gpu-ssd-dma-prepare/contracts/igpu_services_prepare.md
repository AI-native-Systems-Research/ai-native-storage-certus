# Interface Contract: IGpuServices::prepare_memory_for_spdk

## Signature

```rust
#[cfg(feature = "spdk")]
fn prepare_memory_for_spdk(
    &self,
    base64_payload: &str,
    device_index: Option<u32>,
) -> Result<interfaces::DmaBuffer, String>;
```

## Preconditions

1. Component MUST be initialized (`initialize()` called successfully).
2. `base64_payload` MUST be a valid base64 string encoding exactly 72 bytes.
3. The originating process (PyTorch) MUST still be alive (IPC handles expire on allocator process exit).

## Postconditions (success)

1. Returns `Ok(DmaBuffer)` where:
   - `buf.len()` equals the size decoded from the payload.
   - `buf.as_ptr()` is a valid GPU device pointer for the lifetime of the buffer.
   - The buffer is directly usable as a DMA target by SPDK NVMe operations.
2. If memory was not previously pinned:
   - Memory has been pinned.
   - Pinning action was logged (if logger connected).
   - Buffer's drop will unpin then close the IPC handle.
3. If memory was already pinned:
   - No additional pinning occurred.
   - Already-pinned state was logged (if logger connected).
   - Buffer's drop will close the IPC handle without unpinning.

## Postconditions (error)

1. Returns `Err(String)` with a descriptive message.
2. No GPU resources are leaked (any partially-acquired resources are released).
3. Component state is unchanged (no partial pin state left behind).

## Error Conditions

| Condition | Error message pattern |
|-----------|----------------------|
| Not initialized | "Not initialized: call initialize() first" |
| Invalid base64 | "Invalid base64: *" |
| Wrong payload size | "Payload must be exactly 72 bytes, got *" |
| Zero-size buffer | "Buffer size must be > 0" |
| Device set failed | "cudaSetDevice(*) failed: *" |
| IPC open failed | "cudaIpcOpenMemHandle failed: *" |
| Pin failed | "Failed to pin memory: *" |
| Buffer creation failed | "DmaBuffer creation failed: *" |

## Thread Safety

- The function acquires the component state mutex briefly for initialization and pin-state checks.
- The returned `DmaBuffer` is `Send + Sync` and can be passed to any thread.
- CUDA operations are thread-safe (CUDA runtime API is thread-safe by design).

## Feature Gates

- Interface declaration: `#[cfg(feature = "spdk")]`
- Implementation body: requires both `gpu` and `spdk` features active.
- Without `gpu`: returns "GPU support not compiled (enable --features gpu)".
- Without `spdk`: method not present on trait (compile-time gate).
