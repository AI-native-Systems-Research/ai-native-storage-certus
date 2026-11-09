//! Device types for VFIO-attached PCI devices discovered by SPDK.

use std::fmt;

/// PCI Bus-Device-Function address identifying a specific PCI device.
///
/// Displayed in standard notation: `DDDD:BB:DD.F` (e.g., `0000:01:00.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PciAddress {
    /// PCI domain (segment).
    pub domain: u32,
    /// PCI bus number.
    pub bus: u8,
    /// PCI device number.
    pub dev: u8,
    /// PCI function number.
    pub func: u8,
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04x}:{:02x}:{:02x}.{:x}",
            self.domain, self.bus, self.dev, self.func
        )
    }
}

impl PciAddress {
    /// Parse a BDF address in `DDDD:BB:DD.F` or `BB:DD.F` form (hex, any case;
    /// a missing domain is 0), as the `--device-pci` flags and sysfs spell it.
    ///
    /// # Examples
    ///
    /// ```
    /// use spdk_env::PciAddress;
    ///
    /// let a = PciAddress::parse("0000:c1:00.0").unwrap();
    /// assert_eq!((a.domain, a.bus, a.dev, a.func), (0, 0xc1, 0, 0));
    /// assert_eq!(a.to_string(), "0000:c1:00.0");
    /// assert_eq!(PciAddress::parse("61:00.0"), PciAddress::parse("0000:61:00.0"));
    /// assert!(PciAddress::parse("not-a-bdf").is_none());
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (rest, func) = s.rsplit_once('.')?;
        let mut parts: Vec<&str> = rest.split(':').collect();
        let domain = match parts.len() {
            3 => u32::from_str_radix(parts.remove(0), 16).ok()?,
            2 => 0,
            _ => return None,
        };
        let bus = u8::from_str_radix(parts[0], 16).ok()?;
        let dev = u8::from_str_radix(parts[1], 16).ok()?;
        let func = u8::from_str_radix(func, 16).ok()?;
        // A PCI device number is 5 bits and a function number 3.
        (dev < 32 && func < 8).then_some(PciAddress {
            domain,
            bus,
            dev,
            func,
        })
    }
}

/// PCI vendor/device/class identification for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciId {
    /// PCI class code.
    pub class_id: u32,
    /// PCI vendor ID.
    pub vendor_id: u16,
    /// PCI device ID.
    pub device_id: u16,
    /// Subsystem vendor ID.
    pub subvendor_id: u16,
    /// Subsystem device ID.
    pub subdevice_id: u16,
}

/// A VFIO-attached device discovered by SPDK during initialization.
///
/// Instances are immutable snapshots created during initialization and
/// do not track runtime state changes.
#[derive(Debug, Clone)]
pub struct VfioDevice {
    /// PCI BDF address uniquely identifying this device.
    pub address: PciAddress,
    /// Vendor/device/class identification.
    pub id: PciId,
    /// NUMA node the device is attached to (-1 = unknown).
    pub numa_node: i32,
    /// SPDK device type string (e.g., "nvme", "virtio").
    pub device_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sample_addr() -> PciAddress {
        PciAddress {
            domain: 0,
            bus: 1,
            dev: 0,
            func: 0,
        }
    }

    fn sample_id() -> PciId {
        PciId {
            class_id: 0x010802,
            vendor_id: 0x8086,
            device_id: 0x0a54,
            subvendor_id: 0,
            subdevice_id: 0,
        }
    }

    fn sample_device() -> VfioDevice {
        VfioDevice {
            address: sample_addr(),
            id: sample_id(),
            numa_node: 0,
            device_type: "nvme".into(),
        }
    }

    // --- PciAddress tests ---

    #[test]
    fn pci_address_display() {
        assert_eq!(sample_addr().to_string(), "0000:01:00.0");
    }

    #[test]
    fn pci_address_display_large_domain() {
        let addr = PciAddress {
            domain: 0xabcd,
            bus: 0xff,
            dev: 0x1f,
            func: 7,
        };
        assert_eq!(addr.to_string(), "abcd:ff:1f.7");
    }

    #[test]
    fn pci_address_display_all_zeros() {
        let addr = PciAddress {
            domain: 0,
            bus: 0,
            dev: 0,
            func: 0,
        };
        assert_eq!(addr.to_string(), "0000:00:00.0");
    }

    #[test]
    fn pci_address_equality() {
        let a = sample_addr();
        let b = sample_addr();
        assert_eq!(a, b);
    }

