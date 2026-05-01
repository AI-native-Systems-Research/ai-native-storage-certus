//! Actor components — thread-owning components with message-loop semantics.
//!
//! An actor owns a dedicated OS thread and processes messages of type `M`
//! sequentially. Actors are first-class components implementing [`IUnknown`],
//! providing [`ISender<M>`](crate::channel::ISender) as their interface so
//! other components can send messages to them via the standard component model.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use crate::channel::{ChannelError, ISender};
use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;
use crate::numa::CpuSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorError {
    /// `activate()` called on an already-running actor.
    AlreadyActive,
    /// `deactivate()` called on an idle actor.
    NotActive,
    /// Failed to send a message to the actor's inbound channel.
    SendFailed(String),
    /// Thread join timed out during deactivation.
    ShutdownTimeout,
    /// Setting CPU affinity failed.
    AffinityFailed(String),
}

impl fmt::Display for ActorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyActive => write!(f, "actor is already active"),
            Self::NotActive => write!(f, "actor is not active"),
            Self::SendFailed(msg) => write!(f, "send failed: {msg}"),
            Self::ShutdownTimeout => write!(f, "actor shutdown timed out"),
            Self::AffinityFailed(msg) => write!(f, "affinity failed: {msg}"),
        }
    }
}

impl std::error::Error for ActorError {}

// Actor lifecycle states
const STATE_IDLE: u8 = 0;
const STATE_RUNNING: u8 = 1;

/// Trait that users implement to define actor message-handling behavior.
///
/// `M` is the message type. Must be `Send + 'static` to cross thread
/// boundaries.
///
/// # Examples
///
/// ```
/// use component_core::actor::ActorHandler;
///
/// struct PrintHandler;
///
/// impl ActorHandler<String> for PrintHandler {
///     fn handle(&mut self, msg: String) {
///         println!("Got: {msg}");
///     }
/// }
/// ```
pub trait ActorHandler<M: Send + 'static>: Send + 'static {
    /// Called for each message received. Runs on the actor's dedicated thread.
    fn handle(&mut self, msg: M);

    /// Called once when the actor starts (before the message loop).
    /// Default implementation is a no-op.
    fn on_start(&mut self) {}

    /// Called once when the actor is shutting down (after the message loop exits).
    /// Default implementation is a no-op.
    fn on_stop(&mut self) {}

    /// Called when the actor's message queue is empty.
    ///
    /// Override this for actors that have background work (e.g., polling
    /// IO channels, processing completions) that should run even when no
    /// control messages are pending. Return `true` if useful work was done
    /// (resets the idle counter to prevent premature parking). The default
    /// implementation is a no-op that returns `false`.
    fn on_idle(&mut self) -> bool {
        false
    }
}

/// Handle to a running actor. Returned by [`Actor::activate`].
///
/// Provides methods to send messages and deactivate the actor.
/// Dropping the handle without calling [`deactivate`](ActorHandle::deactivate)
/// will close the channel, causing the actor to stop after processing its
/// current message.
///
/// # Examples
///
/// ```
/// use component_core::actor::{Actor, ActorHandler};
///
/// struct Counter { count: u32 }
/// impl ActorHandler<u32> for Counter {
///     fn handle(&mut self, _msg: u32) { self.count += 1; }
/// }
///
/// let actor: Actor<u32, Counter> = Actor::new(
///     Counter { count: 0 },
///     |_| {},
/// );
/// let handle = actor.activate().unwrap();
/// handle.send(1).unwrap();
/// handle.deactivate().unwrap();
/// ```
pub struct ActorHandle<M: Send + 'static> {
    sender: Option<MpscSender<M>>,
    thread: Option<JoinHandle<()>>,
    state: Arc<AtomicU8>,
    /// Reference to the actor's channel for force-close on deactivation.
    channel: Arc<MpscChannel<M>>,
}

impl<M: Send + 'static> fmt::Debug for ActorHandle<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActorHandle")
            .field("active", &self.sender.is_some())
            .finish()
    }
}

