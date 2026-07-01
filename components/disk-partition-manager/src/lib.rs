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
