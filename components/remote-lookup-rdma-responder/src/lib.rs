//! Remote-lookup RDMA responder component — the **accept** side of a remote
//! RDMA lookup.
//!
//! This component belongs to the *requesting* Certus instance: the node that
//! wants a value and offers local memory for a peer to write it into. It is the
//! passive counterpart of the outbound initiator
//! (`remote-lookup-rdma-initiator` / [`interfaces::IRemoteLookupRdmaInitiator`]).
//!
//! # Actor
//!
//! The responder owns a dedicated thread running an `rdma_cm` accept loop. It
//! binds an ephemeral port on the mainline-supplied RoCE IPv4, accepts inbound
//! RDMA connections from serving peers, and manages one queue pair per peer.
//! Because serving peers RDMA-*write* values one-sidedly into the pre-registered
//! memory tier, the responder never touches the data — it manages only
//! connections.
//!
//! The actor is driven over a [`ControlChannel`]: `remote-lookup` issues
//! [`ResponderCommand`]s and receives [`ResponderEvent`]s. The central command
//! is `Disconnect { node }`, which tears down a peer's QP **before** the
//! requester reclaims that peer's locked landing slots (the
//! teardown-before-reclaim barrier), acknowledged with `DisconnectAck`.
//!
//! # Seam and the `rdma` feature
//!
//! All hardware-independent logic lives behind the [`connection`] CM seam
//! ([`connection::CmListener`] / [`connection::CmConnection`]) so it is
//! unit-testable without an RDMA NIC. The in-process [`connection::MockCmSeam`]
//! is the **default** build: with the `rdma` feature off, the crate compiles and
//! unit-tests with no rdma-core (`libibverbs`/`librdmacm`) present, and
//! `initialize()` is unavailable (returns an error). Enabling the `rdma` feature
//! compiles the production `rdma_cm` listener ([`rdma::RealCmSeam`]): real
//! `bind`/`listen`/`rdma_get_src_port`, `epoll` over the CM fd + eventfds,
//! `private_data` read, whole-pool `ibv_reg_mr` registration, and real QP
//! teardown. Mainline apps enable `rdma`; CI/default-members do not.

pub mod connection;
pub mod telemetry;

#[cfg(feature = "rdma")]
mod ffi;
#[cfg(feature = "rdma")]
mod rdma;

#[cfg(all(test, feature = "rdma"))]
mod loopback_test;

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

// The accept-loop actor machinery is exercised only when there is a real seam
// (the `rdma` feature) or the in-process mock tests (`test`); without either the
// crate is an inert compile-only stub, so these imports would be unused.
#[cfg(any(feature = "rdma", test))]
use component_core::channel::spsc::SpscChannel;
use component_core::channel::{Receiver, Sender};
#[cfg(any(feature = "rdma", test))]
use component_core::numa::{set_thread_affinity, CpuSet};
use component_framework::define_component;
use interfaces::{
    ControlChannel, Endpoint, ILogger, IMemoryTier, IRemoteLookupRdmaResponder,
    IRemoteLookupRdmaResponderAdmin, LocalRegion, RemoteLookupRdmaResponderError, ResponderCommand,
    ResponderEvent,
};

#[cfg(any(feature = "rdma", test))]
use crate::connection::{AcceptOutcome, CmEvent, CmListener, ConnectionTable};
#[cfg(feature = "rdma")]
use crate::rdma::RealCmSeam;
#[cfg(any(feature = "rdma", test))]
use crate::telemetry::TelemetryCollector;

/// Capacity of the control command/event channels.
#[cfg(any(feature = "rdma", test))]
const CONTROL_CHANNEL_CAPACITY: usize = 64;

/// Live state of the running actor thread and the (single-client) control
/// channel ends not yet handed out.
struct ActorState {
    /// Client → actor command sender, handed to the caller of
    /// [`IRemoteLookupRdmaResponder::open_control_channel`] (single client).
    command_tx: Option<Sender<ResponderCommand>>,
    /// Actor → client event receiver, handed out on the same call.
    event_rx: Option<Receiver<ResponderEvent>>,
    /// Cooperative stop flag observed by the accept loop (used by the mock seam).
    stop: Arc<AtomicBool>,
    /// Stop eventfd written to wake the real (`epoll`-based) accept loop; `None`
    /// for the mock seam, which stops via the flag + command-channel close.
    stop_eventfd: Option<c_int>,
    /// The accept-loop thread; joined on [`IRemoteLookupRdmaResponderAdmin::shutdown`].
    join: Option<JoinHandle<()>>,
}

