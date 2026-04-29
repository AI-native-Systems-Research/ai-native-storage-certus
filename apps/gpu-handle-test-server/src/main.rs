//! GPU Handle Test Server
//!
//! Demonstrates receiving a CUDA IPC handle from a Python client via
//! Unix domain socket, deserializing it, verifying GPU memory, pinning,
//! and creating a DMA buffer.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use component_core::query_interface;
use gpu_services::GpuServicesComponentV0;
use interfaces::{IGpuServices, ILogger};

const SOCKET_PATH: &str = "/tmp/gpu-services-ipc.sock";

fn main() {
    let component = GpuServicesComponentV0::new();

    let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponentV1::new_default();
    component.logger.connect(logger).unwrap();

    let gpu = query_interface!(component, IGpuServices).expect("IGpuServices not available");

    gpu.initialize().expect("CUDA initialization failed");

    let devices = gpu.get_devices().expect("Failed to get devices");
    for dev in &devices {
        println!(
            "GPU {}: {} (compute {}.{}), {} MB",
            dev.device_index,
            dev.name,
            dev.compute_major,
            dev.compute_minor,
            dev.memory_bytes / (1024 * 1024)
        );
    }

    // Remove stale socket file
    let _ = std::fs::remove_file(SOCKET_PATH);

    let listener = UnixListener::bind(SOCKET_PATH).expect("Failed to bind Unix socket");
    println!("Listening on {}", SOCKET_PATH);

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(e) = handle_client(&mut stream, &*gpu) {
                    eprintln!("Client error: {}", e);
                    let _ = send_error(&mut stream, &e);
                }
            }
            Err(e) => eprintln!("Accept error: {}", e),
        }
    }

    gpu.shutdown().expect("Shutdown failed");
}

fn handle_client(
    stream: &mut std::os::unix::net::UnixStream,
    gpu: &dyn IGpuServices,
) -> Result<(), String> {
    // Read 4-byte length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
    let payload_len = u32::from_le_bytes(len_buf) as usize;

    if payload_len == 0 || payload_len > 1024 {
        return Err(format!("Invalid payload length: {}", payload_len));
    }

    // Read payload
    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload).map_err(|e| e.to_string())?;

    let base64_str =
        std::str::from_utf8(&payload).map_err(|e| format!("Invalid UTF-8: {}", e))?;

    println!("Received payload: {} bytes", payload_len);

    // Deserialize IPC handle
    let handle = gpu.deserialize_ipc_handle(base64_str)?;
    println!("IPC handle deserialized: {} bytes", handle.size());

    // Verify memory
    gpu.verify_memory(&handle)?;
    println!("Memory verified: device type, contiguous");

    // Pin memory
    gpu.pin_memory(&handle)?;
    println!("Memory pinned for DMA");

    // Create DMA buffer
    let dma_buf = gpu.create_dma_buffer(handle)?;
    println!("DMA buffer created: {} bytes", dma_buf.len());

    // Send success ACK
    stream.write_all(&[0x01]).map_err(|e| e.to_string())?;
    println!("ACK sent to client");

    // Buffer will be dropped here, closing the IPC handle
    drop(dma_buf);
    Ok(())
}

fn send_error(stream: &mut std::os::unix::net::UnixStream, msg: &str) -> Result<(), String> {
    stream.write_all(&[0x00]).map_err(|e| e.to_string())?;
    let msg_bytes = msg.as_bytes();
    let len = (msg_bytes.len() as u32).to_le_bytes();
    stream.write_all(&len).map_err(|e| e.to_string())?;
    stream.write_all(msg_bytes).map_err(|e| e.to_string())?;
    Ok(())
}
