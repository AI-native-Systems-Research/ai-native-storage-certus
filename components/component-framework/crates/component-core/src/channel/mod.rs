//! Channel components — first-class SPSC and MPSC message-passing primitives.
//!
//! Channels are components that provide [`Sender`] and [`Receiver`] endpoints
//! for typed, lock-free message passing between actors or threads. Two channel
//! types are provided:
//!
//! - [`SpscChannel`] — single-producer, single-consumer
//! - [`mpsc::MpscChannel`] — multi-producer, single-consumer
//!
//! Both enforce topology constraints at bind time: SPSC rejects a second
//! sender or receiver, while MPSC allows multiple senders but only one
//! receiver.

pub mod crossbeam_bounded;
pub mod crossbeam_unbounded;
pub mod kanal_bounded;
pub mod mpsc;
pub mod queue;
pub mod rtrb_spsc;
pub mod spsc;
pub mod tokio_mpsc;

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};

use crate::interface::Interface;
use queue::RingBuffer;

pub use spsc::SpscChannel;

/// Interface trait for sending typed messages through a channel.
///
/// This is the component-model interface for message producers. Components
/// can declare receptacles of type `dyn ISender<T> + Send + Sync` and bind
/// them to channel components via [`bind()`](crate::binding::bind).
///
/// # Examples
///
/// ```
/// use component_core::channel::{ISender, SpscChannel};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = SpscChannel::<u32>::new(16);
/// let sender: Arc<dyn ISender<u32> + Send + Sync> =
///     query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
///
/// let rx = ch.receiver().unwrap();
/// sender.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub trait ISender<T: Send + 'static>: Send + Sync + 'static {
    /// Send a message. Blocks if the queue is full.
    fn send(&self, value: T) -> Result<(), ChannelError>;
    /// Try to send without blocking.
    fn try_send(&self, value: T) -> Result<(), ChannelError>;
}

/// Interface trait for receiving typed messages from a channel.
///
/// This is the component-model interface for message consumers. Components
/// can declare receptacles of type `dyn IReceiver<T> + Send + Sync` and bind
/// them to channel components via [`bind()`](crate::binding::bind).
///
/// # Examples
///
/// ```
/// use component_core::channel::{IReceiver, SpscChannel};
/// use component_core::iunknown::query;
/// use std::sync::Arc;
///
/// let ch = SpscChannel::<u32>::new(16);
/// let tx = ch.sender().unwrap();
/// let receiver: Arc<dyn IReceiver<u32> + Send + Sync> =
///     query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
///
/// tx.send(99).unwrap();
/// assert_eq!(receiver.recv().unwrap(), 99);
/// ```
pub trait IReceiver<T: Send + 'static>: Send + Sync + 'static {
    /// Receive a message. Blocks if the queue is empty.
    fn recv(&self) -> Result<T, ChannelError>;
    /// Try to receive without blocking.
    fn try_recv(&self) -> Result<T, ChannelError>;
}

impl<T: Send + 'static> Interface for dyn ISender<T> + Send + Sync {}
impl<T: Send + 'static> Interface for dyn IReceiver<T> + Send + Sync {}

/// Error type for channel operations.
///
/// # Examples
///
/// ```
/// use component_core::channel::ChannelError;
///
/// let err = ChannelError::Full;
/// assert_eq!(format!("{err}"), "channel is full");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// Queue is full (for `try_send`).
    Full,
    /// Queue is empty (for `try_recv`).
    Empty,
    /// All senders disconnected; no more messages will arrive.
    Closed,
    /// Topology constraint violated at bind time.
    BindingRejected {
        /// Description of the violated constraint.
        reason: String,
    },
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(f, "channel is full"),
            Self::Empty => write!(f, "channel is empty"),
            Self::Closed => write!(f, "channel is closed"),
            Self::BindingRejected { reason } => {
                write!(f, "binding rejected: {reason}")
            }
        }
    }
}

impl std::error::Error for ChannelError {}

/// Shared state between sender(s) and receiver for signaling.
pub(crate) struct ChannelState<T> {
    pub(crate) queue: RingBuffer<T>,
    pub(crate) sender_count: AtomicUsize,
    pub(crate) receiver_thread: std::sync::Mutex<Option<Thread>>,
    pub(crate) sender_thread: std::sync::Mutex<Option<Thread>>,
    /// Fast-path flags: checked with a Relaxed load before acquiring the
    /// corresponding Mutex. Set before parking, cleared after waking.
    pub(crate) receiver_parked: AtomicBool,
    pub(crate) sender_parked: AtomicBool,
    /// Force-close flag set by actor deactivation to ensure the receiver
    /// exits even when other senders (from IUnknown queries) are still alive.
    pub(crate) force_closed: AtomicBool,
}

