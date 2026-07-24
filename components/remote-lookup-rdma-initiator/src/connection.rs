//! Outbound RDMA connection table and per-host connection state machine.
//!
//! The [`ConnectionTable`] keeps one entry per remote host, keyed by its
//! normalized `"ip:port"` endpoint. A host absent from the table is
//! *disconnected*; a present entry is *connecting*, *connected*, or
//! *disconnecting* (see `ConnState`). Connections are established lazily on
//! first [`push`](ConnectionTable::push), reused across calls, and repaired
//! automatically when a queue pair enters the error state.
//!
//! # Concurrency
//!
//! The outer `Mutex<HashMap<..>>` is held only briefly to look up or insert a
//! host slot. Each slot carries its own `Mutex<ConnState>`, so pushes to
//! *different* hosts proceed concurrently, while pushes to the *same* host
//! serialize on that slot's lock (required: an [`RdmaConn`]'s underlying queue
//! pair is `Send` but not `Sync`). Because establishing a RoCE/CM connection
//! takes seconds, a same-host caller blocks on the slot lock until the in-flight
//! connect completes and then reuses the resulting connection. The transient
//! `Connecting`/`Disconnecting` states are set by the thread holding the slot
//! lock as it drives the transition.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use interfaces::{ILogger, PushStatus, RemoteLookupRdmaInitiatorError};

#[cfg(feature = "rdma")]
use crate::rdma;
use crate::telemetry::TelemetryCollector;

/// Errors from RDMA operations, surfaced across the transport seam.
///
/// Defined here (not in the feature-gated `rdma` module) because the seam traits
/// and the mock transport reference it even when the real rdma-core path is
/// compiled out.
#[derive(Debug, Clone)]
pub enum RdmaError {
    /// A connection could not be established.
    ConnectionFailed(String),
    /// A memory-region registration or buffer allocation failed.
    AllocationFailed(String),
    /// An RDMA write failed.
    WriteFailed(String),
    /// A send/recv operation failed.
    SendRecvFailed(String),
    /// A resource limit was hit (queue depth, CQ entries, …).
    ResourceExhausted(String),
    /// A CM/verbs event error.
    EventError(String),
}

impl fmt::Display for RdmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "RDMA connection failed: {msg}"),
            Self::AllocationFailed(msg) => write!(f, "RDMA allocation failed: {msg}"),
            Self::WriteFailed(msg) => write!(f, "RDMA write failed: {msg}"),
            Self::SendRecvFailed(msg) => write!(f, "RDMA send/recv failed: {msg}"),
            Self::ResourceExhausted(msg) => write!(f, "RDMA resource exhausted: {msg}"),
            Self::EventError(msg) => write!(f, "RDMA event error: {msg}"),
        }
    }
}

impl std::error::Error for RdmaError {}

/// Per-phase breakdown of one connect, in microseconds, for telemetry
/// attribution of cold-connect latency. All zero when the transport does not
/// measure (e.g. the mock).
#[derive(Debug, Clone, Copy, Default)]
pub struct ConnectTiming {
    /// `rdma_cm` address resolution.
    pub resolve_addr_us: u64,
    /// `rdma_cm` route resolution.
    pub resolve_route_us: u64,
    /// Connect handshake through `RDMA_CM_EVENT_ESTABLISHED`.
    pub handshake_us: u64,
    /// Pool memory-region registration (`ibv_reg_mr`).
    pub mr_reg_us: u64,
}

impl ConnectTiming {
    /// True if any phase was measured (i.e. a real, timed connect).
    fn is_measured(&self) -> bool {
        self.resolve_addr_us | self.resolve_route_us | self.handshake_us | self.mr_reg_us != 0
    }
}

/// Establishes outbound RDMA connections. A seam so the connection table can be
/// unit-tested without RDMA hardware.
pub trait RdmaTransport: Send + Sync {
    /// Connect to `addr:port` and return a ready-to-use connection plus the
    /// per-phase timing of establishing it.
    fn connect(
        &self,
        addr: &str,
        port: u16,
    ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError>;
}

/// A live outbound connection capable of RDMA-writing from the local
/// memory-tier pool into a remote region.
pub trait RdmaConn: Send {
    /// Returns `false` if the queue pair has entered the error state and the
    /// connection must be rebuilt.
    fn qp_healthy(&self) -> bool;

