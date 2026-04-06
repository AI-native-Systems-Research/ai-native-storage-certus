//! Kanal bounded channel component.
//!
//! A [`KanalChannel`] wraps a `kanal::bounded` channel as a component
//! providing [`ISender`] and [`IReceiver`]
//! via [`IUnknown`].
//!
//! Supports MPSC topology. Multiple senders allowed, single receiver enforced.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::{ChannelError, IReceiver, ISender};

/// Sender wrapper for kanal channels implementing [`ISender`].
///
/// # Examples
///
/// ```
/// use component_core::channel::kanal_bounded::KanalChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = KanalChannel::<u32>::new(16);
/// let tx: Arc<dyn ISender<u32> + Send + Sync> =
///     query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
/// let rx = ch.receiver().unwrap();
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct KanalSender<T> {
    inner: kanal::Sender<T>,
}

impl<T: Send + 'static> ISender<T> for KanalSender<T> {
    fn send(&self, value: T) -> Result<(), ChannelError> {
        self.inner.send(value).map_err(|_| ChannelError::Closed)
    }

    fn try_send(&self, value: T) -> Result<(), ChannelError> {
        match self.inner.try_send(value) {
            Ok(true) => Ok(()),
            Ok(false) => Err(ChannelError::Full),
            Err(_) => Err(ChannelError::Closed),
        }
    }
}

unsafe impl<T: Send> Send for KanalSender<T> {}
unsafe impl<T: Send> Sync for KanalSender<T> {}

/// Receiver wrapper for kanal channels implementing [`IReceiver`].
///
/// # Examples
///
/// ```
/// use component_core::channel::kanal_bounded::KanalChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = KanalChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
///     query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
/// tx.send(99).unwrap();
/// assert_eq!(rx.recv().unwrap(), 99);
/// ```
pub struct KanalReceiver<T> {
    inner: kanal::Receiver<T>,
}

impl<T: Send + 'static> IReceiver<T> for KanalReceiver<T> {
    fn recv(&self) -> Result<T, ChannelError> {
        self.inner.recv().map_err(|_| ChannelError::Closed)
    }

    fn try_recv(&self) -> Result<T, ChannelError> {
        match self.inner.try_recv() {
            Ok(Some(val)) => Ok(val),
            Ok(None) => Err(ChannelError::Empty),
            Err(_) => Err(ChannelError::Closed),
        }
    }
}

unsafe impl<T: Send> Send for KanalReceiver<T> {}
unsafe impl<T: Send> Sync for KanalReceiver<T> {}

/// Kanal bounded channel component.
///
/// Wraps a `kanal::bounded` channel as a first-class component.
/// Supports MPSC topology: multiple senders allowed, single receiver enforced.
///
/// # Examples
///
/// ```
/// use component_core::channel::kanal_bounded::KanalChannel;
/// use component_core::channel::{IReceiver, ISender};
///
/// let ch = KanalChannel::<u32>::new(16);
/// let tx1 = ch.sender().unwrap();
/// let tx2 = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx1.send(1).unwrap();
/// tx2.send(2).unwrap();
/// let mut msgs = vec![rx.recv().unwrap(), rx.recv().unwrap()];
/// msgs.sort();
/// assert_eq!(msgs, vec![1, 2]);
/// ```
pub struct KanalChannel<T: Send + 'static> {
    tx: kanal::Sender<T>,
    rx: kanal::Receiver<T>,
    receiver_bound: Arc<AtomicBool>,
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    interface_info: Vec<InterfaceInfo>,
}

impl<T: Send + 'static> KanalChannel<T> {
    /// Create a new kanal bounded channel with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::kanal_bounded::KanalChannel;
    ///
    /// let ch = KanalChannel::<u32>::new(64);
    /// ```
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be greater than zero");
        let (tx, rx) = kanal::bounded(capacity);
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

    /// Create a new kanal bounded channel with default capacity (1024).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::kanal_bounded::KanalChannel;
    ///
    /// let ch = KanalChannel::<String>::with_default_capacity();
    /// ```
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Get a sender endpoint. Can be called multiple times (multi-producer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::kanal_bounded::KanalChannel;
    ///
    /// let ch = KanalChannel::<u32>::new(4);
    /// let tx1 = ch.sender().unwrap();
    /// let tx2 = ch.sender().unwrap();
    /// ```
    pub fn sender(&self) -> Result<KanalSender<T>, ChannelError> {
        Ok(KanalSender {
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
    /// use component_core::channel::kanal_bounded::KanalChannel;
    ///
    /// let ch = KanalChannel::<u32>::new(4);
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<KanalReceiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "kanal channel already has a receiver".into(),
            });
        }
        Ok(KanalReceiver {
            inner: self.rx.clone(),
        })
    }
}

impl<T: Send + 'static> IUnknown for KanalChannel<T> {
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            let stored = self.sender_iface.get_or_init(|| {
                let sender = KanalSender {
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
                let receiver = KanalReceiver {
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
            detail: "kanal channel has no receptacles".into(),
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
        let ch = KanalChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn multiple_senders_allowed() {
        let ch = KanalChannel::<u32>::new(4);
        let _tx1 = ch.sender().unwrap();
        let _tx2 = ch.sender().unwrap();
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = KanalChannel::<u32>::new(4);
        let _rx = ch.receiver().unwrap();
        assert!(matches!(
            ch.receiver(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn send_recv_in_order() {
        let ch = KanalChannel::<u32>::new(16);
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
        let ch = KanalChannel::<u32>::new(2);
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
        let ch = KanalChannel::<u32>::new(4);
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
        let ch = KanalChannel::<u32>::new(1024);
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
    fn iunknown_query_isender_ireceiver() {
        let ch = KanalChannel::<u32>::new(16);
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn iunknown_rejects_second_receiver() {
        let ch = KanalChannel::<u32>::new(4);
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = KanalChannel::<u32>::new(4);
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }

    #[test]
    fn iunknown_version() {
        let ch = KanalChannel::<u32>::new(4);
        assert_eq!(ch.version(), "1.0.0");
    }
}