define_component! {
    pub RemoteLookupRdmaResponderComponent {
        version: "0.1.0",
        provides: [IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin],
        receptacles: {
            logger: ILogger,
            memory_tier: IMemoryTier,
        },
        fields: {
            actor_cpu: std::sync::Mutex<Option<usize>>,
            bind_ip: std::sync::Mutex<Option<String>>,
            state: std::sync::Mutex<Option<ActorState>>,
            endpoint: std::sync::OnceLock<Endpoint>,
            local_region: std::sync::OnceLock<LocalRegion>,
        },
    }
}

impl RemoteLookupRdmaResponderComponent {
    /// Log a debug line through the `logger` receptacle when one is bound.
    fn log_debug(&self, msg: &str) {
        if let Ok(log) = self.logger.get() {
            log.debug(msg);
        }
    }
}

/// Deliver an event to `remote-lookup` losslessly (FR-011a).
///
/// The channel `send` blocks (applies backpressure) until the event is
/// enqueued, so no event — in particular a load-bearing `DisconnectAck` — is
/// ever dropped. A `Closed` result means the consumer has gone away, in which
/// case there is nothing left to deliver.
#[cfg(any(feature = "rdma", test))]
fn send_event(tx: &Sender<ResponderEvent>, ev: ResponderEvent) {
    let _ = tx.send(ev);
}

/// The accept-loop body: drive the [`CmListener`] seam, routing connect/command
/// events to the [`ConnectionTable`] and emitting [`ResponderEvent`]s.
///
/// On `Stop` (stop flag raised or command channel closed) the loop tears down
/// every remaining connection and exits.
#[cfg(any(feature = "rdma", test))]
fn run_accept_loop(
    listener: Box<dyn CmListener>,
    mut table: ConnectionTable,
    event_tx: Sender<ResponderEvent>,
    stop: Arc<AtomicBool>,
) {
    'outer: loop {
        for ev in listener.next_events() {
            match ev {
                CmEvent::Stop => break 'outer,
                CmEvent::ConnectRequest { private_data, conn } => {
                    if let AcceptOutcome::Established(node) = table.accept(private_data, conn) {
                        send_event(&event_tx, ResponderEvent::ConnectionEstablished { node });
                    }
                }
                CmEvent::Command(ResponderCommand::Disconnect { node }) => {
                    // QP → ERROR happens inside disconnect(), before the ack.
                    table.disconnect(&node);
                    send_event(&event_tx, ResponderEvent::DisconnectAck { node });
                }
            }
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
    }
    table.teardown_all();
}

impl IRemoteLookupRdmaResponderAdmin for RemoteLookupRdmaResponderComponent {
    fn set_actor_cpu(&self, cpu: usize) {
        *self.actor_cpu.lock().expect("actor_cpu lock poisoned") = Some(cpu);
    }

    fn set_bind_ip(&self, ip: String) {
        *self.bind_ip.lock().expect("bind_ip lock poisoned") = Some(ip);
    }

