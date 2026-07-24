use std::cell::RefCell;

/// Runtime configuration passed to component init hooks.
///
/// `resolved_pci_addrs` is populated by `init_spdk_env` after device discovery
/// and consumed by `init_dispatcher`. Uses interior mutability because the
/// generated composition code passes `&StackConfig` (shared ref) to all hooks.
pub struct StackConfig {
    pub device_pci: Vec<String>,
    pub device_paths: Vec<String>,
    pub drive_count: Option<usize>,
    pub memory_tier_size: usize,
    pub format: bool,
    pub poller_base_cpu: Option<usize>,
    pub max_eviction_attempts: usize,
    pub memory_tier_eviction_threshold: f64,
    /// Explicit zyre group (cluster name) for remote-lookup, from CLI
    /// `--rl-group` or the `CERTUS_RL_GROUP` env var. `None` => the remote-lookup
    /// init hook generates a unique random group so the node is its own
    /// single-node cluster (isolation by default).
    pub rl_group: Option<String>,
    pub resolved_pci_addrs: RefCell<Vec<String>>,
    /// NUMA node of the first selected drive (populated by init_spdk_env).
    pub resolved_numa_node: RefCell<Option<i32>>,
}
