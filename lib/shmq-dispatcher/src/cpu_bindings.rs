//! Report which CPU cores the process's threads are bound to.
//!
//! The pinning itself happens inside the components (NVMe actor threads via the
//! dispatcher, the remote-lookup actor, the shmq poller in [`serve`](crate::serve)),
//! so the server front-ends read the kernel's view back from
//! `/proc/self/task/*/status` rather than re-deriving each component's policy.

use std::fs;

use interfaces::ILogger;

/// One thread's CPU affinity as reported by the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadCpuBinding {
    /// Kernel thread id.
    pub tid: u32,
    /// Thread name (`comm`, at most 15 bytes).
    pub name: String,
    /// The `Cpus_allowed_list` value, e.g. `"4"` or `"0-15,32-47"`.
    pub cpus: String,
}

/// Read the CPU affinity of every thread in this process, sorted by tid.
///
/// Threads that exit while the table is being read are skipped. Returns an
/// empty vector if `/proc/self/task` is unavailable.
///
/// # Examples
///
/// ```
/// let bindings = shmq_dispatcher::thread_cpu_bindings();
/// // The calling thread is always present on Linux.
/// assert!(!bindings.is_empty());
/// assert!(bindings.iter().all(|b| !b.cpus.is_empty()));
/// ```
pub fn thread_cpu_bindings() -> Vec<ThreadCpuBinding> {
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let mut out: Vec<ThreadCpuBinding> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let tid: u32 = e.file_name().to_str()?.parse().ok()?;
            let status = fs::read_to_string(e.path().join("status")).ok()?;
            let mut name = String::new();
            let mut cpus = None;
            for line in status.lines() {
                if let Some(v) = line.strip_prefix("Name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Cpus_allowed_list:") {
                    cpus = Some(v.trim().to_string());
                }
            }
            Some(ThreadCpuBinding {
                tid,
                name,
                cpus: cpus?,
            })
        })
        .collect();
    out.sort_by_key(|b| b.tid);
    out
}

/// Log, at info level, the system's online CPUs and every set of threads bound
/// to a narrower set of cores.
///
/// Threads are grouped by CPU set, one line per set, listing each thread name
/// with its count (e.g. `CPU(s) 4: nvme-actor`, `CPU(s) 0: shmq-worker x16`).
/// Threads allowed on every online CPU are not listed. Each line is prefixed
/// with `prefix` (typically the binary name).
///
/// # Examples
///
/// ```
/// use interfaces::ILogger;
///
/// struct Stdout;
/// impl ILogger for Stdout {
///     fn error(&self, m: &str) { println!("{m}") }
///     fn warn(&self, m: &str) { println!("{m}") }
///     fn info(&self, m: &str) { println!("{m}") }
///     fn debug(&self, m: &str) { println!("{m}") }
/// }
///
/// shmq_dispatcher::log_cpu_bindings(&Stdout, "example");
/// ```
pub fn log_cpu_bindings(logger: &dyn ILogger, prefix: &str) {
    // Compare against the online CPUs rather than the main thread's affinity:
    // the main thread itself may have been pinned (DPDK EAL pins its caller).
    let online = fs::read_to_string("/sys/devices/system/cpu/online")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    logger.info(&format!("{prefix}: system online CPUs = {online}"));

    // CPU set -> (thread name -> count), both in first-seen (tid) order.
    let mut groups: Vec<(String, Vec<(String, usize)>)> = Vec::new();
    for b in thread_cpu_bindings()
        .into_iter()
        .filter(|b| b.cpus != online)
    {
        let names = match groups.iter_mut().find(|(cpus, _)| *cpus == b.cpus) {
            Some((_, names)) => names,
            None => {
                groups.push((b.cpus, Vec::new()));
                &mut groups.last_mut().expect("just pushed").1
            }
        };
        match names.iter_mut().find(|(n, _)| *n == b.name) {
            Some((_, count)) => *count += 1,
            None => names.push((b.name, 1)),
        }
    }

    if groups.is_empty() {
        logger.info(&format!(
            "{prefix}: no threads pinned to specific CPU cores"
        ));
        return;
    }
    for (cpus, names) in groups {
        let threads: Vec<String> = names
            .into_iter()
            .map(|(n, c)| if c > 1 { format!("{n} x{c}") } else { n })
            .collect();
        logger.info(&format!("{prefix}: CPU(s) {cpus}: {}", threads.join(", ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};

    #[derive(Default)]
    struct Capture(Mutex<Vec<String>>);
    impl ILogger for Capture {
        fn error(&self, m: &str) {
            self.0.lock().unwrap().push(m.into());
        }
        fn warn(&self, m: &str) {
            self.0.lock().unwrap().push(m.into());
        }
        fn info(&self, m: &str) {
            self.0.lock().unwrap().push(m.into());
        }
        fn debug(&self, m: &str) {
            self.0.lock().unwrap().push(m.into());
        }
    }

    #[test]
    fn pinned_thread_is_reported_by_name() {
        let online = fs::read_to_string("/sys/devices/system/cpu/online").unwrap();
        if online.trim() == "0" {
            return; // single-CPU host: pinning to CPU 0 is indistinguishable
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let handle = std::thread::Builder::new()
            .name("cpub-test".into())
            .spawn(move || {
                // SAFETY: zeroed cpu_set_t is valid; we pin only this thread.
                unsafe {
                    let mut set: libc::cpu_set_t = std::mem::zeroed();
                    libc::CPU_SET(0, &mut set);
                    libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
                }
                ready_tx.send(()).unwrap();
                let _ = done_rx.recv();
            })
            .unwrap();
        ready_rx.recv().unwrap();

        let log = Capture::default();
        log_cpu_bindings(&log, "t");
        done_tx.send(()).unwrap();
        handle.join().unwrap();

        let lines = log.0.lock().unwrap();
        assert!(lines[0].starts_with("t: system online CPUs = "));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("t: CPU(s) 0: ") && l.contains("cpub-test")),
            "{lines:?}"
        );
    }
}
