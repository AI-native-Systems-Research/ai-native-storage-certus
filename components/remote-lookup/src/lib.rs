//! Remote lookup component.
//!
//! Performs remote cache lookups to other Certus nodes over zyre (discovery +
//! signalling) and RDMA (data path), per `specs/002-remote-lookup-rdma/`. On
//! `initialize` the component creates and joins a zyre node and spawns a single
//! actor poll-loop thread that owns all operation state (research Decision 1).
//! The KEY_QUERY → RDMA protocol is built up across the US1–US7 tasks; until
//! then `batch_lookup` finalizes every key as `NotFound`.

mod actor;
mod operation;
mod server;
mod worker;

/// The v1 zyre wire protocol codec (framing + encode/decode). Public so the
/// `benches/` codec benchmark and out-of-crate tooling can exercise it.
pub mod wire;

/// In-process mock seams for the receptacle interfaces, used by unit tests and
/// the `tests/mesh.rs` multi-node harness (research Decision 8).
pub mod seams;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use component_core::channel::mpsc::MpscChannel;
use component_framework::define_component;
use interfaces::{
    CacheKey, IDispatchMap, IDispatcher, ILogger, IMemoryTier, IRemoteLookup,
    IRemoteLookupRdmaInitiator, IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin, IZyre,
    LookupConfig, NodeConfig, RemoteLookupError,
};

use crate::actor::{ActorHandle, ActorInit, ActorMsg, Deps, OperationRequest, SubmitSender};
use crate::worker::{InitiatorCmd, ServerDeps};

define_component! {
    pub RemoteLookupComponent {
        version: "0.1.0",
        provides: [IRemoteLookup],
        receptacles: {
            zyre: IZyre,
            dispatch_map: IDispatchMap,
            memory_tier: IMemoryTier,
            dispatcher: IDispatcher,
            initiator: IRemoteLookupRdmaInitiator,
            responder: IRemoteLookupRdmaResponder,
            responder_admin: IRemoteLookupRdmaResponderAdmin,
            logger: ILogger,
        },
        fields: {
            // Sending half of the actor's MPSC submission channel, set once by
            // `initialize` when the actor thread is spawned (T013).
            submit_tx: OnceLock<SubmitSender>,
            // The spawned actor thread, joined on deactivate.
            actor: Mutex<Option<ActorHandle>>,
            // The off-loop initiator worker thread (warm + serve). Joined on
            // deactivate, after the actor, so its InitiatorCmd sender is dropped
            // and its command channel closes first.
            worker: Mutex<Option<JoinHandle<()>>>,
            // Monotonic source of per-operation ids.
            op_counter: AtomicU64,
            // Effective configuration, set once by `initialize`. (Refines the
            // task's `config: LookupConfig` to interior mutability: `initialize`
            // takes `&self`, so the field must be settable post-construction.)
            config: OnceLock<LookupConfig>,
            // Count of peers currently visible to the actor's zyre node
            // (incremented on ENTER, decremented on EXIT). Shared with the actor
            // thread; read by tests via [`RemoteLookupComponent::peers_seen`] to
            // implement a discovery barrier.
            peers_seen: Arc<AtomicUsize>,
        },
    }
}

