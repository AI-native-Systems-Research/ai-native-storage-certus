mod gpt;

use std::sync::Mutex;

use component_macros::define_component;
use interfaces::{
    IBlockDevice, IPartitionTable, PartitionConfig, PartitionInfo, PartitionTable,
    PartitionTableError,
};

use crate::gpt::GptManager;

define_component! {
    pub DiskPartitionManager {
        version: "0.1.0",
        provides: [IPartitionTable],
        receptacles: {
            block_device: IBlockDevice,
        },
        fields: {
            state: Mutex<Option<PartitionTable>>,
            ns_id: Mutex<Option<u32>>,
        },
    }
}

impl DiskPartitionManager {
    /// Set the NVMe namespace ID to use for partition table operations.
    /// Defaults to 1 if not called.
    pub fn set_ns_id(&self, ns_id: u32) {
        *self.ns_id.lock().unwrap() = Some(ns_id);
    }

    fn get_ns_id(&self) -> u32 {
        self.ns_id.lock().unwrap().unwrap_or(1)
    }

    /// Try to read an existing partition table, or create one if `force_format`
    /// is true or no valid GPT exists on disk.
    ///
    /// Returns the resolved partition table and a `formatted` flag indicating
    /// whether a new partition table was written. When `formatted` is true,
    /// downstream components should also format their on-disk structures
    /// (e.g., call `IExtentManager::format()` instead of `initialize()`).
    pub fn initialize_or_format(
        &self,
        force_format: bool,
        config: PartitionConfig,
    ) -> Result<(PartitionTable, bool), PartitionTableError> {
        if force_format {
            let table = self.format(config)?;
            return Ok((table, true));
        }

        match self.initialize() {
            Ok(table) => Ok((table, false)),
            Err(PartitionTableError::NoPartitionTable(_))
            | Err(PartitionTableError::CorruptTable(_)) => {
                let table = self.format(config)?;
                Ok((table, true))
            }
            Err(e) => Err(e),
        }
    }
}

impl IPartitionTable for DiskPartitionManager {
    fn initialize(&self) -> Result<PartitionTable, PartitionTableError> {
        let bd = self.block_device.get().map_err(|_| {
            PartitionTableError::NotInitialized("block device not connected".into())
        })?;
        let ns_id = self.get_ns_id();
        let sector_size = bd
            .sector_size(ns_id)
            .map_err(|e| PartitionTableError::IoError(e.to_string()))?;
        let num_sectors = bd
            .num_sectors(ns_id)
            .map_err(|e| PartitionTableError::IoError(e.to_string()))?;

        let gpt_mgr = GptManager::new(bd, ns_id, sector_size, num_sectors)?;
        let table = gpt_mgr.read_gpt()?;
        *self.state.lock().unwrap() = Some(table.clone());
        Ok(table)
    }

    fn format(&self, config: PartitionConfig) -> Result<PartitionTable, PartitionTableError> {
        let bd = self.block_device.get().map_err(|_| {
            PartitionTableError::NotInitialized("block device not connected".into())
        })?;

        let gpt_mgr = GptManager::new(bd, config.ns_id, config.sector_size, config.total_sectors)?;
        let table = gpt_mgr.write_gpt(&config)?;
        *self.state.lock().unwrap() = Some(table.clone());
        Ok(table)
    }

    fn partition_info(&self, index: u32) -> Result<PartitionInfo, PartitionTableError> {
        let state = self.state.lock().unwrap();
        let table = state
            .as_ref()
            .ok_or_else(|| PartitionTableError::NotInitialized("call initialize() first".into()))?;
        table
            .partitions
            .get(index as usize)
            .cloned()
            .ok_or_else(|| {
                PartitionTableError::InvalidPartition(format!(
                    "partition index {index} out of range"
                ))
            })
    }

    fn num_partitions(&self) -> Result<u32, PartitionTableError> {
        let state = self.state.lock().unwrap();
        let table = state
            .as_ref()
            .ok_or_else(|| PartitionTableError::NotInitialized("call initialize() first".into()))?;
        Ok(table.partitions.len() as u32)
    }
}

// ============================================================================
// Kani verification harnesses (Role 2) — component-level (lib.rs) obligations.
// Clean-slate re-run 2026-09-09, tight knobs (--default-unwind 3,
// --harness-timeout 90s, -j 24).
//
// `partition_info` / `num_partitions` are &self methods on the macro-generated
// `DiskPartitionManager`. Constructing one (new_default) builds an InterfaceMap
// (std HashMap -> getrandom/SipHash seeding) plus Arc self-references — the same
// construction class memory-tier documented as a wall. This harness ATTEMPTS the
// DPM-INIT-GATE (G2) establishment (fresh state == None -> NotInitialized) so the
// outcome is evidence-backed (proof or captured signature), never "not attempted".
// Populated-state gates (DPM-COUNT-INDEX-AGREE, DPM-PINFO-*, DPM-NUMP-RETURNS-LEN
// on a non-empty table) are only reachable by initialize/format, whose CRC+I/O
// path is a wall — recorded in the property file, not harnessed here.
// ============================================================================
#[cfg(kani)]
mod verification {
    use super::*;

    /// `DPM-INIT-GATE` (G2). A query on a freshly-constructed manager (state None,
    /// never initialize/format-ed) returns NotInitialized. EXPECT: either a green
    /// establishment proof, or a construction wall (getrandom/SipHash via the
    /// InterfaceMap HashMap seeding) captured as rc=124 / unsupported_construct.
    #[kani::proof]
    fn verify_init_gate_num_partitions() {
        let dpm = DiskPartitionManager::new_default();
        let r = dpm.num_partitions();
        assert!(
            matches!(r, Err(PartitionTableError::NotInitialized(_))),
            "G2: query before initialize -> NotInitialized"
        );
    }
}