impl<M: Send + 'static> ActorHandle<M> {
    /// Send a message to the actor. Blocks if the inbound channel is full.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError::SendFailed`] if the channel is closed.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// let handle = actor.activate().unwrap();
    /// handle.send(42).unwrap();
    /// handle.deactivate().unwrap();
    /// ```
    pub fn send(&self, msg: M) -> Result<(), ActorError> {
        if let Some(ref sender) = self.sender {
            sender
                .send(msg)
                .map_err(|e| ActorError::SendFailed(format!("{e}")))
        } else {
            Err(ActorError::SendFailed("channel closed".into()))
        }
    }

    /// Try to send a message without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError::SendFailed`] if the channel is full or closed.
    pub fn try_send(&self, msg: M) -> Result<(), ActorError> {
        if let Some(ref sender) = self.sender {
            sender
                .try_send(msg)
                .map_err(|e| ActorError::SendFailed(format!("{e}")))
        } else {
            Err(ActorError::SendFailed("channel closed".into()))
        }
    }

    /// Deactivate the actor: close the channel and join the thread.
    ///
    /// The actor finishes processing its current message, then stops.
    /// Remaining messages in the channel are not drained.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError::ShutdownTimeout`] if the thread cannot be joined.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// let handle = actor.activate().unwrap();
    /// handle.deactivate().unwrap();
    /// assert!(!actor.is_active());
    /// ```
    pub fn deactivate(mut self) -> Result<(), ActorError> {
        // Drop the sender
        self.sender.take();
        // Force-close the channel so the receiver thread exits even when
        // other senders (from IUnknown queries) are still alive.
        self.channel.close();

        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| ActorError::ShutdownTimeout)?;
        }

        self.state.store(STATE_IDLE, Ordering::Release);
        Ok(())
    }
}

impl<M: Send + 'static> Drop for ActorHandle<M> {
    fn drop(&mut self) {
        // Drop the sender to signal shutdown
        self.sender.take();
        // Force-close the channel
        self.channel.close();

        // Best-effort join
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        self.state.store(STATE_IDLE, Ordering::Release);
    }
}

