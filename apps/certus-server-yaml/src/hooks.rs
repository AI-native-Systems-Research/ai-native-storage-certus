//! Init hooks — typed functions called by the generated composition code.
//!
//! Each hook receives the queried interface Arc and the runtime StackConfig,
//! performing component-specific initialization logic.

use std::sync::Arc;
use std::time::Duration;

use interfaces::{
    DispatcherConfig, GossipConfig, IDispatchMap, IDispatcher, IGpuServices, IMemoryTier,
    IRemoteLookup, LookupConfig,
};

use crate::config::StackConfig;

#[cfg(feature = "spdk")]
const NVME_CLASS_CODE: u32 = 0x010802;

#[cfg(feature = "spdk-mem")]
#[allow(dead_code)]
pub fn init_spdk_env_dma_only(
    iface: &Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    iface.init().map_err(|e| format!("SPDK init failed: {e}"))
}

#[cfg(feature = "spdk-mem")]
#[allow(dead_code)]
pub fn init_spdk_env_stub(
    _iface: &Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "spdk")]
pub fn init_spdk_env(
    iface: &Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    iface.init().map_err(|e| format!("SPDK init failed: {e}"))?;

    // Resolve device addresses: explicit list or auto-discover via SPDK.
    let addrs = if !config.device_pci.is_empty() {
        config.device_pci.clone()
    } else {
        let count = config.drive_count.unwrap_or(1);
        let devices = iface.devices();
        let mut nvme_devices: Vec<_> = devices
            .iter()
            .filter(|d| d.id.class_id == NVME_CLASS_CODE)
            .collect();
        nvme_devices.sort_by_key(|d| if d.numa_node == 0 { 0 } else { 1 });
        if nvme_devices.len() < count {
            return Err(format!(
                "--drive-count={count} but only {} NVMe device(s) discovered",
                nvme_devices.len()
            ));
        }
        nvme_devices[..count]
            .iter()
            .map(|d| d.address.to_string())
            .collect()
    };

    // Resolve NUMA node of the first selected drive for memory-tier placement.
    let numa_node = addrs.first().and_then(|first_addr| {
        iface
            .devices()
            .iter()
            .find(|d| d.address.to_string() == *first_addr)
            .map(|d| d.numa_node)
    });
    *config.resolved_numa_node.borrow_mut() = numa_node;

    *config.resolved_pci_addrs.borrow_mut() = addrs;
    Ok(())
}

pub fn init_gpu(
    iface: &Arc<dyn IGpuServices + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    iface
        .initialize()
        .map_err(|e| format!("GPU init failed: {e}"))
}

pub fn init_dispatch_map(
    iface: &Arc<dyn IDispatchMap + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    iface
        .initialize()
        .map_err(|e| format!("DispatchMap init failed: {e}"))
}

pub fn init_memory_tier(
    iface: &Arc<dyn IMemoryTier + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    let numa_node = *config.resolved_numa_node.borrow();
    iface
        .initialize(config.memory_tier_size, numa_node)
        .map_err(|e| format!("MemoryTier init failed: {e}"))?;

    // Register the memory-tier pool with CUDA for pinned DMA transfers.
    if let Some((pool_ptr, pool_size)) = iface.pool_info() {
        let err = unsafe {
            gpu_services::cuda_ffi::cudaHostRegister(
                pool_ptr as *mut std::ffi::c_void,
                pool_size,
                0,
            )
        };
        if err != gpu_services::cuda_ffi::CUDA_SUCCESS {
            eprintln!(
                "warning: cudaHostRegister failed (err={err}), \
                 memory-tier transfers will use staged path"
            );
        }
    }

    Ok(())
}

