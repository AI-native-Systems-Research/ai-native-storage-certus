//! CPU affinity set and thread pinning.

use std::fmt;
use std::fs;
use std::io;

use super::NumaError;

/// A set of CPU core IDs representing a thread affinity mask.
///
/// Wraps `libc::cpu_set_t` and provides a safe builder API for constructing
/// affinity masks. CPU IDs must be in the range `0..CPU_SETSIZE` (typically
/// 1024 on Linux x86-64).
///
/// # Examples
///
/// ```
/// use component_core::numa::CpuSet;
///
/// let mut cpus = CpuSet::new();
/// cpus.add(0).unwrap();
/// cpus.add(1).unwrap();
/// assert_eq!(cpus.count(), 2);
/// assert!(cpus.contains(0));
/// assert!(cpus.contains(1));
/// assert!(!cpus.contains(2));
/// ```
pub struct CpuSet {
    inner: libc::cpu_set_t,
}

// libc::cpu_set_t is a plain data struct (array of u64) — safe to send/share.
// SAFETY: cpu_set_t contains only integer data with no pointers or interior
// mutability. It is safe to move between threads and share via immutable refs.
unsafe impl Send for CpuSet {}
unsafe impl Sync for CpuSet {}

impl Clone for CpuSet {
    fn clone(&self) -> Self {
        // SAFETY: cpu_set_t is plain data (array of u64), bit-for-bit copy is correct.
        let mut inner: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        // Copy the bits array manually
        unsafe {
            std::ptr::copy_nonoverlapping(
                &self.inner as *const libc::cpu_set_t as *const u8,
                &mut inner as *mut libc::cpu_set_t as *mut u8,
                std::mem::size_of::<libc::cpu_set_t>(),
            );
        }
        Self { inner }
    }
}

impl fmt::Debug for CpuSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cpus: Vec<usize> = self.iter().collect();
        f.debug_struct("CpuSet").field("cpus", &cpus).finish()
    }
}

const MAX_CPU: usize = libc::CPU_SETSIZE as usize;

impl CpuSet {
    /// Create an empty CPU set.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::new();
    /// assert!(cpus.is_empty());
    /// assert_eq!(cpus.count(), 0);
    /// ```
    pub fn new() -> Self {
        // SAFETY: zeroing a cpu_set_t produces a valid empty set.
        let mut inner: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe { libc::CPU_ZERO(&mut inner) };
        Self { inner }
    }

    /// Create a CPU set containing a single CPU.
    ///
    /// # Errors
    ///
    /// Returns [`NumaError::CpuOutOfRange`] if `cpu_id` >= `CPU_SETSIZE`.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::from_cpu(0).unwrap();
    /// assert_eq!(cpus.count(), 1);
    /// assert!(cpus.contains(0));
    /// ```
    ///
    /// ```
    /// use component_core::numa::{CpuSet, NumaError};
    ///
    /// let err = CpuSet::from_cpu(99999).unwrap_err();
    /// assert!(matches!(err, NumaError::CpuOutOfRange { .. }));
    /// ```
    pub fn from_cpu(cpu_id: usize) -> Result<Self, NumaError> {
        let mut set = Self::new();
        set.add(cpu_id)?;
        Ok(set)
    }

    /// Create a CPU set from an iterator of CPU IDs.
    ///
    /// # Errors
    ///
    /// Returns [`NumaError::CpuOutOfRange`] if any CPU ID >= `CPU_SETSIZE`.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::from_cpus([0, 1, 2, 3]).unwrap();
    /// assert_eq!(cpus.count(), 4);
    /// ```
    pub fn from_cpus(cpus: impl IntoIterator<Item = usize>) -> Result<Self, NumaError> {
        let mut set = Self::new();
        for cpu in cpus {
            set.add(cpu)?;
        }
        Ok(set)
    }

    /// Add a CPU to the set.
    ///
    /// # Errors
    ///
    /// Returns [`NumaError::CpuOutOfRange`] if `cpu_id` >= `CPU_SETSIZE`.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let mut cpus = CpuSet::new();
    /// cpus.add(4).unwrap();
    /// assert!(cpus.contains(4));
    /// ```
    pub fn add(&mut self, cpu_id: usize) -> Result<(), NumaError> {
        if cpu_id >= MAX_CPU {
            return Err(NumaError::CpuOutOfRange {
                cpu: cpu_id,
                max: MAX_CPU - 1,
            });
        }
        // SAFETY: cpu_id is in bounds (< CPU_SETSIZE).
        unsafe { libc::CPU_SET(cpu_id, &mut self.inner) };
        Ok(())
    }

