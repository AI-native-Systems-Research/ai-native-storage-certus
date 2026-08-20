//! Crossbeam unbounded channel component.
//!
//! A [`CrossbeamUnboundedChannel`] wraps a `crossbeam_channel::unbounded`
//! channel as a component providing [`ISender`] and
//! [`IReceiver`] via [`IUnknown`].
//!
//! Supports MPSC topology. Send never blocks (unbounded capacity).
//! `try_send` always succeeds.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::{ChannelError, IReceiver, ISender};

/// Sender wrapper for crossbeam unbounded channels implementing [`ISender`].
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = CrossbeamUnboundedChannel::<u32>::new();
/// let tx: Arc<dyn ISender<u32> + Send + Sync> =
///     query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
/// let rx = ch.receiver().unwrap();
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct CrossbeamUnboundedSender<T> {
    inner: crossbeam_channel::Sender<T>,
}

impl<T: Send + 'static> ISender<T> for CrossbeamUnboundedSender<T> {
    fn send(&self, value: T) -> Result<(), ChannelError> {
        self.inner.send(value).map_err(|_| ChannelError::Closed)
    }

    fn try_send(&self, value: T) -> Result<(), ChannelError> {
        // Unbounded: try_send behaves like send (never full)
        self.inner.send(value).map_err(|_| ChannelError::Closed)
    }
}

unsafe impl<T: Send> Send for CrossbeamUnboundedSender<T> {}
unsafe impl<T: Send> Sync for CrossbeamUnboundedSender<T> {}

/// Receiver wrapper for crossbeam unbounded channels implementing [`IReceiver`].
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = CrossbeamUnboundedChannel::<u32>::new();
/// let tx = ch.sender().unwrap();
/// let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
///     query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
/// tx.send(99).unwrap();
/// assert_eq!(rx.recv().unwrap(), 99);
/// ```
pub struct CrossbeamUnboundedReceiver<T> {
    inner: crossbeam_channel::Receiver<T>,
}

impl<T: Send + 'static> IReceiver<T> for CrossbeamUnboundedReceiver<T> {
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

unsafe impl<T: Send> Send for CrossbeamUnboundedReceiver<T> {}
unsafe impl<T: Send> Sync for CrossbeamUnboundedReceiver<T> {}

/// Crossbeam unbounded channel component.
///
/// Wraps a `crossbeam_channel::unbounded` channel. Send never blocks.
/// Supports MPSC topology: multiple senders allowed, single receiver enforced.
///
/// # Examples
///
/// ```
/// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
/// use component_core::channel::{IReceiver, ISender};
///
/// let ch = CrossbeamUnboundedChannel::<u32>::new();
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// // Send never blocks — unbounded capacity
/// for i in 0..10000 {
///     tx.send(i).unwrap();
/// }
/// for i in 0..10000 {
///     assert_eq!(rx.recv().unwrap(), i);
/// }
/// ```
pub struct CrossbeamUnboundedChannel<T: Send + 'static> {
    tx: crossbeam_channel::Sender<T>,
    rx: crossbeam_channel::Receiver<T>,
    receiver_bound: Arc<AtomicBool>,
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    interface_info: Vec<InterfaceInfo>,
}

impl<T: Send + 'static> CrossbeamUnboundedChannel<T> {
    /// Create a new crossbeam unbounded channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
    ///
    /// let ch = CrossbeamUnboundedChannel::<u32>::new();
    /// ```
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
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

    /// Get a sender endpoint. Can be called multiple times (multi-producer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
    ///
    /// let ch = CrossbeamUnboundedChannel::<u32>::new();
    /// let tx1 = ch.sender().unwrap();
    /// let tx2 = ch.sender().unwrap();
    /// ```
    pub fn sender(&self) -> Result<CrossbeamUnboundedSender<T>, ChannelError> {
        Ok(CrossbeamUnboundedSender {
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
    /// use component_core::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
    ///
    /// let ch = CrossbeamUnboundedChannel::<u32>::new();
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<CrossbeamUnboundedReceiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "crossbeam unbounded channel already has a receiver".into(),
            });
        }
        Ok(CrossbeamUnboundedReceiver {
            inner: self.rx.clone(),
        })
    }
}

impl<T: Send + 'static> Default for CrossbeamUnboundedChannel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> IUnknown for CrossbeamUnboundedChannel<T> {
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            let stored = self.sender_iface.get_or_init(|| {
                let sender = CrossbeamUnboundedSender {
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
                let receiver = CrossbeamUnboundedReceiver {
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
            detail: "crossbeam unbounded channel has no receptacles".into(),
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
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn default_creates_channel() {
        let ch = CrossbeamUnboundedChannel::<u32>::default();
        let _tx = ch.sender().unwrap();
    }

    #[test]
    fn send_never_blocks() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let tx = ch.sender().unwrap();
        // Send 10000 messages without blocking
        for i in 0..10_000 {
            tx.try_send(i).unwrap();
        }
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let _rx = ch.receiver().unwrap();
        assert!(matches!(
            ch.receiver(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn send_recv_in_order() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
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
    fn closure_when_sender_dropped() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
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
        let ch = CrossbeamUnboundedChannel::<u32>::new();
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
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn iunknown_rejects_second_receiver() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }

    #[test]
    fn iunknown_version() {
        let ch = CrossbeamUnboundedChannel::<u32>::new();
        assert_eq!(ch.version(), "1.0.0");
    }
}
