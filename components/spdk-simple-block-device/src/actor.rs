//! Actor-based block device: message types, handler, and client.
//!
//! The block device actor runs all NVMe operations on a dedicated thread,
//! naturally satisfying SPDK's single-thread-per-qpair requirement.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────┐          ┌──────────────────────┐
//! │ BlockDeviceClient│─request─>│ BlockDeviceHandler   │
//! │ (any thread)     │<─reply───│ (dedicated actor thr) │
//! └──────────────────┘          └──────────────────────┘
//! ```
//!
//! - [`BlockIoRequest`] is the message enum sent to the actor.
//! - [`BlockDeviceHandler`] implements [`ActorHandler`] and owns the NVMe state.
//! - [`BlockDeviceClient`] wraps an [`ActorHandle`] and provides a synchronous API.

use crate::error::BlockDeviceError;
use crate::io::{self, InnerState};
use component_framework::actor::{ActorHandle, ActorHandler};
use spdk_env::{DmaBuffer, ISPDKEnv, SpdkEnvError};
use std::sync::mpsc;
use std::sync::Arc;

/// Device geometry returned by [`BlockDeviceClient::open`] and
/// [`BlockDeviceClient::info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Sector size in bytes (e.g., 512 or 4096).
    pub sector_size: u32,
    /// Total number of sectors on the namespace.
    pub num_sectors: u64,
}

/// Messages sent to the block device actor.
///
/// Each variant carries a one-shot reply channel so the caller can wait
/// synchronously for the result.
pub enum BlockIoRequest {
    /// Probe NVMe, attach the first controller, open namespace 1.
    Open {
        reply: mpsc::SyncSender<Result<DeviceInfo, BlockDeviceError>>,
    },
    /// Read into caller-provided DMA buffer starting at `lba` (zero-copy).
    Read {
        lba: u64,
        buf: DmaBuffer,
        reply: mpsc::SyncSender<Result<DmaBuffer, BlockDeviceError>>,
    },
    /// Write from caller-provided DMA buffer starting at `lba` (zero-copy).
    Write {
        lba: u64,
        buf: DmaBuffer,
        reply: mpsc::SyncSender<Result<DmaBuffer, BlockDeviceError>>,
    },
    /// Free the I/O queue pair and detach the controller.
    Close {
        reply: mpsc::SyncSender<Result<(), BlockDeviceError>>,
    },
    /// Query current device info (None if not open).
    GetInfo {
        reply: mpsc::SyncSender<Option<DeviceInfo>>,
    },
}

/// Actor handler that processes [`BlockIoRequest`] messages on a dedicated thread.
///
/// All SPDK NVMe calls happen exclusively on this actor's thread, satisfying
/// the single-thread-per-qpair invariant.
pub struct BlockDeviceHandler {
    env: Arc<dyn ISPDKEnv + Send + Sync>,
    state: Option<InnerState>,
}

impl BlockDeviceHandler {
    /// Create a new handler.
    ///
    /// `env` must be an initialized [`ISPDKEnv`] interface (call `init()` first).
    pub fn new(env: Arc<dyn ISPDKEnv + Send + Sync>) -> Self {
        Self { env, state: None }
    }

    fn device_info(&self) -> Option<DeviceInfo> {
        self.state.as_ref().map(|s| DeviceInfo {
            sector_size: s.sector_size,
            num_sectors: s.num_sectors,
        })
    }
}

impl ActorHandler<BlockIoRequest> for BlockDeviceHandler {
    fn handle(&mut self, msg: BlockIoRequest) {
        match msg {
            BlockIoRequest::Open { reply } => {
                let result = if self.state.is_some() {
                    Err(BlockDeviceError::AlreadyOpen(
                        "Block device already open. Send Close first.".into(),
                    ))
                } else {
                    io::open_device(&*self.env).map(|inner| {
                        let info = DeviceInfo {
                            sector_size: inner.sector_size,
                            num_sectors: inner.num_sectors,
                        };
                        self.state = Some(inner);
                        info
                    })
                };
                let _ = reply.send(result);
            }

            BlockIoRequest::Read {
                lba,
                mut buf,
                reply,
            } => {
                let result = match self.state.as_ref() {
                    None => Err(BlockDeviceError::NotOpen(
                        "Block device not open. Send Open first.".into(),
                    )),
                    Some(inner) => io::read_blocks(inner, lba, &mut buf).map(|()| buf),
                };
                let _ = reply.send(result);
            }

            BlockIoRequest::Write { lba, buf, reply } => {
                let result = match self.state.as_ref() {
                    None => Err(BlockDeviceError::NotOpen(
                        "Block device not open. Send Open first.".into(),
                    )),
                    Some(inner) => io::write_blocks(inner, lba, &buf).map(|()| buf),
                };
                let _ = reply.send(result);
            }

            BlockIoRequest::Close { reply } => {
                let result = match self.state.take() {
                    None => Err(BlockDeviceError::NotOpen(
                        "Block device not open.".into(),
                    )),
                    Some(inner) => {
                        io::close_device(inner);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }

            BlockIoRequest::GetInfo { reply } => {
                let _ = reply.send(self.device_info());
            }
        }
    }

    fn on_stop(&mut self) {
        // Clean up if the actor is stopped while the device is still open.
        if let Some(inner) = self.state.take() {
            io::close_device(inner);
        }
    }
}

/// Synchronous client for the block device actor.
///
/// Wraps an [`ActorHandle<BlockIoRequest>`] and provides blocking methods
/// that send a request and wait for the reply. Can be used from any thread.
///
/// # Examples
///
/// ```ignore
/// use component_framework::actor::Actor;
/// use spdk_simple_block_device::actor::{BlockDeviceHandler, BlockDeviceClient};
///
/// let handler = BlockDeviceHandler::new(env_arc);
/// let actor = Actor::simple(handler);
/// let handle = actor.activate().unwrap();
/// let client = BlockDeviceClient::new(handle);
///
/// let info = client.open().unwrap();
/// println!("sector_size={}", info.sector_size);
///
/// // Zero-copy: DMA buffer is passed to the device directly.
/// let buf = client.alloc_dma_buffer(1).unwrap();
/// let buf = client.read_blocks(0, buf).unwrap();
/// let buf = client.write_blocks(0, buf).unwrap();
///
/// client.close().unwrap();
/// client.shutdown().unwrap();
/// ```
pub struct BlockDeviceClient {
    handle: ActorHandle<BlockIoRequest>,
}

impl BlockDeviceClient {
    /// Create a client from an activated actor handle.
    pub fn new(handle: ActorHandle<BlockIoRequest>) -> Self {
        Self { handle }
    }

    /// Probe NVMe, attach the first controller, and open namespace 1.
    ///
    /// Returns device geometry on success.
    pub fn open(&self) -> Result<DeviceInfo, BlockDeviceError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.handle
            .send(BlockIoRequest::Open { reply: tx })
            .map_err(|e| BlockDeviceError::NotOpen(format!("actor send failed: {e}")))?;
        rx.recv()
            .map_err(|e| BlockDeviceError::NotOpen(format!("actor reply failed: {e}")))?
    }

    /// Read into a caller-provided [`DmaBuffer`] starting at `lba` (zero-copy).
    ///
    /// The buffer is sent to the actor thread and returned after the NVMe
    /// read completes — no copies occur.
    pub fn read_blocks(
        &self,
        lba: u64,
        buf: DmaBuffer,
    ) -> Result<DmaBuffer, BlockDeviceError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.handle
            .send(BlockIoRequest::Read {
                lba,
                buf,
                reply: tx,
            })
            .map_err(|e| BlockDeviceError::ReadFailed(format!("actor send failed: {e}")))?;
        rx.recv()
            .map_err(|e| BlockDeviceError::ReadFailed(format!("actor reply failed: {e}")))?
    }