/// An actor component that owns a thread and processes messages sequentially.
///
/// The actor is a first-class component implementing [`IUnknown`]. It provides
/// [`ISender<M>`](crate::channel::ISender) as its interface, allowing other
/// components to query for a sender and send messages through the standard
/// component model.
///
/// The actor is created in an idle state. Call [`activate`](Actor::activate) to
/// spawn its thread and start the message loop. The returned [`ActorHandle`]
/// is used to send messages and eventually deactivate the actor.
///
/// # NUMA-Local Handler Placement
///
/// When using CPU affinity for NUMA-aware actors, the handler's memory is
/// allocated wherever the constructing thread runs (Linux first-touch policy).
/// For optimal NUMA locality, construct the handler on a thread that is
/// already pinned to the target NUMA node:
///
/// ```no_run
/// use component_core::actor::{Actor, ActorHandler};
/// use component_core::numa::{CpuSet, NumaTopology, set_thread_affinity};
///
/// struct MyHandler { buffer: Vec<u8> }
/// impl ActorHandler<u64> for MyHandler {
///     fn handle(&mut self, _msg: u64) {}
/// }
///
/// // Pin the current thread to node 0, then construct the handler
/// // so its heap allocations land on that NUMA node.
/// let topo = NumaTopology::discover().unwrap();
/// let node0_cpus = topo.node(0).unwrap().cpus();
/// let cs = CpuSet::from_cpus(node0_cpus.iter()).unwrap();
/// set_thread_affinity(&cs).unwrap();
///
/// let handler = MyHandler { buffer: vec![0u8; 4096] };
/// let actor = Actor::simple(handler).with_cpu_affinity(cs);
/// // Handler memory and actor thread are both on NUMA node 0.
/// ```
///
/// If the handler is constructed without pinning, Linux places its memory
/// on whatever node the OS scheduler happens to choose. This is fine for
/// non-latency-sensitive workloads but may add cross-node access penalties
/// for hot data structures.
///
/// # Type Parameters
///
/// * `M` — Message type. Must be `Send + 'static`.
/// * `H` — Handler type implementing [`ActorHandler<M>`].
///
/// # Examples
///
/// ```
/// use component_core::actor::{Actor, ActorHandler};
/// use std::sync::{Arc, Mutex};
///
/// struct Accumulator {
///     sum: Arc<Mutex<i64>>,
/// }
///
/// impl ActorHandler<i64> for Accumulator {
///     fn handle(&mut self, msg: i64) {
///         *self.sum.lock().unwrap() += msg;
///     }
/// }
///
/// let sum = Arc::new(Mutex::new(0i64));
/// let actor = Actor::new(
///     Accumulator { sum: sum.clone() },
///     |panic_info| eprintln!("Actor panicked: {panic_info:?}"),
/// );
///
/// let handle = actor.activate().unwrap();
/// for i in 1..=10 {
///     handle.send(i).unwrap();
/// }
/// handle.deactivate().unwrap();
/// assert_eq!(*sum.lock().unwrap(), 55);
/// ```
///
/// Actors implement [`IUnknown`] and can be queried for [`ISender<M>`](crate::channel::ISender):
///
/// ```
/// use component_core::actor::{Actor, ActorHandler};
/// use component_core::channel::ISender;
/// use component_core::query_interface;
/// use component_core::iunknown::{IUnknown, query};
/// use std::sync::{Arc, Mutex};
///
/// struct Logger {
///     log: Arc<Mutex<Vec<String>>>,
/// }
///
/// impl ActorHandler<String> for Logger {
///     fn handle(&mut self, msg: String) {
///         self.log.lock().unwrap().push(msg);
///     }
/// }
///
/// let log = Arc::new(Mutex::new(Vec::new()));
/// let actor = Actor::new(Logger { log: log.clone() }, |_| {});
///
/// // Query ISender<String> via IUnknown
/// let sender: Arc<dyn ISender<String> + Send + Sync> =
///     query_interface!(&actor, ISender<String>).unwrap();
///
/// let handle = actor.activate().unwrap();
/// sender.send("hello".into()).unwrap();
/// handle.deactivate().unwrap();
///
/// assert_eq!(*log.lock().unwrap(), vec!["hello".to_string()]);
/// ```
pub struct Actor<M, H>
where
    M: Send + 'static,
    H: ActorHandler<M>,
{
    handler: Mutex<Option<H>>,
    error_callback: Arc<dyn Fn(Box<dyn Any + Send>) + Send + Sync>,
    state: Arc<AtomicU8>,
    /// MPSC channel for inbound messages — created at construction time
    /// so `ISender<M>` is available via IUnknown before activation.
    channel: Arc<MpscChannel<M>>,
    /// Receiver stored until activate() takes it.
    receiver: Mutex<Option<MpscReceiver<M>>>,
    /// Lazily created sender interface for IUnknown queries.
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    /// Cached interface metadata for introspection.
    interface_info: Vec<InterfaceInfo>,
    /// Optional CPU affinity — if set, the actor's thread is pinned on activation.
    cpu_affinity: Mutex<Option<CpuSet>>,
}

