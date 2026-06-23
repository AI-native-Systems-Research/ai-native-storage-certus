//! Public server entry point for embedding the RDMA handler in other binaries.

use std::sync::Arc;

use anyhow::Result;
use prost::Message;

use crate::protocol::{self, proto};
use crate::rdma::{RdmaConnection, RdmaListener};
use crate::session::{Session, SessionConfig, MAX_BATCH_SIZE};

const PROTOCOL_VERSION: u32 = 1;
const MSG_BUF_SIZE: usize = 8192;

/// Resolver function type: given a CacheKey, returns Some(data) if found, None otherwise.
pub type Resolver = dyn Fn(u64) -> Option<Vec<u8>> + Send + Sync;

fn handle_session(conn: RdmaConnection, resolver: &Arc<Resolver>) -> Result<()> {
    let session = Session::new(SessionConfig {
        protocol_version: PROTOCOL_VERSION,
        max_batch_size: MAX_BATCH_SIZE,
    });

    let mut recv_mr = conn.register_mr(MSG_BUF_SIZE)?;
    let mut send_mr = conn.register_mr(MSG_BUF_SIZE)?;

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
                    break;
                }
                continue;
            }
            Some(proto::request_message::Payload::Lookup(ref req)) => {
                if let Err(e) = session.validate_batch(req) {
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
                    let resp = session.process_batch(req, |key| resolver(key));
                    protocol::lookup_response(resp)
                }
            }
            Some(proto::request_message::Payload::Close(ref req)) => {
                let resp = session.process_close(req);
                let response_msg = protocol::close_response(resp);
                let encoded = protocol::encode_response(&response_msg);
                send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
                conn.send_msg(&send_mr, encoded.len())?;
                break;
            }
            None => {
                anyhow::bail!("received empty request message");
            }
        };

        let encoded = protocol::encode_response(&response);
        send_mr.buf[..encoded.len()].copy_from_slice(&encoded);
        conn.send_msg(&send_mr, encoded.len())?;
    }

    Ok(())
}

/// Run the RDMA handler server with an optional resolver for cache lookups.
/// Blocks indefinitely, accepting connections and processing sessions.
///
/// If `resolver` is None, all lookups return "not found" (standalone test mode).
/// If `resolver` is Some, each CacheKey is resolved via the provided function.
pub fn run_blocking(addr: &str, port: u16, resolver: Option<Arc<Resolver>>) -> Result<()> {
    let resolver = resolver.unwrap_or_else(|| Arc::new(|_| None));

    eprintln!(
        "[remote-request-handler] Binding RDMA listener on {}:{}...",
        addr, port
    );
    let listener = RdmaListener::bind(addr, port)?;
    eprintln!(
        "[remote-request-handler] Listener ready (protocol_version={}, max_batch_size={})",
        PROTOCOL_VERSION, MAX_BATCH_SIZE
    );

    loop {
        match listener.accept() {
            Ok(conn) => {
                eprintln!("[remote-request-handler] Session accepted");
                match handle_session(conn, &resolver) {
                    Ok(()) => {
                        eprintln!("[remote-request-handler] Session closed normally");
                    }
                    Err(e) => {
                        eprintln!("[remote-request-handler] Session error: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("[remote-request-handler] Accept error: {e}");
                break;
            }
        }
    }

    Ok(())
}
