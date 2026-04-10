# Interface Contract: IMemoryManagement

**Date**: 2026-04-10
**Feature**: 001-spdk-memory-alloc
**Defined in**: `components/interfaces/src/imemory_management.rs` (under `spdk` feature gate)

## Interface Definition

```rust
define_interface! {
    pub IMemoryManagement {
        /// Allocate a DMA-safe memory buffer.
        ///
        /// Returns a `DmaBuffer` of `size` bytes with `align`-byte alignment.
        /// Memory contents are zero-initialized (SPDK only provides zmalloc).
        ///
        /// # Parameters
        /// - `size`: Buffer size in bytes (must be > 0)
        /// - `align`: Required alignment in bytes (must be > 0)
        /// - `numa_node`: Optional NUMA node affinity. `None` = any node.
        ///
        /// # Errors
        /// - `InvalidSize` if size is 0
        /// - `InvalidAlignment` if align is 0
        /// - `EnvNotConnected` if ISPDKEnv receptacle not wired
        /// - `AllocationFailed` if SPDK returns NULL
        fn allocate(
            &self,
            size: usize,
            align: usize,
            numa_node: Option<i32>,
        ) -> Result<DmaBuffer, MemoryAllocatorError>;

        /// Allocate a zero-initialized DMA-safe memory buffer.
        ///
        /// Identical to `allocate` (SPDK only provides zero-init allocation),
        /// but the name signals explicit intent for zero-initialization.
        ///
        /// # Parameters
        /// Same as `allocate`.
        fn zmalloc(
            &self,
            size: usize,
            align: usize,
            numa_node: Option<i32>,
        ) -> Result<DmaBuffer, MemoryAllocatorError>;

        /// Reallocate a DMA buffer to a new size, preserving existing data.
        ///
        /// Allocates a new buffer of `new_size` bytes, copies
        /// `min(old_len, new_size)` bytes from the old buffer, and frees
        /// the old buffer. Stats are updated atomically.
        ///
        /// # Parameters
        /// - `buffer`: The existing DmaBuffer to resize (consumed)
        /// - `new_size`: New buffer size in bytes (must be > 0)
        /// - `align`: Alignment for the new buffer (must be > 0)
        /// - `numa_node`: Optional NUMA node for the new allocation
        ///
        /// # Errors
        /// - `InvalidSize` if new_size is 0
        /// - `InvalidAlignment` if align is 0
        /// - `ReallocFailed` if new allocation fails (old buffer is NOT freed)
        fn reallocate(
            &self,
            buffer: DmaBuffer,
            new_size: usize,
            align: usize,
            numa_node: Option<i32>,
        ) -> Result<DmaBuffer, MemoryAllocatorError>;

        /// Free a DMA buffer and update allocation statistics.
        ///
        /// Takes ownership of the buffer, updates stats, then drops
        /// the buffer (which calls the SPDK deallocator).
        ///
        /// # Parameters
        /// - `buffer`: The DmaBuffer to free (consumed)
        fn free(&self, buffer: DmaBuffer);

        /// Return a snapshot of current allocation statistics.
        ///
        /// The returned stats are consistent (taken under a single lock acquisition).
        fn stats(&self) -> AllocationStats;
    }
}
```

## Preconditions

- ISPDKEnv receptacle must be connected before calling allocate/zmalloc/reallocate.
- SPDK environment must be initialized (via `ISPDKEnv::init()`) before allocation.
- `free` has no preconditions beyond receiving a valid DmaBuffer.
- `stats` has no preconditions; returns zeroed stats if no allocations have occurred.

## Postconditions

- **allocate/zmalloc**: Stats incremented by `size` bytes and +1 count in `zone[numa_node]` (or `zone[-1]` if no NUMA specified).
- **free**: Stats decremented by `buffer.len()` bytes and -1 count in `zone[buffer.numa_node()]`.
- **reallocate**: Stats decremented for old buffer, incremented for new buffer. On error, stats unchanged and old buffer returned in error.
- **stats**: Returns consistent snapshot; does not mutate state.

## Error Behavior

| Error | When | Recovery |
|-------|------|----------|
| `InvalidSize` | size == 0 | Fix parameter, retry |
| `InvalidAlignment` | align == 0 | Fix parameter, retry |
| `EnvNotConnected` | ISPDKEnv receptacle not wired | Connect receptacle first |
| `AllocationFailed` | SPDK returned NULL | Free other buffers, retry or fail gracefully |
| `ReallocFailed` | New allocation failed during realloc | Old buffer preserved in error variant; caller retains ownership |

## Thread Safety

All methods are safe to call concurrently from multiple threads. The implementation uses interior mutability (Mutex) for stats bookkeeping. SPDK allocation functions are internally thread-safe.
