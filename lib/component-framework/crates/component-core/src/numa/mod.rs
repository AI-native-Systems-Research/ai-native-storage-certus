//! NUMA-aware thread pinning and memory allocation.
//!
//! This module provides types for CPU affinity management, NUMA topology
//! discovery, and NUMA-local memory allocation. All operations use Linux
//! system calls via the `libc` crate — no external C libraries are required.
//!
//! # Key Types
//!
//! - [`CpuSet`] — a set of CPU core IDs for thread affinity
//! - [`NumaTopology`] — runtime NUMA layout (nodes and their CPUs)
//! - [`NumaNode`] — a single NUMA node with CPU list and distances
//! - [`NumaAllocator`] — allocates memory bound to a specific NUMA node
//!
//! # Examples
//!
//! ```no_run
//! use component_core::numa::{CpuSet, NumaTopology};
//!
//! // Discover topology
//! let topo = NumaTopology::discover().unwrap();
//! println!("NUMA nodes: {}", topo.node_count());
//!
//! // Get CPUs on node 0
//! let node0 = topo.node(0).unwrap();
//! let cpus: Vec<usize> = node0.cpus().iter().collect();
//! println!("Node 0 CPUs: {cpus:?}");
//! ```

mod allocator;
mod cpuset;
mod topology;

pub use allocator::NumaAllocator;
pub use cpuset::{get_thread_affinity, set_thread_affinity, validate_cpus, CpuSet};
pub use topology::{parse_range_list, NumaNode, NumaTopology};

use std::fmt;

/// Errors from NUMA operations.
///
/// # Examples
///
/// ```
/// use component_core::numa::NumaError;
///
/// let err = NumaError::EmptyCpuSet;
/// assert_eq!(format!("{err}"), "CPU set is empty");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumaError {
    /// The specified CPU ID exceeds the system maximum.
    CpuOutOfRange {
        /// The invalid CPU ID.
        cpu: usize,
        /// The maximum valid CPU ID.
        max: usize,
    },
    /// The specified CPU is offline.
    CpuOffline(usize),
    /// An empty CPU set was provided where at least one CPU is required.
    EmptyCpuSet,
    /// The specified NUMA node ID is not valid.
    InvalidNode(usize),
    /// NUMA topology information could not be read.
    TopologyUnavailable(String),
    /// Setting thread CPU affinity failed.
    AffinityFailed(String),
    /// NUMA-local memory allocation failed.
    AllocationFailed(String),
}

impl fmt::Display for NumaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CpuOutOfRange { cpu, max } => {
                write!(f, "CPU {cpu} out of range (max {max})")
            }
            Self::CpuOffline(cpu) => write!(f, "CPU {cpu} is offline"),
            Self::EmptyCpuSet => write!(f, "CPU set is empty"),
            Self::InvalidNode(node) => write!(f, "invalid NUMA node {node}"),
            Self::TopologyUnavailable(msg) => {
                write!(f, "NUMA topology unavailable: {msg}")
            }
            Self::AffinityFailed(msg) => write!(f, "affinity failed: {msg}"),
            Self::AllocationFailed(msg) => write!(f, "NUMA allocation failed: {msg}"),
        }
    }
}

impl std::error::Error for NumaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numa_error_display() {
        assert_eq!(
            NumaError::CpuOutOfRange { cpu: 999, max: 63 }.to_string(),
            "CPU 999 out of range (max 63)"
        );
        assert_eq!(NumaError::CpuOffline(5).to_string(), "CPU 5 is offline");
        assert_eq!(NumaError::EmptyCpuSet.to_string(), "CPU set is empty");
        assert_eq!(
            NumaError::InvalidNode(99).to_string(),
            "invalid NUMA node 99"
        );
        assert_eq!(
            NumaError::TopologyUnavailable("no sysfs".into()).to_string(),
            "NUMA topology unavailable: no sysfs"
        );
        assert_eq!(
            NumaError::AffinityFailed("EPERM".into()).to_string(),
            "affinity failed: EPERM"
        );
        assert_eq!(
            NumaError::AllocationFailed("mmap failed".into()).to_string(),
            "NUMA allocation failed: mmap failed"
        );
    }

    #[test]
    fn numa_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<NumaError>();
    }

    #[test]
    fn numa_error_is_eq() {
        assert_eq!(NumaError::EmptyCpuSet, NumaError::EmptyCpuSet);
        assert_ne!(NumaError::EmptyCpuSet, NumaError::CpuOffline(0));
    }
}