impl IRemoteLookup for RemoteLookupComponent {
    /// Bring the component up: store the config, create and start the zyre node
    /// (via the `zyre` receptacle), join the configured group, and spawn the
    /// actor poll-loop thread. Idempotent — a second call returns an error.
    ///
    /// The discovery mode is taken from [`LookupConfig::discovery`]: `None`
    /// (default) uses UDP-beacon discovery, `Some(GossipConfig)` uses gossip over
    /// [`LookupConfig::node_endpoint`].
    ///
    /// # Errors
    ///
    /// Returns [`RemoteLookupError::TransportError`] if the `zyre` receptacle is
    /// unbound, node creation/start/join fails, or the component is already
    /// initialized.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::query_interface;
    /// use interfaces::{IRemoteLookup, LookupConfig};
    /// use remote_lookup::RemoteLookupComponent;
    ///
    /// let comp = RemoteLookupComponent::new_default();
    /// let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
    ///     query_interface!(comp, IRemoteLookup).unwrap();
    /// // Without a bound `zyre` receptacle, initialize reports a transport error.
    /// assert!(rl.initialize(LookupConfig::default()).is_err());
    /// ```
    fn initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError> {
        if self.config.get().is_some() {
            return Err(RemoteLookupError::TransportError(
                "remote-lookup: already initialized".into(),
            ));
        }

        let izyre = self.zyre.get().map_err(|e| {
            RemoteLookupError::TransportError(format!(
                "remote-lookup: zyre receptacle unbound: {e}"
            ))
        })?;

        // Resolve the receptacles the actor needs for both the client and server
        // roles. dispatch_map/memory_tier/initiator/responder are required.
        let te = |m: String| RemoteLookupError::TransportError(format!("remote-lookup: {m}"));
        let dispatch_map = self
            .dispatch_map
            .get()
            .map_err(|e| te(format!("dispatch_map receptacle unbound: {e}")))?;
        let memory_tier = self
            .memory_tier
            .get()
            .map_err(|e| te(format!("memory_tier receptacle unbound: {e}")))?;
        let initiator = self
            .initiator
            .get()
            .map_err(|e| te(format!("initiator receptacle unbound: {e}")))?;
        let logger = self.logger.get().ok();

        // Bring up the RDMA responder (T009b/FR-025) *before* creating the zyre
        // node: configure and initialize it, then cache the bound endpoint + pool
        // rkey advertised in KEY_RESPONSE / RDMA_REQUEST — and, so peers can warm
        // a connection to us the moment they discover us, in our zyre ENTER
        // header (connect-hardening).
        let responder_admin = self
            .responder_admin
            .get()
            .map_err(|e| te(format!("responder_admin receptacle unbound: {e}")))?;
        responder_admin.set_bind_ip(config.bind_ip.clone());
        if let Some(cpu) = config.actor_cpu {
            responder_admin.set_actor_cpu(cpu);
        }
        responder_admin
            .initialize()
            .map_err(|e| te(format!("responder initialize failed: {e}")))?;
        let responder = self
            .responder
            .get()
            .map_err(|e| te(format!("responder receptacle unbound: {e}")))?;
        let local_endpoint = responder
            .local_endpoint()
            .map_err(|e| te(format!("responder local_endpoint failed: {e}")))?;
        let local_rkey = responder
            .local_region()
            .map_err(|e| te(format!("responder local_region failed: {e}")))?
            .rkey;
        let control = responder
            .open_control_channel()
            .map_err(|e| te(format!("open control channel failed: {e}")))?;

        // Build the zyre NodeConfig from LookupConfig: discovery + endpoint drive
        // gossip-vs-beacon (research Decision 8); the group is joined below.
        // NodeConfig is #[non_exhaustive], so build from Default and set fields.
        let mut node_cfg = NodeConfig::default();
        node_cfg.gossip = config.discovery.clone();
        node_cfg.endpoint = config.node_endpoint.clone();
        node_cfg.name = Some(format!("remote-lookup:{}", config.group));
        // Advertise our RDMA responder endpoint so a peer warms a connection to
        // us on ENTER, amortizing the cold-connect cost before the first serve.
        node_cfg.headers.insert(
            crate::actor::RDMA_ENDPOINT_HEADER.to_string(),
            format!("{}:{}", local_endpoint.ip, local_endpoint.port),
        );

        let mut node = izyre.create_node(node_cfg).map_err(|e| {
            RemoteLookupError::TransportError(format!("remote-lookup: create_node failed: {e}"))
        })?;

        // Stamp our zyre PeerId into the initiator's outbound connections (D2).
        initiator.set_local_peer_id(node.uuid());

        node.start()
            .map_err(|e| te(format!("node.start failed: {e}")))?;
        node.join(&config.group)
            .map_err(|e| te(format!("node.join failed: {e}")))?;

        // Wire the MPSC submission channel and spawn the actor, moving the
        // started node onto the actor thread (IZyreNode is Send + !Sync).
        let channel = MpscChannel::<ActorMsg>::new(256);
        let (tx, rx) = channel
            .split()
            .map_err(|e| te(format!("channel split failed: {e:?}")))?;

        // Spawn the off-loop initiator worker: it owns the server-role handles
        // (initiator + dispatcher) and performs blocking RDMA connects/serves off
        // the poll loop, posting results back over a clone of the submission
        // channel. The actor holds the sole InitiatorCmd sender, so the worker
        // exits when the actor thread ends (channel closes) — see `shutdown`.
        let (icmd_tx, icmd_rx) = std::sync::mpsc::channel::<InitiatorCmd>();
        let server_deps = ServerDeps {
            dispatch_map: Arc::clone(&dispatch_map),
            dispatcher: self.dispatcher.get().ok(),
            initiator,
            logger: logger.clone(),
        };
        let back = tx.clone();
        let worker_handle = std::thread::Builder::new()
            .name("remote-lookup-initiator".into())
            .spawn(move || crate::worker::run(server_deps, icmd_rx, back))
            .map_err(|e| te(format!("worker spawn failed: {e}")))?;

        let init = ActorInit {
            node,
            deps: Deps {
                dispatch_map,
                memory_tier,
                logger,
            },
            local_endpoint,
            local_rkey,
            group: config.group.clone(),
            config: config.clone(),
            peers_seen: Arc::clone(&self.peers_seen),
            control,
            initiator_tx: icmd_tx,
        };
        let handle = std::thread::Builder::new()
            .name("remote-lookup-actor".into())
            .spawn(move || crate::actor::run(init, rx))
            .map_err(|e| te(format!("actor spawn failed: {e}")))?;

        // Publish state. `config.set` is the initialization latch checked above.
        let _ = self.submit_tx.set(tx);
        *self.actor.lock().unwrap() = Some(ActorHandle::new(handle));
        *self.worker.lock().unwrap() = Some(worker_handle);
        let _ = self.config.set(config);

        if let Ok(logger) = self.logger.get() {
            logger.info("remote-lookup: initialized");
        }
        Ok(())
    }

