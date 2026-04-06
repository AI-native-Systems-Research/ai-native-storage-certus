//! Lock-free ring buffers for channel components.
//!
//! This module provides two bounded, power-of-two ring buffers:
//!
//! - [`RingBuffer`] — single-producer, single-consumer (SPSC) queue using
//!   atomic head/tail pointers
//! - [`MpscRingBuffer`] — multi-producer, single-consumer (MPSC) queue using
//!   per-slot sequence numbers (Vyukov bounded MPMC algorithm, MPSC variant)
//!
//! Both use cache-line padding to prevent false sharing.
//!
//! # Safety
//!
//! The ring buffers use `unsafe` in a controlled manner:
//! - [`UnsafeCell`] for concurrent read/write to *different* slots
//! - [`MaybeUninit`] to avoid requiring `Default` for stored values
//! - Atomic ordering on head/tail/sequence establishes happens-before relationships

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Cache-line-padded wrapper to prevent false sharing.
///
/// Most x86/ARM processors use 64-byte cache lines. Padding the head
/// and tail counters to separate cache lines avoids costly cross-core
/// invalidation.
///
/// # Examples
///
/// ```
/// use component_core::channel::queue::CachePadded;
/// use std::sync::atomic::AtomicUsize;
///
/// let padded = CachePadded::new(AtomicUsize::new(0));
/// assert_eq!(std::mem::size_of_val(&padded), 128);
/// ```
#[repr(C)]
pub struct CachePadded<T> {
    value: T,
    _pad: [u8; 128 - std::mem::size_of::<usize>()],
}

// We use size_of::<usize>() as an approximation. For AtomicUsize, which
// has the same size as usize on all platforms, this works correctly.
// The total struct size is 128 bytes (2 cache lines) to guarantee
// the value sits on its own cache line even with alignment variations.

impl<T> CachePadded<T> {
    /// Wrap a value with cache-line padding.
    pub fn new(value: T) -> Self {
        Self {
            value,
            _pad: [0u8; 128 - std::mem::size_of::<usize>()],
        }
    }
}

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> std::ops::DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

/// A bounded, lock-free SPSC ring buffer.
///
/// Capacity must be a power of two. The buffer uses atomic head/tail
/// pointers with cache-line padding to prevent false sharing between
/// the producer and consumer.
///
/// # Thread Safety
///
/// `RingBuffer` is `Send + Sync` when `T: Send`. Only one thread may call
/// [`push`](RingBuffer::push) and only one thread may call
/// [`pop`](RingBuffer::pop) concurrently. This invariant is enforced at
/// the type level by the [`Sender`](super::Sender) and
/// [`Receiver`](super::Receiver) wrappers.
///
/// # Examples
///
/// ```
/// use component_core::channel::queue::RingBuffer;
///
/// let rb = RingBuffer::<u32>::new(4);
/// assert!(rb.push(1).is_ok());
/// assert!(rb.push(2).is_ok());
/// assert_eq!(rb.pop(), Some(1));
/// assert_eq!(rb.pop(), Some(2));
/// assert_eq!(rb.pop(), None);
/// ```
pub struct RingBuffer<T> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    capacity: usize,
    mask: usize,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    /// Set to `false` when the sender is dropped (signals closure to receiver).
    pub(crate) sender_alive: AtomicBool,
}

