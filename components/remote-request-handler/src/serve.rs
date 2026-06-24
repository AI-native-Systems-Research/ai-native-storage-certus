//! Public server entry point for embedding the RDMA handler in other binaries.

use std::ffi::CStr;
use std::os::raw::c_int;
use std::sync::Arc;

use anyhow::Result;
use prost::Message;

use crate::ffi;
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

fn mtu_to_bytes(mtu: u32) -> u32 {
    match mtu {
        1 => 256,
        2 => 512,
        3 => 1024,
        4 => 2048,
        5 => 4096,
        _ => 0,
    }
}

fn speed_to_string(speed: u8, width: u8) -> String {
    let lane_gbps = match speed {
        1 => 2.5,
        2 => 5.0,
        4 => 10.0,  // FDR10
        8 => 10.0,  // EDR (10 Gbps per lane)
        16 => 14.0, // HDR (14 Gbps per lane after encoding)
        32 => 25.0, // NDR
        64 => 50.0, // XDR
        _ => 0.0,
    };
    let lanes = match width {
        1 => 1,
        2 => 4,
        4 => 8,
        8 => 12,
        16 => 2,
        _ => 1,
    };
    let total_gbps = lane_gbps * lanes as f64;
    format!("{:.0} Gb/s ({} x {:.1} Gb/s)", total_gbps, lanes, lane_gbps)
}

fn log_rdma_devices() {
    // SAFETY: ibv_get_device_list is safe to call with a valid pointer.
    let mut num_devices: c_int = 0;
    let dev_list = unsafe { ffi::ibv_get_device_list(&mut num_devices) };
    if dev_list.is_null() || num_devices == 0 {
        eprintln!("[remote-request-handler] No RDMA devices found");
        return;
    }

    eprintln!(
        "[remote-request-handler] Found {} RDMA device(s):",
        num_devices
    );

    for i in 0..num_devices as isize {
        // SAFETY: dev_list is valid and contains num_devices entries.
        let dev = unsafe { *dev_list.offset(i) };
        if dev.is_null() {
            continue;
        }

        // SAFETY: dev is a valid device pointer.
        let name_ptr = unsafe { ffi::ibv_get_device_name(dev) };
        let name = if name_ptr.is_null() {
            "unknown".to_string()
        } else {
            // SAFETY: name_ptr is a valid C string from ibverbs.
            unsafe { CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .into_owned()
        };

        // SAFETY: dev is valid.
        let ctx = unsafe { ffi::ibv_open_device(dev) };
        if ctx.is_null() {
            eprintln!("  {}: (failed to open)", name);
            continue;
        }

        let mut port_attr = ffi::ibv_port_attr {
            state: 0,
            max_mtu: 0,
            active_mtu: 0,
            gid_tbl_len: 0,
            port_cap_flags: 0,
            max_msg_sz: 0,
            bad_pkey_cntr: 0,
            qkey_viol_cntr: 0,
            pkey_tbl_len: 0,
            lid: 0,
            sm_lid: 0,
            lmc: 0,
            max_vl_num: 0,
            sm_sl: 0,
            subnet_timeout: 0,
            init_type_reply: 0,
            active_width: 0,
            active_speed: 0,
            phys_state: 0,
            link_layer: 0,
            flags: 0,
            port_cap_flags2: 0,
        };

        // SAFETY: ctx is valid, port 1 is the default port.
        let ret = unsafe { ffi::ibv_query_port(ctx, 1, &mut port_attr) };
        if ret == 0 {
            let state_str = if port_attr.state == 4 { "ACTIVE" } else { "DOWN" };
            let link_str = match port_attr.link_layer {
                ffi::IBV_LINK_LAYER_INFINIBAND => "InfiniBand",
                ffi::IBV_LINK_LAYER_ETHERNET => "RoCE/Ethernet",
                _ => "Unknown",
            };
            let mtu = mtu_to_bytes(port_attr.active_mtu);
            let speed = speed_to_string(port_attr.active_speed, port_attr.active_width);

            eprintln!(
                "  {}: {} | {} | MTU {} | {}",
                name, state_str, link_str, mtu, speed
            );
        } else {
            eprintln!("  {}: (failed to query port)", name);
        }

        // SAFETY: ctx was opened successfully.
        unsafe { ffi::ibv_close_device(ctx) };
    }

    // SAFETY: dev_list was allocated by ibv_get_device_list.
    unsafe { ffi::ibv_free_device_list(dev_list) };
}

/// Run the RDMA handler server with an optional resolver for cache lookups.
/// Blocks indefinitely, accepting connections and processing sessions.
///
/// If `resolver` is None, all lookups return "not found" (standalone test mode).
/// If `resolver` is Some, each CacheKey is resolved via the provided function.
pub fn run_blocking(addr: &str, port: u16, resolver: Option<Arc<Resolver>>) -> Result<()> {
    let resolver = resolver.unwrap_or_else(|| Arc::new(|_| None));

    log_rdma_devices();

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