    /// RDMA-write `len` bytes starting at local pool address `local` into the
    /// remote region (`remote_addr`, `rkey`). Blocks until completion.
    ///
    /// # Safety
    ///
    /// `local` must point to `len` valid bytes inside the registered
    /// memory-tier pool region that backs this connection.
    unsafe fn write(
        &self,
        local: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: u32,
    ) -> Result<(), RdmaError>;
}

/// Per-host connection state. Absence of a slot in the table means the host is
/// disconnected.
enum ConnState {
    /// No live connection (initial state, or after a failed connect / teardown).
    Disconnected,
    /// A connection attempt is in progress (held only by the connecting thread).
    Connecting,
    /// A healthy, established connection ready for writes.
    Connected(Box<dyn RdmaConn>),
    /// A teardown is in progress (held only by the disconnecting thread).
    Disconnecting,
}

/// One remote host's connection slot.
struct HostSlot {
    state: Mutex<ConnState>,
}

/// A per-item action decided by the caller after the local memory-tier lookup.
pub enum ItemPlan {
    /// A terminal status decided before any RDMA write is attempted
    /// ([`PushStatus::KeyNotFound`] or [`PushStatus::SizeMismatch`]).
    Done(PushStatus),
    /// A write to perform once the host is connected.
    Write {
        /// Local pool address of the value (from `IMemoryTier::peek`).
        local: *const u8,
        /// Number of bytes to write.
        len: usize,
        /// Remote destination address.
        remote_addr: u64,
        /// Remote key authorizing the write.
        rkey: u32,
    },
}

/// A table of outbound RDMA connections keyed by `"ip:port"` endpoint.
pub struct ConnectionTable {
    transport: Box<dyn RdmaTransport>,
    slots: Mutex<HashMap<String, Arc<HostSlot>>>,
    telemetry: Arc<TelemetryCollector>,
}

impl ConnectionTable {
    /// Create a table that establishes connections via `transport` and records
    /// metrics into `telemetry` (a no-op unless the `telemetry` feature is on).
    pub fn new(transport: Box<dyn RdmaTransport>, telemetry: Arc<TelemetryCollector>) -> Self {
        Self {
            transport,
            slots: Mutex::new(HashMap::new()),
            telemetry,
        }
    }

