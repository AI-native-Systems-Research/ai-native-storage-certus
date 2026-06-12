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
    pub resolved_pci_addrs: RefCell<Vec<String>>,
}