impl<M, H> Actor<M, H>
where
    M: Send + 'static,
    H: ActorHandler<M>,
{
    /// Create a new actor with the given handler.
    ///
    /// Panics in the message handler are silently caught. Use
    /// [`with_capacity`](Actor::with_capacity) to receive
    /// panic notifications via a custom error callback.
    ///
    /// Default channel capacity is 1024.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct MyHandler;
    /// impl ActorHandler<String> for MyHandler {
    ///     fn handle(&mut self, _msg: String) {}
    /// }
    ///
    /// let actor = Actor::simple(MyHandler);
    /// assert!(!actor.is_active());
    /// ```
    pub fn simple(handler: H) -> Self {
        Self::build(handler, 1024, |_| {})
    }

    /// Create a new actor with the given handler and error callback.
    ///
    /// The error callback is invoked when the message handler panics.
    /// Default channel capacity is 1024.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct MyHandler;
    /// impl ActorHandler<String> for MyHandler {
    ///     fn handle(&mut self, _msg: String) {}
    /// }
    ///
    /// let actor = Actor::new(MyHandler, |_| {});
    /// assert!(!actor.is_active());
    /// ```
    pub fn new(
        handler: H,
        error_callback: impl Fn(Box<dyn Any + Send>) + Send + Sync + 'static,
    ) -> Self {
        Self::build(handler, 1024, error_callback)
    }

    /// Create a new actor with custom channel capacity.
    ///
    /// `capacity` must be a power of two. Panics if it is not.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct MyHandler;
    /// impl ActorHandler<u32> for MyHandler {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::with_capacity(MyHandler, 256, |_| {});
    /// ```
    pub fn with_capacity(
        handler: H,
        capacity: usize,
        error_callback: impl Fn(Box<dyn Any + Send>) + Send + Sync + 'static,
    ) -> Self {
        Self::build(handler, capacity, error_callback)
    }

    fn build(
        handler: H,
        capacity: usize,
        error_callback: impl Fn(Box<dyn Any + Send>) + Send + Sync + 'static,
    ) -> Self {
        let channel = MpscChannel::<M>::new(capacity);
        let receiver = channel.receiver().expect("first receiver on new channel");

        let interface_info = vec![InterfaceInfo {
            type_id: TypeId::of::<Arc<dyn ISender<M> + Send + Sync>>(),
            name: "ISender",
        }];

        Self {
            handler: Mutex::new(Some(handler)),
            error_callback: Arc::new(error_callback),
            state: Arc::new(AtomicU8::new(STATE_IDLE)),
            channel: Arc::new(channel),
            receiver: Mutex::new(Some(receiver)),
            sender_iface: OnceLock::new(),
            interface_info,
            cpu_affinity: Mutex::new(None),
        }
    }

    /// Set CPU affinity for this actor's thread (builder pattern).
    ///
    /// When set, the actor's thread will be pinned to the specified CPUs
    /// on activation. Can be called on idle actors to change affinity
    /// between activation cycles.
    ///
    /// For NUMA-local handler placement, construct the handler on a
    /// thread already pinned to the target node before passing it to
    /// the actor constructor. See the [NUMA-Local Handler Placement]
    /// section on [`Actor`] for a full example.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::actor::{Actor, ActorHandler};
    /// use component_core::numa::CpuSet;
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {})
    ///     .with_cpu_affinity(CpuSet::from_cpu(0).unwrap());
    /// ```
    pub fn with_cpu_affinity(self, affinity: CpuSet) -> Self {
        *self.cpu_affinity.lock().unwrap() = Some(affinity);
        self
    }

    /// Set or change the CPU affinity for this actor's thread.
    ///
    /// Only valid when the actor is idle. Returns
    /// [`ActorError::AlreadyActive`] if the actor is running.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::actor::{Actor, ActorHandler, ActorError};
    /// use component_core::numa::CpuSet;
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// actor.set_cpu_affinity(CpuSet::from_cpu(0).unwrap()).unwrap();
    /// ```
    pub fn set_cpu_affinity(&self, affinity: CpuSet) -> Result<(), ActorError> {
        if self.is_active() {
            return Err(ActorError::AlreadyActive);
        }
        *self.cpu_affinity.lock().unwrap() = Some(affinity);
        Ok(())
    }

    /// Get the current CPU affinity, if set.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// assert!(actor.cpu_affinity().is_none());
    /// ```
    pub fn cpu_affinity(&self) -> Option<CpuSet> {
        self.cpu_affinity.lock().unwrap().clone()
    }

    /// Activate the actor: spawn its thread and start the message loop.
    ///
    /// Returns an [`ActorHandle`] for sending messages and deactivating.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError::AlreadyActive`] if the actor is already running.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler, ActorError};
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// let handle = actor.activate().unwrap();
    /// assert!(actor.is_active());
    ///
    /// // Double-activate returns error
    /// // (handle is still alive, so actor is still active)
    /// // We can't activate again from the same Actor while handle exists.
    /// handle.deactivate().unwrap();
    /// ```
    pub fn activate(&self) -> Result<ActorHandle<M>, ActorError> {
        // CAS: IDLE -> RUNNING
        if self
            .state
            .compare_exchange(
                STATE_IDLE,
                STATE_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(ActorError::AlreadyActive);
        }

        let sender = self.channel.sender().expect("MPSC sender creation");
        let receiver = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("receiver already taken — actor activated twice without reset");

        let mut handler = self
            .handler
            .lock()
            .unwrap()
            .take()
            .expect("handler already taken — actor activated twice without reset");

        let error_callback = Arc::clone(&self.error_callback);
        let affinity = self.cpu_affinity.lock().unwrap().clone();

        // Validate CPU IDs before spawning the thread (FR-004).
        if let Some(ref cpus) = affinity {
            crate::numa::validate_cpus(cpus)
                .map_err(|e| ActorError::AffinityFailed(e.to_string()))?;
        }

        // Use a channel to propagate affinity errors from the spawned thread.
        let (startup_tx, startup_rx) = std::sync::mpsc::channel::<Result<(), ActorError>>();

        let thread = thread::spawn(move || {
            // Apply CPU affinity if configured.
            if let Some(ref cpus) = affinity {
                if let Err(e) = crate::numa::set_thread_affinity(cpus) {
                    let _ = startup_tx.send(Err(ActorError::AffinityFailed(e.to_string())));
                    return;
                }
            }
            let _ = startup_tx.send(Ok(()));

            handler.on_start();

            const PARK_THRESHOLD: u64 = 10_000_000;
            const PARK_DURATION: std::time::Duration = std::time::Duration::from_millis(10);
            let mut idle_count: u64 = 0;

            loop {
                match receiver.try_recv() {
                    Ok(msg) => {
                        idle_count = 0;
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handler.handle(msg);
                        }));

                        if let Err(panic_payload) = result {
                            error_callback(panic_payload);
                        }
                    }
                    Err(ChannelError::Empty) => {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handler.on_idle()
                        }));

                        match result {
                            Ok(did_work) => {
                                if did_work {
                                    idle_count = 0;
                                } else {
                                    idle_count += 1;
                                }
                            }
                            Err(panic_payload) => {
                                error_callback(panic_payload);
                                idle_count += 1;
                            }
                        }

                        if idle_count >= PARK_THRESHOLD {
                            receiver.register_for_unpark();
                            thread::park_timeout(PARK_DURATION);
                            idle_count = 0;
                        }
                    }
                    Err(ChannelError::Closed) => break,
                    Err(_) => break,
                }
            }

            handler.on_stop();
        });

        // Wait for the thread to confirm affinity was set successfully.
        match startup_rx.recv() {
            Ok(Err(e)) => {
                // Affinity failed — join the thread and reset state.
                let _ = thread.join();
                self.state.store(STATE_IDLE, Ordering::Release);
                return Err(e);
            }
            Err(_) => {
                // Thread panicked before sending — shouldn't happen but handle it.
                self.state.store(STATE_IDLE, Ordering::Release);
                return Err(ActorError::AffinityFailed(
                    "thread exited before startup".into(),
                ));
            }
            Ok(Ok(())) => {} // All good.
        }

        Ok(ActorHandle {
            sender: Some(sender),
            thread: Some(thread),
            state: Arc::clone(&self.state),
            channel: Arc::clone(&self.channel),
        })
    }

    /// Check whether the actor is currently running.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::actor::{Actor, ActorHandler};
    ///
    /// struct Noop;
    /// impl ActorHandler<u32> for Noop {
    ///     fn handle(&mut self, _msg: u32) {}
    /// }
    ///
    /// let actor = Actor::new(Noop, |_| {});
    /// assert!(!actor.is_active());
    /// ```
    pub fn is_active(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_RUNNING
    }
}