    /// Ensure a connection to `endpoint`, then carry out each planned write.
    ///
    /// Returns one [`PushStatus`] per item in `resolved`, in order. If the host
    /// cannot be connected, every [`ItemPlan::Write`] item is reported as
    /// [`PushStatus::UnableToConnect`] (already-decided [`ItemPlan::Done`] items
    /// keep their status).
    pub fn push(
        &self,
        endpoint: &str,
        resolved: Vec<ItemPlan>,
        logger: &dyn ILogger,
    ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError> {
        let (addr, port) = parse_endpoint(endpoint)?;
        let key = format!("{addr}:{port}");
        let start = Instant::now();

        let slot = {
            let mut slots = self.slots.lock().unwrap();
            Arc::clone(slots.entry(key).or_insert_with(|| {
                Arc::new(HostSlot {
                    state: Mutex::new(ConnState::Disconnected),
                })
            }))
        };

        // Serialize all operations to this host on its slot lock.
        let mut state = slot.state.lock().unwrap();

        let mut out = Vec::with_capacity(resolved.len());
        // Once the connection is deemed unrecoverable for this batch, remaining
        // writes short-circuit to UnableToConnect.
        let mut give_up = false;
        // A single reconnect is allowed per batch to recover from a QP error.
        let mut reconnect_used = false;

        for (idx, item) in resolved.iter().enumerate() {
            let (status, bytes) = match item {
                ItemPlan::Done(status) => (*status, 0),
                ItemPlan::Write {
                    local,
                    len,
                    remote_addr,
                    rkey,
                } if !give_up => {
                    // Per-write trace (addr/rkey/len): useful when correlating a
                    // failing write against the preceding successful ones. Debug
                    // level — the hot path must not log at info.
                    logger.debug(&format!(
                        "remote-lookup-rdma-initiator: write #{idx} -> {addr}:{port} \
                         remote_addr=0x{remote_addr:x} rkey=0x{rkey:x} len={len}"
                    ));
                    // Attempt the write, reconnecting at most once on failure.
                    let status = loop {
                        if !ensure_connected(
                            &mut state,
                            self.transport.as_ref(),
                            &addr,
                            port,
                            logger,
                            &self.telemetry,
                        ) {
                            give_up = true;
                            break PushStatus::UnableToConnect;
                        }

                        // SAFETY: `local`/`len` came from the caller's memory-tier
                        // peek, which returns a pointer and size within the
                        // registered pool that backs this connection.
                        let w_start = Instant::now();
                        let write_res = match &*state {
                            ConnState::Connected(conn) => unsafe {
                                conn.write(*local, *len, *remote_addr, *rkey)
                            },
                            // ensure_connected returned true, so we are Connected.
                            _ => unreachable!("ensure_connected guarantees Connected"),
                        };
                        // Per-write wall-clock (post_send + poll). A 4 MiB write is
                        // ~hundreds of µs on 200G RoCE; a slow/stale connection
                        // shows up here. Debug level — hot path.
                        let w_us = w_start.elapsed().as_micros();

                        match write_res {
                            Ok(()) => {
                                logger.debug(&format!(
                                    "remote-lookup-rdma-initiator: write #{idx} OK in {w_us}us \
                                     ({len} bytes)"
                                ));
                                break PushStatus::Success;
                            }
                            Err(e) => {
                                // Drop the (likely error-state) connection so the
                                // retry rebuilds it.
                                *state = ConnState::Disconnected;
                                if reconnect_used {
                                    logger.warn(&format!(
                                        "remote-lookup-rdma-initiator: write to {addr}:{port} \
                                         (remote_addr=0x{remote_addr:x} len={len}) failed \
                                         after reconnect: {e}"
                                    ));
                                    give_up = true;
                                    break PushStatus::UnableToConnect;
                                }
                                reconnect_used = true;
                                self.telemetry.record_reconnect();
                                logger.warn(&format!(
                                    "remote-lookup-rdma-initiator: write to {addr}:{port} \
                                     (remote_addr=0x{remote_addr:x} len={len}) failed ({e}); \
                                     reconnecting"
                                ));
                            }
                        }
                    };
                    let bytes = if status == PushStatus::Success {
                        *len as u64
                    } else {
                        0
                    };
                    (status, bytes)
                }
                // give_up already set: the host is unreachable for this batch.
                ItemPlan::Write { .. } => (PushStatus::UnableToConnect, 0),
            };
            self.telemetry.record_item(status, bytes);
            out.push(status);
        }

        self.telemetry
            .record_push(start.elapsed().as_micros() as u64);
        Ok(out)
    }

    /// Proactively establish (warm) the connection to `endpoint` without
    /// writing. Reuses a healthy existing connection; otherwise runs the full
    /// connect (the same path [`push`](Self::push) takes lazily). A failed
    /// connect leaves the slot disconnected (nothing cached) and still returns
    /// `Ok(())` — warming must not surface a transient failure as an error.
    /// Errors only on an unparseable endpoint.
    pub fn connect(
        &self,
        endpoint: &str,
        logger: &dyn ILogger,
    ) -> Result<(), RemoteLookupRdmaInitiatorError> {
        let (addr, port) = parse_endpoint(endpoint)?;
        let key = format!("{addr}:{port}");

        let slot = {
            let mut slots = self.slots.lock().unwrap();
            Arc::clone(slots.entry(key).or_insert_with(|| {
                Arc::new(HostSlot {
                    state: Mutex::new(ConnState::Disconnected),
                })
            }))
        };

        let mut state = slot.state.lock().unwrap();
        // Best-effort: ensure_connected records telemetry and leaves the slot
        // Disconnected on failure, so a later connect/push retries.
        let _ = ensure_connected(
            &mut state,
            self.transport.as_ref(),
            &addr,
            port,
            logger,
            &self.telemetry,
        );
        Ok(())
    }

    /// Tear down the connection to a single host, if present. Idempotent.
    pub fn disconnect(&self, endpoint: &str) {
        let (addr, port) = match parse_endpoint(endpoint) {
            Ok(hp) => hp,
            // Nothing could have been stored under an unparseable endpoint.
            Err(_) => return,
        };
        let key = format!("{addr}:{port}");
        let slot = self.slots.lock().unwrap().remove(&key);
        if let Some(slot) = slot {
            let mut state = slot.state.lock().unwrap();
            if matches!(*state, ConnState::Connected(_)) {
                self.telemetry.record_disconnect();
            }
            *state = ConnState::Disconnecting;
            // Dropping the connection (via replacing the state) runs the RDMA
            // teardown in RdmaConnection::drop.
            *state = ConnState::Disconnected;
        }
    }

    /// Tear down all connections.
    pub fn disconnect_all(&self) {
        let drained: Vec<Arc<HostSlot>> = {
            let mut slots = self.slots.lock().unwrap();
            slots.drain().map(|(_, slot)| slot).collect()
        };
        for slot in drained {
            let mut state = slot.state.lock().unwrap();
            if matches!(*state, ConnState::Connected(_)) {
                self.telemetry.record_disconnect();
            }
            *state = ConnState::Disconnecting;
            *state = ConnState::Disconnected;
        }
    }
}

/// Ensure `state` holds a healthy `Connected` connection, connecting if needed.
///
/// Returns `true` if the state is `Connected` on return, `false` if the connect
/// attempt failed (state left `Disconnected`).
fn ensure_connected(
    state: &mut ConnState,
    transport: &dyn RdmaTransport,
    addr: &str,
    port: u16,
    logger: &dyn ILogger,
    telemetry: &TelemetryCollector,
) -> bool {
    if matches!(state, ConnState::Connected(conn) if conn.qp_healthy()) {
        return true;
    }

    // Drop any stale/error-state connection before reconnecting.
    *state = ConnState::Connecting;
    match transport.connect(addr, port) {
        Ok((conn, timing)) => {
            *state = ConnState::Connected(conn);
            telemetry.record_connection_established();
            if timing.is_measured() {
                telemetry.record_connect_phases(
                    timing.resolve_addr_us,
                    timing.resolve_route_us,
                    timing.handshake_us,
                    timing.mr_reg_us,
                );
                logger.info(&format!(
                    "remote-lookup-rdma-initiator: connected {addr}:{port} in \
                     resolve_addr={}us route={}us handshake={}us mr_reg={}us",
                    timing.resolve_addr_us,
                    timing.resolve_route_us,
                    timing.handshake_us,
                    timing.mr_reg_us,
                ));
            }
            true
        }
        Err(e) => {
            logger.warn(&format!(
                "remote-lookup-rdma-initiator: connect to {addr}:{port} failed: {e}"
            ));
            telemetry.record_connection_failed();
            *state = ConnState::Disconnected;
            false
        }
    }
}

/// Parse an `"ip:port"` endpoint into its host and port parts.
pub fn parse_endpoint(endpoint: &str) -> Result<(String, u16), RemoteLookupRdmaInitiatorError> {
    let invalid = || {
        RemoteLookupRdmaInitiatorError::InvalidEndpoint(format!(
            "expected \"ip:port\", got {endpoint:?}"
        ))
    };
    let (host, port) = endpoint.rsplit_once(':').ok_or_else(invalid)?;
    if host.is_empty() {
        return Err(invalid());
    }
    let port: u16 = port.parse().map_err(|_| invalid())?;
    if port == 0 {
        return Err(invalid());
    }
    Ok((host.to_string(), port))
}

// --- Real RDMA transport (links rdma-core via crate::rdma; `rdma` feature) ---

/// Production transport: connects via `rdma_cm` and registers the memory-tier
/// pool as an RDMA memory region on each new connection.
#[cfg(feature = "rdma")]
pub struct RealTransport {
    /// Memory-tier pool base address, stored as `usize` so the transport is
    /// `Send + Sync` (the pointer is only ever handed back to `ibv_reg_mr`).
    pool_base: usize,
    pool_size: usize,
    /// This node's zyre PeerId bytes, stamped into every connect `private_data`
    /// so the remote responder can correlate the QP. Empty ⇒ unstamped.
    local_peer_id: Vec<u8>,
}

#[cfg(feature = "rdma")]
impl RealTransport {
    /// Create a transport that registers the pool at `pool_base`/`pool_size` and
    /// stamps `local_peer_id` into each connection's `private_data`.
    pub fn new(pool_base: *mut u8, pool_size: usize, local_peer_id: Vec<u8>) -> Self {
        Self {
            pool_base: pool_base as usize,
            pool_size,
            local_peer_id,
        }
    }
}

#[cfg(feature = "rdma")]
impl RdmaTransport for RealTransport {
    fn connect(
        &self,
        addr: &str,
        port: u16,
    ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
        let (conn, cm) = rdma::client_connect(addr, port, &self.local_peer_id)
            .map_err(|e| RdmaError::ConnectionFailed(format!("{e:#}")))?;
        let mr_start = Instant::now();
        let pool_mr = conn
            .register_existing_mr(self.pool_base as *const u8, self.pool_size)
            .map_err(|e| RdmaError::AllocationFailed(format!("{e:#}")))?;
        let timing = ConnectTiming {
            resolve_addr_us: cm.resolve_addr_us,
            resolve_route_us: cm.resolve_route_us,
            handshake_us: cm.handshake_us,
            mr_reg_us: mr_start.elapsed().as_micros() as u64,
        };
        Ok((Box::new(RealConn { conn, pool_mr }), timing))
    }
}

/// A live `rdma_cm` connection plus its registered memory-tier pool region.
#[cfg(feature = "rdma")]
struct RealConn {
    conn: rdma::RdmaConnection,
    pool_mr: rdma::MemoryRegion,
}

#[cfg(feature = "rdma")]
impl RdmaConn for RealConn {
    fn qp_healthy(&self) -> bool {
        self.conn.is_qp_healthy()
    }