    /// Remove a CPU from the set.
    ///
    /// No-op if the CPU was not in the set.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let mut cpus = CpuSet::from_cpu(3).unwrap();
    /// cpus.remove(3);
    /// assert!(!cpus.contains(3));
    /// ```
    pub fn remove(&mut self, cpu_id: usize) {
        if cpu_id < MAX_CPU {
            // SAFETY: cpu_id is in bounds.
            unsafe { libc::CPU_CLR(cpu_id, &mut self.inner) };
        }
    }

    /// Check whether a CPU is in the set.
    ///
    /// Returns `false` for out-of-range CPU IDs.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::from_cpu(7).unwrap();
    /// assert!(cpus.contains(7));
    /// assert!(!cpus.contains(8));
    /// ```
    pub fn contains(&self, cpu_id: usize) -> bool {
        if cpu_id >= MAX_CPU {
            return false;
        }
        // SAFETY: cpu_id is in bounds.
        unsafe { libc::CPU_ISSET(cpu_id, &self.inner) }
    }

    /// Return the number of CPUs in the set.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::from_cpus([0, 2, 4]).unwrap();
    /// assert_eq!(cpus.count(), 3);
    /// ```
    pub fn count(&self) -> usize {
        // SAFETY: the cpu_set_t is valid.
        unsafe { libc::CPU_COUNT(&self.inner) as usize }
    }

    /// Check whether the set is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// assert!(CpuSet::new().is_empty());
    /// assert!(!CpuSet::from_cpu(0).unwrap().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Iterate over the CPU IDs in the set, in ascending order.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::CpuSet;
    ///
    /// let cpus = CpuSet::from_cpus([3, 1, 5]).unwrap();
    /// let ids: Vec<usize> = cpus.iter().collect();
    /// assert_eq!(ids, vec![1, 3, 5]);
    /// ```
    pub fn iter(&self) -> CpuSetIter<'_> {
        CpuSetIter { set: self, pos: 0 }
    }

    /// Access the underlying `libc::cpu_set_t` for direct syscall use.
    pub fn as_raw(&self) -> &libc::cpu_set_t {
        &self.inner
    }

    /// Access the underlying `libc::cpu_set_t` mutably.
    pub fn as_raw_mut(&mut self) -> &mut libc::cpu_set_t {
        &mut self.inner
    }
}

impl Default for CpuSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator over CPU IDs in a [`CpuSet`].
pub struct CpuSetIter<'a> {
    set: &'a CpuSet,
    pos: usize,
}

impl<'a> Iterator for CpuSetIter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.pos < MAX_CPU {
            let cpu = self.pos;
            self.pos += 1;
            if self.set.contains(cpu) {
                return Some(cpu);
            }
        }
        None
    }
}

/// Set the calling thread's CPU affinity to the given [`CpuSet`].
///
/// # Errors
///
/// Returns [`NumaError::EmptyCpuSet`] if the set is empty.
/// Returns [`NumaError::AffinityFailed`] if the syscall fails.
///
/// # Examples
///
/// ```no_run
/// use component_core::numa::{CpuSet, set_thread_affinity, get_thread_affinity};
///
/// let cpus = CpuSet::from_cpu(0).unwrap();
/// set_thread_affinity(&cpus).unwrap();
///
/// let actual = get_thread_affinity().unwrap();
/// assert!(actual.contains(0));
/// ```
pub fn set_thread_affinity(cpuset: &CpuSet) -> Result<(), NumaError> {
    if cpuset.is_empty() {
        return Err(NumaError::EmptyCpuSet);
    }
    // SAFETY: pid=0 targets the calling thread. cpuset is a valid cpu_set_t.
    let ret = unsafe {
        libc::sched_setaffinity(
            0, // current thread
            std::mem::size_of::<libc::cpu_set_t>(),
            cpuset.as_raw(),
        )
    };
    if ret != 0 {
        let errno = io::Error::last_os_error();
        return Err(NumaError::AffinityFailed(errno.to_string()));
    }
    Ok(())
}

/// Get the calling thread's CPU affinity.
///
/// # Errors
///
/// Returns [`NumaError::AffinityFailed`] if the syscall fails.
///
/// # Examples
///
/// ```no_run
/// use component_core::numa::get_thread_affinity;
///
/// let cpus = get_thread_affinity().unwrap();
/// assert!(!cpus.is_empty());
/// ```
pub fn get_thread_affinity() -> Result<CpuSet, NumaError> {
    let mut set = CpuSet::new();
    // SAFETY: pid=0 targets the calling thread. set is a valid cpu_set_t.
    let ret = unsafe {
        libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), set.as_raw_mut())
    };
    if ret != 0 {
        let errno = io::Error::last_os_error();
        return Err(NumaError::AffinityFailed(errno.to_string()));
    }
    Ok(set)
}