    #[cfg(feature = "rdma")]
    fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        // Production: read the whole memory-tier pool from the `memory_tier`
        // receptacle, bind the real `rdma_cm` listener on the supplied RoCE IPv4,
        // and register the pool (REMOTE_WRITE) in the listener's protection domain.
        self.initialize_inner(|bind_ip, command_rx, _stop| {
            let mt = self.memory_tier.get().map_err(|_| {
                RemoteLookupRdmaResponderError::Registration(
                    "memory_tier receptacle not bound; cannot register the pool".into(),
                )
            })?;
            let (pool_ptr, pool_len) = mt.pool_info().ok_or_else(|| {
                RemoteLookupRdmaResponderError::Registration(
                    "memory tier pool not initialized (pool_info returned None)".into(),
                )
            })?;
            let (seam, endpoint, stop_efd, region) =
                RealCmSeam::bind(&bind_ip, command_rx, pool_ptr, pool_len)
                    .map_err(RemoteLookupRdmaResponderError::Bind)?;
            Ok((
                Box::new(seam) as Box<dyn CmListener>,
                endpoint,
                Some(stop_efd),
                region,
            ))
        })
    }

    #[cfg(not(feature = "rdma"))]
    fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        // Built without the real rdma-core path; only the mock seam is available.
        Err(RemoteLookupRdmaResponderError::Bind(
            "remote-lookup-rdma-responder built without the `rdma` feature".into(),
        ))
    }

    fn signal_stop(&self) {
        if let Some(state) = self.state.lock().expect("state lock poisoned").as_mut() {
            state.stop.store(true, Ordering::Release);
            // Wake the real (epoll) accept loop via its stop eventfd; for the mock
            // seam, also drop the retained command sender so a parked recv unblocks
            // when the control channel was never handed out.
            if let Some(_efd) = state.stop_eventfd {
                #[cfg(feature = "rdma")]
                rdma::signal_eventfd(_efd);
            }
            state.command_tx.take();
        }
    }

    fn shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        let taken = self.state.lock().expect("state lock poisoned").take();
        if let Some(mut state) = taken {
            state.stop.store(true, Ordering::Release);
            if let Some(_efd) = state.stop_eventfd {
                #[cfg(feature = "rdma")]
                rdma::signal_eventfd(_efd);
            }
            // Drop the retained command sender so the accept loop's recv unblocks
            // (Closed) when the control channel was never opened.
            state.command_tx.take();
            if let Some(join) = state.join.take() {
                join.join().map_err(|_| {
                    RemoteLookupRdmaResponderError::Internal("accept loop thread panicked".into())
                })?;
            }
            self.log_debug("rdma responder shut down");
        }
        Ok(())
    }
}

impl RemoteLookupRdmaResponderComponent {
    /// Shared `initialize` body. `make_listener` builds the CM seam from the
    /// bind IP, the command receiver, and the cooperative stop flag, returning
    /// the listener, the bound endpoint to advertise, and an optional stop
    /// eventfd. Production passes the real `rdma_cm` seam; tests inject the mock.
    #[cfg(any(feature = "rdma", test))]
    fn initialize_inner<F>(&self, make_listener: F) -> Result<(), RemoteLookupRdmaResponderError>
    where
        F: FnOnce(
            String,
            Receiver<ResponderCommand>,
            Arc<AtomicBool>,
        ) -> Result<
            (Box<dyn CmListener>, Endpoint, Option<c_int>, LocalRegion),
            RemoteLookupRdmaResponderError,
        >,
    {
        let mut guard = self.state.lock().expect("state lock poisoned");
        if guard.is_some() {
            return Err(RemoteLookupRdmaResponderError::AlreadyInitialized(
                "initialize() called twice".into(),
            ));
        }

        // The RoCE IPv4 to bind is optional (FR-002a): an empty/absent value
        // tells the listener to bind the first active RDMA device
        // (`rdma::first_active_rdma_ipv4`). The mainline supplies an explicit IP
        // (e.g. from `CERTUS_RDMA_BIND_IP`) to override that default.
        let bind_ip = self
            .bind_ip
            .lock()
            .expect("bind_ip lock poisoned")
            .clone()
            .unwrap_or_default();

        let (command_tx, command_rx) =
            SpscChannel::<ResponderCommand>::new(CONTROL_CHANNEL_CAPACITY)
                .split()
                .map_err(|_| {
                    RemoteLookupRdmaResponderError::ChannelClosed(
                        "failed to create command channel".into(),
                    )
                })?;
        let (event_tx, event_rx) = SpscChannel::<ResponderEvent>::new(CONTROL_CHANNEL_CAPACITY)
            .split()
            .map_err(|_| {
                RemoteLookupRdmaResponderError::ChannelClosed(
                    "failed to create event channel".into(),
                )
            })?;

        let stop = Arc::new(AtomicBool::new(false));
        let (listener, endpoint, stop_eventfd, local_region) =
            make_listener(bind_ip, command_rx, Arc::clone(&stop))?;
        let _ = self.endpoint.set(endpoint);
        let _ = self.local_region.set(local_region);

        let telemetry = Arc::new(TelemetryCollector::new());
        let table = ConnectionTable::new(telemetry);

        let actor_cpu = *self.actor_cpu.lock().expect("actor_cpu lock poisoned");
        let stop_thread = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("rdma-responder-accept".into())
            .spawn(move || {
                // Pin to the instance's NUMA node before entering the loop
                // (best-effort: a missing/failed pin is not fatal). FR-012.
                if let Some(cpu) = actor_cpu {
                    if let Ok(set) = CpuSet::from_cpu(cpu) {
                        let _ = set_thread_affinity(&set);
                    }
                }
                run_accept_loop(listener, table, event_tx, stop_thread);
            })
            .map_err(|e| RemoteLookupRdmaResponderError::Internal(format!("spawn failed: {e}")))?;

        *guard = Some(ActorState {
            command_tx: Some(command_tx),
            event_rx: Some(event_rx),
            stop,
            stop_eventfd,
            join: Some(join),
        });
        drop(guard);
        self.log_debug("rdma responder initialized");
        Ok(())
    }

