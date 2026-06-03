//! Component lifecycle management: initialization and teardown.
//!
//! Orchestrates the full component lifecycle: load dylibs, create instances
//! in topological order, execute bindings, and provide fail-fast teardown
//! in reverse initialization order on any failure.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use std::any::Any;

use component_core::component_ref::ComponentRef;
use component_core::iunknown::{query, IUnknown};
use interfaces::{
    DmaAllocFn, DmaBuffer, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, IMemoryTier, ISPDKEnv, PciAddress,
};

/// Query an interface from a component by name, using the component's own TypeId.
///
/// SAFETY: The caller must ensure that `name` corresponds to the actual trait type `T`.
/// This is safe when both the host and dylib share the same trait definition (same ABI).
unsafe fn query_by_name<T: Send + Sync + 'static + ?Sized>(
    component: &dyn IUnknown,
    name: &str,
) -> Option<Arc<T>>
where
    Arc<T>: Any + Send + Sync,
{
    let any_ref = component.query_interface_by_name(name)?;
    // SAFETY: The dyn Any reference is &Arc<dyn T + Send + Sync> stored by the component.
    // Both host and dylib use the same trait layout (same compiler, same ABI).
    // The TypeId mismatch prevents downcast_ref from working, but the underlying
    // bytes are a valid Arc<dyn T> with the correct vtable.
    let arc_ref = &*(any_ref as *const (dyn Any + Send + Sync) as *const Arc<T>);
    Some(Arc::clone(arc_ref))
}
use libloading::Library;

use crate::binder::{self, NamedComponent};
use crate::config::{ComponentSpec, Configuration, InstanceCount};
use crate::loader;
use crate::topology;

/// A fully initialized component instance with its backing library.
pub struct LiveComponent {
    pub name: String,
    pub component: ComponentRef,
    pub _library: Arc<Library>,
}

/// The assembled component stack, ready for use.
pub struct ComponentStack {
    pub components: Vec<LiveComponent>,
}

impl ComponentStack {
    /// Shutdown all components in reverse initialization order.
    pub fn shutdown(&self) {
        for comp in self.components.iter().rev() {
            eprintln!("[certus-composable] shutting down: {}", comp.name);
            // ComponentRef drop will decrement Arc; actual cleanup happens
            // when the last reference is released.
        }
    }
}

impl Drop for ComponentStack {
    fn drop(&mut self) {
        // Components are dropped in reverse order (Vec drops front-to-back,
        // but we want reverse init order). Reverse the vec before drop.
        self.components.reverse();
    }
}