impl<T> RingBuffer<T> {
    /// Create a new ring buffer with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::RingBuffer;
    ///
    /// let rb = RingBuffer::<u8>::new(16);
    /// ```
    ///
    /// ```should_panic
    /// use component_core::channel::queue::RingBuffer;
    ///
    /// // Not a power of two — panics
    /// let rb = RingBuffer::<u8>::new(3);
    /// ```
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0 && capacity.is_power_of_two(),
            "capacity must be a positive power of two, got {capacity}"
        );

        let buffer: Vec<UnsafeCell<MaybeUninit<T>>> = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();

        Self {
            buffer: buffer.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            sender_alive: AtomicBool::new(true),
        }
    }

    /// Try to push a value into the buffer (non-blocking).
    ///
    /// Returns `Ok(())` if the value was enqueued, or `Err(value)` giving
    /// the value back if the buffer is full.
    ///
    /// # Safety Invariant
    ///
    /// Only one thread may call `push` at a time (single-producer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::RingBuffer;
    ///
    /// let rb = RingBuffer::<u32>::new(2);
    /// assert!(rb.push(10).is_ok());
    /// assert!(rb.push(20).is_ok());
    /// assert_eq!(rb.push(30).unwrap_err(), 30); // full, value returned
    /// ```
    pub fn push(&self, value: T) -> Result<(), T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail.wrapping_sub(head) >= self.capacity {
            return Err(value); // full — give value back
        }

        let slot = tail & self.mask;

        // SAFETY: We are the sole producer. The slot at `tail` is not being
        // read by the consumer because `tail - head < capacity`, meaning the
        // consumer's `head` has not reached this slot. We write to the slot
        // and then publish `tail + 1` with Release ordering, which ensures
        // the write is visible before the consumer sees the new tail.
        unsafe {
            (*self.buffer[slot].get()).write(value);
        }

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Try to pop a value from the buffer (non-blocking).
    ///
    /// Returns `Some(value)` if an element was dequeued, `None` if the buffer
    /// is empty.
    ///
    /// # Safety Invariant
    ///
    /// Only one thread may call `pop` at a time (single-consumer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::RingBuffer;
    ///
    /// let rb = RingBuffer::<u32>::new(4);
    /// rb.push(42);
    /// assert_eq!(rb.pop(), Some(42));
    /// assert_eq!(rb.pop(), None);
    /// ```
    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            return None; // empty
        }

        let slot = head & self.mask;

        // SAFETY: We are the sole consumer. The slot at `head` has been
        // written by the producer (because `head < tail`, and the producer
        // published tail with Release after writing). We read the value
        // and then publish `head + 1` with Release ordering, freeing the
        // slot for the producer to reuse.
        let value = unsafe { (*self.buffer[slot].get()).assume_init_read() };

        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Returns `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    /// Returns the number of elements currently in the buffer.
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        // Drop any remaining elements in the buffer
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        for i in head..tail {
            let slot = i & self.mask;
            // SAFETY: We have exclusive access during Drop (&mut self).
            // All slots between head and tail are initialized.
            unsafe {
                (*self.buffer[slot].get()).assume_init_drop();
            }
        }
    }
}

// SAFETY: RingBuffer is Send if T is Send (values are moved across threads).
// It is Sync because push and pop operate on disjoint slots protected by
// atomic head/tail ordering. The single-producer/single-consumer invariant
// is enforced by the Sender/Receiver wrappers.
unsafe impl<T: Send> Send for RingBuffer<T> {}
unsafe impl<T: Send> Sync for RingBuffer<T> {}

// ---------------------------------------------------------------------------
// MpscRingBuffer — lock-free multi-producer, single-consumer bounded queue
// ---------------------------------------------------------------------------

/// A slot in the MPSC ring buffer, containing a sequence counter and a value.
struct MpscSlot<T> {
    sequence: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}

/// A bounded, lock-free MPSC ring buffer using per-slot sequence numbers.
///
/// This implements a variant of the Vyukov bounded MPMC queue, specialised
/// for multiple producers and a single consumer. Producers claim slots via
/// CAS on a shared `tail` counter; the single consumer reads without CAS
/// (only an atomic load and store).
///
/// Capacity must be a power of two.
///
/// # Thread Safety
///
/// `MpscRingBuffer` is `Send + Sync` when `T: Send`. Multiple threads may
/// call [`push`](MpscRingBuffer::push) concurrently. Only one thread may
/// call [`pop`](MpscRingBuffer::pop) at a time (single-consumer).
///
/// # Examples
///
/// ```
/// use component_core::channel::queue::MpscRingBuffer;
///
/// let rb = MpscRingBuffer::<u32>::new(4);
/// assert!(rb.push(1).is_ok());
/// assert!(rb.push(2).is_ok());
/// assert_eq!(rb.pop(), Some(1));
/// assert_eq!(rb.pop(), Some(2));
/// assert_eq!(rb.pop(), None);
/// ```
pub struct MpscRingBuffer<T> {
    slots: Box<[MpscSlot<T>]>,
    capacity: usize,
    mask: usize,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    /// Set to `false` when the last sender is dropped (signals closure to receiver).
    pub(crate) sender_alive: AtomicBool,
}

