//! rtrb SPSC channel component.
//!
//! An [`RtrbChannel`] wraps an `rtrb::RingBuffer` as a component providing
//! [`ISender`] and [`IReceiver`] via
//! [`IUnknown`].
//!
//! SPSC only — only one sender and one receiver are permitted. The rtrb
//! `Producer` and `Consumer` are `Send` but not `Sync`, so they are wrapped
//! in `Mutex` to satisfy the `ISender`/`IReceiver` `Sync` requirement.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::RegistryError;
use crate::interface::{InterfaceInfo, ReceptacleInfo};
use crate::iunknown::IUnknown;

use super::{ChannelError, IReceiver, ISender};

/// Sender wrapper for rtrb channels implementing [`ISender`].
///
/// Wraps `rtrb::Producer<T>` in a `Mutex` because `Producer` is `Send`
/// but not `Sync`.
///
/// # Examples
///
/// ```
/// use component_core::channel::rtrb_spsc::RtrbChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = RtrbChannel::<u32>::new(16);
/// let tx: Arc<dyn ISender<u32> + Send + Sync> =
///     query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
/// let rx = ch.receiver().unwrap();
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct RtrbSender<T> {
    inner: Mutex<rtrb::Producer<T>>,
}

impl<T: Send + 'static> ISender<T> for RtrbSender<T> {
    fn send(&self, value: T) -> Result<(), ChannelError> {
        let mut val = value;
        loop {
            let mut guard = self.inner.lock().unwrap();
            match guard.push(val) {
                Ok(()) => return Ok(()),
                Err(rtrb::PushError::Full(returned)) => {
                    val = returned;
                    drop(guard);
                    std::thread::park_timeout(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    fn try_send(&self, value: T) -> Result<(), ChannelError> {
        let mut guard = self.inner.lock().unwrap();
        guard.push(value).map_err(|_| ChannelError::Full)
    }
}

/// Receiver wrapper for rtrb channels implementing [`IReceiver`].
///
/// Wraps `rtrb::Consumer<T>` in a `Mutex` because `Consumer` is `Send`
/// but not `Sync`.
///
/// # Examples
///
/// ```
/// use component_core::channel::rtrb_spsc::RtrbChannel;
/// use component_core::channel::{IReceiver, ISender};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = RtrbChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
///     query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
/// tx.send(99).unwrap();
/// assert_eq!(rx.recv().unwrap(), 99);
/// ```
pub struct RtrbReceiver<T> {
    inner: Mutex<rtrb::Consumer<T>>,
}

impl<T: Send + 'static> IReceiver<T> for RtrbReceiver<T> {
    fn recv(&self) -> Result<T, ChannelError> {
        loop {
            let mut guard = self.inner.lock().unwrap();
            match guard.pop() {
                Ok(val) => return Ok(val),
                Err(_) => {
                    drop(guard);
                    std::thread::park_timeout(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    fn try_recv(&self) -> Result<T, ChannelError> {
        let mut guard = self.inner.lock().unwrap();
        guard.pop().map_err(|_| ChannelError::Empty)
    }
}

/// rtrb SPSC channel component.
///
/// Wraps an `rtrb::RingBuffer` as a first-class component. SPSC only:
/// only one sender and one receiver are permitted.
///
/// # Examples
///
/// ```
/// use component_core::channel::rtrb_spsc::RtrbChannel;
/// use component_core::channel::{IReceiver, ISender};
///
/// let ch = RtrbChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
///
/// ```
/// use component_core::channel::rtrb_spsc::RtrbChannel;
/// use component_core::channel::ChannelError;
///
/// let ch = RtrbChannel::<u32>::new(4);
/// let _tx = ch.sender().unwrap();
/// // Second sender is rejected (SPSC)
/// assert!(matches!(ch.sender(), Err(ChannelError::BindingRejected { .. })));
/// ```
pub struct RtrbChannel<T: Send + 'static> {
    producer: Mutex<Option<rtrb::Producer<T>>>,
    consumer: Mutex<Option<rtrb::Consumer<T>>>,
    sender_bound: Arc<AtomicBool>,
    receiver_bound: Arc<AtomicBool>,
    sender_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    receiver_iface: OnceLock<Box<dyn Any + Send + Sync>>,
    interface_info: Vec<InterfaceInfo>,
}

impl<T: Send + 'static> RtrbChannel<T> {
    /// Create a new rtrb SPSC channel with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::rtrb_spsc::RtrbChannel;
    ///
    /// let ch = RtrbChannel::<u32>::new(64);
    /// ```
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be greater than zero");
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
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
            producer: Mutex::new(Some(producer)),
            consumer: Mutex::new(Some(consumer)),
            sender_bound: Arc::new(AtomicBool::new(false)),
            receiver_bound: Arc::new(AtomicBool::new(false)),
            sender_iface: OnceLock::new(),
            receiver_iface: OnceLock::new(),
            interface_info,
        }
    }

    /// Create a new rtrb SPSC channel with default capacity (1024).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::rtrb_spsc::RtrbChannel;
    ///
    /// let ch = RtrbChannel::<String>::with_default_capacity();
    /// ```
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Get the sender endpoint. SPSC: only one sender is permitted.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::BindingRejected`] if a sender is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::rtrb_spsc::RtrbChannel;
    ///
    /// let ch = RtrbChannel::<u32>::new(4);
    /// let tx = ch.sender().unwrap();
    /// ```
    pub fn sender(&self) -> Result<RtrbSender<T>, ChannelError> {
        if self
            .sender_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "rtrb SPSC channel already has a sender".into(),
            });
        }
        let producer = self
            .producer
            .lock()
            .unwrap()
            .take()
            .expect("producer already taken");
        Ok(RtrbSender {
            inner: Mutex::new(producer),
        })
    }

    /// Get the receiver endpoint. SPSC: only one receiver is permitted.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::BindingRejected`] if a receiver is already bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::rtrb_spsc::RtrbChannel;
    ///
    /// let ch = RtrbChannel::<u32>::new(4);
    /// let rx = ch.receiver().unwrap();
    /// ```
    pub fn receiver(&self) -> Result<RtrbReceiver<T>, ChannelError> {
        if self
            .receiver_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ChannelError::BindingRejected {
                reason: "rtrb SPSC channel already has a receiver".into(),
            });
        }
        let consumer = self
            .consumer
            .lock()
            .unwrap()
            .take()
            .expect("consumer already taken");
        Ok(RtrbReceiver {
            inner: Mutex::new(consumer),
        })
    }
}

