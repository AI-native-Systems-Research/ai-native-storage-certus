//! SPDK/DPDK environment initialization, singleton management, and device enumeration.

use crate::checks;
use crate::device::{PciAddress, PciId, VfioDevice};
use crate::error::SpdkEnvError;
use crate::SPDKEnvComponent;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};

/// Process-global flag enforcing singleton SPDK environment.
static SPDK_ENV_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Perform the full initialization sequence for an SPDKEnvComponent.
///
/// Order: singleton check -> VFIO checks -> permissions ->
/// hugepages -> SPDK env init -> PCI enumeration.
pub(crate) fn do_init(comp: &SPDKEnvComponent) -> Result<(), SpdkEnvError> {
    // 1. Singleton check.
    if SPDK_ENV_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(SpdkEnvError::AlreadyInitialized(
            "SPDK environment already active in this process (singleton constraint).".into(),
        ));
    }

    // From here on, if we fail we must clear the singleton flag.
    let result = do_init_inner(comp);
    if result.is_err() {
        SPDK_ENV_ACTIVE.store(false, Ordering::Release);
    }
    result
}

/// Inner initialization that runs after singleton flag is acquired.
fn do_init_inner(comp: &SPDKEnvComponent) -> Result<(), SpdkEnvError> {
    // 3. Pre-flight checks.
    checks::check_vfio_available()?;
    checks::check_vfio_permissions()?;
    checks::check_hugepages()?;

    eprintln!("[spdk-env] Pre-flight checks passed. Initializing SPDK/DPDK environment...");

    // 4. Initialize SPDK environment.
    init_spdk_env()?;

    eprintln!("[spdk-env] SPDK/DPDK environment initialized. Enumerating devices...");

    // 5. Enumerate PCI devices.
    let devices = enumerate_devices(comp)?;

    eprintln!(
        "[spdk-env] Device enumeration complete. Found {} device(s).",
        devices.len()
    );

    // Store discovered devices.
    let mut dev_lock = comp
        .discovered_devices
        .write()
        .expect("devices lock poisoned");
    *dev_lock = devices;

    // Mark as initialized.
    comp.initialized
        .store(true, std::sync::atomic::Ordering::Release);

    Ok(())
}

/// Initialize the SPDK/DPDK environment via C FFI.
fn init_spdk_env() -> Result<(), SpdkEnvError> {
    let app_name = CString::new("certus-spdk-env").expect("CString::new failed");

    // SAFETY: spdk_env_opts_init zeroes the struct and sets default values.
    // The struct is stack-allocated and fully owned by us.
    let mut opts: spdk_sys::spdk_env_opts = unsafe { std::mem::zeroed() };

    unsafe {
        spdk_sys::spdk_env_opts_init(&mut opts);
    }

    // Ensure opts_size is explicitly set (some SPDK/DPDK builds expect this).
    opts.opts_size = std::mem::size_of::<spdk_sys::spdk_env_opts>();

    opts.name = app_name.as_ptr();
    // Don't require specific cores — let DPDK use the default.
    opts.shm_id = -1;

    // SAFETY: spdk_env_init is called exactly once (enforced by singleton flag).
    // The opts struct is valid for the duration of the call.
    let rc = unsafe { spdk_sys::spdk_env_init(&opts) };
    if rc != 0 {
        return Err(SpdkEnvError::InitFailed(format!(
            "spdk_env_init() returned {rc}. Check DPDK EAL log output for details."
        )));
    }

    // Mark the interfaces crate that the SPDK environment is active so that
    // DmaBuffer drop handlers know it is safe to call SPDK deallocators.
    interfaces::set_spdk_env_active(true);

    Ok(())
}

/// Enumerate all PCI devices visible to SPDK after environment initialization.
///
/// We call `spdk_pci_enumerate` with the NVMe driver to discover devices,
/// but return **non-zero** from the callback so the devices are NOT attached
/// (claimed) by the PCI driver. This leaves them available for
/// `spdk_nvme_probe` to claim later in the block device component.
///
/// Device info is collected directly in the enumerate callback context.
fn enumerate_devices(_comp: &SPDKEnvComponent) -> Result<Vec<VfioDevice>, SpdkEnvError> {
    let mut devices = Vec::new();

    unsafe {
        /// Callback for `spdk_pci_enumerate`: reads device info into the
        /// context vector but returns 1 (non-zero) so the device is NOT
        /// attached. This preserves the device for later `spdk_nvme_probe`.
        extern "C" fn enum_cb(
            ctx: *mut std::ffi::c_void,
            dev: *mut spdk_sys::spdk_pci_device,
        ) -> i32 {
            let devices = unsafe { &mut *(ctx as *mut Vec<VfioDevice>) };

            let addr = unsafe { spdk_sys::spdk_pci_device_get_addr(dev) };
            let id = unsafe { spdk_sys::spdk_pci_device_get_id(dev) };
            let numa = unsafe { spdk_sys::spdk_pci_device_get_numa_id(dev) };

            let dev_type = unsafe {
                if !(*dev).type_.is_null() {
                    std::ffi::CStr::from_ptr((*dev).type_)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "unknown".to_string()
                }
            };

            devices.push(VfioDevice {
                address: PciAddress {
                    domain: addr.domain,
                    bus: addr.bus,
                    dev: addr.dev,
                    func: addr.func,
                },
                id: PciId {
                    class_id: id.class_id,
                    vendor_id: id.vendor_id,
                    device_id: id.device_id,
                    subvendor_id: id.subvendor_id,
                    subdevice_id: id.subdevice_id,
                },
                numa_node: numa,
                device_type: dev_type,
            });

            // Return non-zero: do NOT attach/claim the device.
            1
        }

        let nvme_name = std::ffi::CString::new("nvme").unwrap();
        let driver = spdk_sys::spdk_pci_get_driver(nvme_name.as_ptr());
        if !driver.is_null() {
            let rc = spdk_sys::spdk_pci_enumerate(
                driver,
                Some(enum_cb),
                &mut devices as *mut Vec<VfioDevice> as *mut std::ffi::c_void,
            );
            if rc != 0 {
                eprintln!("[spdk-env] warning: spdk_pci_enumerate returned {rc}");
            }
        } else {
            eprintln!(
                "[spdk-env] warning: NVMe PCI driver not registered \
                 (spdk_pci_get_driver(\"nvme\") returned NULL). \
                 Ensure libspdk_nvme is linked with +whole-archive."
            );
        }
    }

    Ok(devices)
}

/// Call `spdk_env_fini()` and clear the singleton flag.
pub(crate) fn do_fini() {
    // Mark SPDK inactive so DmaBuffer drops will avoid calling SPDK
    // deallocators while the environment is being torn down.
    interfaces::set_spdk_env_active(false);

    // SAFETY: spdk_env_fini is safe to call after successful spdk_env_init.
    // We only call this from Drop when initialized is true.
    unsafe {
        spdk_sys::spdk_env_fini();
    }

    SPDK_ENV_ACTIVE.store(false, Ordering::Release);
}