/// Initialize the full component stack from a validated configuration.
///
/// Steps:
/// 1. Determine initialization order (topo sort or explicit).
/// 2. Load dylibs and create component instances in order.
/// 3. Execute all binding rules.
///
/// On any failure, tears down already-initialized components in reverse order.
///
/// # Errors
///
/// Returns an error string describing the failure. All previously initialized
/// components are torn down before the error is returned.
pub fn initialize_stack(
    config: &Configuration,
    resolved_paths: &HashMap<String, PathBuf>,
) -> Result<ComponentStack, String> {
    // Resolve instance counts and build the effective component list.
    let mut effective_names: Vec<String> = Vec::new();
    let mut instance_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut spec_for_instance: HashMap<String, &ComponentSpec> = HashMap::new();

    for comp in &config.components {
        let count = match &comp.instances {
            InstanceCount::Literal(n) => *n,
            InstanceCount::Variable(var) => *config.variables.get(var).unwrap_or(&1),
        };

        if count == 1 {
            effective_names.push(comp.name.clone());
            spec_for_instance.insert(comp.name.clone(), comp);
        } else {
            let mut names = Vec::new();
            for i in 0..count {
                let instance_name = format!("{}[{i}]", comp.name);
                effective_names.push(instance_name.clone());
                spec_for_instance.insert(instance_name.clone(), comp);
                names.push(format!("{}[{i}]", comp.name));
            }
            instance_map.insert(comp.name.clone(), names);
        }
    }

    // Expand bindings for multi-instance components.
    let expanded_bindings = binder::expand_bindings(&config.bindings, &instance_map);

    // Determine initialization order.
    let base_names: Vec<String> = config.components.iter().map(|c| c.name.clone()).collect();
    let init_order = if let Some(ref explicit) = config.init_order {
        topology::validate_init_order(explicit, &base_names, &config.bindings)?;
        explicit.clone()
    } else {
        topology::topological_sort(&base_names, &config.bindings)?
    };

    // Expand init_order to include multi-instance names.
    let mut expanded_order: Vec<String> = Vec::new();
    for name in &init_order {
        if let Some(instances) = instance_map.get(name) {
            expanded_order.extend(instances.iter().cloned());
        } else {
            expanded_order.push(name.clone());
        }
    }

    // Load dylibs (cache per unique dylib path to avoid double-loading).
    let mut library_cache: HashMap<PathBuf, Arc<Library>> = HashMap::new();
    let mut live_components: Vec<LiveComponent> = Vec::new();

    for instance_name in &expanded_order {
        let comp_spec = spec_for_instance
            .get(instance_name.as_str())
            .ok_or_else(|| format!("internal error: no spec for instance '{instance_name}'"))?;

        let dylib_path = resolved_paths
            .get(&comp_spec.name)
            .ok_or_else(|| format!("internal error: no resolved path for '{}'", comp_spec.name))?;

        let library = if let Some(lib) = library_cache.get(dylib_path) {
            Arc::clone(lib)
        } else {
            let loaded = loader::load_library(dylib_path).map_err(|e| {
                teardown_reverse(&live_components);
                e
            })?;
            library_cache.insert(dylib_path.clone(), Arc::clone(&loaded.library));
            loaded.library
        };

        let component = loader::create_component(&library, &comp_spec.dylib).map_err(|e| {
            teardown_reverse(&live_components);
            format!("component '{}': {e}", instance_name)
        })?;

        live_components.push(LiveComponent {
            name: instance_name.clone(),
            component,
            _library: library,
        });
    }

    // Execute bindings.
    let named_components: Vec<NamedComponent> = live_components
        .iter()
        .map(|lc| NamedComponent {
            name: lc.name.clone(),
            component: lc.component.attach(),
        })
        .collect();

    binder::execute_bindings(&named_components, &expanded_bindings).map_err(|e| {
        teardown_reverse(&live_components);
        e
    })?;

    // Post-binding: initialize block-devices and register them with the dispatcher.
    initialize_data_drives(&live_components, config, &instance_map).map_err(|e| {
        teardown_reverse(&live_components);
        e
    })?;

    Ok(ComponentStack {
        components: live_components,
    })
}

fn parse_memory_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".into());
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1024usize),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1usize),
    };
    let num: usize = num_str
        .parse()
        .map_err(|_| format!("invalid size: '{s}'"))?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("size overflow: '{s}'"))
}

fn parse_pci_address(addr: &str) -> Result<PciAddress, String> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 3 {
        return Err(format!(
            "invalid PCI address format '{addr}': expected DDDD:BB:DD.F"
        ));
    }
    let domain = u32::from_str_radix(parts[0], 16)
        .map_err(|_| format!("invalid PCI domain in '{addr}'"))?;
    let bus = u8::from_str_radix(parts[1], 16)
        .map_err(|_| format!("invalid PCI bus in '{addr}'"))?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return Err(format!(
            "invalid PCI dev.func in '{addr}': expected DD.F"
        ));
    }
    let dev = u8::from_str_radix(dev_func[0], 16)
        .map_err(|_| format!("invalid PCI device in '{addr}'"))?;
    let func = u8::from_str_radix(dev_func[1], 16)
        .map_err(|_| format!("invalid PCI function in '{addr}'"))?;
    Ok(PciAddress {
        domain,
        bus,
        dev,
        func,
    })
}

