//! Component binding orchestration.
//!
//! Executes binding rules by calling `connect_receptacle_raw` on target
//! components, connecting them to source component instances via the
//! `IUnknown` interface discovery mechanism.

use std::collections::HashMap;

use component_core::component_ref::ComponentRef;
use component_core::iunknown::IUnknown;

use crate::config::BindingRule;

/// A named component instance ready for binding.
pub struct NamedComponent {
    pub name: String,
    pub component: ComponentRef,
}

/// Execute all binding rules, connecting component receptacles to providers.
///
/// For each binding rule, looks up the source and target by name,
/// then calls `connect_receptacle_raw` on the target with the source
/// as the provider.
///
/// # Errors
///
/// Returns an error describing the first binding failure encountered.
pub fn execute_bindings(
    components: &[NamedComponent],
    bindings: &[BindingRule],
) -> Result<(), String> {
    let component_map: HashMap<&str, &ComponentRef> = components
        .iter()
        .map(|nc| (nc.name.as_str(), &nc.component))
        .collect();

    for (i, binding) in bindings.iter().enumerate() {
        let target = component_map.get(binding.target.as_str()).ok_or_else(|| {
            format!(
                "binding[{i}]: target '{}' not found in loaded components",
                binding.target
            )
        })?;

        let source = component_map.get(binding.source.as_str()).ok_or_else(|| {
            format!(
                "binding[{i}]: source '{}' not found in loaded components",
                binding.source
            )
        })?;

        // connect_receptacle_raw takes the provider as &dyn IUnknown.
        let source_ref: &dyn IUnknown = &***source;
        target
            .connect_receptacle_raw(&binding.receptacle, source_ref)
            .map_err(|e| {
                format!(
                    "binding[{i}]: failed to connect {}.{} <- {}: {e}",
                    binding.target, binding.receptacle, binding.source
                )
            })?;
    }

    Ok(())
}

/// Expand bindings for multi-instance components.
///
/// When a binding references a component that has multiple instances
/// (named `name[0]`, `name[1]`, etc.), this function expands the
/// binding to apply to all instances of that component.
pub fn expand_bindings(
    bindings: &[BindingRule],
    instance_names: &HashMap<String, Vec<String>>,
) -> Vec<BindingRule> {
    let mut expanded = Vec::new();

    for binding in bindings {
        let targets = instance_names
            .get(&binding.target)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let sources = instance_names
            .get(&binding.source)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        if targets.is_empty() && sources.is_empty() {
            // Single-instance to single-instance: keep as-is.
            expanded.push(binding.clone());
        } else if !targets.is_empty() && sources.is_empty() {
            // Each target instance binds to the same single source.
            for target_name in targets {
                expanded.push(BindingRule {
                    target: target_name.clone(),
                    receptacle: binding.receptacle.clone(),
                    source: binding.source.clone(),
                });
            }
        } else if targets.is_empty() && !sources.is_empty() {
            // Single target binds to first source instance (or all — depends on receptacle).
            // Default: bind to first instance.
            if let Some(first_source) = sources.first() {
                expanded.push(BindingRule {
                    target: binding.target.clone(),
                    receptacle: binding.receptacle.clone(),
                    source: first_source.clone(),
                });
            }
        } else {
            // Multi-to-multi: pair by index.
            for (target_name, source_name) in targets.iter().zip(sources.iter()) {
                expanded.push(BindingRule {
                    target: target_name.clone(),
                    receptacle: binding.receptacle.clone(),
                    source: source_name.clone(),
                });
            }
        }
    }

    expanded
}
