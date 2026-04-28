use interfaces::{
    ClientChannels, Command, Completion, DmaAllocFn, DmaBuffer, NvmeBlockError,
};
use std::sync::{Arc, Mutex};

use crate::error;

pub(crate) struct BlockDeviceClient {
    channels: ClientChannels,
    alloc: DmaAllocFn,
    sector_size: u32,
    ns_id: u32,
}

impl BlockDeviceClient {
    pub fn new(channels: ClientChannels, alloc: DmaAllocFn, sector_size: u32, ns_id: u32) -> Self {
        Self {
            channels,
            alloc,
            sector_size,
            ns_id,
        }
    }

    pub fn alloc_buffer(&self, size: usize) -> Result<DmaBuffer, NvmeBlockError> {
        let align = self.sector_size as usize;
        (self.alloc)(size, align, None).map_err(|e| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::DmaAllocationFailed(e))
        })
    }

    pub fn write_blocks(
        &self,
        lba: u64,
        data: &[u8],
    ) -> Result<(), interfaces::ExtentManagerError> {
        let num_blocks =
            (data.len() + self.sector_size as usize - 1) / self.sector_size as usize;
        let buf_size = num_blocks * self.sector_size as usize;

        let mut buf = self.alloc_buffer(buf_size).map_err(error::nvme_to_em)?;
        buf.as_mut_slice()[..data.len()].copy_from_slice(data);
        if data.len() < buf_size {
            for b in &mut buf.as_mut_slice()[data.len()..buf_size] {
                *b = 0;
            }
        }

        #[allow(clippy::arc_with_non_send_sync)]
        let buf = Arc::new(buf);

        for i in 0..num_blocks {
            let block_lba = lba + i as u64;
            let block_start = i * self.sector_size as usize;
            let block_end = block_start + self.sector_size as usize;

            let mut block_buf = self
                .alloc_buffer(self.sector_size as usize)
                .map_err(error::nvme_to_em)?;
            block_buf.as_mut_slice()[..self.sector_size as usize]
                .copy_from_slice(&buf.as_slice()[block_start..block_end]);

            #[allow(clippy::arc_with_non_send_sync)]
            let block_buf = Arc::new(block_buf);

            self.channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: self.ns_id,
                    lba: block_lba,
                    buf: block_buf,
                })
                .map_err(|_| error::io_error("write command send failed"))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::WriteDone { result, .. }) => {
                    result.map_err(error::nvme_to_em)?;
                }
                Ok(Completion::Error { error: e, .. }) => {
                    return Err(error::nvme_to_em(e));
                }
                Ok(_) => {
                    return Err(error::io_error("unexpected completion type"));
                }
                Err(_) => {
                    return Err(error::io_error("write completion recv failed"));
                }
            }
        }

        Ok(())
    }

    pub fn read_blocks(
        &self,
        lba: u64,
        num_bytes: usize,
    ) -> Result<Vec<u8>, interfaces::ExtentManagerError> {
        let num_blocks =
            (num_bytes + self.sector_size as usize - 1) / self.sector_size as usize;
        let mut result = Vec::with_capacity(num_bytes);

        for i in 0..num_blocks {
            let block_lba = lba + i as u64;
            let buf = self
                .alloc_buffer(self.sector_size as usize)
                .map_err(error::nvme_to_em)?;
            let buf = Arc::new(Mutex::new(buf));

            self.channels
                .command_tx
                .send(Command::ReadSync {
                    ns_id: self.ns_id,
                    lba: block_lba,
                    buf: Arc::clone(&buf),
                })
                .map_err(|_| error::io_error("read command send failed"))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::ReadDone { result: res, .. }) => {
                    res.map_err(error::nvme_to_em)?;
                    let locked = buf.lock().unwrap();
                    let remaining = num_bytes - result.len();
                    let to_copy = remaining.min(self.sector_size as usize);
                    result.extend_from_slice(&locked.as_slice()[..to_copy]);
                }
                Ok(Completion::Error { error: e, .. }) => {
                    return Err(error::nvme_to_em(e));
                }
                Ok(_) => {
                    return Err(error::io_error("unexpected completion type"));
                }
                Err(_) => {
                    return Err(error::io_error("read completion recv failed"));
                }
            }
        }

        Ok(result)
    }

    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }
}
