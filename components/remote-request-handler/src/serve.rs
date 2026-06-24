//! Public server entry point for embedding the RDMA handler in other binaries.

use std::ffi::CStr;
use std::os::raw::c_int;
use std::sync::Arc;

use anyhow::Result;
use interfaces::ILogger;
use prost::Message;

use crate::ffi;
use crate::protocol::{self, proto};
use crate::rdma::{RdmaConnection, RdmaListener};
use crate::session::{Session, SessionConfig, MAX_BATCH_SIZE};

const PROTOCOL_VERSION: u32 = 1;
const MSG_BUF_SIZE: usize = 8192;

/// A resolved cache entry: pointer to data in the memory-tier pool and its size.
pub struct ResolvedEntry {
    pub ptr: *const u8,
    pub size: u32,
}

// SAFETY: The pointer references memory in the memory-tier pool which is
// a long-lived mmap'd region. The caller holds a read reference that
// prevents eviction while this entry is in use.
unsafe impl Send for ResolvedEntry {}
unsafe impl Sync for ResolvedEntry {}

/// Resolver function: given a CacheKey, returns pointer+size if found in memory-tier.
/// The resolver must hold a read reference on the key until the returned entry is consumed.
pub type Resolver = dyn Fn(u64) -> Option<ResolvedEntry> + Send + Sync;

/// Release callback: called after RDMA Write completes to release the read reference.
pub type ReleaseCallback = dyn Fn(u64) + Send + Sync;

/// Memory pool descriptor for pre-registration.
pub struct PoolRegion {
    pub base: *mut u8,
    pub size: usize,
}

// SAFETY: Pool is a long-lived mmap region valid for the process lifetime.
unsafe impl Send for PoolRegion {}
unsafe impl Sync for PoolRegion {}

fn handle_session(
    conn: &RdmaConnection,
    resolver: &Arc<Resolver>,
    release: &Arc<ReleaseCallback>,
    pool: &Option<Arc<PoolRegion>>,
    logger: &Arc<dyn ILogger + Send + Sync>,
) -> Result<()> {
    let session = Session::new(SessionConfig {
        protocol_version: PROTOCOL_VERSION,
        max_batch_size: MAX_BATCH_SIZE,
    });

    let mut recv_mr = conn.register_mr(MSG_BUF_SIZE)?;
    let mut send_mr = conn.register_mr(MSG_BUF_SIZE)?;

    // Pre-register the memory-tier pool as one large MR for the session lifetime.
    let pool_mr = if let Some(ref p) = pool {
        Some(conn.register_existing_mr(p.base, p.size)?)
    } else {
        None
    };

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
                    let results =
                        process_batch_with_rdma_write(conn, req, resolver, release, &pool_mr);
                    let resp = proto::BatchLookupResponse {
                        batch_id: req.batch_id,
                        results,
                    };
                    session.record_batch();
                    protocol::lookup_response(resp)
                }
            }
            Some(proto::request_message::Payload::Close(ref req)) => {
                let batches = session.batches_processed();
                let resp = proto::CloseResponse {
                    batches_total: batches,
                };
                logger.debug(&format!(
                    "remote-request-handler: close (reason={}, batches={})",
                    req.reason, batches
                ));
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

/// Two-phase batch processing: resolve all keys first, then post all RDMA Writes.
///
/// Phase 1 (Resolve): Acquire all dispatch-map read references and collect pointers.
///   This batches the lock contention up front so it doesn't interleave with RDMA posting.
///
/// Phase 2 (Write): Post all RDMA Writes in a tight loop — unsignaled except the last.
///   The NIC receives a continuous stream of work without stalls between entries.
///
/// Phase 3 (Release): Release all read references after writes complete.
fn process_batch_with_rdma_write(
    conn: &RdmaConnection,
    req: &proto::BatchLookupRequest,
    resolver: &Arc<Resolver>,
    release: &Arc<ReleaseCallback>,
    pool_mr: &Option<crate::rdma::MemoryRegion>,
) -> Vec<proto::EntryResult> {
    let n = req.entries.len();

    // --- Phase 1: Resolve all keys (collect pointers, hold read refs) ---
    struct Resolved {
        ptr: *const u8,
        #[allow(dead_code)]
        size: u32,
        write_len: usize,
        idx: usize,
    }
    // SAFETY: ptr comes from memory-tier pool (Send-safe, stable until release)
    unsafe impl Send for Resolved {}

    let mut resolved: Vec<Resolved> = Vec::with_capacity(n);
    let mut results: Vec<proto::EntryResult> = vec![
        proto::EntryResult {
            cache_key: 0,
            success: false,
            bytes_written: 0,
            error_code: proto::ErrorCode::KeyNotFound as i32,
            error_message: "key not found".into(),
        };
        n
    ];

    for (i, entry) in req.entries.iter().enumerate() {
        results[i].cache_key = entry.cache_key;
        if let Some(r) = resolver(entry.cache_key) {
            let write_len = (r.size).min(entry.max_size) as usize;
            resolved.push(Resolved {
                ptr: r.ptr,
                size: r.size,
                write_len,
                idx: i,
            });
        }
    }

    if resolved.is_empty() {
        return results;
    }

    // --- Phase 2: Post all RDMA Writes in a tight loop ---
    let last_resolved_idx = resolved.len() - 1;

    for (wi, r) in resolved.iter().enumerate() {
        let entry = &req.entries[r.idx];
        let is_last = wi == last_resolved_idx;

        let post_result = if let Some(ref pmr) = pool_mr {
            if is_last {
                conn.rdma_write_from_pool(pmr, r.ptr, r.write_len, entry.remote_addr, entry.rkey)
            } else {
                conn.post_rdma_write_unsignaled(
                    pmr,
                    r.ptr,
                    r.write_len,
                    entry.remote_addr,
                    entry.rkey,
                )
            }
        } else {
            (|| -> Result<()> {
                let local_mr = conn.register_existing_mr(r.ptr, r.write_len)?;
                conn.rdma_write(&local_mr, r.write_len, entry.remote_addr, entry.rkey)
            })()
        };

        match post_result {
            Ok(()) => {
                results[r.idx] = proto::EntryResult {
                    cache_key: entry.cache_key,
                    success: true,
                    bytes_written: r.write_len as u32,
                    error_code: proto::ErrorCode::Unspecified as i32,
                    error_message: String::new(),
                };
            }
            Err(e) => {
                results[r.idx] = proto::EntryResult {
                    cache_key: entry.cache_key,
                    success: false,
                    bytes_written: 0,
                    error_code: proto::ErrorCode::RdmaWriteFailed as i32,
                    error_message: e.to_string(),
                };
            }
        }
    }

    // --- Phase 3: Release all read references ---
    for r in &resolved {
        release(req.entries[r.idx].cache_key);
    }

    results
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
        4 => 10.0,
        8 => 10.0,
        16 => 14.0,
        32 => 25.0,
        64 => 50.0,
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

fn log_rdma_devices(logger: &dyn ILogger) {
    let mut num_devices: c_int = 0;
    // SAFETY: ibv_get_device_list is safe to call with a valid pointer.
    let dev_list = unsafe { ffi::ibv_get_device_list(&mut num_devices) };
    if dev_list.is_null() || num_devices == 0 {
        logger.warn("remote-request-handler: no RDMA devices found");
        return;
    }

    logger.info(&format!(
        "remote-request-handler: found {} RDMA device(s)",
        num_devices
    ));

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
            logger.warn(&format!("remote-request-handler: {}: failed to open", name));
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

            logger.info(&format!(
                "remote-request-handler:   {}: {} | {} | MTU {} | {}",
                name, state_str, link_str, mtu, speed
            ));
        } else {
            logger.warn(&format!(
                "remote-request-handler: {}: failed to query port",
                name
            ));
        }

        // SAFETY: ctx was opened successfully.
        unsafe { ffi::ibv_close_device(ctx) };
    }

    // SAFETY: dev_list was allocated by ibv_get_device_list.
    unsafe { ffi::ibv_free_device_list(dev_list) };
}