// Only the full-remote profile's generated composition calls this hook.
#[allow(dead_code)]
pub fn init_remote_lookup(
    iface: &Arc<dyn IRemoteLookup + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    // Create/join the zyre node, bring up the RDMA responder, and spawn the
    // actor. The responder binds the RoCE device named by CERTUS_RDMA_BIND_IP,
    // or auto-detects the first active RDMA device when it is unset (empty
    // bind_ip). The actor CPU pin tracks the poller base when set.
    //
    // Timing and discovery are tunable from the environment because the built-in
    // defaults (op_deadline 50ms, UDP-beacon discovery) are wrong for real RDMA
    // (cold connects run long) and for cross-subnet clusters (beacon does not
    // route). Unset variables keep the LookupConfig default.
    let bind_ip = std::env::var("CERTUS_RDMA_BIND_IP").unwrap_or_default();
    let mut cfg = LookupConfig {
        bind_ip,
        actor_cpu: config.poller_base_cpu,
        ..Default::default()
    };

    // Cluster membership: an explicit group (CLI --rl-group or CERTUS_RL_GROUP,
    // resolved by clap in main) joins the named cluster; otherwise generate a
    // process-unique group so this node forms its own single-node cluster and
    // does not interfere with other users sharing the test fabric.
    cfg.group = match &config.rl_group {
        Some(g) => g.clone(),
        None => {
            let g = random_group();
            eprintln!(
                "remote-lookup: no --rl-group/CERTUS_RL_GROUP set; using isolated \
                 group \"{g}\" (single-node). Set --rl-group to cluster nodes."
            );
            g
        }
    };
    if let Some(ms) = env_parse::<u64>("CERTUS_RL_OP_DEADLINE_MS") {
        cfg.op_deadline = Duration::from_millis(ms);
    }
    if let Some(ms) = env_parse::<u64>("CERTUS_RL_PHASE1_MS") {
        cfg.phase1_timeout = Duration::from_millis(ms);
    }
    // How long batch_lookup blocks the caller (decoupled from op_deadline). Unset
    // keeps the caller coupled to op_deadline; set it shorter for async fill.
    if let Some(ms) = env_parse::<u64>("CERTUS_RL_CALLER_WAIT_MS") {
        cfg.caller_wait = Some(Duration::from_millis(ms));
    }
    // Grace after finalize before an orphaned landing slot is force-reclaimed
    // (peer QP torn down, then buffer freed).
    if let Some(ms) = env_parse::<u64>("CERTUS_RL_TEARDOWN_MS") {
        cfg.connection_teardown_timeout = Duration::from_millis(ms);
    }
    if let Some(pct) = env_parse::<u8>("CERTUS_RL_QUORUM_PCT") {
        cfg.quorum_pct = pct;
    }
    // Gossip discovery for clusters that span subnets (UDP beacon does not
    // reach): the hub node sets CERTUS_RL_GOSSIP_BIND (e.g. "tcp://*:9999"),
    // every node sets CERTUS_RL_GOSSIP_CONNECT to the hub endpoint. Bind wins if
    // both are set. Gossip mode also needs this node's own ZRE data mailbox.
    if let Some(bind) = env_str("CERTUS_RL_GOSSIP_BIND") {
        cfg.discovery = Some(GossipConfig::bind(bind));
    } else if let Some(connect) = env_str("CERTUS_RL_GOSSIP_CONNECT") {
        cfg.discovery = Some(GossipConfig::connect(connect));
    }
    if let Some(ep) = env_str("CERTUS_RL_NODE_ENDPOINT") {
        cfg.node_endpoint = Some(ep);
    }

    iface
        .initialize(cfg)
        .map_err(|e| format!("RemoteLookup init failed: {e}"))
}

/// Generate a process-unique zyre group so an unconfigured node forms its own
/// single-node cluster instead of colliding with other users' nodes on a shared
/// test fabric. Uses PID + wall-clock nanoseconds — no extra crate dependency.
fn random_group() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("remote_lookup_{pid:x}_{nanos:x}")
}

/// Read an environment variable, treating unset or empty as absent.
fn env_str(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

/// Read and parse an environment variable, ignoring unset/empty/unparseable.
fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    env_str(key).and_then(|v| v.parse::<T>().ok())
}

pub fn init_dispatcher(
    iface: &Arc<dyn IDispatcher + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    let mut data_pci_addrs = config.resolved_pci_addrs.borrow().clone();

    // When SPDK is not used, generate placeholder addresses for the requested drive count.
    if data_pci_addrs.is_empty() {
        let count = if !config.device_paths.is_empty() {
            config.device_paths.len()
        } else {
            config.drive_count.unwrap_or(1)
        };
        data_pci_addrs = (0..count).map(|i| format!("0000:00:0{i}.0")).collect();
    }

    iface
        .initialize(DispatcherConfig {
            data_pci_addrs,
            format_on_init: config.format,
            poller_base_cpu: config.poller_base_cpu,
            max_eviction_attempts: config.max_eviction_attempts,
            memory_tier_eviction_threshold: config.memory_tier_eviction_threshold,
            ..Default::default()
        })
        .map_err(|e| format!("Dispatcher init failed: {e}"))
}
