//! Key mapping between Python OffloadKey (bytes) and Rust CacheKey (u64).

use interfaces::CacheKey;

/// Convert a Python u64 key directly to a CacheKey.
///
/// vLLM's OffloadKey is opaque bytes, but our Python layer converts to u64
/// before crossing the FFI boundary. This function exists as the single
/// point of translation should the mapping logic change.
#[inline]
pub fn to_cache_key(py_key: u64) -> CacheKey {
    py_key
}

/// Convert a batch of Python u64 keys to CacheKeys.
#[inline]
pub fn to_cache_keys(py_keys: &[u64]) -> Vec<CacheKey> {
    py_keys.iter().copied().map(to_cache_key).collect()
}
