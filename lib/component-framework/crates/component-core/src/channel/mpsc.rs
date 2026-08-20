//! Multi-producer, single-consumer (MPSC) channel component.
//!
//! An [`MpscChannel`] allows multiple senders but only one receiver.
//! Internally it uses a lock-free [`MpscRingBuffer`]
//! with per-slot sequence numbers (Vyukov algorithm). Both the producer and
//! consumer paths are lock-free — no `Mutex` is involved in the data path.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::queue::MpscRingBuffer;
use super::{ChannelError, IReceiver, ISender};

/// Shared state between MPSC sender(s) and receiver.
pub(crate) struct MpscState<T> {
    pub(crate) queue: MpscRingBuffer<T>,
    pub(crate) sender_count: AtomicUsize,
    pub(crate) receiver_thread: Mutex<Option<Thread>>,
    pub(crate) sender_thread: Mutex<Option<Thread>>,
    /// Force-close flag set by actor deactivation to ensure the receiver
    /// exits even when other senders (from IUnknown queries) are still alive.
    pub(crate) force_closed: AtomicBool,
}

/// MPSC channel component.
///
/// A first-class component providing multiple sender endpoints and one
/// receiver endpoint. Both sender and receiver paths are lock-free.
///
/// # Examples
///
/// ```
/// use component_core::channel::mpsc::MpscChannel;
///
/// let ch = MpscChannel::<u32>::new(16);
/// let tx1 = ch.sender().unwrap();
/// let tx2 = ch.sender().unwrap(); // MPSC allows multiple senders
/// let rx = ch.receiver().unwrap();
///
/// tx1.send(1).unwrap();
/// tx2.send(2).unwrap();
///
/// let mut msgs = vec![rx.recv().unwrap(), rx.recv().unwrap()];
/// msgs.sort();
/// assert_eq!(msgs, vec![1, 2]);
/// ```
pub struct MpscChannel<T: Send + 'static> {
    state: Arc<MpscState<T>>,
    receiver_bound: Arc<AtomicBool>,
    /// Lazily created sender interface for IUnknown queries.
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    /// Lazily created receiver interface for IUnknown queries.
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    /// Cached interface metadata for introspection.
    interface_info: Vec<InterfaceInfo>,
}

/// Lock-free MPSC sender that pushes directly to the ring buffer via CAS.
///
/// Multiple `MpscSender` instances can coexist and push concurrently
/// without any mutex.
pub struct MpscSender<T: Send + 'static> {
    state: Arc<MpscState<T>>,
}

impl<T: Send + 'static> std::fmt::Debug for MpscSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MpscSender").finish()
    }
}