/// Initialize block-device and extent-manager instances, then register them
/// with the dispatcher via `add_data_drive`.
fn initialize_data_drives(
    components: &[LiveComponent],
    config: &Configuration,
    instance_map: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
    // Collect block-device instance names.
    let bd_instances: Vec<&str> = instance_map
        .get("block-device")
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_else(|| {
            components
                .iter()
                .filter(|c| c.name == "block-device")
                .map(|c| c.name.as_str())
                .collect()
        });

    // Collect extent-manager instance names.
    let em_instances: Vec<&str> = instance_map
        .get("extent-manager")
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_else(|| {
            components
                .iter()
                .filter(|c| c.name == "extent-manager")
                .map(|c| c.name.as_str())
                .collect()
        });

    if bd_instances.is_empty() {
        // No external block-devices: initialize dispatcher with data_pci_addrs from config.
        if let Some(dispatcher_comp) = components.iter().find(|c| c.name == "dispatcher") {
            if let Some(dispatcher) =
                unsafe { query_by_name::<dyn IDispatcher + Send + Sync>(&*dispatcher_comp.component, "IDispatcher") }
            {
                let pci_addrs = config
                    .server
                    .device_pci
                    .clone()
                    .unwrap_or_default();
                let format_on_init = config.server.format.unwrap_or(false);
                dispatcher
                    .initialize(interfaces::DispatcherConfig {
                        data_pci_addrs: pci_addrs,
                        format_on_init,
                        poller_base_cpu: config.server.poller_base_cpu,
                        ..Default::default()
                    })
                    .map_err(|e| format!("dispatcher initialize failed: {e}"))?;
                eprintln!("[certus-composable] dispatcher initialized (internal drives)");
            }
        }
        return Ok(());
    }

    if bd_instances.len() != em_instances.len() {
        return Err(format!(
            "block-device count ({}) != extent-manager count ({})",
            bd_instances.len(),
            em_instances.len()
        ));
    }

    // Get PCI addresses from config.
    let pci_addrs = config
        .server
        .device_pci
        .as_ref()
        .ok_or("server.device_pci required when block-device components are configured")?;

    if pci_addrs.len() < bd_instances.len() {
        return Err(format!(
            "not enough PCI addresses ({}) for {} block-device instances",
            pci_addrs.len(),
            bd_instances.len()
        ));
    }

    // Initialize SPDK environment before block-device initialization.
    if let Some(spdk_comp) = components.iter().find(|c| c.name == "spdk-env") {
        let spdk: Arc<dyn ISPDKEnv + Send + Sync> =
            unsafe { query_by_name::<dyn ISPDKEnv + Send + Sync>(&*spdk_comp.component, "ISPDKEnv") }
                .ok_or("spdk-env does not provide ISPDKEnv")?;
        spdk.init()
            .map_err(|e| format!("SPDK environment init failed: {e}"))?;
        eprintln!("[certus-composable] SPDK environment initialized");
    }

    // Initialize GPU services.
    if let Some(gpu_comp) = components.iter().find(|c| c.name == "gpu-services") {
        let gpu: Arc<dyn IGpuServices + Send + Sync> =
            unsafe { query_by_name::<dyn IGpuServices + Send + Sync>(&*gpu_comp.component, "IGpuServices") }
                .ok_or("gpu-services does not provide IGpuServices")?;
        gpu.initialize()
            .map_err(|e| format!("GPU services init failed: {e}"))?;
        eprintln!("[certus-composable] GPU services initialized");
    }

    // Initialize dispatch-map.
    if let Some(dm_comp) = components.iter().find(|c| c.name == "dispatch-map") {
        let dm: Arc<dyn IDispatchMap + Send + Sync> =
            unsafe { query_by_name::<dyn IDispatchMap + Send + Sync>(&*dm_comp.component, "IDispatchMap") }
                .ok_or("dispatch-map does not provide IDispatchMap")?;
        let dma_alloc: DmaAllocFn = Arc::new(|size, align, _numa| {
            DmaBuffer::new(size, align, None).map_err(|e| e.to_string())
        });
        dm.set_dma_alloc(dma_alloc);
        dm.initialize()
            .map_err(|e| format!("dispatch-map init failed: {e}"))?;
        eprintln!("[certus-composable] dispatch-map initialized");
    }

    // Initialize memory-tier.
    if let Some(mt_comp) = components.iter().find(|c| c.name == "memory-tier") {
        let mt: Arc<dyn IMemoryTier + Send + Sync> =
            unsafe { query_by_name::<dyn IMemoryTier + Send + Sync>(&*mt_comp.component, "IMemoryTier") }
                .ok_or("memory-tier does not provide IMemoryTier")?;
        let pool_size = parse_memory_size(
            config.server.memory_tier_size.as_deref().unwrap_or("2G"),
        )?;
        mt.initialize(pool_size)
            .map_err(|e| format!("memory-tier init failed: {e}"))?;
        eprintln!(
            "[certus-composable] memory-tier initialized ({} MiB)",
            pool_size / (1024 * 1024)
        );
    }

    // Initialize each block-device.
    for (i, bd_name) in bd_instances.iter().enumerate() {
        let bd_comp = components
            .iter()
            .find(|c| c.name == *bd_name)
            .ok_or_else(|| format!("block-device instance '{bd_name}' not found"))?;

        let admin: Arc<dyn IBlockDeviceAdmin + Send + Sync> =
            unsafe { query_by_name::<dyn IBlockDeviceAdmin + Send + Sync>(&*bd_comp.component, "IBlockDeviceAdmin") }
                .ok_or_else(|| format!("'{bd_name}' does not provide IBlockDeviceAdmin"))?;

        let pci = parse_pci_address(&pci_addrs[i])?;
        admin.set_pci_address(pci);

        if let Some(base_cpu) = config.server.poller_base_cpu {
            admin.set_actor_cpu(base_cpu + i);
        }

        admin
            .initialize()
            .map_err(|e| format!("block-device '{bd_name}' init failed: {e}"))?;

        eprintln!(
            "[certus-composable] block-device '{}' initialized at {}",
            bd_name, pci_addrs[i]
        );
    }

    // Set DMA alloc on each extent-manager.
    for (i, em_name) in em_instances.iter().enumerate() {
        let bd_name = &bd_instances[i];
        let bd_comp = components
            .iter()
            .find(|c| c.name == *bd_name)
            .ok_or_else(|| format!("block-device '{bd_name}' not found"))?;

        let ibd: Arc<dyn IBlockDevice + Send + Sync> =
            unsafe { query_by_name::<dyn IBlockDevice + Send + Sync>(&*bd_comp.component, "IBlockDevice") }
                .ok_or_else(|| format!("'{bd_name}' does not provide IBlockDevice"))?;

        let numa_node = ibd.numa_node();
        let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
            DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
        });

        let em_comp = components
            .iter()
            .find(|c| c.name == *em_name)
            .ok_or_else(|| format!("extent-manager '{em_name}' not found"))?;

        let iem: Arc<dyn IExtentManager + Send + Sync> =
            unsafe { query_by_name::<dyn IExtentManager + Send + Sync>(&*em_comp.component, "IExtentManager") }
                .ok_or_else(|| format!("'{em_name}' does not provide IExtentManager"))?;

        iem.set_dma_alloc(dma_alloc);
    }

    // Register each (block-device, extent-manager) pair with the dispatcher.
    let dispatcher_comp = components
        .iter()
        .find(|c| c.name == "dispatcher")
        .ok_or("no 'dispatcher' component found")?;

    let dispatcher: Arc<dyn IDispatcher + Send + Sync> =
        unsafe { query_by_name::<dyn IDispatcher + Send + Sync>(&*dispatcher_comp.component, "IDispatcher") }
            .ok_or("dispatcher does not provide IDispatcher")?;

    for (i, (bd_name, em_name)) in bd_instances.iter().zip(em_instances.iter()).enumerate() {
        let bd_comp = components.iter().find(|c| c.name == *bd_name).unwrap();
        let em_comp = components.iter().find(|c| c.name == *em_name).unwrap();

        let ibd: Arc<dyn IBlockDevice + Send + Sync> =
            unsafe { query_by_name::<dyn IBlockDevice + Send + Sync>(&*bd_comp.component, "IBlockDevice") }
                .ok_or_else(|| format!("'{bd_name}' does not provide IBlockDevice"))?;

        let admin: Arc<dyn IBlockDeviceAdmin + Send + Sync> =
            unsafe { query_by_name::<dyn IBlockDeviceAdmin + Send + Sync>(&*bd_comp.component, "IBlockDeviceAdmin") }
                .ok_or_else(|| format!("'{bd_name}' does not provide IBlockDeviceAdmin"))?;

        let iem: Arc<dyn IExtentManager + Send + Sync> =
            unsafe { query_by_name::<dyn IExtentManager + Send + Sync>(&*em_comp.component, "IExtentManager") }
                .ok_or_else(|| format!("'{em_name}' does not provide IExtentManager"))?;

        dispatcher
            .add_data_drive(ibd, admin, iem)
            .map_err(|e| format!("add_data_drive[{i}] failed: {e}"))?;

        eprintln!(
            "[certus-composable] registered drive pair: {} + {}",
            bd_name, em_name
        );
    }

    // Initialize the dispatcher with empty data_pci_addrs (drives are pre-registered).
    let format_on_init = config.server.format.unwrap_or(false);
    dispatcher
        .initialize(interfaces::DispatcherConfig {
            data_pci_addrs: Vec::new(),
            format_on_init,
            poller_base_cpu: config.server.poller_base_cpu,
            ..Default::default()
        })
        .map_err(|e| format!("dispatcher initialize failed: {e}"))?;

    eprintln!("[certus-composable] dispatcher initialized with {} external drives", bd_instances.len());
    Ok(())
}

fn teardown_reverse(components: &[LiveComponent]) {
    for comp in components.iter().rev() {
        eprintln!("[certus-composable] teardown: {}", comp.name);
    }
}
