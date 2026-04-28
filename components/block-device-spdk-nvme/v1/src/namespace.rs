//! NVMe namespace management operations.
//!
//! Functions for probing namespaces and validating LBA ranges.
//! Namespace create, format, and delete operations are dispatched via
//! SPDK admin commands when the bindings are available; otherwise they
//! return [`NvmeBlockError::NotSupported`].

use interfaces::{NamespaceInfo, NvmeBlockError};

use crate::controller::NvmeNamespaceInfo;

/// Probe all active namespaces on a controller.
///
/// Returns a list of [`NamespaceInfo`] for client consumption.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
#[allow(dead_code)]
pub(crate) unsafe fn probe(ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr) -> Vec<NamespaceInfo> {
    let mut result = Vec::new();

    // SAFETY: ctrlr_ptr is valid.
    let num_ns = spdk_sys::spdk_nvme_ctrlr_get_num_ns(ctrlr_ptr);

    for ns_id in 1..=num_ns {
        let ns_ptr = spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id);
        if ns_ptr.is_null() {
            continue;
        }

        if !spdk_sys::spdk_nvme_ns_is_active(ns_ptr) {
            continue;
        }

        let num_sectors = spdk_sys::spdk_nvme_ns_get_num_sectors(ns_ptr);
        let sector_size = spdk_sys::spdk_nvme_ns_get_sector_size(ns_ptr);

        result.push(NamespaceInfo {
            ns_id,
            num_sectors,
            sector_size,
        });
    }

    result
}

/// Convert internal namespace info to the client-facing type.
pub(crate) fn to_namespace_info(ns: &NvmeNamespaceInfo) -> NamespaceInfo {
    NamespaceInfo {
        ns_id: ns.ns_id,
        num_sectors: ns.num_sectors,
        sector_size: ns.sector_size,
    }
}

/// Convert a list of internal namespace infos to client-facing types.
pub(crate) fn to_namespace_info_list(namespaces: &[NvmeNamespaceInfo]) -> Vec<NamespaceInfo> {
    namespaces.iter().map(to_namespace_info).collect()
}

/// Create a new namespace on the controller (always uses lbaf=0).
///
/// Use `format()` after creation to change the LBA format.
/// When `size_sectors` is 0, uses all remaining unallocated capacity.
/// `current_namespaces` is the actor's up-to-date namespace list (used for
/// capacity computation when SPDK's cached identify data is stale).
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
pub(crate) unsafe fn create(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    size_sectors: u64,
    current_namespaces: &[NvmeNamespaceInfo],
) -> Result<u32, NvmeBlockError> {
    let sector_size = current_namespaces
        .first()
        .map(|ns| ns.sector_size as u64)
        .unwrap_or(512);

    let effective_size = if size_sectors == 0 {
        unallocated_sectors(ctrlr_ptr, current_namespaces, sector_size)?
    } else {
        size_sectors
    };

    // SAFETY: zeroed spdk_nvme_ns_data is a valid default (lbaf=0).
    let mut ns_data: spdk_sys::spdk_nvme_ns_data = std::mem::zeroed();
    ns_data.nsze = effective_size;
    ns_data.ncap = effective_size;

    // SAFETY: ctrlr_ptr is valid; ns_data is properly initialized.
    let ns_id = spdk_sys::spdk_nvme_ctrlr_create_ns(ctrlr_ptr, &mut ns_data);
    if ns_id == 0 {
        return Err(NvmeBlockError::NotSupported(
            "spdk_nvme_ctrlr_create_ns failed — controller may not support namespace management"
                .into(),
        ));
    }
    Ok(ns_id)
}

/// Query unallocated NVM capacity and return it as a sector count.
///
/// Reads `unvmcap` (unallocated NVM capacity in bytes) from the Identify
/// Controller data and divides by the sector size of LBA format 0.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
/// NVMe Identify Controller byte offsets for capacity fields (128-bit LE).
const TNVMCAP_OFFSET: usize = 280;
const UNVMCAP_OFFSET: usize = 296;

