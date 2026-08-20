//! NUMA topology discovery via sysfs.

use std::fs;
use std::io;

use super::cpuset::CpuSet;
use super::NumaError;

/// A single NUMA node with its associated CPU set and distance table.
///
/// # Examples
///
/// ```no_run
/// use component_core::numa::NumaTopology;
///
/// let topo = NumaTopology::discover().unwrap();
/// let node = topo.node(0).unwrap();
/// println!("Node {} has {} CPUs", node.id(), node.cpus().count());
/// ```
#[derive(Debug, Clone)]
pub struct NumaNode {
    id: usize,
    cpus: CpuSet,
    distances: Vec<u32>,
}

impl NumaNode {
    /// The NUMA node ID.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// assert_eq!(topo.node(0).unwrap().id(), 0);
    /// ```
    pub fn id(&self) -> usize {
        self.id
    }

    /// The CPUs belonging to this node.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// let cpus = topo.node(0).unwrap().cpus();
    /// assert!(!cpus.is_empty());
    /// ```
    pub fn cpus(&self) -> &CpuSet {
        &self.cpus
    }

    /// NUMA distances to all nodes. Index is the target node ID.
    ///
    /// Local distance is typically 10; cross-node distance is higher.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// let node = topo.node(0).unwrap();
    /// // Local distance
    /// assert_eq!(node.distance_to(0), Some(10));
    /// ```
    pub fn distances(&self) -> &[u32] {
        &self.distances
    }

    /// Get the NUMA distance from this node to another node.
    ///
    /// Returns `None` if `other_node` is out of range.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// let node = topo.node(0).unwrap();
    /// assert!(node.distance_to(0).is_some());
    /// assert!(node.distance_to(9999).is_none());
    /// ```
    pub fn distance_to(&self, other_node: usize) -> Option<u32> {
        self.distances.get(other_node).copied()
    }
}

/// Runtime representation of the system's NUMA layout.
///
/// Discovered by reading `/sys/devices/system/node/` on Linux.
/// On non-NUMA systems, falls back to a single node containing all online CPUs.
///
/// # Examples
///
/// ```no_run
/// use component_core::numa::NumaTopology;
///
/// let topo = NumaTopology::discover().unwrap();
/// println!("{} NUMA node(s)", topo.node_count());
/// for node in topo.nodes() {
///     let cpus: Vec<usize> = node.cpus().iter().collect();
///     println!("  Node {}: CPUs {:?}", node.id(), cpus);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct NumaTopology {
    nodes: Vec<NumaNode>,
}

impl NumaTopology {
    /// Discover the system's NUMA topology from sysfs.
    ///
    /// Falls back to a single node containing all online CPUs if NUMA
    /// information is unavailable (e.g., in a VM without NUMA support).
    ///
    /// # Errors
    ///
    /// Returns [`NumaError::TopologyUnavailable`] only if even the fallback
    /// (reading online CPU count) fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// assert!(topo.node_count() >= 1);
    /// ```
    pub fn discover() -> Result<Self, NumaError> {
        match discover_from_sysfs() {
            Ok(topo) => Ok(topo),
            Err(_) => {
                // Fallback: single node with all online CPUs
                let cpus = super::cpuset::read_online_cpus().map_err(|e| {
                    NumaError::TopologyUnavailable(format!("cannot read online CPUs: {e}"))
                })?;
                Ok(Self {
                    nodes: vec![NumaNode {
                        id: 0,
                        cpus,
                        distances: vec![10],
                    }],
                })
            }
        }
    }

    /// Number of NUMA nodes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// assert!(topo.node_count() >= 1);
    /// ```
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get a NUMA node by ID.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// assert!(topo.node(0).is_some());
    /// ```
    pub fn node(&self, id: usize) -> Option<&NumaNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// All NUMA nodes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// for node in topo.nodes() {
    ///     println!("Node {}", node.id());
    /// }
    /// ```
    pub fn nodes(&self) -> &[NumaNode] {
        &self.nodes
    }

    /// Find which NUMA node a CPU belongs to.
    ///
    /// Returns `None` if the CPU is not found in any node.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// // CPU 0 should be on some node
    /// assert!(topo.node_for_cpu(0).is_some());
    /// ```
    pub fn node_for_cpu(&self, cpu_id: usize) -> Option<usize> {
        self.nodes
            .iter()
            .find(|n| n.cpus.contains(cpu_id))
            .map(|n| n.id)
    }

    /// Get a [`CpuSet`] containing all online CPUs across all nodes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaTopology;
    ///
    /// let topo = NumaTopology::discover().unwrap();
    /// let all = topo.online_cpus();
    /// assert!(!all.is_empty());
    /// ```
    pub fn online_cpus(&self) -> CpuSet {
        let mut all = CpuSet::new();
        for node in &self.nodes {
            for cpu in node.cpus.iter() {
                let _ = all.add(cpu);
            }
        }
        all
    }
}

