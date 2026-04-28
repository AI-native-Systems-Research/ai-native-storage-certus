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

/// Create a new namespace on the controller.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
pub(crate) unsafe fn create(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    size_sectors: u64,
    sector_size: u32,
) -> Result<u32, NvmeBlockError> {
    if !sector_size.is_power_of_two() || sector_size < 512 {
        return Err(NvmeBlockError::NotSupported(format!(
            "sector_size must be a power of two >= 512, got {sector_size}"
        )));
    }

    let target_lbads = sector_size.trailing_zeros();
    let format_idx = find_lba_format(ctrlr_ptr, target_lbads)?;

    // SAFETY: zeroed spdk_nvme_ns_data is a valid default.
    let mut ns_data: spdk_sys::spdk_nvme_ns_data = std::mem::zeroed();
    ns_data.nsze = size_sectors;
    ns_data.ncap = size_sectors;
    ns_data.flbas.set_format(format_idx as u8);

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

/// Find the LBA format index on the controller that matches `target_lbads` (log2 of sector size).
///
/// First tries cached namespace identify data (fast path). If no usable data
/// is found (e.g. all namespaces deleted), issues a raw Identify Namespace
/// admin command with NSID=0xFFFFFFFF to query the common namespace
/// capabilities directly from the controller.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
unsafe fn find_lba_format(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    target_lbads: u32,
) -> Result<u32, NvmeBlockError> {
    // Fast path: scan cached namespace identify data.
    let num_ns = spdk_sys::spdk_nvme_ctrlr_get_num_ns(ctrlr_ptr);
    for ns_id in 1..=num_ns {
        let ns_ptr = spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id);
        if ns_ptr.is_null() {
            continue;
        }
        let ns_data = spdk_sys::spdk_nvme_ns_get_data(ns_ptr);
        if ns_data.is_null() {
            continue;
        }
        if let Some(idx) = scan_lbaf(&*ns_data, target_lbads) {
            return Ok(idx);
        }
        return Err(NvmeBlockError::NotSupported(format!(
            "controller does not support sector size {} (lbads={})",
            1u32 << target_lbads,
            target_lbads
        )));
    }

    // Slow path: issue Identify Namespace (CNS=0x00, NSID=0xFFFFFFFF) to get
    // the common namespace capabilities with the full LBA format table.
    identify_common_ns_lba_format(ctrlr_ptr, target_lbads)
}

/// Scan the lbaf table in namespace identify data for a matching lbads value.
fn scan_lbaf(ns_data: &spdk_sys::spdk_nvme_ns_data, target_lbads: u32) -> Option<u32> {
    (0..64u32).find(|&i| ns_data.lbaf[i as usize].lbads() == target_lbads)
}

/// Issue Identify Namespace with NSID=0xFFFFFFFF to query common LBA formats.
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.  Must be called
/// from the actor thread (same thread that processes admin completions).
unsafe fn identify_common_ns_lba_format(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    target_lbads: u32,
) -> Result<u32, NvmeBlockError> {
    use std::sync::atomic::{AtomicBool, Ordering};

    // SAFETY: DMA-accessible buffer for the 4096-byte identify data.
    let buf = spdk_sys::spdk_zmalloc(
        std::mem::size_of::<spdk_sys::spdk_nvme_ns_data>(),
        4096,
        std::ptr::null_mut(),
        -1i32,
        0u32,
    );
    if buf.is_null() {
        return Err(NvmeBlockError::NotSupported(
            "failed to allocate DMA buffer for identify command".into(),
        ));
    }

    let done = AtomicBool::new(false);
    let done_ptr: *mut AtomicBool = &done as *const AtomicBool as *mut AtomicBool;

    // Build Identify command: opcode=0x06, CNS=0x00 (in cdw10), NSID=0xFFFFFFFF.
    let mut cmd: spdk_sys::spdk_nvme_cmd = std::mem::zeroed();
    cmd.set_opc(0x06); // SPDK_NVME_OPC_IDENTIFY
    cmd.nsid = 0xFFFF_FFFF;
    cmd.__bindgen_anon_1.cdw10 = 0x00; // CNS=0x00 (Identify Namespace)

    unsafe extern "C" fn identify_cb(
        ctx: *mut std::os::raw::c_void,
        _cpl: *const spdk_sys::spdk_nvme_cpl,
    ) {
        let flag = &*(ctx as *const AtomicBool);
        flag.store(true, Ordering::Release);
    }

    let rc = spdk_sys::spdk_nvme_ctrlr_cmd_admin_raw(
        ctrlr_ptr,
        &mut cmd,
        buf,
        std::mem::size_of::<spdk_sys::spdk_nvme_ns_data>() as u32,
        Some(identify_cb),
        done_ptr as *mut std::os::raw::c_void,
    );

    if rc != 0 {
        spdk_sys::spdk_free(buf);
        return Err(NvmeBlockError::NotSupported(format!(
            "identify admin command failed to submit (rc={rc})"
        )));
    }

    // Poll for completion.
    while !done.load(Ordering::Acquire) {
        spdk_sys::spdk_nvme_ctrlr_process_admin_completions(ctrlr_ptr);
    }

    let ns_data = &*(buf as *const spdk_sys::spdk_nvme_ns_data);
    let result = scan_lbaf(ns_data, target_lbads);
    spdk_sys::spdk_free(buf);

    result.ok_or_else(|| {
        NvmeBlockError::NotSupported(format!(
            "controller does not support sector size {} (lbads={})",
            1u32 << target_lbads,
            target_lbads
        ))
    })
}

/// Format an existing namespace (erases all data).
///
/// # Safety
///
/// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
pub(crate) unsafe fn format(
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    ns_id: u32,
) -> Result<(), NvmeBlockError> {
    // SAFETY: zeroed spdk_nvme_format is a valid default (LBA format 0, no secure erase).
    let mut format_opts: spdk_sys::spdk_nvme_format = std::mem::zeroed();

    // SAFETY: ctrlr_ptr is valid.
    let rc = spdk_sys::spdk_nvme_ctrlr_format(ctrlr_ptr, ns_id, &mut format_opts);
    if rc != 0 {
        return Err(NvmeBlockError::NotSupported(format!(
            "spdk_nvme_ctrlr_format(ns_id={ns_id}) failed with rc={rc}"
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