    /// Submit a batch of `(key, size)` entries to the actor and block until the
    /// operation finalizes, returning one positional result per entry. An empty
    /// input returns immediately; an uninitialized component returns `NotFound`
    /// for every entry. (The actor currently answers all-`NotFound` until the
    /// KEY_QUERY → RDMA protocol lands in US1–US3.)
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::query_interface;
    /// use interfaces::{CacheKey, IRemoteLookup, RemoteLookupError};
    /// use remote_lookup::RemoteLookupComponent;
    ///
    /// let comp = RemoteLookupComponent::new_default();
    /// let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
    ///     query_interface!(comp, IRemoteLookup).unwrap();
    ///
    /// let entries: Vec<(CacheKey, u32)> = vec![(1, 4096), (2, 4096)];
    /// let results = rl.batch_lookup(&entries);
    /// assert_eq!(results.len(), 2);
    /// assert_eq!(results[0], Err(RemoteLookupError::NotFound));
    /// ```
    fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>> {
        if entries.is_empty() {
            return Vec::new();
        }
        let all_not_found = || -> Vec<_> {
            entries
                .iter()
                .map(|_| Err(RemoteLookupError::NotFound))
                .collect()
        };

        // Not initialized (no actor) ⇒ nothing is remotely available.
        let Some(tx) = self.submit_tx.get() else {
            return all_not_found();
        };

        let op_id = self.op_counter.fetch_add(1, Ordering::Relaxed);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let req = OperationRequest {
            op_id,
            entries: entries.to_vec(),
            done: done_tx,
        };
        if tx.send(ActorMsg::Submit(req)).is_err() {
            return all_not_found();
        }
        // `caller_wait` decouples the caller's patience from the operation's
        // lifetime. `None` keeps the historical behavior — block until the actor
        // finalizes at `op_deadline` (a dropped actor closes the channel).
        // `Some(w)` returns NotFound after `w`, but the operation keeps running to
        // `op_deadline`: it goes on fetching/retrying and publishes any landed key
        // to the local tier (publish-on-success), so a slow or recovering fetch
        // populates the cache for the next lookup rather than blocking this caller.
        match self.config.get().and_then(|c| c.caller_wait) {
            Some(wait) => done_rx
                .recv_timeout(wait)
                .unwrap_or_else(|_| all_not_found()),
            None => done_rx.recv().unwrap_or_else(|_| all_not_found()),
        }
    }

