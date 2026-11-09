//! DMA-safe buffer for SPDK NVMe I/O operations.
//!
//! The type definition has been moved to the `interfaces` crate.
//! This module re-exports it for backwards compatibility.

pub use interfaces::DmaBuffer;
