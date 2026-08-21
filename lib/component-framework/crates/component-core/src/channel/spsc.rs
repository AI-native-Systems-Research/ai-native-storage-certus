//! Single-producer, single-consumer (SPSC) channel component.
//!
//! An [`SpscChannel`] is a first-class component that provides exactly one
//! [`Sender`] and one [`Receiver`] endpoint, backed by a lock-free ring buffer.
//!
//! Binding constraints: only one sender and one receiver are permitted.
//! Attempting to obtain a second sender or receiver returns
//! [`ChannelError::BindingRejected`].

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::queue::RingBuffer;
use super::{ChannelError, ChannelState, IReceiver, ISender, Receiver, Sender};

/// SPSC channel component.
///
/// A first-class component providing one sender and one receiver endpoint
/// backed by a lock-free ring buffer. Enforces single-producer,
/// single-consumer topology at bind time.
///
/// # Examples
///
/// ```
/// use component_core::channel::spsc::SpscChannel;
///
/// let ch = SpscChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
///
/// ```
/// use component_core::channel::spsc::SpscChannel;
/// use component_core::channel::ChannelError;
///
/// let ch = SpscChannel::<u32>::new(4);
/// let _tx = ch.sender().unwrap();
///
/// // Second sender is rejected
/// let err = ch.sender().unwrap_err();
/// assert!(matches!(err, ChannelError::BindingRejected { .. }));
/// ```
pub struct SpscChannel<T: Send + 'static> {
    state: Arc<ChannelState<T>>,
    sender_bound: Arc<AtomicBool>,
    receiver_bound: Arc<AtomicBool>,
    /// Lazily created sender interface for IUnknown queries.
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    /// Lazily created receiver interface for IUnknown queries.
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    /// Cached interface metadata for introspection.
    interface_info: Vec<InterfaceInfo>,
}

impl<T: Send + 'static> SpscChannel<T> {
    /// Create a new SPSC channel with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// let ch = SpscChannel::<u32>::new(64);
    /// ```
    pub fn new(capacity: usize) -> Self {
        let queue = RingBuffer::new(capacity);
        let state = Arc::new(ChannelState {
            queue,
            sender_count: std::sync::atomic::AtomicUsize::new(0),
            receiver_thread: std::sync::Mutex::new(None),
            sender_thread: std::sync::Mutex::new(None),
            receiver_parked: std::sync::atomic::AtomicBool::new(false),
            sender_parked: std::sync::atomic::AtomicBool::new(false),
            force_closed: std::sync::atomic::AtomicBool::new(false),
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
            sender_bound: Arc::new(AtomicBool::new(false)),
            receiver_bound: Arc::new(AtomicBool::new(false)),
            sender_iface: OnceLock::new(),
            receiver_iface: OnceLock::new(),
            interface_info,
        }
    }

    /// Create a new SPSC channel with default capacity (1024).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// let ch = SpscChannel::<String>::with_default_capacity();
    /// ```
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Create a new SPSC channel intended for use on a specific NUMA node.
    ///
    /// NUMA locality is achieved via Linux's **first-touch policy**: pin the
    /// calling thread to CPUs on the target NUMA node *before* calling this
    /// constructor, and the kernel will allocate the ring buffer pages on that
    /// node. The `node` parameter is stored for informational/benchmark
    /// labelling purposes only.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// // Pin thread to NUMA-local CPUs first, then construct:
    /// let ch = SpscChannel::<u64>::new_numa(64, 0);
    /// let tx = ch.sender().unwrap();
    /// let rx = ch.receiver().unwrap();
    /// tx.send(42).unwrap();
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// ```
    pub fn new_numa(capacity: usize, _node: usize) -> Self {
        Self::new(capacity)
    }

    /// Split the channel into its sender and receiver endpoints in one call.
    ///
    /// This is a convenience wrapper around [`sender()`](SpscChannel::sender)
    /// and [`receiver()`](SpscChannel::receiver).
    ///
    /// # Errors
    ///
    /// Returns a [`ChannelError`] if either endpoint is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// let ch = SpscChannel::<u32>::new(16);
    /// let (tx, rx) = ch.split().unwrap();
    ///
    /// tx.send(42).unwrap();
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// ```
    pub fn split(&self) -> Result<(Sender<T>, Receiver<T>), ChannelError> {
        let tx = self.sender()?;
        let rx = self.receiver()?;
        Ok((tx, rx))
    }

    /// Get the sender endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::BindingRejected`] if a sender is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// let ch = SpscChannel::<u32>::new(4);
    /// let tx = ch.sender().unwrap();
    /// ```
    pub fn sender(&self) -> Result<Sender<T>, ChannelError> {
        // CAS: false -> true
        if self
            .sender_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "SPSC channel already has a sender".into(),
            });
        }

        Ok(Sender::new(
            Arc::clone(&self.state),
            Some(Arc::clone(&self.sender_bound)),
        ))
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
    /// use component_core::channel::spsc::SpscChannel;
    ///
    /// let ch = SpscChannel::<u32>::new(4);
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<Receiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "SPSC channel already has a receiver".into(),
            });
        }

        Ok(Receiver::new(
            Arc::clone(&self.state),
            Some(Arc::clone(&self.receiver_bound)),
        ))
    }
}