/// Sender endpoint for typed message passing.
///
/// For SPSC channels, `Sender` is not `Clone`. For MPSC channels, `Sender`
/// can be cloned to create additional producers.
///
/// When all senders are dropped, the channel is closed and the receiver
/// will receive [`ChannelError::Closed`] after draining remaining messages.
///
/// # Examples
///
/// ```
/// use component_core::channel::SpscChannel;
///
/// let ch = SpscChannel::<u32>::new(4);
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx.send(42).unwrap();
/// assert_eq!(rx.recv().unwrap(), 42);
/// ```
pub struct Sender<T: Send + 'static> {
    state: Arc<ChannelState<T>>,
    // Track whether this sender tracks the bound count for the channel component
    bound_flag: Option<Arc<AtomicBool>>,
}

impl<T: Send + 'static> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender").finish()
    }
}

impl<T: Send + 'static> Sender<T> {
    pub(crate) fn new(state: Arc<ChannelState<T>>, bound_flag: Option<Arc<AtomicBool>>) -> Self {
        state.sender_count.fetch_add(1, Ordering::Release);
        Self { state, bound_flag }
    }

    /// Send a message. Blocks if the queue is full.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Closed`] if the receiver has been dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::SpscChannel;
    ///
    /// let ch = SpscChannel::<u32>::new(4);
    /// let tx = ch.sender().unwrap();
    /// let rx = ch.receiver().unwrap();
    ///
    /// tx.send(1).unwrap();
    /// tx.send(2).unwrap();
    /// assert_eq!(rx.recv().unwrap(), 1);
    /// assert_eq!(rx.recv().unwrap(), 2);
    /// ```
    pub fn send(&self, value: T) -> Result<(), ChannelError> {
        let mut val = value;
        loop {
            match self.state.queue.push(val) {
                Ok(()) => {
                    if self.state.receiver_parked.load(Ordering::Relaxed) {
                        if let Ok(guard) = self.state.receiver_thread.lock() {
                            if let Some(ref t) = *guard {
                                t.unpark();
                            }
                        }
                    }
                    return Ok(());
                }
                Err(returned) => {
                    val = returned;
                    {
                        let mut guard = self.state.sender_thread.lock().unwrap();
                        *guard = Some(thread::current());
                    }
                    self.state.sender_parked.store(true, Ordering::Release);
                    thread::park_timeout(std::time::Duration::from_millis(1));
                    self.state.sender_parked.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    /// Try to send without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Full`] if the queue is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::{SpscChannel, ChannelError};
    ///
    /// let ch = SpscChannel::<u32>::new(2);
    /// let tx = ch.sender().unwrap();
    ///
    /// assert!(tx.try_send(1).is_ok());
    /// assert!(tx.try_send(2).is_ok());
    /// assert_eq!(tx.try_send(3).unwrap_err(), ChannelError::Full);
    /// ```
    pub fn try_send(&self, value: T) -> Result<(), ChannelError> {
        match self.state.queue.push(value) {
            Ok(()) => {
                if self.state.receiver_parked.load(Ordering::Relaxed) {
                    if let Ok(guard) = self.state.receiver_thread.lock() {
                        if let Some(ref t) = *guard {
                            t.unpark();
                        }
                    }
                }
                Ok(())
            }
            Err(_returned) => Err(ChannelError::Full),
        }
    }
}

impl<T: Send + 'static> ISender<T> for Sender<T> {
    fn send(&self, value: T) -> Result<(), ChannelError> {
        Sender::send(self, value)
    }

    fn try_send(&self, value: T) -> Result<(), ChannelError> {
        Sender::try_send(self, value)
    }
}

impl<T: Send + 'static> Drop for Sender<T> {
    fn drop(&mut self) {
        let prev = self.state.sender_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            // Last sender dropped — signal closure
            self.state
                .queue
                .sender_alive
                .store(false, Ordering::Release);
            // Wake the receiver so it can see the closed signal
            if let Ok(guard) = self.state.receiver_thread.lock() {
                if let Some(ref t) = *guard {
                    t.unpark();
                }
            }
        }

        // Release the binding slot if applicable
        if let Some(ref flag) = self.bound_flag {
            flag.store(false, Ordering::Release);
        }
    }
}