impl<T: Send + 'static> MpscSender<T> {
    /// Send a message. Blocks if the queue is full.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Closed`] if the receiver has been dropped.
    pub fn send(&self, value: T) -> Result<(), ChannelError> {
        let mut val = value;
        loop {
            match self.state.queue.push(val) {
                Ok(()) => {
                    if let Ok(guard) = self.state.receiver_thread.lock() {
                        if let Some(ref t) = *guard {
                            t.unpark();
                        }
                    }
                    return Ok(());
                }
                Err(returned) => {
                    val = returned;
                    // Park briefly and retry
                    {
                        let mut guard = self.state.sender_thread.lock().unwrap();
                        *guard = Some(thread::current());
                    }
                    thread::park_timeout(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    /// Try to send without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Full`] if the queue is full.
    pub fn try_send(&self, value: T) -> Result<(), ChannelError> {
        match self.state.queue.push(value) {
            Ok(()) => {
                if let Ok(guard) = self.state.receiver_thread.lock() {
                    if let Some(ref t) = *guard {
                        t.unpark();
                    }
                }
                Ok(())
            }
            Err(_returned) => Err(ChannelError::Full),
        }
    }
}

impl<T: Send + 'static> super::ISender<T> for MpscSender<T> {
    fn send(&self, value: T) -> Result<(), super::ChannelError> {
        MpscSender::send(self, value)
    }

    fn try_send(&self, value: T) -> Result<(), super::ChannelError> {
        MpscSender::try_send(self, value)
    }
}

impl<T: Send + 'static> Clone for MpscSender<T> {
    fn clone(&self) -> Self {
        self.state
            .sender_count
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<T: Send + 'static> Drop for MpscSender<T> {
    fn drop(&mut self) {
        let prev = self.state.sender_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            // Last sender dropped — signal closure
            self.state
                .queue
                .sender_alive
                .store(false, Ordering::Release);
            if let Ok(guard) = self.state.receiver_thread.lock() {
                if let Some(ref t) = *guard {
                    t.unpark();
                }
            }
        }
    }
}

// SAFETY: MpscSender uses lock-free CAS on the MpscRingBuffer. The underlying
// state uses atomics and Mutex only for thread parking (not data path).
unsafe impl<T: Send + 'static> Send for MpscSender<T> {}
unsafe impl<T: Send + 'static> Sync for MpscSender<T> {}

/// Receiver endpoint for the MPSC channel.
///
/// Only one `MpscReceiver` exists per channel. When all senders are dropped
/// and the queue is drained, [`recv`](MpscReceiver::recv) returns
/// [`ChannelError::Closed`].
///
/// # Examples
///
/// ```
/// use component_core::channel::mpsc::MpscChannel;
///
/// let ch = MpscChannel::<u32>::new(4);
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx.send(10).unwrap();
/// assert_eq!(rx.recv().unwrap(), 10);
/// ```
pub struct MpscReceiver<T: Send + 'static> {
    state: Arc<MpscState<T>>,
    bound_flag: Option<Arc<AtomicBool>>,
}

impl<T: Send + 'static> std::fmt::Debug for MpscReceiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MpscReceiver").finish()
    }
}

impl<T: Send + 'static> MpscReceiver<T> {
    pub(crate) fn new(state: Arc<MpscState<T>>, bound_flag: Option<Arc<AtomicBool>>) -> Self {
        Self { state, bound_flag }
    }

    /// Receive a message. Blocks if the queue is empty.
    ///
    /// Returns [`ChannelError::Closed`] when all senders are dropped and
    /// the queue is fully drained.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::{MpscChannel};
    /// use component_core::channel::ChannelError;
    ///
    /// let ch = MpscChannel::<u32>::new(4);
    /// let tx = ch.sender().unwrap();
    /// let rx = ch.receiver().unwrap();
    ///
    /// tx.send(42).unwrap();
    /// drop(tx);
    ///
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// assert_eq!(rx.recv().unwrap_err(), ChannelError::Closed);
    /// ```
    pub fn recv(&self) -> Result<T, ChannelError> {
        loop {
            if let Some(value) = self.state.queue.pop() {
                // Wake sender if it's parked
                if let Ok(guard) = self.state.sender_thread.lock() {
                    if let Some(ref t) = *guard {
                        t.unpark();
                    }
                }
                return Ok(value);
            }

            // Queue is empty — check if channel is closed
            let naturally_closed = !self.state.queue.sender_alive.load(Ordering::Acquire)
                && self.state.sender_count.load(Ordering::Acquire) == 0;
            let force_closed = self.state.force_closed.load(Ordering::Acquire);

            if naturally_closed || force_closed {
                // Double-check: try one more pop in case a value was pushed
                // between our pop() and the closed check
                if let Some(value) = self.state.queue.pop() {
                    return Ok(value);
                }
                return Err(ChannelError::Closed);
            }

            // Park until a sender wakes us
            {
                let mut guard = self.state.receiver_thread.lock().unwrap();
                *guard = Some(thread::current());
            }
            thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }

    /// Try to receive without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Empty`] if no messages are available.
    /// Returns [`ChannelError::Closed`] if all senders are dropped and
    /// the queue is empty.
    pub fn try_recv(&self) -> Result<T, ChannelError> {
        if let Some(value) = self.state.queue.pop() {
            // Wake sender if it's parked
            if let Ok(guard) = self.state.sender_thread.lock() {
                if let Some(ref t) = *guard {
                    t.unpark();
                }
            }
            Ok(value)
        } else {
            let naturally_closed = !self.state.queue.sender_alive.load(Ordering::Acquire)
                && self.state.sender_count.load(Ordering::Acquire) == 0;
            let force_closed = self.state.force_closed.load(Ordering::Acquire);

            if naturally_closed || force_closed {
                Err(ChannelError::Closed)
            } else {
                Err(ChannelError::Empty)
            }
        }
    }

    /// Register the current thread for wakeup by senders.
    ///
    /// After calling this, any sender that pushes a message will call
    /// `unpark()` on the registered thread. Used by the actor polling
    /// loop before calling `thread::park_timeout()`.
    pub fn register_for_unpark(&self) {
        *self.state.receiver_thread.lock().unwrap() = Some(thread::current());
    }
}