impl<T> MpscRingBuffer<T> {
    /// Create a new MPSC ring buffer with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::MpscRingBuffer;
    ///
    /// let rb = MpscRingBuffer::<u8>::new(16);
    /// ```
    ///
    /// ```should_panic
    /// use component_core::channel::queue::MpscRingBuffer;
    ///
    /// // Not a power of two — panics
    /// let rb = MpscRingBuffer::<u8>::new(3);
    /// ```
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0 && capacity.is_power_of_two(),
            "capacity must be a positive power of two, got {capacity}"
        );

        let slots: Vec<MpscSlot<T>> = (0..capacity)
            .map(|i| MpscSlot {
                sequence: AtomicUsize::new(i),
                value: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect();

        Self {
            slots: slots.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            sender_alive: AtomicBool::new(true),
        }
    }

    /// Try to push a value into the buffer (non-blocking, lock-free).
    ///
    /// Multiple threads may call `push` concurrently. Returns `Ok(())` if
    /// the value was enqueued, or `Err(value)` giving the value back if
    /// the buffer is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::MpscRingBuffer;
    ///
    /// let rb = MpscRingBuffer::<u32>::new(2);
    /// assert!(rb.push(10).is_ok());
    /// assert!(rb.push(20).is_ok());
    /// assert_eq!(rb.push(30).unwrap_err(), 30); // full, value returned
    /// ```
    pub fn push(&self, value: T) -> Result<(), T> {
        let mut pos = self.tail.load(Ordering::Relaxed);

        loop {
            let slot = &self.slots[pos & self.mask];
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = seq as isize - pos as isize;

            if diff == 0 {
                // Slot is available — try to claim it
                match self.tail.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: We have exclusive access to this slot via CAS.
                        // No other producer can claim the same slot, and the consumer
                        // won't read it until we publish the sequence number.
                        unsafe {
                            (*slot.value.get()).write(value);
                        }
                        // Publish: set sequence to pos + 1 so the consumer can read it
                        slot.sequence.store(pos.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(actual) => {
                        // Lost the race — retry with updated position
                        pos = actual;
                    }
                }
            } else if diff < 0 {
                // Slot not yet consumed — buffer is full
                return Err(value);
            } else {
                // Another producer is working on this slot — reload tail
                pos = self.tail.load(Ordering::Relaxed);
            }
        }
    }

    /// Try to pop a value from the buffer (non-blocking).
    ///
    /// Returns `Some(value)` if an element was dequeued, `None` if the buffer
    /// is empty.
    ///
    /// # Safety Invariant
    ///
    /// Only one thread may call `pop` at a time (single-consumer).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::channel::queue::MpscRingBuffer;
    ///
    /// let rb = MpscRingBuffer::<u32>::new(4);
    /// rb.push(42);
    /// assert_eq!(rb.pop(), Some(42));
    /// assert_eq!(rb.pop(), None);
    /// ```
    pub fn pop(&self) -> Option<T> {
        let pos = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[pos & self.mask];
        let seq = slot.sequence.load(Ordering::Acquire);
        let diff = seq as isize - (pos.wrapping_add(1)) as isize;

        if diff < 0 {
            // Slot not yet written — buffer is empty
            return None;
        }

        // SAFETY: We are the sole consumer. The producer has finished writing
        // (sequence == pos + 1 means the write is published). We read the
        // value and then set sequence to pos + capacity, freeing the slot for
        // producers to reuse.
        let value = unsafe { (*slot.value.get()).assume_init_read() };

        // Release the slot: set sequence to pos + capacity so producers can reclaim it
        slot.sequence
            .store(pos.wrapping_add(self.capacity), Ordering::Release);
        self.head.store(pos.wrapping_add(1), Ordering::Release);

        Some(value)
    }

    /// Returns `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    /// Returns the number of elements currently in the buffer.
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<T> Drop for MpscRingBuffer<T> {
    fn drop(&mut self) {
        // Drop any remaining elements in the buffer
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        for i in head..tail {
            let slot = &self.slots[i & self.mask];
            // SAFETY: We have exclusive access during Drop (&mut self).
            // All slots between head and tail are initialized (their sequences
            // indicate completed writes).
            unsafe {
                (*slot.value.get()).assume_init_drop();
            }
        }
    }
}

// SAFETY: MpscRingBuffer is Send if T is Send (values are moved across threads).
// It is Sync because push uses CAS on tail and per-slot sequences to serialize
// writes, and pop operates on a disjoint head counter. The single-consumer
// invariant is enforced by the MpscReceiver wrapper.
unsafe impl<T: Send> Send for MpscRingBuffer<T> {}
unsafe impl<T: Send> Sync for MpscRingBuffer<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_power_of_two() {
        let rb = RingBuffer::<u32>::new(4);
        assert_eq!(rb.capacity(), 4);
        assert!(rb.is_empty());
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn new_rejects_non_power_of_two() {
        let _rb = RingBuffer::<u32>::new(3);
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn new_rejects_zero() {
        let _rb = RingBuffer::<u32>::new(0);
    }

    #[test]
    fn push_pop_single_element() {
        let rb = RingBuffer::<u32>::new(4);
        assert!(rb.push(42).is_ok());
        assert_eq!(rb.pop(), Some(42));
    }

    #[test]
    fn push_pop_to_capacity() {
        let rb = RingBuffer::<u32>::new(4);
        for i in 0..4 {
            assert!(rb.push(i).is_ok());
        }
        assert!(rb.push(99).is_err()); // full
        for i in 0..4 {
            assert_eq!(rb.pop(), Some(i));
        }
        assert_eq!(rb.pop(), None); // empty
    }

    #[test]
    fn push_on_full_returns_err_with_value() {
        let rb = RingBuffer::<u32>::new(2);
        assert!(rb.push(1).is_ok());
        assert!(rb.push(2).is_ok());
        assert_eq!(rb.push(3).unwrap_err(), 3);
    }

    #[test]
    fn pop_on_empty_returns_none() {
        let rb = RingBuffer::<u32>::new(4);
        assert_eq!(rb.pop(), None);
    }

    #[test]
    fn fifo_ordering_1000_elements() {
        let rb = RingBuffer::<u32>::new(1024);
        for i in 0..1000 {
            assert!(rb.push(i).is_ok());
        }
        for i in 0..1000 {
            assert_eq!(rb.pop(), Some(i));
        }
    }

    #[test]
    fn wraparound_behavior() {
        let rb = RingBuffer::<u32>::new(4);
        // Fill and drain multiple times to exercise wraparound
        for round in 0..10 {
            for i in 0..4 {
                assert!(rb.push(round * 4 + i).is_ok());
            }
            for i in 0..4 {
                assert_eq!(rb.pop(), Some(round * 4 + i));
            }
        }
    }

    #[test]
    fn sender_alive_flag() {
        let rb = RingBuffer::<u32>::new(4);
        assert!(rb.sender_alive.load(Ordering::Acquire));
        rb.sender_alive.store(false, Ordering::Release);
        assert!(!rb.sender_alive.load(Ordering::Acquire));
    }

    #[test]
    fn len_and_is_empty() {
        let rb = RingBuffer::<u32>::new(4);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);

        rb.push(1).unwrap();
        assert!(!rb.is_empty());
        assert_eq!(rb.len(), 1);

        rb.push(2).unwrap();
        assert_eq!(rb.len(), 2);

        rb.pop();
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn send_sync_bounds() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RingBuffer<u32>>();
        assert_send_sync::<RingBuffer<String>>();
    }

    #[test]
    fn drop_cleans_up_remaining_elements() {
        use std::sync::atomic::AtomicUsize;

        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct DropCounter;
        impl Drop for DropCounter {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);

        {
            let rb = RingBuffer::<DropCounter>::new(4);
            rb.push(DropCounter).unwrap();
            rb.push(DropCounter).unwrap();
            rb.push(DropCounter).unwrap();
            // Pop one, so 2 remain
            let _ = rb.pop();
            // rb drops here — should drop the remaining 2
        }

        // 1 from pop() + 2 from Drop = 3
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn concurrent_spsc_100k_messages() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(RingBuffer::<u64>::new(1024));
        let rb_producer = Arc::clone(&rb);
        let rb_consumer = Arc::clone(&rb);

        const COUNT: u64 = 100_000;

        let producer = thread::spawn(move || {
            let mut val = 0u64;
            while val < COUNT {
                match rb_producer.push(val) {
                    Ok(()) => val += 1,
                    Err(_) => std::hint::spin_loop(),
                }
            }
        });

        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(COUNT as usize);
            while received.len() < COUNT as usize {
                if let Some(val) = rb_consumer.pop() {
                    received.push(val);
                } else {
                    std::hint::spin_loop();
                }
            }
            received
        });

        producer.join().unwrap();
        let received = consumer.join().unwrap();

        assert_eq!(received.len(), COUNT as usize);
        for (i, &val) in received.iter().enumerate() {
            assert_eq!(val, i as u64, "FIFO violation at index {i}");
        }
    }

    #[test]
    fn closure_signal_after_drain() {
        let rb = RingBuffer::<u32>::new(4);
        rb.push(1).unwrap();
        rb.push(2).unwrap();

        // Simulate sender disconnect
        rb.sender_alive.store(false, Ordering::Release);

        // Can still drain existing messages
        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));

        // Now empty and sender gone
        assert_eq!(rb.pop(), None);
        assert!(!rb.sender_alive.load(Ordering::Acquire));
    }

    // -----------------------------------------------------------------------
    // MpscRingBuffer tests
    // -----------------------------------------------------------------------

    #[test]
    fn mpsc_new_with_power_of_two() {
        let rb = MpscRingBuffer::<u32>::new(4);
        assert_eq!(rb.capacity(), 4);
        assert!(rb.is_empty());
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn mpsc_new_rejects_non_power_of_two() {
        let _rb = MpscRingBuffer::<u32>::new(3);
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn mpsc_new_rejects_zero() {
        let _rb = MpscRingBuffer::<u32>::new(0);
    }

    #[test]
    fn mpsc_push_pop_single_element() {
        let rb = MpscRingBuffer::<u32>::new(4);
        assert!(rb.push(42).is_ok());
        assert_eq!(rb.pop(), Some(42));
    }

    #[test]
    fn mpsc_push_pop_to_capacity() {
        let rb = MpscRingBuffer::<u32>::new(4);
        for i in 0..4 {
            assert!(rb.push(i).is_ok());
        }
        assert!(rb.push(99).is_err()); // full
        for i in 0..4 {
            assert_eq!(rb.pop(), Some(i));
        }
        assert_eq!(rb.pop(), None); // empty
    }

    #[test]
    fn mpsc_push_on_full_returns_err_with_value() {
        let rb = MpscRingBuffer::<u32>::new(2);
        assert!(rb.push(1).is_ok());
        assert!(rb.push(2).is_ok());
        assert_eq!(rb.push(3).unwrap_err(), 3);
    }

    #[test]
    fn mpsc_pop_on_empty_returns_none() {
        let rb = MpscRingBuffer::<u32>::new(4);
        assert_eq!(rb.pop(), None);
    }

    #[test]
    fn mpsc_fifo_ordering_single_producer() {
        let rb = MpscRingBuffer::<u32>::new(1024);
        for i in 0..1000 {
            assert!(rb.push(i).is_ok());
        }
        for i in 0..1000 {
            assert_eq!(rb.pop(), Some(i));
        }
    }

    #[test]
    fn mpsc_wraparound_behavior() {
        let rb = MpscRingBuffer::<u32>::new(4);
        for round in 0..10 {
            for i in 0..4 {
                assert!(rb.push(round * 4 + i).is_ok());
            }
            for i in 0..4 {
                assert_eq!(rb.pop(), Some(round * 4 + i));
            }
        }
    }

    #[test]
    fn mpsc_sender_alive_flag() {
        let rb = MpscRingBuffer::<u32>::new(4);
        assert!(rb.sender_alive.load(Ordering::Acquire));
        rb.sender_alive.store(false, Ordering::Release);
        assert!(!rb.sender_alive.load(Ordering::Acquire));
    }

    #[test]
    fn mpsc_len_and_is_empty() {
        let rb = MpscRingBuffer::<u32>::new(4);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);

        rb.push(1).unwrap();
        assert!(!rb.is_empty());
        assert_eq!(rb.len(), 1);

        rb.push(2).unwrap();
        assert_eq!(rb.len(), 2);

        rb.pop();
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn mpsc_send_sync_bounds() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MpscRingBuffer<u32>>();
        assert_send_sync::<MpscRingBuffer<String>>();
    }

    #[test]
    fn mpsc_drop_cleans_up_remaining_elements() {
        use std::sync::atomic::AtomicUsize;

        static MPSC_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct DropCounter;
        impl Drop for DropCounter {
            fn drop(&mut self) {
                MPSC_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        MPSC_DROP_COUNT.store(0, Ordering::Relaxed);

        {
            let rb = MpscRingBuffer::<DropCounter>::new(4);
            rb.push(DropCounter).unwrap();
            rb.push(DropCounter).unwrap();
            rb.push(DropCounter).unwrap();
            let _ = rb.pop();
        }

        // 1 from pop() + 2 from Drop = 3
        assert_eq!(MPSC_DROP_COUNT.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn mpsc_concurrent_multi_producer_100k() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(MpscRingBuffer::<u64>::new(1024));
        const PRODUCERS: u64 = 8;
        const PER_PRODUCER: u64 = 12_500;
        const TOTAL: u64 = PRODUCERS * PER_PRODUCER;

        let mut handles = vec![];
        for pid in 0..PRODUCERS {
            let rb_clone = Arc::clone(&rb);
            handles.push(thread::spawn(move || {
                for i in 0..PER_PRODUCER {
                    let val = pid * PER_PRODUCER + i;
                    loop {
                        match rb_clone.push(val) {
                            Ok(()) => break,
                            Err(_) => std::hint::spin_loop(),
                        }
                    }
                }
            }));
        }

        let rb_consumer = Arc::clone(&rb);
        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(TOTAL as usize);
            while received.len() < TOTAL as usize {
                if let Some(val) = rb_consumer.pop() {
                    received.push(val);
                } else {
                    std::hint::spin_loop();
                }
            }
            received
        });

        for h in handles {
            h.join().unwrap();
        }
        let mut received = consumer.join().unwrap();

        assert_eq!(received.len(), TOTAL as usize);
        // Values may arrive out-of-order across producers, but all must be present
        received.sort();
        let expected: Vec<u64> = (0..TOTAL).collect();
        assert_eq!(received, expected);
    }
}
