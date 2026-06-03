//! Unit tests for topological sort and init_order validation.

/// Simplified binding for testing purposes.
struct TestBinding {
    target: String,
    source: String,
    receptacle: String,
}

fn to_binding_rules(bindings: &[TestBinding]) -> Vec<serde_json::Value> {
    bindings
        .iter()
        .map(|b| {
            serde_json::json!({
                "target": b.target,
                "source": b.source,
                "receptacle": b.receptacle,
            })
        })
        .collect()
}

fn topo_sort(names: &[&str], edges: &[(&str, &str)]) -> Result<Vec<String>, String> {
    // Kahn's algorithm (mirror of topology.rs logic for testing).
    use std::collections::{HashMap, VecDeque};

    let mut in_degree: HashMap<&str, usize> = names.iter().map(|&n| (n, 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = names.iter().map(|&n| (n, Vec::new())).collect();

    for &(source, target) in edges {
        adj.get_mut(source).unwrap().push(target);
        *in_degree.get_mut(target).unwrap() += 1;
    }

    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut zero: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&n, _)| n)
        .collect();
    zero.sort();
    for n in zero {
        queue.push_back(n);
    }

    let mut order: Vec<String> = Vec::new();
    while let Some(node) = queue.pop_front() {
        order.push(node.to_string());
        let mut next: Vec<&str> = Vec::new();
        for &neighbor in adj.get(node).unwrap() {
            let deg = in_degree.get_mut(neighbor).unwrap();
            *deg -= 1;
            if *deg == 0 {
                next.push(neighbor);
            }
        }
        next.sort();
        for n in next {
            queue.push_back(n);
        }
    }

    if order.len() != names.len() {
        let remaining: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(&n, _)| n)
            .collect();
        return Err(format!("cycle: {:?}", remaining));
    }

    Ok(order)
}

#[test]
fn test_linear_chain() {
    // A -> B -> C (C depends on B, B depends on A)
    let names = ["a", "b", "c"];
    let edges = [("a", "b"), ("b", "c")];
    let order = topo_sort(&names, &edges).unwrap();
    let pos_a = order.iter().position(|x| x == "a").unwrap();
    let pos_b = order.iter().position(|x| x == "b").unwrap();
    let pos_c = order.iter().position(|x| x == "c").unwrap();
    assert!(pos_a < pos_b);
    assert!(pos_b < pos_c);
}

#[test]
fn test_diamond_dependency() {
    // A -> B, A -> C, B -> D, C -> D
    let names = ["a", "b", "c", "d"];
    let edges = [("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")];
    let order = topo_sort(&names, &edges).unwrap();
    let pos_a = order.iter().position(|x| x == "a").unwrap();
    let pos_b = order.iter().position(|x| x == "b").unwrap();
    let pos_c = order.iter().position(|x| x == "c").unwrap();
    let pos_d = order.iter().position(|x| x == "d").unwrap();
    assert!(pos_a < pos_b);
    assert!(pos_a < pos_c);
    assert!(pos_b < pos_d);
    assert!(pos_c < pos_d);
}

#[test]
fn test_cycle_detection() {
    // A -> B -> C -> A (cycle)
    let names = ["a", "b", "c"];
    let edges = [("a", "b"), ("b", "c"), ("c", "a")];
    let result = topo_sort(&names, &edges);
    assert!(result.is_err());
}

#[test]
fn test_no_dependencies() {
    let names = ["c", "a", "b"];
    let edges: [(&str, &str); 0] = [];
    let order = topo_sort(&names, &edges).unwrap();
    // All should appear, deterministic (sorted) order
    assert_eq!(order.len(), 3);
    assert_eq!(order[0], "a");
    assert_eq!(order[1], "b");
    assert_eq!(order[2], "c");
}

#[test]
fn test_init_order_validation_valid() {
    // If explicit order respects all deps, it's valid.
    let names = ["a", "b", "c"];
    let edges = [("a", "b"), ("b", "c")];
    let explicit_order = vec!["a", "b", "c"];

    // Validate: for each edge (source, target), source must come before target.
    let positions: std::collections::HashMap<&str, usize> = explicit_order
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect();

    for (source, target) in &edges {
        assert!(positions[source] < positions[target]);
    }
}

#[test]
fn test_init_order_validation_invalid() {
    // Order violates dependency.
    let edges = [("a", "b")];
    let explicit_order = vec!["b", "a"]; // b before a, but a must come first

    let positions: std::collections::HashMap<&str, usize> = explicit_order
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect();

    let mut violation_found = false;
    for (source, target) in &edges {
        if positions[source] >= positions[target] {
            violation_found = true;
        }
    }
    assert!(violation_found);
}
