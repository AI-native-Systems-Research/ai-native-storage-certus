// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]

//! Integration tests for block-device-filesys component.

use std::sync::{Arc, Mutex};

use block_device_filesys::{
    BlockDeviceFilesysComponent, ClientChannels, Command, Completion, IBlockDevice, NvmeBlockError,
};
use interfaces::DmaBuffer;

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
) -> (
    Arc<BlockDeviceFilesysComponent>,
    ClientChannels,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("test-device.img");
    let path_str = path.to_str().unwrap();

    let comp = BlockDeviceFilesysComponent::create(path_str, block_size, num_blocks);
    comp.initialize().expect("initialize failed");
    let channels = comp.connect_client().expect("connect_client failed");

    (comp, channels, dir)
}

#[test]
fn initialize_creates_backing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("init-test.img");
    let path_str = path.to_str().unwrap();

    let comp = BlockDeviceFilesysComponent::create(path_str, 4096, 256);
    comp.initialize().expect("init failed");

    assert!(path.exists());
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(meta.len(), 4096 * 256);

    comp.shutdown().unwrap();
}

#[test]
fn initialize_errors_on_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mismatch-test.img");

    // Create a file with wrong size
    std::fs::write(&path, vec![0u8; 1000]).unwrap();

    let comp = BlockDeviceFilesysComponent::create(path.to_str().unwrap(), 4096, 256);
    let result = comp.initialize();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, NvmeBlockError::NotInitialized(_)));
}

#[test]
fn write_sync_read_sync_roundtrip() {
    let (comp, channels, _dir) = setup_component(4096, 256);

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
fn async_read_write_completions() {
    let (comp, channels, _dir) = setup_component(4096, 256);

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
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::WriteDone { handle, result } => {
            assert!(result.is_ok());
            assert_eq!(handle.0, 1); // First operation gets handle 1
        }
        _ => panic!("expected WriteDone, got {:?}", completion),
    }

    // Now read it back async
    let read_buf = alloc_dma_buffer(4096);
    let read_buf_arc = Arc::new(Mutex::new(read_buf));

    channels
        .command_tx
        .send(Command::ReadAsync {
            ns_id: 1,
            lba: 0,
            buf: Arc::clone(&read_buf_arc),
            timeout_ms: 5000,
        })
        .unwrap();

    let completion = channels.completion_rx.recv().unwrap();
    match completion {
        Completion::ReadDone { handle, result } => {
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
fn write_zeros_produces_zero_data() {
    let (comp, channels, _dir) = setup_component(4096, 256);

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
fn lba_out_of_range_error() {
    let (comp, channels, _dir) = setup_component(4096, 16);

    let buf = alloc_dma_buffer(4096);
    let buf_arc = Arc::new(Mutex::new(buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 1,
            lba: 20, // Beyond device (16 blocks)
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
fn invalid_namespace_error() {
    let (comp, channels, _dir) = setup_component(4096, 16);

    let buf = alloc_dma_buffer(4096);
    let buf_arc = Arc::new(Mutex::new(buf));

    channels
        .command_tx
        .send(Command::ReadSync {
            ns_id: 2, // Invalid — only ns_id=1 supported
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
fn ns_probe_returns_single_namespace() {
    let (comp, channels, _dir) = setup_component(4096, 1024);

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
fn unsupported_operations_return_not_supported() {
    let (comp, channels, _dir) = setup_component(4096, 16);

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
fn device_info_methods() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("info-test.img");
    let comp = BlockDeviceFilesysComponent::create(path.to_str().unwrap(), 4096, 2048);
    comp.initialize().unwrap();

    assert_eq!(comp.sector_size(1).unwrap(), 4096);
    assert_eq!(comp.num_sectors(1).unwrap(), 2048);
    assert_eq!(comp.block_size(), 4096);
    assert_eq!(comp.num_io_queues(), 1);
    assert!(comp.max_queue_depth() > 0);
    assert_eq!(comp.max_transfer_size(), 4096 * 256);
    assert_eq!(comp.numa_node(), -1);
    assert_eq!(comp.nvme_version(), "N/A (file-backed)");

    comp.shutdown().unwrap();
}

#[test]
fn multiple_clients_independent_channels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi-client.img");
    let comp = BlockDeviceFilesysComponent::create(path.to_str().unwrap(), 4096, 256);
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
fn invalid_file_path_initialization_error() {
    let comp = BlockDeviceFilesysComponent::create("/nonexistent/path/dev.img", 4096, 256);
    let result = comp.initialize();
    assert!(result.is_err());
}

#[test]
fn data_integrity_multi_block_patterns() {
    let (comp, channels, _dir) = setup_component(4096, 256);
    let block_size = 4096;
    let num_test_blocks = 64u64;

    // Write a unique pattern to each block: block N gets bytes [N, N, N, ...].
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

    // Read back every block and verify the pattern matches.
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

    // Overwrite a subset (blocks 10..20) with a different pattern, verify
    // both the overwritten range and the untouched blocks.
    for lba in 10..20 {
        let pattern = 0xFE_u8;
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
        let _ = channels.completion_rx.recv().unwrap();
    }

    for lba in 0..num_test_blocks {
        let expected = if (10..20).contains(&lba) {
            0xFE_u8
        } else {
            (lba & 0xFF) as u8
        };
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
        let _ = channels.completion_rx.recv().unwrap();

        let guard = read_buf_arc.lock().unwrap();
        assert!(
            guard.as_slice().iter().all(|&b| b == expected),
            "integrity check failed at lba {lba}: expected all 0x{expected:02X}"
        );
    }

    comp.shutdown().unwrap();
}

#[test]
fn component_provides_iblock_device() {
    use component_core::IUnknown;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("iunknown-test.img");
    let comp = BlockDeviceFilesysComponent::create(path.to_str().unwrap(), 4096, 16);

    let ifaces = comp.provided_interfaces();
    assert!(ifaces.iter().any(|i| i.name == "IBlockDevice"));
}
