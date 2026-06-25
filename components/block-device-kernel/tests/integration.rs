// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]

//! Integration tests for block-device-kernel component.
//!
//! These tests require a real block device (default: /dev/nvme0n1).
//! Override with env var TEST_BLOCK_DEVICE.
//!
//! Run with: cargo test -p block-device-kernel --test integration -- --ignored

use std::sync::{Arc, Mutex};

use block_device_kernel::{
    BlockDeviceKernelComponent, ClientChannels, Command, Completion, IBlockDevice, NvmeBlockError,
};
use interfaces::DmaBuffer;

fn test_device() -> String {
    std::env::var("TEST_BLOCK_DEVICE").unwrap_or_else(|_| "/dev/nvme0n1".to_string())
}

unsafe extern "C" fn aligned_free(ptr: *mut std::ffi::c_void) {
    libc::free(ptr);
}

fn alloc_dma_buffer(size: usize) -> DmaBuffer {
    // SAFETY: posix_memalign returns 512-byte aligned memory required for O_DIRECT.
    unsafe {
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let ret = libc::posix_memalign(&mut ptr, 512, size);
        assert_eq!(ret, 0, "posix_memalign failed");
        std::ptr::write_bytes(ptr as *mut u8, 0, size);
        DmaBuffer::from_raw(ptr, size, aligned_free, -1).unwrap()
    }
}

fn alloc_dma_buffer_with_data(data: &[u8]) -> DmaBuffer {
    let mut buf = alloc_dma_buffer(data.len());
    buf.as_mut_slice().copy_from_slice(data);
    buf
}

fn setup_component(
    block_size: u32,
    num_blocks: u64,
) -> (Arc<BlockDeviceKernelComponent>, ClientChannels) {
    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, block_size, num_blocks);
    comp.initialize().expect("initialize failed");
    let channels = comp.connect_client().expect("connect_client failed");
    (comp, channels)
}

#[test]
#[ignore]
fn initialize_opens_device() {
    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 256);
    comp.initialize().expect("init failed");
    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn initialize_auto_detects_size() {
    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 0);
    comp.initialize().expect("init failed");
    assert!(comp.num_sectors(1).unwrap() > 0);
    comp.shutdown().unwrap();
}

#[test]
fn initialize_errors_on_nonexistent_path() {
    let comp = BlockDeviceKernelComponent::create("/dev/nonexistent_blk_device_xyz", 4096, 256);
    let result = comp.initialize();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, NvmeBlockError::NotInitialized(_)));
}

#[test]
fn initialize_rejects_non_block_device() {
    let comp = BlockDeviceKernelComponent::create("/dev/null", 4096, 256);
    let result = comp.initialize();
    assert!(result.is_err());
}

