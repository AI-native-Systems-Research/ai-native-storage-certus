//! Crossbeam bounded channel component.
//!
//! A [`CrossbeamBoundedChannel`] wraps a `crossbeam_channel::bounded` channel as
//! a component providing [`ISender`] and
//! [`IReceiver`] via [`IUnknown`].
//!
//! Supports both SPSC and MPSC topologies. Multiple senders are allowed
//! (sender is `Clone`); only one receiver is permitted.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::{ChannelError, IReceiver, ISender};

/// Sender wrapper for crossbeam bounded channels implementing [`ISender`].
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = CrossbeamBoundedChannel::<u32>::new(16);
/// let tx: Arc<dyn ISender<u32> + Send + Sync> =
///     query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
/// let rx = ch.receiver().unwrap();
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct CrossbeamSender<T> {
    inner: crossbeam_channel::Sender<T>,
}

impl<T: Send + 'static> ISender<T> for CrossbeamSender<T> {
    fn send(&self, value: T) -> Result<(), ChannelError> {
        self.inner.send(value).map_err(|_| ChannelError::Closed)
    }

    fn try_send(&self, value: T) -> Result<(), ChannelError> {
        self.inner.try_send(value).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => ChannelError::Full,
            crossbeam_channel::TrySendError::Disconnected(_) => ChannelError::Closed,
        })
    }
}

// SAFETY: crossbeam_channel::Sender is Send + Sync when T: Send
unsafe impl<T: Send> Send for CrossbeamSender<T> {}
unsafe impl<T: Send> Sync for CrossbeamSender<T> {}

/// Receiver wrapper for crossbeam bounded channels implementing [`IReceiver`].
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = CrossbeamBoundedChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
///     query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct CrossbeamReceiver<T> {
    inner: crossbeam_channel::Receiver<T>,
}

impl<T: Send + 'static> IReceiver<T> for CrossbeamReceiver<T> {
    fn recv(&self) -> Result<T, ChannelError> {
        self.inner.recv().map_err(|_| ChannelError::Closed)
    }

    fn try_recv(&self) -> Result<T, ChannelError> {
        self.inner.try_recv().map_err(|e| match e {
            crossbeam_channel::TryRecvError::Empty => ChannelError::Empty,
            crossbeam_channel::TryRecvError::Disconnected => ChannelError::Closed,
        })
    }
}

// SAFETY: crossbeam_channel::Receiver is Send + Sync when T: Send
unsafe impl<T: Send> Send for CrossbeamReceiver<T> {}
unsafe impl<T: Send> Sync for CrossbeamReceiver<T> {}

/// Crossbeam bounded channel component.
///
/// Wraps a `crossbeam_channel::bounded` channel as a first-class component.
/// Supports MPSC topology: multiple senders allowed, single receiver enforced.
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
/// use component_core::channel::{IReceiver, ISender};
///
/// let ch = CrossbeamBoundedChannel::<u32>::new(16);
/// let tx1 = ch.sender().unwrap();
/// let tx2 = ch.sender().unwrap(); // multiple senders OK
/// let rx = ch.receiver().unwrap();
///
/// tx1.send(1).unwrap();
/// tx2.send(2).unwrap();
/// let mut msgs = vec![rx.recv().unwrap(), rx.recv().unwrap()];
/// msgs.sort();
/// assert_eq!(msgs, vec![1, 2]);
/// ```
///
/// ```
/// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
/// use component_core::channel::ChannelError;
///
/// let ch = CrossbeamBoundedChannel::<u32>::new(4);
/// let _rx = ch.receiver().unwrap();
/// // Second receiver is rejected
/// assert!(matches!(ch.receiver(), Err(ChannelError::BindingRejected { .. })));
/// ```
pub struct CrossbeamBoundedChannel<T: Send + 'static> {
    tx: crossbeam_channel::Sender<T>,
    rx: crossbeam_channel::Receiver<T>,
    receiver_bound: Arc<AtomicBool>,
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    interface_info: Vec<InterfaceInfo>,
}