impl<T: Send + 'static> IUnknown for SpscChannel<T> {
    /// Query for [`ISender`] or [`IReceiver`] interfaces.
    ///
    /// SPSC binding enforcement applies: the first successful query for
    /// `ISender` or `IReceiver` claims that endpoint. Subsequent queries
    /// for the same endpoint return `None`.
    ///
    /// The CAS flags are shared with [`sender()`](SpscChannel::sender) and
    /// [`receiver()`](SpscChannel::receiver), so the direct API and the
    /// `IUnknown` query path are mutually exclusive.
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            // SPSC: only one sender allowed
            if self
                .sender_bound
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return None;
            }
            let stored = self.sender_iface.get_or_init(|| {
                let sender = Sender::new(
                    Arc::clone(&self.state),
                    Some(Arc::clone(&self.sender_bound)),
                );
                let arc: Arc<dyn ISender<T> + Send + Sync> = Arc::new(sender);
                Box::new(arc)
            });
            Some(&**stored)
        } else if id == TypeId::of::<Arc<dyn IReceiver<T> + Send + Sync>>() {
            // SPSC: only one receiver allowed
            if self
                .receiver_bound
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return None;
            }
            let stored = self.receiver_iface.get_or_init(|| {
                let receiver = Receiver::new(
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
            detail: "SPSC channel has no receptacles".into(),
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
        let ch = SpscChannel::<u32>::new(4);
        // Can get sender and receiver
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn default_capacity() {
        let ch = SpscChannel::<u32>::with_default_capacity();
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn send_recv_in_order() {
        let ch = SpscChannel::<u32>::new(16);
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
    fn try_send_try_recv() {
        let ch = SpscChannel::<u32>::new(2);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_ok());
        assert_eq!(tx.try_send(3).unwrap_err(), ChannelError::Full);

        assert_eq!(rx.try_recv().unwrap(), 1);
        assert_eq!(rx.try_recv().unwrap(), 2);
        assert_eq!(rx.try_recv().unwrap_err(), ChannelError::Empty);
    }

    #[test]
    fn second_sender_rejected() {
        let ch = SpscChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        let result = ch.sender();
        assert!(matches!(result, Err(ChannelError::BindingRejected { .. })));
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = SpscChannel::<u32>::new(4);
        let _rx = ch.receiver().unwrap();
        let result = ch.receiver();
        assert!(matches!(result, Err(ChannelError::BindingRejected { .. })));
    }

    #[test]
    fn sender_disconnect_frees_slot() {
        let ch = SpscChannel::<u32>::new(4);
        {
            let _tx = ch.sender().unwrap();
            // tx drops here
        }
        // Slot should be available again
        let _tx2 = ch.sender().unwrap();
    }

    #[test]
    fn receiver_disconnect_frees_slot() {
        let ch = SpscChannel::<u32>::new(4);
        {
            let _rx = ch.receiver().unwrap();
        }
        let _rx2 = ch.receiver().unwrap();
    }

    #[test]
    fn closure_when_sender_dropped() {
        let ch = SpscChannel::<u32>::new(4);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);

        // Can still drain existing messages
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap(), 2);
        // Then closed
        assert_eq!(rx.recv().unwrap_err(), ChannelError::Closed);
    }

    #[test]
    fn cross_thread_send_recv() {
        let ch = SpscChannel::<u64>::new(1024);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        let producer = thread::spawn(move || {
            for i in 0..1000u64 {
                tx.send(i).unwrap();
            }
        });

        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(1000);
            for _ in 0..1000 {
                received.push(rx.recv().unwrap());
            }
            received
        });

        producer.join().unwrap();
        let received = consumer.join().unwrap();

        let expected: Vec<u64> = (0..1000).collect();
        assert_eq!(received, expected);
    }

    #[test]
    fn sequential_100k_messages_zero_loss() {
        let ch = SpscChannel::<u32>::new(1024);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();

        let producer = thread::spawn(move || {
            for i in 0..100_000u32 {
                tx.send(i).unwrap();
            }
        });

        let consumer = thread::spawn(move || {
            let mut count = 0u32;
            for _ in 0..100_000 {
                let val = rx.recv().unwrap();
                assert_eq!(val, count);
                count += 1;
            }
            count
        });

        producer.join().unwrap();
        let count = consumer.join().unwrap();
        assert_eq!(count, 100_000);
    }

    #[test]
    fn iunknown_query_isender_ireceiver() {
        let ch = SpscChannel::<u32>::new(16);
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();

        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn iunknown_spsc_rejects_second_sender_query() {
        let ch = SpscChannel::<u32>::new(4);
        let _tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        // Second query should fail
        assert!(query::<dyn ISender<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_spsc_rejects_second_receiver_query() {
        let ch = SpscChannel::<u32>::new(4);
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_and_direct_api_mutually_exclusive() {
        // If sender() was called, query for ISender returns None
        let ch = SpscChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        assert!(query::<dyn ISender<u32> + Send + Sync>(&ch).is_none());

        // If query was called first, sender() fails
        let ch2 = SpscChannel::<u32>::new(4);
        let _tx2: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch2).unwrap();
        assert!(matches!(
            ch2.sender(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = SpscChannel::<u32>::new(4);
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }

    #[test]
    fn iunknown_version() {
        let ch = SpscChannel::<u32>::new(4);
        assert_eq!(ch.version(), "1.0.0");
    }

    #[test]
    fn iunknown_no_receptacles() {
        let ch = SpscChannel::<u32>::new(4);
        assert!(ch.receptacles().is_empty());
    }
}