/// Run the RDMA handler server with data transfer via RDMA Write.
///
/// - `resolver`: returns a pointer+size to cached data for a given key (holds read ref)
/// - `release`: called after RDMA Write completes to release the read reference
/// - `pool`: memory-tier pool region for pre-registration (avoids per-entry MR reg/dereg)
/// - If `resolver` is None, all lookups return "not found" (standalone test mode)
pub fn run_blocking(
    addr: &str,
    port: u16,
    resolver: Option<Arc<Resolver>>,
    release: Option<Arc<ReleaseCallback>>,
    pool: Option<Arc<PoolRegion>>,
    logger: Arc<dyn ILogger + Send + Sync>,
) -> Result<()> {
    let resolver = resolver.unwrap_or_else(|| Arc::new(|_| None));
    let release: Arc<ReleaseCallback> = release.unwrap_or_else(|| Arc::new(|_| {}));

    log_rdma_devices(logger.as_ref());

    logger.info(&format!(
        "remote-request-handler: binding RDMA listener on {}:{}",
        addr, port
    ));
    let listener = RdmaListener::bind(addr, port)?;
    logger.info(&format!(
        "remote-request-handler: listener ready (protocol_version={}, max_batch_size={})",
        PROTOCOL_VERSION, MAX_BATCH_SIZE
    ));

    if pool.is_some() {
        logger.info(&format!(
            "remote-request-handler: memory-tier pool pre-registration enabled ({} MiB)",
            pool.as_ref().unwrap().size / (1024 * 1024)
        ));
    }

    loop {
        match listener.accept() {
            Ok(conn) => {
                logger.info("remote-request-handler: session accepted");
                match handle_session(&conn, &resolver, &release, &pool, &logger) {
                    Ok(()) => {
                        logger.info("remote-request-handler: session closed normally");
                    }
                    Err(e) => {
                        logger.warn(&format!("remote-request-handler: session error: {e}"));
                    }
                }
            }
            Err(e) => {
                logger.error(&format!("remote-request-handler: accept error: {e}"));
                break;
            }
        }
    }

    Ok(())
}
