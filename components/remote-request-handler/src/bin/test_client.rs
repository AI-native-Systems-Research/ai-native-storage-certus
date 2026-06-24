//! Standalone test client for the remote request handler.
//!
//! Connects to a running handler, performs a version handshake,
//! executes configurable batched lookups, and disconnects cleanly.
//! Optionally verifies data integrity via CRC32.

use anyhow::{bail, Result};
use clap::Parser;
use prost::Message;

use remote_request_handler::protocol::{self, proto};
use remote_request_handler::rdma;

const PROTOCOL_VERSION: u32 = 1;
const MSG_BUF_SIZE: usize = 8192;

#[derive(Parser, Debug)]
#[command(name = "test-client", about = "Test client for remote-request-handler")]
struct Args {
    /// Handler address (IPv4).
    #[arg(long, default_value = "127.0.0.1")]
    addr: String,

    /// Handler port.
    #[arg(long, default_value_t = 18515)]
    port: u16,

    /// Number of entries per batch.
    #[arg(long, default_value_t = 16)]
    batch_size: u32,

    /// Number of batch iterations to perform.
    #[arg(long, default_value_t = 1)]
    iterations: u32,

    /// Client identifier (for telemetry/logging on the server).
    #[arg(long, default_value = "test-client")]
    client_id: String,

    /// Result buffer size per entry in bytes (should match object size on server).
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    result_buf_size: usize,

    /// Verify data integrity via CRC32 after each batch.
    #[arg(long)]
    verify: bool,

    /// Expected fill byte for verification (default 0xAB from cudaMemset).
    #[arg(long, default_value_t = 0xAB)]
    expected_fill: u8,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!(
        "Remote Request Handler Test Client\n\
         ==================================\n\
         Target: {}:{}\n\
         Batch size: {} entries\n\
         Iterations: {}\n\
         Result buf: {} bytes\n\
         Verify: {}\n\
         Client ID: {}\n",
        args.addr,
        args.port,
        args.batch_size,
        args.iterations,
        args.result_buf_size,
        if args.verify { "CRC32" } else { "disabled" },
        args.client_id
    );

    // Connect to the handler via RDMA
    println!("Connecting...");
    let conn = rdma::client_connect(&args.addr, args.port)?;
    println!("Connected!");

    // Register memory regions for message exchange
    let mut send_mr = conn.register_mr(MSG_BUF_SIZE)?;
    let mut recv_mr = conn.register_mr(MSG_BUF_SIZE)?;

    // Register per-entry result buffers
    let mut result_mrs: Vec<rdma::MemoryRegion> = Vec::new();
    for _ in 0..args.batch_size {
        result_mrs.push(conn.register_mr(args.result_buf_size)?);
    }

    // --- Handshake ---
    println!("Sending handshake (version={})...", PROTOCOL_VERSION);
    let handshake_req = protocol::handshake_request(proto::HandshakeRequest {
        protocol_version: PROTOCOL_VERSION,
        client_id: args.client_id.clone(),
    });
    let encoded = protocol::encode_request(&handshake_req);
    send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
    conn.send_msg(&send_mr, encoded.len())?;

    let nbytes = conn.recv_msg(&mut recv_mr)?;
    let response = proto::ResponseMessage::decode(&recv_mr.buf[..nbytes])?;
    match response.payload {
        Some(proto::response_message::Payload::Handshake(ref h)) => {
            if !h.accepted {
                bail!(
                    "Handshake rejected: {} (server version={})",
                    h.error_message,
                    h.server_version
                );
            }
            println!(
                "Handshake accepted! Server version={}, max_batch_size={}",
                h.server_version, h.max_batch_size
            );
        }
        _ => bail!("Expected handshake response, got something else"),
    }

    // --- Batch Lookups ---
    let start = std::time::Instant::now();
    let mut total_entries = 0u64;
    let mut total_bytes_written = 0u64;
    let mut crc_errors = 0u64;
    let mut crc_pass = 0u64;
    let mut batch_latencies_us: Vec<f64> = Vec::with_capacity(args.iterations as usize);