/// Read a 128-bit LE value from the Identify Controller data at `offset`.
unsafe fn read_cdata_u128(cdata: *const spdk_sys::spdk_nvme_ctrlr_data, offset: usize) -> u128 {
    let base = cdata as *const u8;
    let lo = std::ptr::read_unaligned(base.add(offset) as *const u64);
    let hi = std::ptr::read_unaligned(base.add(offset + 8) as *const u64);
    lo as u128 | ((hi as u128) << 64)
}

unsafe fn unallocated_sectors(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    current_namespaces: &[NvmeNamespaceInfo],
    target_sector_size: u64,
) -> Result<u64, NvmeBlockError> {
    // SAFETY: ctrlr_ptr is valid; returns a pointer to the 4096-byte Identify Controller data.
    let cdata = spdk_sys::spdk_nvme_ctrlr_get_data(ctrlr_ptr);
    if cdata.is_null() {
        return Err(NvmeBlockError::NotSupported(
            "failed to get controller identify data".into(),
        ));
    }

    let unvmcap_bytes = read_cdata_u128(cdata, UNVMCAP_OFFSET);
    let tnvmcap_bytes = read_cdata_u128(cdata, TNVMCAP_OFFSET);

    // Prefer unvmcap if reported by the controller.
    if unvmcap_bytes > 0 {
        return Ok((unvmcap_bytes / target_sector_size as u128) as u64);
    }

    // Fallback: compute from tnvmcap minus allocated namespace sizes.
    // Uses the actor's refreshed namespace list (not SPDK's stale cache).
    if tnvmcap_bytes > 0 {
        let allocated: u128 = current_namespaces
            .iter()
            .map(|ns| ns.num_sectors as u128 * ns.sector_size as u128)
            .sum();
        let remaining = tnvmcap_bytes.saturating_sub(allocated);
        if remaining == 0 {
            return Err(NvmeBlockError::NotSupported(
                "no unallocated capacity remaining on controller".into(),
            ));
        }
        return Ok((remaining / target_sector_size as u128) as u64);
    }

    Err(NvmeBlockError::NotSupported(
        "controller does not report NVM capacity (tnvmcap/unvmcap both zero)".into(),
    ))
}

/// Format an existing namespace (erases all data).
///
/// Sets the LBA format to `lbaf` (e.g., 0 for 512B, 2 for 4KiB on most
/// controllers). After a successful format, the caller must issue a
/// controller reset so the host re-reads the namespace's identify data
/// and recognizes the new sector size.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
pub(crate) unsafe fn format(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    ns_id: u32,
    lbaf: u8,
) -> Result<(), NvmeBlockError> {
    // SAFETY: zeroed spdk_nvme_format is a valid default (no secure erase).
    let mut format_opts: spdk_sys::spdk_nvme_format = std::mem::zeroed();
    format_opts.set_lbaf((lbaf & 0x0F) as u32);

    // SAFETY: ctrlr_ptr is valid.
    let rc = spdk_sys::spdk_nvme_ctrlr_format(ctrlr_ptr, ns_id, &mut format_opts);
    if rc != 0 {
        return Err(NvmeBlockError::NotSupported(format!(
            "spdk_nvme_ctrlr_format(ns_id={ns_id}, lbaf={lbaf}) failed with rc={rc}"
        )));
    }
    Ok(())
}

/// Delete an existing namespace.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
pub(crate) unsafe fn delete(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    ns_id: u32,
) -> Result<(), NvmeBlockError> {
    // SAFETY: ctrlr_ptr is valid.
    let rc = spdk_sys::spdk_nvme_ctrlr_delete_ns(ctrlr_ptr, ns_id);
    if rc != 0 {
        return Err(NvmeBlockError::NotSupported(format!(
            "spdk_nvme_ctrlr_delete_ns(ns_id={ns_id}) failed with rc={rc}"
        )));
    }
    Ok(())
}

