//! Pre-flight checks for VFIO availability, permissions, and hugepages.
//!
//! These checks run before SPDK/DPDK initialization to provide actionable
//! error messages instead of opaque DPDK EAL failures.

use crate::error::SpdkEnvError;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Verify that VFIO is available on the system.
///
/// Checks:
/// 1. `/dev/vfio` directory exists
/// 2. `vfio-pci` kernel module is loaded (via `/sys/bus/pci/drivers/vfio-pci/`)
pub fn check_vfio_available() -> Result<(), SpdkEnvError> {
    check_vfio_available_at("/dev/vfio", "/sys/bus/pci/drivers/vfio-pci")
}

/// Testable version with configurable paths.
pub(crate) fn check_vfio_available_at(
    dev_vfio: &str,
    sysfs_driver: &str,
) -> Result<(), SpdkEnvError> {
    if !Path::new(dev_vfio).exists() {
        return Err(SpdkEnvError::VfioNotAvailable(format!(
            "{dev_vfio} not found. Ensure the vfio-pci kernel module is loaded: modprobe vfio-pci"
        )));
    }

    if !Path::new(sysfs_driver).exists() {
        return Err(SpdkEnvError::VfioNotAvailable(format!(
            "vfio-pci driver not loaded ({sysfs_driver} not found). Run: modprobe vfio-pci"
        )));
    }

    Ok(())
}

/// Verify read/write permissions on VFIO device paths.
///
/// Checks `/dev/vfio`, `/dev/vfio/vfio` (container device), and any IOMMU
/// group entries under `/dev/vfio/`.
pub fn check_vfio_permissions() -> Result<(), SpdkEnvError> {
    check_vfio_permissions_at("/dev/vfio")
}

/// Testable version with configurable base path.
pub(crate) fn check_vfio_permissions_at(dev_vfio: &str) -> Result<(), SpdkEnvError> {
    let vfio_dir = Path::new(dev_vfio);

    // Check the directory is readable (we need to list entries, not write).
    check_path_readable(vfio_dir)?;

    // Check the container device.
    let container = vfio_dir.join("vfio");
    if container.exists() {
        check_path_rw(&container)?;
    }

    // Check IOMMU group device files (numeric directories under /dev/vfio/).
    if let Ok(entries) = fs::read_dir(vfio_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // IOMMU groups are numeric directories.
            if name_str.chars().all(|c| c.is_ascii_digit()) {
                check_path_rw(&entry.path())?;
            }
        }
    }

    Ok(())
}

