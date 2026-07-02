//! Block device I/O layer with sector-aligned reads/writes and partition offset.

use crate::on_disk::Superblock;
use interfaces::iblock_device::{ClientChannels, Command, Completion};
use interfaces::DmaAllocFn;
use std::sync::{Arc, Mutex};

/// Wraps an IBlockDevice's client channels with partition-aware sector-aligned I/O.
pub struct BlockDeviceClient {
    channels: ClientChannels,
    alloc: DmaAllocFn,
    pub sector_size: u32,
    ns_id: u32,
    base_lba: u64,
}

impl BlockDeviceClient {
    pub fn new(
        channels: ClientChannels,
        alloc: DmaAllocFn,
        sector_size: u32,
        ns_id: u32,
        base_lba: u64,
    ) -> Self {
        Self {
            channels,
            alloc,
            sector_size,
            ns_id,
            base_lba,
        }
    }

    /// Read `count` sectors starting at `lba` (relative to partition base).
    pub fn read_sectors(&self, lba: u64, count: u64) -> Result<Vec<u8>, String> {
        let sector_size = self.sector_size as usize;
        let mut result = Vec::with_capacity(count as usize * sector_size);

        for i in 0..count {
            let abs_lba = self.base_lba + lba + i;
            let buf = (self.alloc)(sector_size, sector_size, None)
                .map_err(|e| format!("DMA alloc failed: {e}"))?;
            let buf = Arc::new(Mutex::new(buf));

            let cmd = Command::ReadSync {
                ns_id: self.ns_id,
                lba: abs_lba,
                buf: buf.clone(),
            };
            self.channels
                .command_tx
                .send(cmd)
                .map_err(|e| format!("send ReadSync failed: {e}"))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::ReadDone {
                    result: Ok(()), ..
                }) => {}
                Ok(Completion::ReadDone {
                    result: Err(e), ..
                }) => {
                    return Err(format!("ReadSync error at LBA {abs_lba}: {e}"));
                }
                Ok(other) => {
                    return Err(format!("unexpected completion: {other:?}"));
                }
                Err(e) => {
                    return Err(format!("recv completion failed: {e}"));
                }
            }

            let locked = buf.lock().unwrap();
            result.extend_from_slice(&locked.as_slice()[..sector_size]);
        }

        Ok(result)
    }

    /// Write data starting at `lba` (relative to partition base).
    /// Data is padded to sector alignment if needed.
    pub fn write_sectors(&self, lba: u64, data: &[u8]) -> Result<(), String> {
        let sector_size = self.sector_size as usize;
        let num_sectors = data.len().div_ceil(sector_size);

        for i in 0..num_sectors {
            let abs_lba = self.base_lba + lba + i as u64;
            let mut buf = (self.alloc)(sector_size, sector_size, None)
                .map_err(|e| format!("DMA alloc failed: {e}"))?;

            // Copy sector data (zero-padded for partial last sector)
            let start = i * sector_size;
            let end = ((i + 1) * sector_size).min(data.len());
            let slice_len = end - start;
            let buf_slice = buf.as_mut_slice();
            buf_slice[..slice_len].copy_from_slice(&data[start..end]);
            if slice_len < sector_size {
                buf_slice[slice_len..sector_size].fill(0);
            }

            let buf = Arc::new(buf);
            let cmd = Command::WriteSync {
                ns_id: self.ns_id,
                lba: abs_lba,
                buf: buf.clone(),
            };
            self.channels
                .command_tx
                .send(cmd)
                .map_err(|e| format!("send WriteSync failed: {e}"))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::WriteDone {
                    result: Ok(()), ..
                }) => {}
                Ok(Completion::WriteDone {
                    result: Err(e), ..
                }) => {
                    return Err(format!("WriteSync error at LBA {abs_lba}: {e}"));
                }
                Ok(other) => {
                    return Err(format!("unexpected completion: {other:?}"));
                }
                Err(e) => {
                    return Err(format!("recv completion failed: {e}"));
                }
            }
        }

        Ok(())
    }

    /// Write a superblock to LBA 0 of the partition.
    pub fn write_superblock(&self, sb: &Superblock) -> Result<(), String> {
        let data = sb.serialize(self.sector_size as usize);
        self.write_sectors(0, &data)
    }

    /// Read and parse the superblock from LBA 0 of the partition.
    pub fn read_superblock(&self) -> Result<Option<Superblock>, String> {
        let data = self.read_sectors(0, 1)?;
        Ok(Superblock::deserialize(&data))
    }

    /// Write serialized region data at the given sector offset.
    pub fn write_region(&self, offset_sectors: u64, data: &[u8]) -> Result<(), String> {
        self.write_sectors(offset_sectors, data)
    }

    /// Read region data from the given sector offset.
    pub fn read_region(&self, offset_sectors: u64, size_sectors: u64) -> Result<Vec<u8>, String> {
        self.read_sectors(offset_sectors, size_sectors)
    }
}