    unsafe fn write(
        &self,
        local: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: u32,
    ) -> Result<(), RdmaError> {
        self.conn
            .rdma_write_from_pool(&self.pool_mr, local, len, remote_addr, rkey)
            // `{e:#}` renders the full anyhow source chain (e.g. the underlying
            // "work completion error: status=12 (RETRY_EXC_ERR) … qp_state=…"),
            // not just the top context — otherwise the WC status is lost.
            .map_err(|e| RdmaError::WriteFailed(format!("{e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A logger that discards everything (tests don't assert on log output).
    struct NullLogger;
    impl ILogger for NullLogger {
        fn info(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn error(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    struct MockConn {
        healthy: AtomicBool,
        /// If set, the next write fails and drives the QP unhealthy.
        fail_next_write: AtomicBool,
    }

    impl RdmaConn for MockConn {
        fn qp_healthy(&self) -> bool {
            self.healthy.load(Ordering::SeqCst)
        }

        unsafe fn write(
            &self,
            _local: *const u8,
            _len: usize,
            _remote_addr: u64,
            _rkey: u32,
        ) -> Result<(), RdmaError> {
            if self.fail_next_write.swap(false, Ordering::SeqCst) {
                self.healthy.store(false, Ordering::SeqCst);
                return Err(RdmaError::WriteFailed("mock write failure".into()));
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockTransport {
        /// Shared so tests can assert how many connects happened.
        connect_attempts: Arc<AtomicUsize>,
        /// Number of initial connect attempts that should fail.
        fail_first_n_connects: usize,
        /// The first successfully-connected conn will fail its first write.
        fail_first_write: AtomicBool,
    }

    impl RdmaTransport for MockTransport {
        fn connect(
            &self,
            _addr: &str,
            _port: u16,
        ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
            let n = self.connect_attempts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first_n_connects {
                return Err(RdmaError::ConnectionFailed("mock connect failure".into()));
            }
            let conn = MockConn {
                healthy: AtomicBool::new(true),
                fail_next_write: AtomicBool::new(
                    self.fail_first_write.swap(false, Ordering::SeqCst),
                ),
            };
            // The mock does not measure phases; timing is left zeroed.
            Ok((Box::new(conn), ConnectTiming::default()))
        }
    }

    fn write_plan(remote_addr: u64, rkey: u32, len: usize) -> ItemPlan {
        ItemPlan::Write {
            local: remote_addr as *const u8, // arbitrary; mock ignores it
            len,
            remote_addr,
            rkey,
        }
    }

    /// Build a table with a fresh (no-op unless `telemetry` feature) collector.
    fn table_with(transport: MockTransport) -> ConnectionTable {
        ConnectionTable::new(Box::new(transport), Arc::new(TelemetryCollector::new()))
    }

    #[test]
    fn parse_endpoint_valid() {
        assert_eq!(
            parse_endpoint("192.168.1.10:9090").unwrap(),
            ("192.168.1.10".to_string(), 9090)
        );
    }

    #[test]
    fn parse_endpoint_rejects_bad_input() {
        for bad in ["no-port", "host:0", "host:99999", ":9090", "host:abc"] {
            assert!(
                parse_endpoint(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn statuses_mapped_in_order() {
        let table = table_with(MockTransport::default());
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x1000, 7, 256),
            ItemPlan::Done(PushStatus::SizeMismatch),
        ];
        let out = table.push("10.0.0.1:5000", resolved, &NullLogger).unwrap();
        assert_eq!(
            out,
            vec![
                PushStatus::KeyNotFound,
                PushStatus::Success,
                PushStatus::SizeMismatch,
            ]
        );
    }

    #[test]
    fn connect_failure_yields_unable_to_connect_for_writes_only() {
        let transport = MockTransport {
            fail_first_n_connects: usize::MAX, // always fail
            ..Default::default()
        };
        let table = table_with(transport);
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x2000, 1, 64),
            write_plan(0x3000, 2, 64),
        ];
        let out = table.push("10.0.0.2:5000", resolved, &NullLogger).unwrap();
        assert_eq!(
            out,
            vec![
                PushStatus::KeyNotFound,
                PushStatus::UnableToConnect,
                PushStatus::UnableToConnect,
            ]
        );
    }

    #[test]
    fn write_failure_triggers_single_reconnect_then_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            fail_first_write: AtomicBool::new(true),
            ..Default::default()
        };
        let table = table_with(transport);
        // Two writes: the first fails (QP goes unhealthy), forcing a reconnect;
        // the retry and the second write both succeed.
        let resolved = vec![write_plan(0xA000, 3, 128), write_plan(0xB000, 4, 128)];
        let out = table.push("10.0.0.3:5000", resolved, &NullLogger).unwrap();
        assert_eq!(out, vec![PushStatus::Success, PushStatus::Success]);
        // Exactly one reconnect: the initial connect plus one repair.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn disconnect_forces_fresh_connection() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);

        let out1 = table
            .push("10.0.0.4:5000", vec![write_plan(1, 1, 8)], &NullLogger)
            .unwrap();
        assert_eq!(out1, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        table.disconnect("10.0.0.4:5000");

        let out2 = table
            .push("10.0.0.4:5000", vec![write_plan(2, 2, 8)], &NullLogger)
            .unwrap();
        assert_eq!(out2, vec![PushStatus::Success]);
        // Disconnect dropped the slot, so a second connect was required.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reused_connection_does_not_reconnect() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);
        // First push connects; second push to the same endpoint reuses it.
        table
            .push("10.0.0.5:5000", vec![write_plan(1, 1, 8)], &NullLogger)
            .unwrap();
        let out = table
            .push("10.0.0.5:5000", vec![write_plan(2, 2, 8)], &NullLogger)
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        // A single connect served both pushes.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_endpoint_is_method_error() {
        let table = table_with(MockTransport::default());
        let err = table.push("garbage", vec![], &NullLogger).unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::InvalidEndpoint(_)
        ));
    }

    #[test]
    fn warm_connect_establishes_and_push_reuses() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            ..Default::default()
        };
        let table = table_with(transport);
        // Warm establishes the connection up front...
        table.connect("10.0.0.5:5000", &NullLogger).unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        // ...so a subsequent push to the same endpoint reuses it (no reconnect).
        let out = table
            .push("10.0.0.5:5000", vec![write_plan(1, 1, 8)], &NullLogger)
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn warm_connect_failure_is_ok_and_caches_nothing() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let transport = MockTransport {
            connect_attempts: Arc::clone(&attempts),
            fail_first_n_connects: 1,
            ..Default::default()
        };
        let table = table_with(transport);
        // A failed warm still returns Ok (never surfaces a transient failure)...
        table.connect("10.0.0.7:5000", &NullLogger).unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        // ...and caches nothing, so the next push retries the connect and wins.
        let out = table
            .push("10.0.0.7:5000", vec![write_plan(1, 1, 8)], &NullLogger)
            .unwrap();
        assert_eq!(out, vec![PushStatus::Success]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn warm_connect_invalid_endpoint_is_method_error() {
        let table = table_with(MockTransport::default());
        let err = table.connect("garbage", &NullLogger).unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::InvalidEndpoint(_)
        ));
    }

    // --- Telemetry wiring (only meaningful with the `telemetry` feature) ---

    #[cfg(feature = "telemetry")]
    fn table_with_telemetry(
        transport: MockTransport,
    ) -> (ConnectionTable, Arc<TelemetryCollector>) {
        let telemetry = Arc::new(TelemetryCollector::new());
        let table = ConnectionTable::new(Box::new(transport), Arc::clone(&telemetry));
        (table, telemetry)
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_push_outcomes() {
        let (table, tm) = table_with_telemetry(MockTransport::default());
        let resolved = vec![
            ItemPlan::Done(PushStatus::KeyNotFound),
            write_plan(0x1000, 7, 256),
            ItemPlan::Done(PushStatus::SizeMismatch),
        ];
        table.push("10.0.0.9:5000", resolved, &NullLogger).unwrap();
        assert_eq!(tm.items_success(), 1);
        assert_eq!(tm.items_key_not_found(), 1);
        assert_eq!(tm.items_size_mismatch(), 1);
        assert_eq!(tm.bytes_written(), 256);
        assert_eq!(tm.connections_established(), 1);
        assert_eq!(tm.pushes(), 1);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_reconnect() {
        let transport = MockTransport {
            fail_first_write: AtomicBool::new(true),
            ..Default::default()
        };
        let (table, tm) = table_with_telemetry(transport);
        table
            .push("10.0.0.10:5000", vec![write_plan(1, 1, 64)], &NullLogger)
            .unwrap();
        assert_eq!(tm.reconnects(), 1);
        // Initial connect + one repair.
        assert_eq!(tm.connections_established(), 2);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_connection_failure() {
        let transport = MockTransport {
            fail_first_n_connects: usize::MAX,
            ..Default::default()
        };
        let (table, tm) = table_with_telemetry(transport);
        table
            .push("10.0.0.11:5000", vec![write_plan(1, 1, 64)], &NullLogger)
            .unwrap();
        assert!(tm.connection_failures() >= 1);
        assert_eq!(tm.items_unable_to_connect(), 1);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_records_disconnect() {
        let (table, tm) = table_with_telemetry(MockTransport::default());
        table
            .push("10.0.0.12:5000", vec![write_plan(1, 1, 8)], &NullLogger)
            .unwrap();
        table.disconnect("10.0.0.12:5000");
        assert_eq!(tm.disconnects(), 1);
    }
}