impl<T: Send + 'static> IUnknown for RtrbChannel<T> {
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        if id == TypeId::of::<Arc<dyn ISender<T> + Send + Sync>>() {
            if self
                .sender_bound
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return None;
            }
            let stored = self.sender_iface.get_or_init(|| {
                let producer = self
                    .producer
                    .lock()
                    .unwrap()
                    .take()
                    .expect("producer already taken");
                let sender = RtrbSender {
                    inner: Mutex::new(producer),
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
                let consumer = self
                    .consumer
                    .lock()
                    .unwrap()
                    .take()
                    .expect("consumer already taken");
                let receiver = RtrbReceiver {
                    inner: Mutex::new(consumer),
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
            detail: "rtrb channel has no receptacles".into(),
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
        let ch = RtrbChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        let _rx = ch.receiver().unwrap();
    }

    #[test]
    fn second_sender_rejected() {
        let ch = RtrbChannel::<u32>::new(4);
        let _tx = ch.sender().unwrap();
        assert!(matches!(
            ch.sender(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn second_receiver_rejected() {
        let ch = RtrbChannel::<u32>::new(4);
        let _rx = ch.receiver().unwrap();
        assert!(matches!(
            ch.receiver(),
            Err(ChannelError::BindingRejected { .. })
        ));
    }

    #[test]
    fn send_recv_in_order() {
        let ch = RtrbChannel::<u32>::new(16);
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
        let ch = RtrbChannel::<u32>::new(2);
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
    fn cross_thread_100k_zero_loss() {
        let ch = RtrbChannel::<u32>::new(1024);
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
        let ch = RtrbChannel::<u32>::new(16);
        let tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn iunknown_spsc_rejects_second_sender() {
        let ch = RtrbChannel::<u32>::new(4);
        let _tx: Arc<dyn ISender<u32> + Send + Sync> =
            query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn ISender<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_spsc_rejects_second_receiver() {
        let ch = RtrbChannel::<u32>::new(4);
        let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
            query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
        assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
    }

    #[test]
    fn iunknown_provided_interfaces() {
        let ch = RtrbChannel::<u32>::new(4);
        let ifaces = ch.provided_interfaces();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "ISender");
        assert_eq!(ifaces[1].name, "IReceiver");
    }

    #[test]
    fn iunknown_version() {
        let ch = RtrbChannel::<u32>::new(4);
        assert_eq!(ch.version(), "1.0.0");
    }
}