    for iter in 0..args.iterations {
        // Clear result buffers before each batch (to detect actual writes)
        if args.verify {
            for mr in &mut result_mrs {
                mr.buf.fill(0);
            }
        }

        // Build batch request with result buffer addresses
        let entries: Vec<proto::LookupEntry> = (0..args.batch_size)
            .map(|i| proto::LookupEntry {
                cache_key: (iter as u64) * (args.batch_size as u64) + (i as u64) + 1,
                remote_addr: result_mrs[i as usize].addr(),
                rkey: result_mrs[i as usize].rkey(),
                max_size: args.result_buf_size as u32,
            })
            .collect();

        let batch_req = protocol::lookup_request(proto::BatchLookupRequest {
            batch_id: iter + 1,
            entries,
        });

        let encoded = protocol::encode_request(&batch_req);
        send_mr.buf[..encoded.len()].copy_from_slice(&encoded);

        let batch_start = std::time::Instant::now();
        conn.send_msg(&send_mr, encoded.len())?;

        // Receive batch response
        let nbytes = conn.recv_msg(&mut recv_mr)?;
        let batch_elapsed_us = batch_start.elapsed().as_secs_f64() * 1_000_000.0;
        batch_latencies_us.push(batch_elapsed_us);
        let response = proto::ResponseMessage::decode(&recv_mr.buf[..nbytes])?;
        match response.payload {
            Some(proto::response_message::Payload::Lookup(ref l)) => {
                let ok_count = l.results.iter().filter(|r| r.success).count();
                let err_count = l.results.len() - ok_count;
                total_entries += l.results.len() as u64;

                for (i, result) in l.results.iter().enumerate() {
                    if result.success {
                        total_bytes_written += result.bytes_written as u64;
                    }

                    // CRC verification
                    if args.verify && result.success && result.bytes_written > 0 {
                        let data = &result_mrs[i].buf[..result.bytes_written as usize];
                        let crc = crc32fast::hash(data);

                        // Compute expected CRC for the fill pattern
                        let expected_data =
                            vec![args.expected_fill; result.bytes_written as usize];
                        let expected_crc = crc32fast::hash(&expected_data);

                        if crc == expected_crc {
                            crc_pass += 1;
                        } else {
                            crc_errors += 1;
                            if crc_errors <= 5 {
                                eprintln!(
                                    "  CRC MISMATCH key={}: got 0x{:08X}, expected 0x{:08X} ({} bytes)",
                                    result.cache_key, crc, expected_crc, result.bytes_written
                                );
                            }
                        }
                    }
                }

                if args.iterations <= 10 {
                    println!(
                        "  Batch {}: {} ok, {} not_found/error",
                        l.batch_id, ok_count, err_count
                    );
                }
            }
            _ => bail!("Expected lookup response for batch {}", iter + 1),
        }
    }

    let elapsed = start.elapsed();
    let elapsed_s = elapsed.as_secs_f64();
    println!(
        "\nCompleted {} iterations ({} total entries) in {:.3}ms",
        args.iterations,
        total_entries,
        elapsed_s * 1000.0
    );
    if args.iterations > 0 {
        println!(
            "Average: {:.1} us/batch, {:.1} us/entry",
            elapsed.as_micros() as f64 / args.iterations as f64,
            elapsed.as_micros() as f64 / total_entries as f64,
        );
    }
    if total_bytes_written > 0 {
        let throughput_gbs = total_bytes_written as f64 / (1024.0 * 1024.0 * 1024.0) / elapsed_s;
        println!(
            "Data transferred: {:.1} MiB, throughput: {:.3} GB/s",
            total_bytes_written as f64 / (1024.0 * 1024.0),
            throughput_gbs
        );
    }
    if !batch_latencies_us.is_empty() {
        let min = batch_latencies_us.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = batch_latencies_us.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = batch_latencies_us.iter().sum::<f64>() / batch_latencies_us.len() as f64;
        println!(
            "Batch latency: min={:.1} us, mean={:.1} us, max={:.1} us",
            min, mean, max
        );
    }

    // CRC summary
    if args.verify {
        println!("\nCRC32 Verification:");
        println!("  Pass: {}", crc_pass);
        println!("  Fail: {}", crc_errors);
        if crc_errors > 0 {
            println!("  STATUS: FAIL");
        } else if crc_pass > 0 {
            println!("  STATUS: PASS");
        } else {
            println!("  STATUS: NO DATA (all lookups returned not-found)");
        }
    }

    // --- Close ---
    println!("\nSending close request...");
    let close_req = protocol::close_request(proto::CloseRequest {
        reason: "test complete".into(),
    });
    let encoded = protocol::encode_request(&close_req);
    send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
    conn.send_msg(&send_mr, encoded.len())?;

    let nbytes = conn.recv_msg(&mut recv_mr)?;
    let response = proto::ResponseMessage::decode(&recv_mr.buf[..nbytes])?;
    match response.payload {
        Some(proto::response_message::Payload::Close(ref c)) => {
            println!("Close acknowledged: {} batches processed", c.batches_total);
        }
        _ => bail!("Expected close response"),
    }

    println!("Done.");

    Ok(())
}