impl<M, H> IUnknown for Actor<M, H>
where
    M: Send + 'static,
    H: ActorHandler<M>,
{
    /// Query for the [`ISender<M>`](crate::channel::ISender) interface.
    ///
    /// Returns a sender that can be used to send messages to this actor.
    /// Multiple queries succeed (MPSC channel allows multiple senders).
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<M> + Send + Sync>>() {
            let stored = self.sender_iface.get_or_init(|| {
                let sender = self.channel.sender().expect("MPSC sender creation");
                let arc: Arc<dyn ISender<M> + Send + Sync> = Arc::new(sender);
                Box::new(arc)
            });
            Some(&**stored)
        } else {
            None
        }
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn provided_interfaces(&self) -> &[InterfaceInfo] {
        &self.interface_info
    }

    fn receptacles(&self) -> &[ReceptacleInfo] {
        &[]
    }

    fn connect_receptacle_raw(
        &self,
        _receptacle_name: &str,
        _provider: &dyn IUnknown,
    ) -> Result<(), RegistryError> {
        Err(RegistryError::BindingFailed {
            detail: "actor has no receptacles".into(),
        })
    }
}

/// Spawn a forwarder thread that reads messages from a [`crate::channel::Receiver`] and
/// sends them to an [`ActorHandle`].
///
/// When the channel closes, the actor is deactivated and the thread exits.
/// Returns a [`JoinHandle`] that the caller can join to wait for completion.
///
/// This replaces the common boilerplate pattern of manually spawning a
/// forwarding thread with a recv loop.
///
/// # Examples
///
/// ```
/// use component_core::actor::{Actor, ActorHandler, pipe};
/// use component_core::channel::spsc::SpscChannel;
/// use std::sync::{Arc, Mutex};
///
/// struct Collector { items: Arc<Mutex<Vec<u32>>> }
/// impl ActorHandler<u32> for Collector {
///     fn handle(&mut self, msg: u32) {
///         self.items.lock().unwrap().push(msg);
///     }
/// }
///
/// let items = Arc::new(Mutex::new(Vec::new()));
/// let ch = SpscChannel::<u32>::new(16);
/// let (tx, rx) = ch.split().unwrap();
///
/// let actor = Actor::simple(Collector { items: items.clone() });
/// let handle = actor.activate().unwrap();
///
/// let fwd = pipe(rx, handle);
///
/// tx.send(1).unwrap();
/// tx.send(2).unwrap();
/// drop(tx); // close channel -> pipe deactivates actor
///
/// fwd.join().unwrap();
/// assert_eq!(*items.lock().unwrap(), vec![1, 2]);
/// ```
pub fn pipe<M: Send + 'static>(
    receiver: crate::channel::Receiver<M>,
    handle: ActorHandle<M>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(msg) = receiver.recv() {
            if handle.send(msg).is_err() {
                break;
            }
        }
        let _ = handle.deactivate();
    })
}