impl<T: Send + 'static> IReceiver<T> for MpscReceiver<T> {
    fn recv(&self) -> Result<T, ChannelError> {
        MpscReceiver::recv(self)
    }

    fn try_recv(&self) -> Result<T, ChannelError> {
        MpscReceiver::try_recv(self)
    }
}

impl<T: Send + 'static> Drop for MpscReceiver<T> {
    fn drop(&mut self) {
        if let Some(ref flag) = self.bound_flag {
            flag.store(false, Ordering::Release);
        }
    }
}

// SAFETY: MpscReceiver is the sole consumer. The underlying MpscState uses
// atomics for the data path and Mutex only for thread parking.
unsafe impl<T: Send + 'static> Send for MpscReceiver<T> {}
unsafe impl<T: Send + 'static> Sync for MpscReceiver<T> {}

impl<T: Send + 'static> MpscChannel<T> {
    /// Create a new MPSC channel with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// let ch = MpscChannel::<u32>::new(64);
    /// ```
    pub fn new(capacity: usize) -> Self {
        let queue = MpscRingBuffer::new(capacity);
        let state = Arc::new(MpscState {
            queue,
            sender_count: AtomicUsize::new(0),
            receiver_thread: Mutex::new(None),
            sender_thread: Mutex::new(None),
            force_closed: AtomicBool::new(false),
        });

        let interface_info = vec![
            InterfaceInfo {
                type_id: TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>(),
                name: "ISender",
            },
            InterfaceInfo {
                type_id: TypeId::of::<Arc<dyn IReceiver<T> + Send + Sync>>(),
                name: "IReceiver",
            },
        ];

        Self {
            state,
            receiver_bound: Arc::new(AtomicBool::new(false)),
            sender_iface: OnceLock::new(),
            receiver_iface: OnceLock::new(),
            interface_info,
        }
    }

    /// Create a new MPSC channel with default capacity (1024).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// let ch = MpscChannel::<String>::with_default_capacity();
    /// ```
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Create a new MPSC channel intended for use on a specific NUMA node.
    ///
    /// For best NUMA locality, construct the channel on a thread that is
    /// already pinned to CPUs on the target node. The kernel's first-touch
    /// memory policy will then allocate the ring buffer pages on that node.
    ///
    /// The `node` parameter is stored for documentation and debugging
    /// purposes but does not call `set_mempolicy` (which can interfere with
    /// the Rust allocator). Pin the calling thread first with
    /// [`set_thread_affinity`](crate::numa::set_thread_affinity).
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// // For true NUMA locality, call from a thread pinned to the target node.
    /// let ch = MpscChannel::<u64>::new_numa(64, 0);
    /// let tx = ch.sender().unwrap();
    /// let rx = ch.receiver().unwrap();
    /// tx.send(42).unwrap();
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// ```
    pub fn new_numa(capacity: usize, _node: usize) -> Self {
        Self::new(capacity)
    }

    /// Split the channel into a sender and receiver endpoint in one call.
    ///
    /// This is a convenience wrapper around [`sender()`](MpscChannel::sender)
    /// and [`receiver()`](MpscChannel::receiver).
    ///
    /// # Errors
    ///
    /// Returns a [`ChannelError`] if the receiver endpoint is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// let ch = MpscChannel::<u32>::new(16);
    /// let (tx, rx) = ch.split().unwrap();
    ///
    /// tx.send(42).unwrap();
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// ```
    pub fn split(&self) -> Result<(MpscSender<T>, MpscReceiver<T>), ChannelError> {
        let tx = self.sender()?;
        let rx = self.receiver()?;
        Ok((tx, rx))
    }

    /// Force-close the channel, causing the receiver to see [`ChannelError::Closed`]
    /// after draining any remaining messages.
    ///
    /// This is used by actor deactivation to ensure the receiver thread exits
    /// even when other senders (e.g., from IUnknown queries) are still alive.
    pub(crate) fn close(&self) {
        self.state.force_closed.store(true, Ordering::Release);
        // Wake the receiver so it can see the closed signal
        if let Ok(guard) = self.state.receiver_thread.lock() {
            if let Some(ref t) = *guard {
                t.unpark();
            }
        }
    }

    /// Get a sender endpoint. Can be called multiple times (multi-producer).
    ///
    /// Each call creates an independent sender. When all senders are dropped,
    /// the channel closes.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// let ch = MpscChannel::<u32>::new(4);
    /// let tx1 = ch.sender().unwrap();
    /// let tx2 = ch.sender().unwrap(); // multiple senders OK
    /// ```
    pub fn sender(&self) -> Result<MpscSender<T>, ChannelError> {
        self.state.sender_count.fetch_add(1, Ordering::Release);
        Ok(MpscSender {
            state: Arc::clone(&self.state),
        })
    }

    /// Get the receiver endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::BindingRejected`] if a receiver is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::mpsc::MpscChannel;
    ///
    /// let ch = MpscChannel::<u32>::new(4);
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<MpscReceiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "MPSC channel already has a receiver".into(),
            });
        }

        Ok(MpscReceiver::new(
            Arc::clone(&self.state),
            Some(Arc::clone(&self.receiver_bound)),
        ))
    }
}

