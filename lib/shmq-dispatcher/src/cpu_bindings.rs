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

/// Log, at info level, the process CPU set and every thread that has been
/// bound to a narrower set of cores (i.e. explicitly pinned).
///
/// Each line is prefixed with `prefix` (typically the binary name).
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
    let bindings = thread_cpu_bindings();
    let pid = std::process::id();
    // The main thread (tid == pid) keeps the process-wide allowed set.
    let process_cpus = bindings
        .iter()
        .find(|b| b.tid == pid)
        .map(|b| b.cpus.clone())
        .unwrap_or_else(|| "unknown".into());
    logger.info(&format!("{prefix}: process CPU set = {process_cpus}"));

    let pinned: Vec<&ThreadCpuBinding> =
        bindings.iter().filter(|b| b.cpus != process_cpus).collect();
    if pinned.is_empty() {
        logger.info(&format!(
            "{prefix}: no threads pinned to specific CPU cores"
        ));
        return;
    }
    for b in pinned {
        logger.info(&format!(
            "{prefix}: thread '{}' (tid {}) bound to CPU(s) {}",
            b.name, b.tid, b.cpus
        ));
    }
}