    #[test]
    fn pci_address_inequality_different_bus() {
        let a = sample_addr();
        let b = PciAddress { bus: 2, ..a };
        assert_ne!(a, b);
    }

    #[test]
    fn pci_address_inequality_different_func() {
        let a = sample_addr();
        let b = PciAddress { func: 1, ..a };
        assert_ne!(a, b);
    }

    #[test]
    fn pci_address_hash_equal_values_same_bucket() {
        let a = sample_addr();
        let b = sample_addr();
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn pci_address_hash_different_values_different_bucket() {
        let a = sample_addr();
        let b = PciAddress { bus: 2, ..a };
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn pci_address_copy() {
        let a = sample_addr();
        let b = a; // Copy
        assert_eq!(a, b); // a still usable
    }

    #[test]
    fn pci_address_debug() {
        let addr = sample_addr();
        let dbg = format!("{:?}", addr);
        assert!(dbg.contains("PciAddress"));
        assert!(dbg.contains("domain"));
        assert!(dbg.contains("bus"));
    }

    // --- PciId tests ---

    #[test]
    fn pci_id_equality() {
        let a = sample_id();
        let b = sample_id();
        assert_eq!(a, b);
    }

    #[test]
    fn pci_id_inequality() {
        let a = sample_id();
        let b = PciId {
            vendor_id: 0x1234,
            ..a
        };
        assert_ne!(a, b);
    }

    #[test]
    fn pci_id_copy() {
        let a = sample_id();
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn pci_id_debug() {
        let id = sample_id();
        let dbg = format!("{:?}", id);
        assert!(dbg.contains("PciId"));
        assert!(dbg.contains("vendor_id"));
        assert!(dbg.contains("class_id"));
    }

    #[test]
    fn pci_id_all_fields() {
        let id = PciId {
            class_id: 0x020000,
            vendor_id: 0x15b3,
            device_id: 0x1017,
            subvendor_id: 0x15b3,
            subdevice_id: 0x0020,
        };
        assert_eq!(id.class_id, 0x020000);
        assert_eq!(id.vendor_id, 0x15b3);
        assert_eq!(id.device_id, 0x1017);
        assert_eq!(id.subvendor_id, 0x15b3);
        assert_eq!(id.subdevice_id, 0x0020);
    }

    // --- VfioDevice tests ---

    #[test]
    fn vfio_device_clone() {
        let dev = sample_device();
        let dev2 = dev.clone();
        assert_eq!(dev.address, dev2.address);
        assert_eq!(dev.id, dev2.id);
        assert_eq!(dev.numa_node, dev2.numa_node);
        assert_eq!(dev.device_type, dev2.device_type);
    }

    #[test]
    fn vfio_device_debug() {
        let dev = sample_device();
        let dbg = format!("{:?}", dev);
        assert!(dbg.contains("VfioDevice"));
        assert!(dbg.contains("nvme"));
        assert!(dbg.contains("numa_node"));
    }

    #[test]
    fn vfio_device_unknown_numa() {
        let dev = VfioDevice {
            numa_node: -1,
            ..sample_device()
        };
        assert_eq!(dev.numa_node, -1);
    }

    #[test]
    fn vfio_device_various_types() {
        for dtype in &["nvme", "virtio", "vfio-user", "unknown"] {
            let dev = VfioDevice {
                device_type: dtype.to_string(),
                ..sample_device()
            };
            assert_eq!(dev.device_type, *dtype);
        }
    }

    // --- PciAddress::parse ---

    #[test]
    fn parse_full_bdf() {
        let a = PciAddress::parse("0000:c1:00.0").unwrap();
        assert_eq!((a.domain, a.bus, a.dev, a.func), (0, 0xc1, 0, 0));
    }

    #[test]
    fn parse_round_trips_display() {
        for s in ["0000:61:00.0", "0001:ff:1f.7", "abcd:00:00.1"] {
            assert_eq!(PciAddress::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn parse_short_form_and_case() {
        assert_eq!(
            PciAddress::parse("C4:00.0"),
            PciAddress::parse("0000:c4:00.0")
        );
        assert_eq!(
            PciAddress::parse("  0000:62:00.0\n"),
            PciAddress::parse("0000:62:00.0")
        );
    }

    #[test]
    fn parse_rejects_malformed() {
        for s in [
            "",
            "c1:00",
            "0000:c1:00",
            "0000:c1:00.8",
            "0000:c1:20.0",
            "0000:zz:00.0",
            "1:2:3:4.0",
            "0000:100:00.0",
        ] {
            assert!(PciAddress::parse(s).is_none(), "{s:?} should not parse");
        }
    }
}