    /// Join an additional zyre group. Routed to the actor thread, which owns the
    /// zyre node. Returns an error if the component is not initialized.
    fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError> {
        let tx = self.submit_tx.get().ok_or_else(|| {
            RemoteLookupError::TransportError("remote-lookup: not initialized".into())
        })?;
        tx.send(ActorMsg::Join(endpoint.to_string()))
            .map_err(|e| RemoteLookupError::TransportError(format!("remote-lookup: {e:?}")))
    }

    /// Leave the configured zyre group. Routed to the actor thread.
    fn leave_cluster(&self) -> Result<(), RemoteLookupError> {
        let tx = self.submit_tx.get().ok_or_else(|| {
            RemoteLookupError::TransportError("remote-lookup: not initialized".into())
        })?;
        tx.send(ActorMsg::Leave)
            .map_err(|e| RemoteLookupError::TransportError(format!("remote-lookup: {e:?}")))
    }
}

impl RemoteLookupComponent {
    /// Number of peers currently visible to the actor's zyre node (ENTER minus
    /// EXIT). Used by tests as a discovery barrier before driving the protocol.
    pub fn peers_seen(&self) -> usize {
        self.peers_seen.load(Ordering::Relaxed)
    }

    /// Signal the actor to stop **without** waiting for it to exit. Used for a
    /// two-phase teardown: tell every actor in a group to stop polling its zyre
    /// node before any node is destroyed. Destroying one node while another
    /// actor's poll loop is mid-`try_recv` on the shared czmq context trips a
    /// `zpoller` assertion. Idempotent and safe before `initialize`.
    pub fn signal_shutdown(&self) {
        if let Some(tx) = self.submit_tx.get() {
            let _ = tx.send(ActorMsg::Shutdown);
        }
    }

    /// Signal the actor (see [`signal_shutdown`](Self::signal_shutdown)) and then
    /// join its thread. Idempotent and safe to call before `initialize`.
    pub fn shutdown(&self) {
        self.signal_shutdown();
        if let Some(handle) = self.actor.lock().unwrap().take() {
            handle.join();
        }
        // Join the initiator worker after the actor: the actor owned the sole
        // InitiatorCmd sender, so its channel is now closed and the worker's
        // `recv` loop exits (it may finish an in-flight serve first).
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RemoteLookupComponent {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<RemoteLookupComponent> {
        RemoteLookupComponent::new_default()
    }

    fn make_entries(count: usize) -> Vec<(CacheKey, u32)> {
        (0..count).map(|i| (i as CacheKey, 4096u32)).collect()
    }

    #[test]
    fn batch_lookup_returns_not_found_for_each_entry() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let entries = make_entries(5);
        let results = rl.batch_lookup(&entries);

        assert_eq!(results.len(), 5);
        for r in &results {
            assert_eq!(*r, Err(RemoteLookupError::NotFound));
        }
    }

    #[test]
    fn batch_lookup_returns_empty_vec_for_empty_input() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let results = rl.batch_lookup(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn batch_lookup_preserves_positional_order() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let entries = make_entries(10);
        let results = rl.batch_lookup(&entries);

        assert_eq!(results.len(), entries.len());
    }

    #[test]
    fn join_cluster_errors_when_uninitialized() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        // Routed to the actor, which does not exist until `initialize`.
        assert!(rl.join_cluster("some-group").is_err());
    }

    #[test]
    fn leave_cluster_errors_when_uninitialized() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        assert!(rl.leave_cluster().is_err());
    }

    #[test]
    fn batch_lookup_accepts_cache_key_size_slice() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let entries: &[(CacheKey, u32)] = &[(42, 4096)];
        let results = rl.batch_lookup(entries);
        assert_eq!(results.len(), 1);
    }
}