/// Validate that all CPUs in the set are online on this system.
///
/// Reads `/sys/devices/system/cpu/online` to determine which CPUs are
/// available. Returns an error for any CPU that is not online.
///
/// # Errors
///
/// - [`NumaError::CpuOffline`] if a CPU in the set is not online
/// - [`NumaError::AffinityFailed`] if the online CPU list cannot be read
pub fn validate_cpus(cpuset: &CpuSet) -> Result<(), NumaError> {
    if cpuset.is_empty() {
        return Err(NumaError::EmptyCpuSet);
    }

    let online = read_online_cpus().map_err(|e| NumaError::AffinityFailed(e.to_string()))?;

    for cpu in cpuset.iter() {
        if !online.contains(cpu) {
            return Err(NumaError::CpuOffline(cpu));
        }
    }
    Ok(())
}

/// Read the set of online CPUs from sysfs.
pub(crate) fn read_online_cpus() -> Result<CpuSet, io::Error> {
    let content = fs::read_to_string("/sys/devices/system/cpu/online")?;
    parse_range_list_to_cpuset(content.trim())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Parse a sysfs range list (e.g. "0-15,32-47") into a CpuSet.
pub(crate) fn parse_range_list_to_cpuset(s: &str) -> Result<CpuSet, NumaError> {
    let ids = super::topology::parse_range_list(s)?;
    CpuSet::from_cpus(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_set() {
        let cpus = CpuSet::new();
        assert!(cpus.is_empty());
        assert_eq!(cpus.count(), 0);
    }

    #[test]
    fn from_cpu_creates_singleton() {
        let cpus = CpuSet::from_cpu(5).unwrap();
        assert_eq!(cpus.count(), 1);
        assert!(cpus.contains(5));
        assert!(!cpus.contains(0));
    }

    #[test]
    fn from_cpu_rejects_out_of_range() {
        let err = CpuSet::from_cpu(99999).unwrap_err();
        assert!(matches!(err, NumaError::CpuOutOfRange { cpu: 99999, .. }));
    }

    #[test]
    fn from_cpus_builds_set() {
        let cpus = CpuSet::from_cpus([0, 3, 7]).unwrap();
        assert_eq!(cpus.count(), 3);
        assert!(cpus.contains(0));
        assert!(cpus.contains(3));
        assert!(cpus.contains(7));
        assert!(!cpus.contains(1));
    }

    #[test]
    fn add_and_remove() {
        let mut cpus = CpuSet::new();
        cpus.add(10).unwrap();
        assert!(cpus.contains(10));
        cpus.remove(10);
        assert!(!cpus.contains(10));
    }

    #[test]
    fn add_rejects_out_of_range() {
        let mut cpus = CpuSet::new();
        assert!(cpus.add(MAX_CPU).is_err());
    }

    #[test]
    fn contains_returns_false_for_out_of_range() {
        let cpus = CpuSet::new();
        assert!(!cpus.contains(MAX_CPU + 1));
    }

    #[test]
    fn remove_out_of_range_is_noop() {
        let mut cpus = CpuSet::from_cpu(0).unwrap();
        cpus.remove(MAX_CPU + 1); // should not panic
        assert!(cpus.contains(0));
    }

    #[test]
    fn iter_returns_sorted_cpus() {
        let cpus = CpuSet::from_cpus([5, 1, 3]).unwrap();
        let ids: Vec<usize> = cpus.iter().collect();
        assert_eq!(ids, vec![1, 3, 5]);
    }

    #[test]
    fn iter_empty_set() {
        let cpus = CpuSet::new();
        let ids: Vec<usize> = cpus.iter().collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn clone_preserves_contents() {
        let original = CpuSet::from_cpus([2, 4, 6]).unwrap();
        let cloned = original.clone();
        assert_eq!(cloned.count(), 3);
        assert!(cloned.contains(2));
        assert!(cloned.contains(4));
        assert!(cloned.contains(6));
    }

    #[test]
    fn default_is_empty() {
        let cpus = CpuSet::default();
        assert!(cpus.is_empty());
    }

    #[test]
    fn debug_format() {
        let cpus = CpuSet::from_cpus([1, 3]).unwrap();
        let s = format!("{cpus:?}");
        assert!(s.contains("1"));
        assert!(s.contains("3"));
    }

    #[test]
    fn set_thread_affinity_rejects_empty() {
        let cpus = CpuSet::new();
        let err = set_thread_affinity(&cpus).unwrap_err();
        assert_eq!(err, NumaError::EmptyCpuSet);
    }

    #[test]
    fn get_thread_affinity_returns_non_empty() {
        let cpus = get_thread_affinity().unwrap();
        assert!(!cpus.is_empty());
    }

    #[test]
    fn validate_cpus_rejects_empty() {
        let cpus = CpuSet::new();
        let err = validate_cpus(&cpus).unwrap_err();
        assert_eq!(err, NumaError::EmptyCpuSet);
    }

    #[test]
    fn read_online_cpus_returns_non_empty() {
        let cpus = read_online_cpus().unwrap();
        assert!(!cpus.is_empty());
    }
}