/// Read NUMA topology from `/sys/devices/system/node/`.
fn discover_from_sysfs() -> Result<NumaTopology, io::Error> {
    let online_nodes = fs::read_to_string("/sys/devices/system/node/online")?;
    let node_ids = parse_range_list(online_nodes.trim())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let mut nodes = Vec::with_capacity(node_ids.len());

    for node_id in node_ids {
        let cpulist_path = format!("/sys/devices/system/node/node{node_id}/cpulist");
        let cpulist = fs::read_to_string(&cpulist_path)?;
        let cpu_ids = parse_range_list(cpulist.trim())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let cpus = CpuSet::from_cpus(cpu_ids)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let distances = read_distances(node_id)?;

        nodes.push(NumaNode {
            id: node_id,
            cpus,
            distances,
        });
    }

    Ok(NumaTopology { nodes })
}

/// Read NUMA distances for a node from sysfs.
fn read_distances(node_id: usize) -> Result<Vec<u32>, io::Error> {
    let path = format!("/sys/devices/system/node/node{node_id}/distance");
    let content = fs::read_to_string(&path)?;
    content
        .split_whitespace()
        .map(|s| {
            s.parse::<u32>()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })
        .collect()
}

/// Parse a sysfs range list string (e.g., "0-15,32-47") into a sorted list
/// of individual IDs.
///
/// Supported formats:
/// - Single values: `"5"` → `[5]`
/// - Ranges: `"0-3"` → `[0, 1, 2, 3]`
/// - Mixed: `"0-3,7,10-12"` → `[0, 1, 2, 3, 7, 10, 11, 12]`
///
/// # Errors
///
/// Returns [`NumaError::TopologyUnavailable`] if parsing fails.
///
/// # Examples
///
/// ```
/// use component_core::numa::parse_range_list;
///
/// assert_eq!(parse_range_list("0-3").unwrap(), vec![0, 1, 2, 3]);
/// assert_eq!(parse_range_list("0-1,4-5").unwrap(), vec![0, 1, 4, 5]);
/// assert_eq!(parse_range_list("7").unwrap(), vec![7]);
/// ```
pub fn parse_range_list(s: &str) -> Result<Vec<usize>, NumaError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start: usize = start_str.trim().parse().map_err(|_| {
                NumaError::TopologyUnavailable(format!("invalid range start: {start_str}"))
            })?;
            let end: usize = end_str.trim().parse().map_err(|_| {
                NumaError::TopologyUnavailable(format!("invalid range end: {end_str}"))
            })?;
            for id in start..=end {
                result.push(id);
            }
        } else {
            let id: usize = part.parse().map_err(|_| {
                NumaError::TopologyUnavailable(format!("invalid CPU/node ID: {part}"))
            })?;
            result.push(id);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_value() {
        assert_eq!(parse_range_list("5").unwrap(), vec![5]);
    }

    #[test]
    fn parse_single_range() {
        assert_eq!(parse_range_list("0-3").unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn parse_mixed_ranges_and_singles() {
        assert_eq!(
            parse_range_list("0-2,5,10-12").unwrap(),
            vec![0, 1, 2, 5, 10, 11, 12]
        );
    }

    #[test]
    fn parse_single_element_range() {
        assert_eq!(parse_range_list("0-0").unwrap(), vec![0]);
    }

    #[test]
    fn parse_empty_string() {
        assert_eq!(parse_range_list("").unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn parse_invalid_returns_error() {
        assert!(parse_range_list("abc").is_err());
        assert!(parse_range_list("1-abc").is_err());
    }

    #[test]
    fn discover_returns_at_least_one_node() {
        let topo = NumaTopology::discover().unwrap();
        assert!(topo.node_count() >= 1);
    }

    #[test]
    fn discover_nodes_have_cpus() {
        let topo = NumaTopology::discover().unwrap();
        for node in topo.nodes() {
            assert!(!node.cpus().is_empty(), "node {} has no CPUs", node.id());
        }
    }

    #[test]
    fn all_cpus_in_exactly_one_node() {
        let topo = NumaTopology::discover().unwrap();
        let all_cpus = topo.online_cpus();
        for cpu in all_cpus.iter() {
            let mut count = 0;
            for node in topo.nodes() {
                if node.cpus().contains(cpu) {
                    count += 1;
                }
            }
            assert_eq!(count, 1, "CPU {cpu} appears in {count} nodes");
        }
    }

    #[test]
    fn node_for_cpu_finds_correct_node() {
        let topo = NumaTopology::discover().unwrap();
        for node in topo.nodes() {
            for cpu in node.cpus().iter() {
                assert_eq!(topo.node_for_cpu(cpu), Some(node.id()));
            }
        }
    }

    #[test]
    fn node_for_cpu_returns_none_for_invalid() {
        let topo = NumaTopology::discover().unwrap();
        assert!(topo.node_for_cpu(99999).is_none());
    }

    #[test]
    fn node_by_id() {
        let topo = NumaTopology::discover().unwrap();
        let first = &topo.nodes()[0];
        assert_eq!(topo.node(first.id()).unwrap().id(), first.id());
    }

    #[test]
    fn node_distances_present() {
        let topo = NumaTopology::discover().unwrap();
        for node in topo.nodes() {
            assert!(
                !node.distances().is_empty(),
                "node {} has no distances",
                node.id()
            );
            // Local distance should be the minimum
            let local = node.distance_to(node.id());
            assert!(local.is_some(), "node {} missing local distance", node.id());
        }
    }
}