impl<T: Send + 'static> IUnknown for MpscChannel<T> {
    /// Query for [`ISender`] or [`IReceiver`] interfaces.
    ///
    /// MPSC allows multiple senders: the `ISender` query always succeeds,
    /// returning a shared `MpscSender` that callers can clone. The
    /// `IReceiver` query enforces single-consumer — the second query
    /// returns `None`.
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            // MPSC: multiple senders allowed — always return the stored sender
            let stored = self.sender_iface.get_or_init(|| {
                let sender = MpscSender {
                    state: Arc::clone(&self.state),
                };
                self.state.sender_count.fetch_add(1, Ordering::Release);
                let arc: Arc<dyn ISender<T> + Send + Sync> = Arc::new(sender);
                Box::new(arc)
            });
            Some(&**stored)
        } else if id == TypeId::of::<Arc<dyn IReceiver<T> + Send + Sync>>() {
            // MPSC: only one receiver allowed
            if self
                .receiver_bound
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return None;
            }
            let stored = self.receiver_iface.get_or_init(|| {
                let receiver = MpscReceiver::new(
                    Arc::clone(&self.state),
                    Some(Arc::clone(&self.receiver_bound)),
                );
                let arc: Arc<dyn IReceiver<T> + Send + Sync> = Arc::new(receiver);
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
            detail: "MPSC channel has no receptacles".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iunknown::query;
    use std::thread;

    #[test]
    fn new_creates_channel() {
        let ch = MpscChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn multiple_senders_allowed() {
        let ch = MpscChannel::<u32>::new(4);
        let _tx1 = ch.sender().unwrap();
        let _tx2 = ch.sender().unwrap();
        let _tx3 = ch.sender().unwrap();
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = MpscChannel::<u32>::new(4);
        let _rx = ch.receiver().unwrap();
        let result = ch.receiver();
        assert!(matches!(result, Err(ChannelError::BindingRejected { .. })));
    }

    #[test]
    fn send_recv_single_producer() {
        let ch = MpscChannel::<u32>::new(16);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        for i in 0..10 {
            tx.send(i).unwrap();
        }
        for i in 0..10 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    }

    #[test]
    fn closure_when_all_senders_dropped() {
        let ch = MpscChannel::<u32>::new(4);
        let tx1 = ch.sender().unwrap();
        let tx2 = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        tx1.send(1).unwrap();
        tx2.send(2).unwrap();

        drop(tx1);
        drop(tx2);

        // Drain remaining
        let mut msgs = vec![rx.recv().unwrap(), rx.recv().unwrap()];
        msgs.sort();
        assert_eq!(msgs, vec![1, 2]);

        // Now closed
        assert_eq!(rx.recv().unwrap_err(), ChannelError::Closed);
    }

    #[test]
    fn concurrent_multi_producer() {
        let ch = MpscChannel::<u32>::new(1024);
        let rx = ch.receiver().unwrap();

        let mut handles = vec![];
        for producer_id in 0..8u32 {
            let tx = ch.sender().unwrap();
            handles.push(thread::spawn(move || {
                for i in 0..1000u32 {
                    tx.send(producer_id * 1000 + i).unwrap();
                }
            }));
        }

        let consumer = thread::spawn(move || {
            let mut count = 0u32;
            for _ in 0..8000 {
                let _ = rx.recv().unwrap();
                count += 1;
            }
            count
        });

        for h in handles {
            h.join().unwrap();
        }

        let count = consumer.join().unwrap();
        assert_eq!(count, 8000);
    }

    #[test]
    fn concurrent_8_producers_10k_each_zero_loss() {
        let ch = MpscChannel::<u64>::new(4096);
        let rx = ch.receiver().unwrap();

        let mut handles = vec![];
        for pid in 0..8u64 {
            let tx = ch.sender().unwrap();
            handles.push(thread::spawn(move || {
                for i in 0..10_000u64 {
                    tx.send(pid * 10_000 + i).unwrap();
                }
            }));
        }

        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(80_000);
            loop {
                match rx.recv() {
                    Ok(val) => received.push(val),
                    Err(ChannelError::Closed) => break,
                    Err(e) => panic!("unexpected error: {e:?}"),
                }
            }
            received
        });

        for h in handles {
            h.join().unwrap();
        }
        // Drop all sender copies from channel itself
        drop(ch);

        let received = consumer.join().unwrap();
        assert_eq!(
            received.len(),
            80_000,
            "expected 80000 messages, got {}",
            received.len()
        );
    }

    #[test]
    fn receiver_disconnect_frees_slot() {
        let ch = MpscChannel::<u32>::new(4);
        {
            let _rx = ch.receiver().unwrap();
        }
        let _rx2 = ch.receiver().unwrap();
    }

    #[test]
    fn iunknown_query_isender_ireceiver() {
        let ch = MpscChannel::<u32>::new(16);
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();

        tx.send(10).unwrap();
        assert_eq!(rx.recv().unwrap(), 10);
    }

    #[test]
    fn iunknown_mpsc_sender_query_always_succeeds() {
        let ch = MpscChannel::<u32>::new(4);
        // Multiple ISender queries return the same Arc (MPSC allows multi-producer)
        let _tx1: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let _tx2: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
    }

    #[test]
    fn iunknown_mpsc_rejects_second_receiver_query() {
        let ch = MpscChannel::<u32>::new(4);
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = MpscChannel::<u32>::new(4);
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }
}