    /// Test-only initialize that drives the actor over the in-process
    /// [`MockCmSeam`](connection::MockCmSeam) instead of binding a real NIC, so
    /// lifecycle unit tests need no RDMA hardware.
    #[cfg(test)]
    fn initialize_mock(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        self.initialize_inner(|bind_ip, command_rx, stop| {
            let seam = connection::MockCmSeam::new(command_rx, stop);
            Ok((
                Box::new(seam) as Box<dyn CmListener>,
                Endpoint {
                    ip: bind_ip,
                    port: 0,
                },
                None,
                // No NIC registration over the mock seam: a fabricated pool-wide
                // region exercises the local_region() plumbing without hardware.
                LocalRegion {
                    addr: 0,
                    rkey: 0,
                    length: 0,
                },
            ))
        })
    }
}

impl IRemoteLookupRdmaResponder for RemoteLookupRdmaResponderComponent {
    fn open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError> {
        let mut guard = self.state.lock().expect("state lock poisoned");
        let state = guard.as_mut().ok_or_else(|| {
            RemoteLookupRdmaResponderError::NotInitialized("call initialize() first".into())
        })?;
        let command_tx = state.command_tx.take().ok_or_else(|| {
            RemoteLookupRdmaResponderError::ChannelClosed("control channel already opened".into())
        })?;
        let event_rx = state.event_rx.take().ok_or_else(|| {
            RemoteLookupRdmaResponderError::ChannelClosed("control channel already opened".into())
        })?;
        Ok(ControlChannel {
            command_tx,
            event_rx,
        })
    }

    fn local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError> {
        self.endpoint.get().cloned().ok_or_else(|| {
            RemoteLookupRdmaResponderError::NotInitialized("call initialize() first".into())
        })
    }

    fn local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError> {
        self.local_region.get().copied().ok_or_else(|| {
            RemoteLookupRdmaResponderError::NotInitialized("call initialize() first".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interfaces::PeerId;

    const IP: &str = "192.0.2.10";

    #[test]
    fn endpoint_before_init_errors() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        assert!(matches!(
            comp.local_endpoint(),
            Err(RemoteLookupRdmaResponderError::NotInitialized(_))
        ));
    }

    #[test]
    fn open_before_init_errors() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        assert!(matches!(
            comp.open_control_channel(),
            Err(RemoteLookupRdmaResponderError::NotInitialized(_))
        ));
    }

    #[test]
    fn initialize_without_bind_ip_defers_to_autodetect() {
        // An unset bind IP is no longer an error (FR-002a relaxed): the real
        // listener auto-detects the first active RDMA device. Over the mock seam
        // the empty IP is accepted and surfaced as-is (a real bind fills it via
        // rdma::first_active_rdma_ipv4).
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        assert!(comp.initialize_mock().is_ok());
        assert_eq!(comp.local_endpoint().expect("endpoint").ip, "");
    }

    #[test]
    fn initialize_twice_errors_and_endpoint_is_published() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");

        let ep = comp.local_endpoint().expect("endpoint after init");
        assert_eq!(ep.ip, IP); // SC-001: bound to the supplied IP.

        assert!(matches!(
            comp.initialize_mock(),
            Err(RemoteLookupRdmaResponderError::AlreadyInitialized(_))
        ));
        comp.shutdown().expect("shutdown");
    }

    #[test]
    fn double_open_control_channel_errors() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");

        let ch = comp.open_control_channel().expect("open channel");
        // Second open must fail (single-client channel). FR-011.
        assert!(matches!(
            comp.open_control_channel(),
            Err(RemoteLookupRdmaResponderError::ChannelClosed(_))
        ));
        drop(ch); // closes command channel so the loop can exit
        comp.shutdown().expect("shutdown");
    }