    /// Write from a caller-provided [`DmaBuffer`] starting at `lba` (zero-copy).
    ///
    /// The buffer is sent to the actor thread and returned after the NVMe
    /// write completes — no copies occur. The caller can reuse the buffer.
    pub fn write_blocks(
        &self,
        lba: u64,
        buf: DmaBuffer,
    ) -> Result<DmaBuffer, BlockDeviceError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.handle
            .send(BlockIoRequest::Write {
                lba,
                buf,
                reply: tx,
            })
            .map_err(|e| BlockDeviceError::WriteFailed(format!("actor send failed: {e}")))?;
        rx.recv()
            .map_err(|e| BlockDeviceError::WriteFailed(format!("actor reply failed: {e}")))?
    }

    /// Allocate a DMA buffer sized for `num_sectors` sectors.
    ///
    /// Convenience method that uses the cached sector size from [`open`].
    /// Returns an error if the device has not been opened.
    pub fn alloc_dma_buffer(&self, num_sectors: u32) -> Result<DmaBuffer, BlockDeviceError> {
        let info = self.info().ok_or_else(|| {
            BlockDeviceError::NotOpen("Cannot allocate DMA buffer: device not open.".into())
        })?;
        let size = num_sectors as usize * info.sector_size as usize;
        DmaBuffer::new(size, info.sector_size as usize, None).map_err(|e| match e {
            SpdkEnvError::DmaAllocationFailed(msg) => {
                BlockDeviceError::DmaAllocationFailed(msg)
            }
            other => BlockDeviceError::DmaAllocationFailed(other.to_string()),
        })
    }

    /// Close the block device: free the I/O queue pair and detach the controller.
    pub fn close(&self) -> Result<(), BlockDeviceError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.handle
            .send(BlockIoRequest::Close { reply: tx })
            .map_err(|e| BlockDeviceError::NotOpen(format!("actor send failed: {e}")))?;
        rx.recv()
            .map_err(|e| BlockDeviceError::NotOpen(format!("actor reply failed: {e}")))?
    }

    /// Query current device info. Returns `None` if the device is not open.
    pub fn info(&self) -> Option<DeviceInfo> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.handle
            .send(BlockIoRequest::GetInfo { reply: tx })
            .ok()?;
        rx.recv().ok()?
    }

    /// Deactivate the actor, joining its thread.
    ///
    /// If the device is still open, the actor's `on_stop` handler will
    /// close it automatically.
    pub fn shutdown(self) -> Result<(), crate::error::BlockDeviceError> {
        self.handle
            .deactivate()
            .map_err(|e| BlockDeviceError::NotOpen(format!("actor shutdown failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_debug() {
        let info = DeviceInfo {
            sector_size: 512,
            num_sectors: 1024,
        };
        let dbg = format!("{:?}", info);
        assert!(dbg.contains("512"));
        assert!(dbg.contains("1024"));
    }

    #[test]
    fn device_info_clone() {
        let info = DeviceInfo {
            sector_size: 4096,
            num_sectors: 2048,
        };
        let info2 = info;
        assert_eq!(info, info2);
    }

    #[test]
    fn device_info_equality() {
        let a = DeviceInfo {
            sector_size: 512,
            num_sectors: 100,
        };
        let b = DeviceInfo {
            sector_size: 512,
            num_sectors: 100,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn device_info_inequality() {
        let a = DeviceInfo {
            sector_size: 512,
            num_sectors: 100,
        };
        let b = DeviceInfo {
            sector_size: 4096,
            num_sectors: 100,
        };
        assert_ne!(a, b);
    }
}
