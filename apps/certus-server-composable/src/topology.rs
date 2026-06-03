//! Topological sort for component initialization ordering.
//!
//! Uses Kahn's algorithm (BFS-based) to derive initialization order from
//! binding dependencies. Detects circular dependencies and validates
//! explicit `init_order` overrides against the dependency graph.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::config::BindingRule;

/// Compute the initialization order for components via topological sort.
///
/// Each binding `{ target, receptacle, source }` creates an edge:
/// `target` depends on `source` (source must be initialized first).
///
/// Uses Kahn's algorithm with lexicographic tie-breaking for determinism.
///
/// # Errors
///
/// Returns an error if a cycle is detected in the dependency graph.
pub fn topological_sort(
    component_names: &[String],
    bindings: &[BindingRule],
) -> Result<Vec<String>, String> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

    for name in component_names {
        in_degree.entry(name.as_str()).or_insert(0);
        adjacency.entry(name.as_str()).or_default();
    }

    for binding in bindings {
        if !in_degree.contains_key(binding.target.as_str())
            || !in_degree.contains_key(binding.source.as_str())
        {
            continue;
        }
        // target depends on source → edge from source to target.
        adjacency
            .entry(binding.source.as_str())
            .or_default()
            .push(binding.target.as_str());
        *in_degree.entry(binding.target.as_str()).or_insert(0) += 1;
    }

    // Kahn's algorithm with lexicographic tie-breaking.
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut zero_degree: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();
    zero_degree.sort();
    for name in zero_degree {
        queue.push_back(name);
    }

    let mut order: Vec<String> = Vec::new();

    while let Some(node) = queue.pop_front() {
        order.push(node.to_string());

        let mut next_nodes: Vec<&str> = Vec::new();
        if let Some(neighbors) = adjacency.get(node) {
            for &neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    next_nodes.push(neighbor);
                }
            }
        }
        next_nodes.sort();
        for n in next_nodes {
            queue.push_back(n);
        }
    }

    if order.len() != component_names.len() {
        let remaining: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg > 0)
            .map(|(&name, _)| name)
            .collect();
        return Err(format!(
            "circular dependency detected involving: {}",
            remaining.join(", ")
        ));
    }

    Ok(order)
}

/// Validate an explicit init_order against the dependency graph.
///
/// The explicit order must be a valid topological ordering: for every
/// binding, the source must appear before the target in the order.
///
/// # Errors
///
/// Returns an error if the explicit order violates any dependency.
pub fn validate_init_order(
    init_order: &[String],
    component_names: &[String],
    bindings: &[BindingRule],
) -> Result<(), String> {
    let name_set: HashSet<&str> = component_names.iter().map(|s| s.as_str()).collect();
    let order_set: HashSet<&str> = init_order.iter().map(|s| s.as_str()).collect();

    // Check that all components are represented.
    for name in &name_set {
        if !order_set.contains(name) {
            return Err(format!("init_order is missing component '{name}'"));
        }
    }
    for name in &order_set {
        if !name_set.contains(name) {
            return Err(format!("init_order references unknown component '{name}'"));
        }
    }

    // Build position map.
    let position: HashMap<&str, usize> = init_order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    // Check all dependency edges are respected.
    for binding in bindings {
        if !name_set.contains(binding.target.as_str())
            || !name_set.contains(binding.source.as_str())
        {
            continue;
        }
        let source_pos = position.get(binding.source.as_str()).unwrap();
        let target_pos = position.get(binding.target.as_str()).unwrap();
        if source_pos >= target_pos {
            return Err(format!(
                "init_order violation: '{}' must come before '{}' \
                 (binding: {}.{} <- {})",
                binding.source, binding.target, binding.target, binding.receptacle, binding.source
            ));
        }
    }

    Ok(())
}
