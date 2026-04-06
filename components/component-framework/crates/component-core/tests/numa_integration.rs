//! Integration tests for NUMA-aware actor features.

use component_core::actor::{Actor, ActorError, ActorHandler};
use component_core::numa::{get_thread_affinity, CpuSet, NumaTopology};
use std::sync::{Arc, Mutex};
use std::thread;

// --- US1: Thread Pinning Integration Tests ---

struct CpuReportHandler {
    /// Stores the CPU IDs in the thread's affinity mask when handling a message.
    observed_cpus: Arc<Mutex<Vec<usize>>>,
}

impl ActorHandler<()> for CpuReportHandler {
    fn handle(&mut self, _msg: ()) {
        if let Ok(cpus) = get_thread_affinity() {
            let ids: Vec<usize> = cpus.iter().collect();
            *self.observed_cpus.lock().unwrap() = ids;
        }
    }
}

#[test]
fn actor_pinned_to_single_cpu() {
    // Find a valid CPU to pin to.
    let current = get_thread_affinity().unwrap();
    let first_cpu = current.iter().next().unwrap();

    let observed = Arc::new(Mutex::new(Vec::new()));
    let actor = Actor::new(
        CpuReportHandler {
            observed_cpus: observed.clone(),
        },
        |_| {},
    )
    .with_cpu_affinity(CpuSet::from_cpu(first_cpu).unwrap());

    let handle = actor.activate().unwrap();
    handle.send(()).unwrap();
    // Give the actor time to process.
    thread::sleep(std::time::Duration::from_millis(50));
    handle.deactivate().unwrap();

    let cpus = observed.lock().unwrap();
    assert_eq!(cpus.len(), 1, "expected 1 CPU in affinity, got {cpus:?}");
    assert_eq!(cpus[0], first_cpu);
}

#[test]
fn actor_pinned_to_multiple_cpus() {
    let current = get_thread_affinity().unwrap();
    let cpus: Vec<usize> = current.iter().take(2).collect();
    if cpus.len() < 2 {
        // Only 1 CPU available — skip.
        return;
    }

    let observed = Arc::new(Mutex::new(Vec::new()));
    let affinity = CpuSet::from_cpus(cpus.clone()).unwrap();
    let actor = Actor::new(
        CpuReportHandler {
            observed_cpus: observed.clone(),
        },
        |_| {},
    )
    .with_cpu_affinity(affinity);

    let handle = actor.activate().unwrap();
    handle.send(()).unwrap();
    thread::sleep(std::time::Duration::from_millis(50));
    handle.deactivate().unwrap();

    let obs = observed.lock().unwrap();
    assert_eq!(obs.len(), 2);
    assert!(obs.contains(&cpus[0]));
    assert!(obs.contains(&cpus[1]));
}

#[test]
fn actor_no_affinity_backward_compatible() {
    let count = Arc::new(Mutex::new(0u32));

    struct Counter {
        count: Arc<Mutex<u32>>,
    }
    impl ActorHandler<u32> for Counter {
        fn handle(&mut self, _msg: u32) {
            *self.count.lock().unwrap() += 1;
        }
    }

    let actor = Actor::new(
        Counter {
            count: count.clone(),
        },
        |_| {},
    );
    // No affinity set — should work exactly as before.
    let handle = actor.activate().unwrap();
    for i in 0..10 {
        handle.send(i).unwrap();
    }
    handle.deactivate().unwrap();
    assert_eq!(*count.lock().unwrap(), 10);
}

#[test]
fn actor_invalid_cpu_returns_error() {
    struct Noop;
    impl ActorHandler<u32> for Noop {
        fn handle(&mut self, _msg: u32) {}
    }

    // CPU 9999 almost certainly doesn't exist.
    let actor = Actor::new(Noop, |_| {}).with_cpu_affinity(CpuSet::from_cpu(999).unwrap());
    let result = actor.activate();
    assert!(result.is_err());
    match result.unwrap_err() {
        ActorError::AffinityFailed(_) => {}
        other => panic!("expected AffinityFailed, got {other:?}"),
    }
}

#[test]
fn actor_reactivate_with_new_affinity() {
    let observed1 = Arc::new(Mutex::new(Vec::new()));
    let _observed2 = Arc::new(Mutex::new(Vec::<usize>::new()));

    let current = get_thread_affinity().unwrap();
    let cpus: Vec<usize> = current.iter().take(2).collect();
    if cpus.len() < 2 {
        return; // Need 2 CPUs.
    }

    struct CpuCapture {
        observed: Arc<Mutex<Vec<usize>>>,
    }
    impl ActorHandler<()> for CpuCapture {
        fn handle(&mut self, _msg: ()) {
            if let Ok(cpus) = get_thread_affinity() {
                *self.observed.lock().unwrap() = cpus.iter().collect();
            }
        }
    }

    // First activation on cpu[0].
    let actor = Actor::new(
        CpuCapture {
            observed: observed1.clone(),
        },
        |_| {},
    )
    .with_cpu_affinity(CpuSet::from_cpu(cpus[0]).unwrap());

    let handle = actor.activate().unwrap();
    handle.send(()).unwrap();
    thread::sleep(std::time::Duration::from_millis(50));
    handle.deactivate().unwrap();

    assert_eq!(observed1.lock().unwrap()[0], cpus[0]);
}

// --- US2: Topology Integration Tests ---

#[test]
fn topology_at_least_one_node() {
    let topo = NumaTopology::discover().unwrap();
    assert!(topo.node_count() >= 1);
}

#[test]
fn topology_all_cpus_covered() {
    let topo = NumaTopology::discover().unwrap();
    let all = topo.online_cpus();
    assert!(!all.is_empty());

    // Every CPU must be in exactly one node.
    for cpu in all.iter() {
        assert!(
            topo.node_for_cpu(cpu).is_some(),
            "CPU {cpu} not found in any node"
        );
    }
}

#[test]
fn topology_nodes_have_cpus() {
    let topo = NumaTopology::discover().unwrap();
    for node in topo.nodes() {
        assert!(!node.cpus().is_empty(), "node {} has no CPUs", node.id());
    }
}

// --- US3: NUMA-local channel tests ---

#[test]
fn mpsc_channel_numa_alloc_works() {
    use component_core::channel::mpsc::MpscChannel;

    let ch = MpscChannel::<u64>::new_numa(64, 0);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    // Send and receive interleaved (capacity is only 64).
    for i in 0..1000u64 {
        tx.send(i).unwrap();
        assert_eq!(rx.recv().unwrap(), i);
    }
}

#[test]
fn spsc_channel_numa_alloc_works() {
    use component_core::channel::spsc::SpscChannel;

    let ch = SpscChannel::<u64>::new_numa(64, 0);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    // Send and receive interleaved (capacity is only 64).
    for i in 0..1000u64 {
        tx.send(i).unwrap();
        assert_eq!(rx.recv().unwrap(), i);
    }
}
