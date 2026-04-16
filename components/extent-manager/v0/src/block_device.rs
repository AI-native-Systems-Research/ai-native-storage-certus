//! Block device wrapper bridging `IBlockDevice` channel-based actor model
//! to synchronous 4KiB block reads/writes.

use std::sync::{Arc, Mutex};

use interfaces::{ClientChannels, Command, Completion, DmaBuffer, IBlockDevice, NvmeBlockError};

use crate::metadata::BLOCK_SIZE;

/// Type alias for a pluggable DMA buffer allocator.
///
/// Signature: `(size, alignment, numa_node) -> Result<DmaBuffer, String>`.
/// Production code uses `DmaBuffer::new` (SPDK hugepages); tests use
/// a heap-backed allocator via `DmaBuffer::from_raw`.
pub(crate) type DmaAllocFn =
    Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>;

/// Synchronous block I/O wrapper around an `IBlockDevice` connection.
///
/// Bridges the channel-based actor model (Command/Completion) to simple
/// `read_block`/`write_block` calls used by the extent manager internals.
pub(crate) struct BlockDevice {
    channels: ClientChannels,
    dma_alloc: DmaAllocFn,
    ns_id: u32,
    sector_size: u32,
    num_sectors: u64,
    numa_node: i32,
}

impl BlockDevice {
    /// Default DMA allocator using SPDK hugepage memory.
    fn default_dma_alloc() -> DmaAllocFn {
        Arc::new(|size, align, numa_node| {
            DmaBuffer::new(size, align, numa_node).map_err(|e| format!("{e}"))
        })
    }

    /// Create a new `BlockDevice` by connecting to an `IBlockDevice` provider.
    ///
    /// Uses the default SPDK DMA allocator for buffer allocation.
    ///
    /// # Arguments
    ///
    /// * `ibd` - The block device interface (typically from a component receptacle).
    /// * `ns_id` - NVMe namespace identifier.
    ///
    /// # Errors
    ///
    /// Returns `NvmeBlockError` if the client connection or device queries fail.
    pub(crate) fn new(
        ibd: &Arc<dyn IBlockDevice + Send + Sync>,
        ns_id: u32,
    ) -> Result<Self, NvmeBlockError> {
        Self::new_with_alloc(ibd, ns_id, Self::default_dma_alloc())
    }

    /// Create a new `BlockDevice` with a custom DMA allocator.
    ///
    /// This allows tests to inject a heap-backed allocator that does not
    /// require the SPDK runtime.
    pub(crate) fn new_with_alloc(
        ibd: &Arc<dyn IBlockDevice + Send + Sync>,
        ns_id: u32,
        dma_alloc: DmaAllocFn,
    ) -> Result<Self, NvmeBlockError> {
        let channels = ibd.connect_client()?;
        let sector_size = ibd.sector_size(ns_id)?;
        let num_sectors = ibd.num_sectors(ns_id)?;
        let numa_node = ibd.numa_node();

        Ok(Self {
            channels,
            dma_alloc,
            ns_id,
            sector_size,
            num_sectors,
            numa_node,
        })
    }

    /// Read a 4KiB block at the given logical block address.
    ///
    /// Allocates a DMA buffer, sends a synchronous read command, receives
    /// the completion, and copies the data into `buf`.
    ///
    /// # Errors
    ///
    /// Returns a string error on DMA allocation failure, channel errors,
    /// or I/O failures reported by the block device.
    pub(crate) fn read_block(&self, lba: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), String> {
        let dma_buf =
            (self.dma_alloc)(BLOCK_SIZE, self.sector_size as usize, Some(self.numa_node))?;
        let dma_arc = Arc::new(Mutex::new(dma_buf));

        self.channels
            .command_tx
            .send(Command::ReadSync {
                ns_id: self.ns_id,
                lba,
                buf: Arc::clone(&dma_arc),
            })
            .map_err(|e| format!("send ReadSync failed: {e}"))?;

        let completion = self
            .channels
            .completion_rx
            .recv()
            .map_err(|e| format!("recv completion failed: {e}"))?;

        match completion {
            Completion::ReadDone { result, .. } => {
                result.map_err(|e| format!("read I/O error: {e}"))?;
            }
            Completion::Error { error, .. } => {
                return Err(format!("block device error: {error}"));
            }
            other => {
                return Err(format!("unexpected completion for read: {other:?}"));
            }
        }

        let locked = dma_arc
            .lock()
            .map_err(|e| format!("DMA buffer lock poisoned: {e}"))?;
        buf.copy_from_slice(&locked.as_slice()[..BLOCK_SIZE]);
        Ok(())
    }

    /// Write a 4KiB block at the given logical block address.
    ///
    /// Allocates a DMA buffer, copies `buf` into it, sends a synchronous
    /// write command, and receives the completion.
    ///
    /// # Errors
    ///
    /// Returns a string error on DMA allocation failure, channel errors,
    /// or I/O failures reported by the block device.
    pub(crate) fn write_block(&self, lba: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), String> {
        let mut dma_buf =
            (self.dma_alloc)(BLOCK_SIZE, self.sector_size as usize, Some(self.numa_node))?;
        dma_buf.as_mut_slice()[..BLOCK_SIZE].copy_from_slice(buf);
        // DmaBuffer is Send but not Sync (raw pointer interior). The IBlockDevice
        // Command::WriteSync API requires Arc<DmaBuffer>; ownership is transferred
        // to the actor thread and not shared concurrently.
        #[allow(clippy::arc_with_non_send_sync)]
        let dma_arc = Arc::new(dma_buf);

        self.channels
            .command_tx
            .send(Command::WriteSync {
                ns_id: self.ns_id,
                lba,
                buf: dma_arc,
            })
            .map_err(|e| format!("send WriteSync failed: {e}"))?;

        let completion = self
            .channels
            .completion_rx
            .recv()
            .map_err(|e| format!("recv completion failed: {e}"))?;

        match completion {
            Completion::WriteDone { result, .. } => {
                result.map_err(|e| format!("write I/O error: {e}"))?;
            }
            Completion::Error { error, .. } => {
                return Err(format!("block device error: {error}"));
            }
            other => {
                return Err(format!("unexpected completion for write: {other:?}"));
            }
        }

        Ok(())
    }

    /// Return the total number of 4KiB blocks available on the device.
    pub(crate) fn block_count(&self) -> u64 {
        (self.num_sectors * self.sector_size as u64) / BLOCK_SIZE as u64
    }

    /// Return the NVMe namespace ID.
    pub(crate) fn ns_id(&self) -> u32 {
        self.ns_id
    }
}