/// Spawn a forwarder thread that reads messages from an [`MpscReceiver`]
/// and sends them to an [`ActorHandle`].
///
/// MPSC variant of [`pipe`]. When the channel closes, the actor is
/// deactivated and the thread exits.
///
/// # Examples
///
/// ```
/// use component_core::actor::{Actor, ActorHandler, pipe_mpsc};
/// use component_core::channel::mpsc::MpscChannel;
/// use std::sync::{Arc, Mutex};
///
/// struct Collector { items: Arc<Mutex<Vec<u32>>> }
/// impl ActorHandler<u32> for Collector {
///     fn handle(&mut self, msg: u32) {
///         self.items.lock().unwrap().push(msg);
///     }
/// }
///
/// let items = Arc::new(Mutex::new(Vec::new()));
/// let ch = MpscChannel::<u32>::new(16);
/// let (tx, rx) = ch.split().unwrap();
///
/// let actor = Actor::simple(Collector { items: items.clone() });
/// let handle = actor.activate().unwrap();
///
/// let fwd = pipe_mpsc(rx, handle);
///
/// tx.send(1).unwrap();
/// tx.send(2).unwrap();
/// drop(tx); // close channel -> pipe deactivates actor
///
/// fwd.join().unwrap();
/// assert_eq!(*items.lock().unwrap(), vec![1, 2]);
/// ```
pub fn pipe_mpsc<M: Send + 'static>(
    receiver: crate::channel::mpsc::MpscReceiver<M>,
    handle: ActorHandle<M>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(msg) = receiver.recv() {
            if handle.send(msg).is_err() {
                break;
            }
        }
        let _ = handle.deactivate();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_interface;
    use std::sync::{Arc, Mutex};

    struct CountHandler {
        count: Arc<Mutex<u32>>,
    }

    impl ActorHandler<u32> for CountHandler {
        fn handle(&mut self, _msg: u32) {
            *self.count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn new_creates_idle_actor() {
        let actor = Actor::new(
            CountHandler {
                count: Arc::new(Mutex::new(0)),
            },
            |_| {},
        );
        assert!(!actor.is_active());
    }

    #[test]
    fn activate_returns_handle_and_sets_active() {
        let actor = Actor::new(
            CountHandler {
                count: Arc::new(Mutex::new(0)),
            },
            |_| {},
        );
        let handle = actor.activate().unwrap();
        assert!(actor.is_active());
        handle.deactivate().unwrap();
    }

    #[test]
    fn deactivate_joins_thread_and_sets_idle() {
        let actor = Actor::new(
            CountHandler {
                count: Arc::new(Mutex::new(0)),
            },
            |_| {},
        );
        let handle = actor.activate().unwrap();
        handle.deactivate().unwrap();
        assert!(!actor.is_active());
    }

    #[test]
    fn double_activate_returns_already_active() {
        let count = Arc::new(Mutex::new(0));
        let actor = Actor::new(CountHandler { count }, |_| {});
        let handle = actor.activate().unwrap();
        let result = actor.activate();
        assert_eq!(result.unwrap_err(), ActorError::AlreadyActive);
        handle.deactivate().unwrap();
    }

    #[test]
    fn send_messages_processed_sequentially() {
        let log = Arc::new(Mutex::new(Vec::new()));

        struct OrderHandler {
            log: Arc<Mutex<Vec<u32>>>,
        }

        impl ActorHandler<u32> for OrderHandler {
            fn handle(&mut self, msg: u32) {
                self.log.lock().unwrap().push(msg);
            }
        }

        let actor = Actor::new(OrderHandler { log: log.clone() }, |_| {});
        let handle = actor.activate().unwrap();

        for i in 0..100 {
            handle.send(i).unwrap();
        }

        handle.deactivate().unwrap();

        let log = log.lock().unwrap();
        let expected: Vec<u32> = (0..100).collect();
        assert_eq!(*log, expected);
    }

    #[test]
    fn messages_processed_on_different_thread() {
        let actor_thread_id = Arc::new(Mutex::new(None));

        struct ThreadIdHandler {
            tid: Arc<Mutex<Option<thread::ThreadId>>>,
        }

        impl ActorHandler<()> for ThreadIdHandler {
            fn handle(&mut self, _msg: ()) {
                *self.tid.lock().unwrap() = Some(thread::current().id());
            }
        }

        let actor = Actor::new(
            ThreadIdHandler {
                tid: actor_thread_id.clone(),
            },
            |_| {},
        );
        let handle = actor.activate().unwrap();
        handle.send(()).unwrap();
        handle.deactivate().unwrap();

        let actor_tid = actor_thread_id.lock().unwrap().unwrap();
        assert_ne!(actor_tid, thread::current().id());
    }

    #[test]
    fn panic_recovery_with_error_callback() {
        let panic_count = Arc::new(Mutex::new(0u32));
        let msg_count = Arc::new(Mutex::new(0u32));

        struct PanicHandler {
            msg_count: Arc<Mutex<u32>>,
        }

        impl ActorHandler<u32> for PanicHandler {
            fn handle(&mut self, msg: u32) {
                if msg == 42 {
                    panic!("intentional panic");
                }
                *self.msg_count.lock().unwrap() += 1;
            }
        }

        let panic_count_clone = panic_count.clone();
        let actor = Actor::new(
            PanicHandler {
                msg_count: msg_count.clone(),
            },
            move |_| {
                *panic_count_clone.lock().unwrap() += 1;
            },
        );

        let handle = actor.activate().unwrap();

        handle.send(1).unwrap();
        handle.send(42).unwrap(); // will panic
        handle.send(3).unwrap();

        handle.deactivate().unwrap();

        assert_eq!(*panic_count.lock().unwrap(), 1);
        assert_eq!(*msg_count.lock().unwrap(), 2); // messages 1 and 3
    }

    #[test]
    fn on_start_and_on_stop_called() {
        let started = Arc::new(Mutex::new(false));
        let stopped = Arc::new(Mutex::new(false));

        struct LifecycleHandler {
            started: Arc<Mutex<bool>>,
            stopped: Arc<Mutex<bool>>,
        }

        impl ActorHandler<()> for LifecycleHandler {
            fn handle(&mut self, _msg: ()) {}
            fn on_start(&mut self) {
                *self.started.lock().unwrap() = true;
            }
            fn on_stop(&mut self) {
                *self.stopped.lock().unwrap() = true;
            }
        }

        let actor = Actor::new(
            LifecycleHandler {
                started: started.clone(),
                stopped: stopped.clone(),
            },
            |_| {},
        );

        let handle = actor.activate().unwrap();
        // Give the thread a moment to start
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(*started.lock().unwrap());

        handle.deactivate().unwrap();
        assert!(*stopped.lock().unwrap());
    }

    #[test]
    fn actor_error_display() {
        assert_eq!(
            ActorError::AlreadyActive.to_string(),
            "actor is already active"
        );
        assert_eq!(ActorError::NotActive.to_string(), "actor is not active");
        assert_eq!(
            ActorError::SendFailed("test".into()).to_string(),
            "send failed: test"
        );
        assert_eq!(
            ActorError::ShutdownTimeout.to_string(),
            "actor shutdown timed out"
        );
        assert_eq!(
            ActorError::AffinityFailed("EPERM".into()).to_string(),
            "affinity failed: EPERM"
        );
    }

    #[test]
    fn cpu_affinity_default_is_none() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }
        let actor = Actor::new(Noop, |_| {});
        assert!(actor.cpu_affinity().is_none());
    }

    #[test]
    fn set_cpu_affinity_while_idle() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }
        let actor = Actor::new(Noop, |_| {});
        let cpus = CpuSet::from_cpu(0).unwrap();
        actor.set_cpu_affinity(cpus).unwrap();
        assert!(actor.cpu_affinity().is_some());
    }

    #[test]
    fn set_cpu_affinity_rejected_while_active() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }
        let actor = Actor::new(Noop, |_| {});
        let handle = actor.activate().unwrap();
        let cpus = CpuSet::from_cpu(0).unwrap();
        let err = actor.set_cpu_affinity(cpus).unwrap_err();
        assert_eq!(err, ActorError::AlreadyActive);
        handle.deactivate().unwrap();
    }

    #[test]
    fn with_cpu_affinity_builder() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }
        let actor = Actor::new(Noop, |_| {}).with_cpu_affinity(CpuSet::from_cpu(0).unwrap());
        assert!(actor.cpu_affinity().is_some());
    }

    #[test]
    fn no_affinity_backward_compatible() {
        // Actors without affinity should behave identically to before.
        let count = Arc::new(Mutex::new(0u32));
        let actor = Actor::new(
            CountHandler {
                count: count.clone(),
            },
            |_| {},
        );
        let handle = actor.activate().unwrap();
        handle.send(1).unwrap();
        handle.send(2).unwrap();
        handle.deactivate().unwrap();
        assert_eq!(*count.lock().unwrap(), 2);
    }

    #[test]
    fn iunknown_query_isender() {
        let log = Arc::new(Mutex::new(Vec::new()));

        struct LogHandler {
            log: Arc<Mutex<Vec<u32>>>,
        }
        impl ActorHandler<u32> for LogHandler {
            fn handle(&mut self, msg: u32) {
                self.log.lock().unwrap().push(msg);
            }
        }

        let actor = Actor::new(LogHandler { log: log.clone() }, |_| {});

        // Query ISender before activation
        let sender: Arc<dyn ISender<u32> + Send + Sync> =
            query_interface!(&actor, ISender<u32>).unwrap();

        let handle = actor.activate().unwrap();
        sender.send(10).unwrap();
        sender.send(20).unwrap();
        handle.deactivate().unwrap();

        assert_eq!(*log.lock().unwrap(), vec![10, 20]);
    }

    #[test]
    fn iunknown_isender_and_handle_coexist() {
        let log = Arc::new(Mutex::new(Vec::new()));

        struct LogHandler {
            log: Arc<Mutex<Vec<u32>>>,
        }
        impl ActorHandler<u32> for LogHandler {
            fn handle(&mut self, msg: u32) {
                self.log.lock().unwrap().push(msg);
            }
        }

        let actor = Actor::new(LogHandler { log: log.clone() }, |_| {});

        let sender: Arc<dyn ISender<u32> + Send + Sync> =
            query_interface!(&actor, ISender<u32>).unwrap();

        let handle = actor.activate().unwrap();

        // Both paths can send messages
        handle.send(1).unwrap();
        sender.send(2).unwrap();
        handle.send(3).unwrap();

        handle.deactivate().unwrap();

        let log = log.lock().unwrap();
        assert_eq!(log.len(), 3);
        assert!(log.contains(&1));
        assert!(log.contains(&2));
        assert!(log.contains(&3));
    }

    #[test]
    fn iunknown_provided_interfaces() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }

        let actor = Actor::new(Noop, |_| {});
        let ifaces = actor.provided_interfaces();
        assert_eq!(ifaces.len(), 1);
        assert_eq!(ifaces[0].name, "ISender");
    }

    #[test]
    fn iunknown_version() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }

        let actor = Actor::new(Noop, |_| {});
        assert_eq!(actor.version(), "1.0.0");
    }

    #[test]
    fn iunknown_no_receptacles() {
        struct Noop;
        impl ActorHandler<u32> for Noop {
            fn handle(&mut self, _msg: u32) {}
        }

        let actor = Actor::new(Noop, |_| {});
        assert!(actor.receptacles().is_empty());
    }
}
