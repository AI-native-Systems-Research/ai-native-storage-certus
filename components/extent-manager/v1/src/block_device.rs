use interfaces::{ClientChannels, Command, Completion, DmaAllocFn, DmaBuffer, NvmeBlockError};
use std::sync::{Arc, Mutex};

use crate::metadata::BLOCK_SIZE;

pub(crate) struct BlockDeviceClient {
    channels: ClientChannels,
    alloc: DmaAllocFn,
}

impl BlockDeviceClient {
    pub fn new(channels: ClientChannels, alloc: DmaAllocFn) -> Self {
        BlockDeviceClient { channels, alloc }
    }

    pub fn alloc_buffer(&self) -> Result<DmaBuffer, NvmeBlockError> {
        (self.alloc)(BLOCK_SIZE, BLOCK_SIZE, None).map_err(|e| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::DmaAllocationFailed(e))
        })
    }

    pub fn write_block(
        &self,
        ns_id: u32,
        lba: u64,
        data: &[u8; BLOCK_SIZE],
    ) -> Result<(), NvmeBlockError> {
        let mut buf = self.alloc_buffer()?;
        buf.as_mut_slice()[..BLOCK_SIZE].copy_from_slice(data);
        #[allow(clippy::arc_with_non_send_sync)]
        let buf = Arc::new(buf);

        self.channels
            .command_tx
            .send(Command::WriteSync { ns_id, lba, buf })
            .map_err(|_| NvmeBlockError::ClientDisconnected("write command send failed".into()))?;

        match self.channels.completion_rx.recv() {
            Ok(Completion::WriteDone { result, .. }) => result,
            Ok(Completion::Error { error, .. }) => Err(error),
            Ok(_) => Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed("unexpected completion".into()),
            )),
            Err(_) => Err(NvmeBlockError::ClientDisconnected(
                "write completion recv failed".into(),
            )),
        }
    }

    pub fn read_block(&self, ns_id: u32, lba: u64) -> Result<[u8; BLOCK_SIZE], NvmeBlockError> {
        let buf = self.alloc_buffer()?;
        let buf = Arc::new(Mutex::new(buf));

        self.channels
            .command_tx
            .send(Command::ReadSync {
                ns_id,
                lba,
                buf: Arc::clone(&buf),
            })
            .map_err(|_| NvmeBlockError::ClientDisconnected("read command send failed".into()))?;

        match self.channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result?;
                let locked = buf.lock().unwrap();
                let mut data = [0u8; BLOCK_SIZE];
                data.copy_from_slice(&locked.as_slice()[..BLOCK_SIZE]);
                Ok(data)
            }
            Ok(Completion::Error { error, .. }) => Err(error),
            Ok(_) => Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::ReadFailed("unexpected completion".into()),
            )),
            Err(_) => Err(NvmeBlockError::ClientDisconnected(
                "read completion recv failed".into(),
            )),
        }
    }
}
