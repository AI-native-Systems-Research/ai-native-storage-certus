//! Standalone handler server for testing the remote-request-handler RDMA path.
//!
//! Listens for connections, processes handshakes, handles batched lookups
//! (resolving via a stub dispatcher that always returns "not found"),
//! and handles close requests.

use anyhow::{bail, Result};
use clap::Parser;
use prost::Message;

use remote_request_handler::protocol::{self, proto};
use remote_request_handler::rdma::{RdmaConnection, RdmaListener};
use remote_request_handler::session::{Session, SessionConfig, MAX_BATCH_SIZE};

const PROTOCOL_VERSION: u32 = 1;
const MSG_BUF_SIZE: usize = 8192;

#[derive(Parser, Debug)]
#[command(name = "handler-server", about = "RDMA remote request handler server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0")]
    addr: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 18515)]
    port: u16,
}

fn handle_session(conn: RdmaConnection) -> Result<()> {
    let session = Session::new(SessionConfig {
        protocol_version: PROTOCOL_VERSION,
        max_batch_size: MAX_BATCH_SIZE,
    });

    let mut recv_mr = conn.register_mr(MSG_BUF_SIZE)?;
    let mut send_mr = conn.register_mr(MSG_BUF_SIZE)?;

    println!("  Session: waiting for handshake...");

    // Receive and process messages in a loop
    loop {
        let nbytes = conn.recv_msg(&mut recv_mr)?;

        let request = proto::RequestMessage::decode(&recv_mr.buf[..nbytes])?;
        let response = match request.payload {
            Some(proto::request_message::Payload::Handshake(ref req)) => {
                let resp = session.process_handshake(req);
                let accepted = resp.accepted;
                let response_msg = protocol::handshake_response(resp);
                let encoded = protocol::encode_response(&response_msg);
                send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
                conn.send_msg(&send_mr, encoded.len())?;

                if !accepted {
                    println!("  Session: handshake rejected (version mismatch)");
                    break;
                }
                println!("  Session: handshake accepted, ready for requests");
                continue;
            }
            Some(proto::request_message::Payload::Lookup(ref req)) => {
                if let Err(e) = session.validate_batch(req) {
                    println!("  Session: batch validation failed: {e}");
                    let error_resp = proto::BatchLookupResponse {
                        batch_id: req.batch_id,
                        results: vec![proto::EntryResult {
                            cache_key: 0,
                            success: false,
                            bytes_written: 0,
                            error_code: proto::ErrorCode::BatchTooLarge as i32,
                            error_message: e.to_string(),
                        }],
                    };
                    protocol::lookup_response(error_resp)
                } else {
                    // Process batch — stub dispatcher returns None for all keys
                    let resp = session.process_batch(req, |_key| {
                        // In production, this would call IDispatcher::lookup(key)
                        // For testing, return some dummy data for even keys
                        None
                    });
                    println!(
                        "  Session: processed batch {} ({} entries)",
                        req.batch_id,
                        req.entries.len()
                    );
                    protocol::lookup_response(resp)
                }
            }
            Some(proto::request_message::Payload::Close(ref req)) => {
                let resp = session.process_close(req);
                println!(
                    "  Session: close requested (reason: {}), batches_total={}",
                    req.reason, resp.batches_total
                );
                let response_msg = protocol::close_response(resp);
                let encoded = protocol::encode_response(&response_msg);
                send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
                conn.send_msg(&send_mr, encoded.len())?;
                break;
            }
            None => {
                bail!("received empty request message");
            }
        };

        let encoded = protocol::encode_response(&response);
        send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
        conn.send_msg(&send_mr, encoded.len())?;
    }

    println!("  Session: closed");
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!(
        "Remote Request Handler Server\n\
         ==============================\n\
         Listening on: {}:{}\n\
         Protocol version: {}\n\
         Max batch size: {}\n",
        args.addr, args.port, PROTOCOL_VERSION, MAX_BATCH_SIZE
    );

    let listener = RdmaListener::bind(&args.addr, args.port)?;
    println!("Server ready, waiting for connections...\n");

    loop {
        println!("Accepting connection...");
        match listener.accept() {
            Ok(conn) => {
                println!("Connection accepted!");
                if let Err(e) = handle_session(conn) {
                    eprintln!("Session error: {e}");
                }
            }
            Err(e) => {
                eprintln!("Accept error: {e}");
                break;
            }
        }
    }

    Ok(())
}