impl<T: Send + 'static> CrossbeamBoundedChannel<T> {
    /// Create a new crossbeam bounded channel with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
    ///
    /// let ch = CrossbeamBoundedChannel::<u32>::new(64);
    /// ```
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be greater than zero");
        let (tx, rx) = crossbeam_channel::bounded(capacity);
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
            tx,
            rx,
            receiver_bound: Arc::new(AtomicBool::new(false)),
            sender_iface: OnceLock::new(),
            receiver_iface: OnceLock::new(),
            interface_info,
        }
    }

    /// Create a new crossbeam bounded channel with default capacity (1024).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
    ///
    /// let ch = CrossbeamBoundedChannel::<String>::with_default_capacity();
    /// ```
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Get a sender endpoint. Can be called multiple times (multi-producer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
    ///
    /// let ch = CrossbeamBoundedChannel::<u32>::new(4);
    /// let tx1 = ch.sender().unwrap();
    /// let tx2 = ch.sender().unwrap();
    /// ```
    pub fn sender(&self) -> Result<CrossbeamSender<T>, ChannelError> {
        Ok(CrossbeamSender {
            inner: self.tx.clone(),
        })
    }

    /// Get the receiver endpoint. Only one receiver is permitted.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::BindingRejected`] if a receiver is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_bounded::CrossbeamBoundedChannel;
    ///
    /// let ch = CrossbeamBoundedChannel::<u32>::new(4);
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<CrossbeamReceiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "crossbeam bounded channel already has a receiver".into(),
            });
        }
        Ok(CrossbeamReceiver {
            inner: self.rx.clone(),
        })
    }
}

impl<T: Send + 'static> IUnknown for CrossbeamBoundedChannel<T> {
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            let stored = self.sender_iface.get_or_init(|| {
                let sender = CrossbeamSender {
                    inner: self.tx.clone(),
                };
                let arc: Arc<dyn ISender<T> + Send + Sync> = Arc::new(sender);
                Box::new(arc)
            });
            Some(&**stored)
        } else if id == TypeId::of::<Arc<dyn IReceiver<T> + Send + Sync>>() {
            if self
                .receiver_bound
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return None;
            }
            let stored = self.receiver_iface.get_or_init(|| {
                let receiver = CrossbeamReceiver {
                    inner: self.rx.clone(),
                };
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
            detail: "crossbeam bounded channel has no receptacles".into(),
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
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn multiple_senders_allowed() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let _tx1 = ch.sender().unwrap();
        let _tx2 = ch.sender().unwrap();
        let _tx3 = ch.sender().unwrap();
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let _rx = ch.receiver().unwrap();
        assert!(matches!(
            ch.receiver(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn send_recv_in_order() {
        let ch = CrossbeamBoundedChannel::<u32>::new(16);
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
        let ch = CrossbeamBoundedChannel::<u32>::new(2);
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
    fn closure_when_sender_dropped() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let tx = ch.sender().unwrap();
        let rx = ch.receiver().unwrap();
        tx.send(1).unwrap();
        drop(tx);
        drop(ch);
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap_err(), ChannelError::Closed);
    }

    #[test]
    fn cross_thread_100k_zero_loss() {
        let ch = CrossbeamBoundedChannel::<u32>::new(1024);
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
        assert_eq!(consumer.join().unwrap(), 100_000);
    }

    #[test]
    fn mpsc_concurrent() {
        let ch = CrossbeamBoundedChannel::<u32>::new(1024);
        let rx = ch.receiver().unwrap();
        let mut handles = vec![];
        for pid in 0..4u32 {
            let tx = ch.sender().unwrap();
            handles.push(thread::spawn(move || {
                for i in 0..1000u32 {
                    tx.send(pid * 1000 + i).unwrap();
                }
            }));
        }
        let consumer = thread::spawn(move || {
            let mut count = 0;
            for _ in 0..4000 {
                let _ = rx.recv().unwrap();
                count += 1;
            }
            count
        });
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(consumer.join().unwrap(), 4000);
    }

    #[test]
    fn iunknown_query_isender_ireceiver() {
        let ch = CrossbeamBoundedChannel::<u32>::new(16);
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn iunknown_sender_always_succeeds() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let _tx1: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let _tx2: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
    }

    #[test]
    fn iunknown_rejects_second_receiver() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }

    #[test]
    fn iunknown_version() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        assert_eq!(ch.version(), "1.0.0");
    }

    #[test]
    fn iunknown_no_receptacles() {
        let ch = CrossbeamBoundedChannel::<u32>::new(4);
        assert!(ch.receptacles().is_empty());
    }
}