/// Validate that a namespace ID exists in the given list.
pub(crate) fn validate_ns_id(
    namespaces: &[NvmeNamespaceInfo],
    ns_id: u32,
) -> Result<&NvmeNamespaceInfo, NvmeBlockError> {
    namespaces
        .iter()
        .find(|ns| ns.ns_id == ns_id)
        .ok_or_else(|| {
            NvmeBlockError::InvalidNamespace(format!(
                "namespace {ns_id} not found — use NsProbe to discover available namespaces"
            ))
        })
}

/// Validate that an LBA range is within bounds for a namespace.
pub(crate) fn validate_lba_range(
    ns: &NvmeNamespaceInfo,
    lba: u64,
    num_blocks: u64,
) -> Result<(), NvmeBlockError> {
    if lba + num_blocks > ns.num_sectors {
        return Err(NvmeBlockError::LbaOutOfRange(format!(
            "lba={lba} + num_blocks={num_blocks} = {} exceeds namespace {}'s sector count {}",
            lba + num_blocks,
            ns.ns_id,
            ns.num_sectors,
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_namespaces() -> Vec<NvmeNamespaceInfo> {
        vec![
            NvmeNamespaceInfo {
                ns_id: 1,
                num_sectors: 1_000_000,
                sector_size: 512,
            },
            NvmeNamespaceInfo {
                ns_id: 2,
                num_sectors: 500_000,
                sector_size: 4096,
            },
        ]
    }

    #[test]
    fn validate_ns_id_found() {
        let namespaces = sample_namespaces();
        let ns = validate_ns_id(&namespaces, 1).unwrap();
        assert_eq!(ns.ns_id, 1);
        assert_eq!(ns.num_sectors, 1_000_000);
    }

    #[test]
    fn validate_ns_id_not_found() {
        let namespaces = sample_namespaces();
        let err = validate_ns_id(&namespaces, 99).unwrap_err();
        assert!(matches!(err, NvmeBlockError::InvalidNamespace(_)));
    }

    #[test]
    fn validate_lba_range_ok() {
        let ns = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 1000,
            sector_size: 512,
        };
        assert!(validate_lba_range(&ns, 0, 1000).is_ok());
        assert!(validate_lba_range(&ns, 500, 500).is_ok());
        assert!(validate_lba_range(&ns, 0, 1).is_ok());
    }

    #[test]
    fn validate_lba_range_out_of_bounds() {
        let ns = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 1000,
            sector_size: 512,
        };
        let err = validate_lba_range(&ns, 999, 2).unwrap_err();
        assert!(matches!(err, NvmeBlockError::LbaOutOfRange(_)));
    }

    #[test]
    fn validate_lba_range_zero_blocks() {
        let ns = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 1000,
            sector_size: 512,
        };
        assert!(validate_lba_range(&ns, 999, 0).is_ok());
    }

    #[test]
    fn to_namespace_info_conversion() {
        let internal = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 2048,
            sector_size: 4096,
        };
        let client_info = to_namespace_info(&internal);
        assert_eq!(client_info.ns_id, 1);
        assert_eq!(client_info.num_sectors, 2048);
        assert_eq!(client_info.sector_size, 4096);
    }

    #[test]
    fn to_namespace_info_list_conversion() {
        let internals = sample_namespaces();
        let client_infos = to_namespace_info_list(&internals);
        assert_eq!(client_infos.len(), 2);
        assert_eq!(client_infos[0].ns_id, 1);
        assert_eq!(client_infos[1].ns_id, 2);
    }

    // Note: create(), format(), and delete() now call real SPDK functions.
    // They cannot be unit-tested with null pointers. Integration tests
    // with real hardware cover these paths via the actor's NsCreate,
    // NsFormat, and NsDelete commands.
}