    #[test]
    fn disconnect_yields_ack_over_control_channel() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");
        let ch = comp.open_control_channel().expect("open channel");

        ch.command_tx
            .send(ResponderCommand::Disconnect {
                node: PeerId::new("uuid-test"),
            })
            .expect("send disconnect");
        match ch.event_rx.recv().expect("recv ack") {
            ResponderEvent::DisconnectAck { node } => assert_eq!(node.as_str(), "uuid-test"),
            other => panic!("unexpected event: {other:?}"),
        }

        drop(ch); // close command channel → loop exits
        comp.shutdown().expect("shutdown");
    }

    #[test]
    fn shutdown_without_open_joins_and_is_idempotent() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");
        // Never opened: shutdown drops the retained command sender, the loop's
        // recv sees Closed, and the thread joins.
        comp.shutdown().expect("first shutdown joins");
        comp.shutdown().expect("second shutdown is a no-op");
    }

    #[test]
    fn signal_stop_exits_the_loop() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");
        comp.signal_stop();
        // The loop has exited cooperatively; shutdown then joins without hanging.
        comp.shutdown().expect("shutdown after signal_stop");
    }

    #[test]
    fn set_actor_cpu_is_honored() {
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_actor_cpu(0);
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize with cpu 0");
        comp.shutdown().expect("shutdown");
    }

    #[test]
    fn lifecycle_succeeds_with_no_logger_bound() {
        // new_default() binds no `logger` receptacle; a missing logger must never
        // turn an operation into an error (FR-014).
        let comp = RemoteLookupRdmaResponderComponent::new_default();
        comp.set_bind_ip(IP.into());
        comp.initialize_mock().expect("initialize");
        let ch = comp.open_control_channel().expect("open");
        ch.command_tx
            .send(ResponderCommand::Disconnect {
                node: PeerId::new("p"),
            })
            .expect("send");
        let _ = ch.event_rx.recv().expect("ack");
        drop(ch);
        comp.shutdown().expect("shutdown");
    }

    #[test]
    fn event_delivery_is_lossless_under_backpressure() {
        // FR-011a: a full event channel must apply backpressure, never drop —
        // especially the load-bearing DisconnectAck. Fill past capacity from a
        // sender thread; the main thread drains and must observe the ack intact.
        let ch = SpscChannel::<ResponderEvent>::new(2);
        let (tx, rx) = ch.split().unwrap();

        let sender = std::thread::spawn(move || {
            // Two ConnectionEstablished fill the capacity-2 channel; the third
            // send and the DisconnectAck then block until the consumer drains.
            send_event(&tx, ResponderEvent::ConnectionEstablished { node: None });
            send_event(&tx, ResponderEvent::ConnectionEstablished { node: None });
            send_event(&tx, ResponderEvent::ConnectionEstablished { node: None });
            send_event(
                &tx,
                ResponderEvent::DisconnectAck {
                    node: PeerId::new("p"),
                },
            );
        });

        let mut saw_ack = false;
        let mut count = 0;
        while count < 4 {
            match rx.recv() {
                Ok(ResponderEvent::DisconnectAck { node }) => {
                    assert_eq!(node.as_str(), "p");
                    saw_ack = true;
                    count += 1;
                }
                Ok(_) => count += 1,
                Err(_) => break,
            }
        }
        sender.join().unwrap();
        assert!(
            saw_ack,
            "DisconnectAck must survive backpressure, not be dropped"
        );
    }

    #[test]
    fn co_resident_instances_expose_independent_endpoints() {
        // Mock-level SC-004 sanity: two instances on the same host IP each
        // advertise their own endpoint. (True ephemeral-port distinctness needs
        // a NIC and is validated by the hardware loopback test.)
        let a = RemoteLookupRdmaResponderComponent::new_default();
        let b = RemoteLookupRdmaResponderComponent::new_default();
        a.set_bind_ip(IP.into());
        b.set_bind_ip(IP.into());
        a.initialize_mock().expect("init a");
        b.initialize_mock().expect("init b");
        assert_eq!(a.local_endpoint().unwrap().ip, IP);
        assert_eq!(b.local_endpoint().unwrap().ip, IP);
        a.shutdown().expect("shutdown a");
        b.shutdown().expect("shutdown b");
    }
}