/// Receiver endpoint for typed message passing.
///
/// Only one receiver exists per channel. When all senders are dropped and
/// the queue is drained, [`recv`](Receiver::recv) returns
/// [`ChannelError::Closed`].
///
/// # Examples
///
/// ```
/// use component_core::channel::SpscChannel;
///
/// let ch = SpscChannel::<u32>::new(4);
/// let tx = ch.sender().unwrap();
/// let rx = ch.receiver().unwrap();
///
/// tx.send(10).unwrap();
/// assert_eq!(rx.recv().unwrap(), 10);
/// ```
pub struct Receiver<T: Send + 'static> {
    state: Arc<ChannelState<T>>,
    bound_flag: Option<Arc<AtomicBool>>,
}

impl<T: Send + 'static> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receiver").finish()
    }
}

impl<T: Send + 'static> Receiver<T> {
    pub(crate) fn new(state: Arc<ChannelState<T>>, bound_flag: Option<Arc<AtomicBool>>) -> Self {
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
    /// use component_core::channel::{SpscChannel, ChannelError};
    ///
    /// let ch = SpscChannel::<u32>::new(4);
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
                if self.state.sender_parked.load(Ordering::Relaxed) {
                    if let Ok(guard) = self.state.sender_thread.lock() {
                        if let Some(ref t) = *guard {
                            t.unpark();
                        }
                    }
                }
                return Ok(value);
            }

            let naturally_closed = !self.state.queue.sender_alive.load(Ordering::Acquire)
                && self.state.sender_count.load(Ordering::Acquire) == 0;
            let force_closed = self.state.force_closed.load(Ordering::Acquire);

            if naturally_closed || force_closed {
                if let Some(value) = self.state.queue.pop() {
                    return Ok(value);
                }
                return Err(ChannelError::Closed);
            }

            {
                let mut guard = self.state.receiver_thread.lock().unwrap();
                *guard = Some(thread::current());
            }
            self.state.receiver_parked.store(true, Ordering::Release);
            thread::park_timeout(std::time::Duration::from_millis(1));
            self.state.receiver_parked.store(false, Ordering::Relaxed);
        }
    }

    /// Try to receive without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelError::Empty`] if no messages are available.
    /// Returns [`ChannelError::Closed`] if all senders are dropped and
    /// the queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::{SpscChannel, ChannelError};
    ///
    /// let ch = SpscChannel::<u32>::new(4);
    /// let tx = ch.sender().unwrap();
    /// let rx = ch.receiver().unwrap();
    ///
    /// assert_eq!(rx.try_recv().unwrap_err(), ChannelError::Empty);
    /// tx.send(1).unwrap();
    /// assert_eq!(rx.try_recv().unwrap(), 1);
    /// ```
    pub fn try_recv(&self) -> Result<T, ChannelError> {
        if let Some(value) = self.state.queue.pop() {
            if self.state.sender_parked.load(Ordering::Relaxed) {
                if let Ok(guard) = self.state.sender_thread.lock() {
                    if let Some(ref t) = *guard {
                        t.unpark();
                    }
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
}

impl<T: Send + 'static> IReceiver<T> for Receiver<T> {
    fn recv(&self) -> Result<T, ChannelError> {
        Receiver::recv(self)
    }

    fn try_recv(&self) -> Result<T, ChannelError> {
        Receiver::try_recv(self)
    }
}

impl<T: Send + 'static> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let Some(ref flag) = self.bound_flag {
            flag.store(false, Ordering::Release);
        }
    }
}

// Sender and Receiver are Send + Sync when T: Send
// SAFETY: The underlying ChannelState uses atomic operations and Mutex for
// thread coordination. The RingBuffer's push/pop are safe for SPSC use, and
// MPSC safety is handled at a higher level.
unsafe impl<T: Send + 'static> Send for Sender<T> {}
unsafe impl<T: Send + 'static> Sync for Sender<T> {}
unsafe impl<T: Send + 'static> Send for Receiver<T> {}
unsafe impl<T: Send + 'static> Sync for Receiver<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_error_display() {
        assert_eq!(ChannelError::Full.to_string(), "channel is full");
        assert_eq!(ChannelError::Empty.to_string(), "channel is empty");
        assert_eq!(ChannelError::Closed.to_string(), "channel is closed");
        assert_eq!(
            ChannelError::BindingRejected {
                reason: "test".into()
            }
            .to_string(),
            "binding rejected: test"
        );
    }

    #[test]
    fn sender_receiver_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Sender<u32>>();
        assert_send_sync::<Receiver<u32>>();
    }
}