/// Check that the current user has read access to a path (e.g. a directory).
fn check_path_readable(path: &Path) -> Result<(), SpdkEnvError> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getegid() };

    match fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.mode();
            let file_uid = meta.uid();
            let file_gid = meta.gid();

            let has_access = if uid == file_uid {
                mode & 0o400 == 0o400
            } else if gid == file_gid {
                mode & 0o040 == 0o040
            } else {
                mode & 0o004 == 0o004
            };

            if !has_access {
                return Err(SpdkEnvError::PermissionDenied(format!(
                    "{} (need read for uid={uid}, gid={gid}; current mode={mode:04o}, \
                     owner={file_uid}:{file_gid}). Consider adding your user to the \
                     appropriate group or configuring udev rules.",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(e) => Err(SpdkEnvError::PermissionDenied(format!(
            "cannot stat {}: {e}",
            path.display()
        ))),
    }
}

/// Check that the current user has read+write access to a path.
fn check_path_rw(path: &Path) -> Result<(), SpdkEnvError> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getegid() };

    match fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.mode();
            let file_uid = meta.uid();
            let file_gid = meta.gid();

            let has_access = if uid == file_uid {
                // Owner: check owner read+write bits.
                mode & 0o600 == 0o600
            } else if gid == file_gid {
                // Group: check group read+write bits.
                mode & 0o060 == 0o060
            } else {
                // Other: check other read+write bits.
                mode & 0o006 == 0o006
            };

            if !has_access {
                return Err(SpdkEnvError::PermissionDenied(format!(
                    "{} (need read+write for uid={uid}, gid={gid}; current mode={mode:04o}, \
                     owner={file_uid}:{file_gid}). Consider adding your user to the \
                     appropriate group or configuring udev rules.",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(e) => Err(SpdkEnvError::PermissionDenied(format!(
            "cannot stat {}: {e}",
            path.display()
        ))),
    }
}

/// Verify that hugepages are available for DPDK.
///
/// Checks both 2MB and 1GB hugepage pools in sysfs.
pub fn check_hugepages() -> Result<(), SpdkEnvError> {
    check_hugepages_at(
        "/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages",
        "/sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages",
    )
}

/// Testable version with configurable paths.
pub(crate) fn check_hugepages_at(path_2mb: &str, path_1gb: &str) -> Result<(), SpdkEnvError> {
    let count_2mb = read_hugepage_count(path_2mb);
    let count_1gb = read_hugepage_count(path_1gb);

    if count_2mb + count_1gb == 0 {
        return Err(SpdkEnvError::HugepagesNotConfigured(
            "No hugepages allocated. Allocate with: \
             echo 1024 > /proc/sys/vm/nr_hugepages (or via kernel boot parameter hugepages=1024)"
                .into(),
        ));
    }

    Ok(())
}

/// Read the hugepage count from a sysfs file. Returns 0 if the file
/// doesn't exist or can't be parsed.
fn read_hugepage_count(path: &str) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // --- check_vfio_available ---

    #[test]
    fn vfio_available_missing_dev_vfio() {
        let result = check_vfio_available_at("/nonexistent/vfio", "/nonexistent/driver");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::VfioNotAvailable(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn vfio_available_missing_driver() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();

        let result = check_vfio_available_at(dev_vfio.to_str().unwrap(), "/nonexistent/vfio-pci");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::VfioNotAvailable(_)));
        assert!(err.to_string().contains("vfio-pci driver not loaded"));
    }

    #[test]
    fn vfio_available_both_present() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        let sysfs_driver = tmp.path().join("vfio-pci");
        fs::create_dir(&dev_vfio).unwrap();
        fs::create_dir(&sysfs_driver).unwrap();

        let result =
            check_vfio_available_at(dev_vfio.to_str().unwrap(), sysfs_driver.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn vfio_available_error_includes_modprobe_hint() {
        let result = check_vfio_available_at("/nonexistent/vfio", "/nonexistent/driver");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("modprobe vfio-pci"));
    }

    // --- check_hugepages ---

    #[test]
    fn hugepages_none_configured() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        let hp_1gb = tmp.path().join("hp_1gb");
        fs::write(&hp_2mb, "0\n").unwrap();
        fs::write(&hp_1gb, "0\n").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), hp_1gb.to_str().unwrap());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SpdkEnvError::HugepagesNotConfigured(_)
        ));
    }

    #[test]
    fn hugepages_2mb_available() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        fs::write(&hp_2mb, "1024\n").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), "/nonexistent/1gb");
        assert!(result.is_ok());
    }

    #[test]
    fn hugepages_1gb_available() {
        let tmp = TempDir::new().unwrap();
        let hp_1gb = tmp.path().join("hp_1gb");
        fs::write(&hp_1gb, "4\n").unwrap();

        let result = check_hugepages_at("/nonexistent/2mb", hp_1gb.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn hugepages_missing_files() {
        let result = check_hugepages_at("/nonexistent/2mb", "/nonexistent/1gb");
        assert!(result.is_err());
    }

    #[test]
    fn hugepages_both_available() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        let hp_1gb = tmp.path().join("hp_1gb");
        fs::write(&hp_2mb, "512\n").unwrap();
        fs::write(&hp_1gb, "2\n").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), hp_1gb.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn hugepages_non_numeric_content() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        let hp_1gb = tmp.path().join("hp_1gb");
        fs::write(&hp_2mb, "not-a-number\n").unwrap();
        fs::write(&hp_1gb, "garbage\n").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), hp_1gb.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn hugepages_empty_files() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        let hp_1gb = tmp.path().join("hp_1gb");
        fs::write(&hp_2mb, "").unwrap();
        fs::write(&hp_1gb, "").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), hp_1gb.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn hugepages_error_includes_allocation_hint() {
        let result = check_hugepages_at("/nonexistent/2mb", "/nonexistent/1gb");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("nr_hugepages"));
    }

    #[test]
    fn hugepages_whitespace_trimmed() {
        let tmp = TempDir::new().unwrap();
        let hp_2mb = tmp.path().join("hp_2mb");
        fs::write(&hp_2mb, "  256  \n").unwrap();

        let result = check_hugepages_at(hp_2mb.to_str().unwrap(), "/nonexistent/1gb");
        assert!(result.is_ok());
    }

    // --- read_hugepage_count ---

    #[test]
    fn read_hugepage_count_valid() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nr_hugepages");
        fs::write(&path, "1024\n").unwrap();
        assert_eq!(read_hugepage_count(path.to_str().unwrap()), 1024);
    }

    #[test]
    fn read_hugepage_count_zero() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nr_hugepages");
        fs::write(&path, "0\n").unwrap();
        assert_eq!(read_hugepage_count(path.to_str().unwrap()), 0);
    }

    #[test]
    fn read_hugepage_count_missing_file() {
        assert_eq!(read_hugepage_count("/nonexistent/nr_hugepages"), 0);
    }

    #[test]
    fn read_hugepage_count_non_numeric() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nr_hugepages");
        fs::write(&path, "abc\n").unwrap();
        assert_eq!(read_hugepage_count(path.to_str().unwrap()), 0);
    }

    // --- check_vfio_permissions ---

    #[test]
    fn vfio_permissions_accessible() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        fs::set_permissions(&container, fs::Permissions::from_mode(0o666)).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn vfio_permissions_no_container() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        // Should pass — container doesn't exist yet (pre-device-binding state).
        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn vfio_permissions_iommu_group_numeric() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        // Create numeric IOMMU group entry.
        let group = dev_vfio.join("42");
        fs::write(&group, "").unwrap();
        fs::set_permissions(&group, fs::Permissions::from_mode(0o666)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn vfio_permissions_non_numeric_entries_skipped() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        // Non-numeric entries should be skipped even with restrictive perms.
        let non_numeric = dev_vfio.join("not-a-group");
        fs::write(&non_numeric, "").unwrap();
        fs::set_permissions(&non_numeric, fs::Permissions::from_mode(0o000)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn vfio_permissions_container_no_access() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        fs::set_permissions(&container, fs::Permissions::from_mode(0o000)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        // As owner with mode 0o000, we have no read+write — should fail.
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::PermissionDenied(_)));
        assert!(err.to_string().contains("vfio"));
    }

    #[test]
    fn vfio_permissions_iommu_group_no_access() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let group = dev_vfio.join("7");
        fs::write(&group, "").unwrap();
        fs::set_permissions(&group, fs::Permissions::from_mode(0o000)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::PermissionDenied(_)));
    }

    #[test]
    fn vfio_permissions_error_includes_uid_gid() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        fs::set_permissions(&container, fs::Permissions::from_mode(0o000)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("uid="));
        assert!(err.contains("gid="));
    }

    #[test]
    fn vfio_permissions_error_includes_udev_hint() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        fs::set_permissions(&container, fs::Permissions::from_mode(0o000)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("udev"));
    }

    #[test]
    fn vfio_permissions_read_only_insufficient() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        // Owner can read but not write.
        fs::set_permissions(&container, fs::Permissions::from_mode(0o400)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn vfio_permissions_write_only_insufficient() {
        let tmp = TempDir::new().unwrap();
        let dev_vfio = tmp.path().join("vfio");
        fs::create_dir(&dev_vfio).unwrap();
        fs::set_permissions(&dev_vfio, fs::Permissions::from_mode(0o755)).unwrap();

        let container = dev_vfio.join("vfio");
        fs::write(&container, "").unwrap();
        // Owner can write but not read.
        fs::set_permissions(&container, fs::Permissions::from_mode(0o200)).unwrap();

        let result = check_vfio_permissions_at(dev_vfio.to_str().unwrap());
        assert!(result.is_err());
    }

    // --- check_path_rw ---

    #[test]
    fn check_path_rw_owner_rw() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test_file");
        fs::write(&file, "").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check_path_rw(&file).is_ok());
    }

    #[test]
    fn check_path_rw_world_rw() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test_file");
        fs::write(&file, "").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(check_path_rw(&file).is_ok());
    }

    #[test]
    fn check_path_rw_nonexistent_path() {
        let result = check_path_rw(Path::new("/nonexistent/path/file"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::PermissionDenied(_)));
        assert!(err.to_string().contains("cannot stat"));
    }
}