#[test]
#[ignore]
fn write_sync_read_sync_roundtrip() {
    let (comp, channels) = setup_component(4096, 256);

    let write_data = vec![0xAB_u8; 4096];
    let write_buf = alloc_dma_buffer_with_data(&write_data);
    let write_buf_arc = Arc::new(write_buf);

    channels
        .command_tx
        .send(Command::WriteSync {
            ns_id: 1,
            lba: 5,
            buf: write_buf_arc,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    assert!(matches!(
        completion,
        Completion::WriteDone { result: Ok(()), .. }
    ));

    let read_buf = alloc_dma_buffer(4096);
    let read_buf_arc = Arc::new(Mutex::new(read_buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 1,
            lba: 5,
            buf: Arc::clone(&read_buf_arc),
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    assert!(matches!(
        completion,
        Completion::ReadDone { result: Ok(()), .. }
    ));

    let guard = read_buf_arc.lock().unwrap();
    assert_eq!(guard.as_slice(), &write_data[..]);

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn async_read_write_completions() {
    let (comp, channels) = setup_component(4096, 256);

    let write_data = vec![0xCD_u8; 4096];
    let write_buf = alloc_dma_buffer_with_data(&write_data);
    let write_buf_arc = Arc::new(write_buf);

    channels
        .command_tx
        .send(Command::WriteAsync {
            ns_id: 1,
            lba: 0,
            buf: write_buf_arc,
            timeout_ms: 5000,
            tag: 0,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::WriteDone { handle, result, .. } => {
            assert!(result.is_ok());
            assert_eq!(handle.0, 1);
        }
        _ => panic!("expected WriteDone, got {:?}", completion),
    }

    let read_buf = alloc_dma_buffer(4096);
    let read_buf_arc = Arc::new(Mutex::new(read_buf));

    channels
        .command_tx
        .send(Command::ReadAsync {
            ns_id: 1,
            lba: 0,
            buf: Arc::clone(&read_buf_arc),
            timeout_ms: 5000,
            tag: 0,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::ReadDone { handle, result, .. } => {
            assert!(result.is_ok());
            assert_eq!(handle.0, 2);
        }
        _ => panic!("expected ReadDone"),
    }

    let guard = read_buf_arc.lock().unwrap();
    assert_eq!(guard.as_slice(), &write_data[..]);

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn write_zeros_produces_zero_data() {
    let (comp, channels) = setup_component(4096, 256);

    // First write non-zero data
    let write_data = vec![0xFF_u8; 4096 * 4];
    let write_buf = alloc_dma_buffer_with_data(&write_data);
    channels
        .command_tx
        .send(Command::WriteSync {
            ns_id: 1,
            lba: 0,
            buf: Arc::new(write_buf),
        })
        .unwrap();
    let _ = channels.completion_rx.recv().unwrap();

    // Now write zeros
    channels
        .command_tx
        .send(Command::WriteZeros {
            ns_id: 1,
            lba: 0,
            num_blocks: 4,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    assert!(matches!(
        completion,
        Completion::WriteZerosDone { result: Ok(()), .. }
    ));

    // Read back and verify zeros
    let read_buf = alloc_dma_buffer(4096 * 4);
    let read_buf_arc = Arc::new(Mutex::new(read_buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 1,
            lba: 0,
            buf: Arc::clone(&read_buf_arc),
        })
        .unwrap();
    let _ = channels.completion_rx.recv().unwrap();

    let guard = read_buf_arc.lock().unwrap();
    assert!(guard.as_slice().iter().all(|&b| b == 0));

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn lba_out_of_range_error() {
    let (comp, channels) = setup_component(4096, 16);

    let buf = alloc_dma_buffer(4096);
    let buf_arc = Arc::new(Mutex::new(buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 1,
            lba: 20,
            buf: buf_arc,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::ReadDone { result: Err(e), .. } => {
            assert!(matches!(e, NvmeBlockError::LbaOutOfRange(_)));
        }
        _ => panic!("expected LbaOutOfRange error"),
    }

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn invalid_namespace_error() {
    let (comp, channels) = setup_component(4096, 16);

    let buf = alloc_dma_buffer(4096);
    let buf_arc = Arc::new(Mutex::new(buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 2,
            lba: 0,
            buf: buf_arc,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::ReadDone { result: Err(e), .. } => {
            assert!(matches!(e, NvmeBlockError::InvalidNamespace(_)));
        }
        _ => panic!("expected InvalidNamespace error"),
    }

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn ns_probe_returns_single_namespace() {
    let (comp, channels) = setup_component(4096, 1024);

    channels.command_tx.send(Command::NsProbe).unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::NsProbeResult { namespaces } => {
            assert_eq!(namespaces.len(), 1);
            assert_eq!(namespaces[0].ns_id, 1);
            assert_eq!(namespaces[0].num_sectors, 1024);
            assert_eq!(namespaces[0].sector_size, 4096);
        }
        _ => panic!("expected NsProbeResult"),
    }

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn unsupported_operations_return_not_supported() {
    let (comp, channels) = setup_component(4096, 16);

    channels
        .command_tx
        .send(Command::NsCreate { size_sectors: 100 })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::Error { error, .. } => {
            assert!(matches!(error, NvmeBlockError::NotSupported(_)));
        }
        _ => panic!("expected NotSupported error"),
    }

    channels.command_tx.send(Command::ControllerReset).unwrap();
    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::Error { error, .. } => {
            assert!(matches!(error, NvmeBlockError::NotSupported(_)));
        }
        _ => panic!("expected NotSupported error"),
    }

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn device_info_methods() {
    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 2048);
    comp.initialize().unwrap();

    assert_eq!(comp.sector_size(1).unwrap(), 4096);
    assert_eq!(comp.num_sectors(1).unwrap(), 2048);
    assert_eq!(comp.block_size(), 4096);
    assert_eq!(comp.num_io_queues(), 1);
    assert!(comp.max_queue_depth() > 0);
    assert_eq!(comp.max_transfer_size(), 4096 * 256);
    assert_eq!(comp.numa_node(), -1);
    assert_eq!(comp.nvme_version(), "N/A (kernel block device)");

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn multiple_clients_independent_channels() {
    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 256);
    comp.initialize().unwrap();

    let ch1 = comp.connect_client().unwrap();
    let ch2 = comp.connect_client().unwrap();

    // Client 1 writes
    let data1 = vec![0x11_u8; 4096];
    let buf1 = alloc_dma_buffer_with_data(&data1);
    ch1.command_tx
        .send(Command::WriteSync {
            ns_id: 1,
            lba: 0,
            buf: Arc::new(buf1),
        })
        .unwrap();
    let _ = ch1.completion_rx.recv().unwrap();

    // Client 2 writes
    let data2 = vec![0x22_u8; 4096];
    let buf2 = alloc_dma_buffer_with_data(&data2);
    ch2.command_tx
        .send(Command::WriteSync {
            ns_id: 1,
            lba: 1,
            buf: Arc::new(buf2),
        })
        .unwrap();
    let _ = ch2.completion_rx.recv().unwrap();

    // Client 1 reads what client 2 wrote
    let read_buf = alloc_dma_buffer(4096);
    let read_buf_arc = Arc::new(Mutex::new(read_buf));
    ch1.command_tx
        .send(Command::ReadSync {
            ns_id: 1,
            lba: 1,
            buf: Arc::clone(&read_buf_arc),
        })
        .unwrap();
    let _ = ch1.completion_rx.recv().unwrap();

    let guard = read_buf_arc.lock().unwrap();
    assert_eq!(guard.as_slice(), &data2[..]);

    comp.shutdown().unwrap();
}

#[test]
#[ignore]
fn component_provides_iblock_device() {
    use component_core::IUnknown;

    let device = test_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 16);

    let ifaces = comp.provided_interfaces();
    assert!(ifaces.iter().any(|i| i.name == "IBlockDevice"));
}

#[test]
#[ignore]
fn data_integrity_multi_block_patterns() {
    let (comp, channels) = setup_component(4096, 256);
    let block_size = 4096;
    let num_test_blocks = 64u64;

    for lba in 0..num_test_blocks {
        let pattern = (lba & 0xFF) as u8;
        let data = vec![pattern; block_size];
        let buf = alloc_dma_buffer_with_data(&data);
        channels
            .command_tx
            .send(Command::WriteSync {
                ns_id: 1,
                lba,
                buf: Arc::new(buf),
            })
            .unwrap();
        let completion = channels.completion_rx.recv().unwrap();
        assert!(
            matches!(completion, Completion::WriteDone { result: Ok(()), .. }),
            "write failed at lba {lba}"
        );
    }

    for lba in 0..num_test_blocks {
        let expected_pattern = (lba & 0xFF) as u8;
        let read_buf = alloc_dma_buffer(block_size);
        let read_buf_arc = Arc::new(Mutex::new(read_buf));
        channels
            .command_tx
            .send(Command::ReadSync {
                ns_id: 1,
                lba,
                buf: Arc::clone(&read_buf_arc),
            })
            .unwrap();
        let completion = channels.completion_rx.recv().unwrap();
        assert!(
            matches!(completion, Completion::ReadDone { result: Ok(()), .. }),
            "read failed at lba {lba}"
        );

        let guard = read_buf_arc.lock().unwrap();
        let slice = guard.as_slice();
        for (i, &byte) in slice.iter().enumerate() {
            assert_eq!(
                byte, expected_pattern,
                "data mismatch at lba {lba}, offset {i}: expected 0x{expected_pattern:02X}, got 0x{byte:02X}"
            );
        }
    }

    comp.shutdown().unwrap();
}
