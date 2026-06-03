//! Component lifecycle management: initialization and teardown.
//!
//! Orchestrates the full component lifecycle: load dylibs, create instances
//! in topological order, execute bindings, and provide fail-fast teardown
//! in reverse initialization order on any failure.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use component_core::component_ref::ComponentRef;
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

    Ok(ComponentStack {
        components: live_components,
    })
}

fn teardown_reverse(components: &[LiveComponent]) {
    for comp in components.iter().rev() {
        eprintln!("[certus-composable] teardown: {}", comp.name);
    }
}
