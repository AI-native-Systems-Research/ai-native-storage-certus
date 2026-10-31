//! The message pool: registered 8 KiB buffers for every message the endpoint
//! sends or receives.
//!
//! One pool per endpoint, grown in registered chunks as connections need
//! buffers and never shrunk: a buffer freed by one connection's teardown is
//! reused by the next, so after warm-up a reconnect registers nothing. Message
//! buffers are addressed by `lkey` only — no `rkey` of theirs is ever on the
//! wire — so, unlike the memory-tier pool and landing regions, nothing depends
//! on their registration surviving a reconnect; it is the cost that matters.
//!
//! A buffer belongs to exactly one place at a time: the free list, a posted
//! work request's slab slot (as [`Work::Send`] or [`Work::Recv`]), or a
//! connection's outbox. It is read or written only while it is in none of the
//! posted ones, through raw pointers to that buffer alone — never a slice over
//! the chunk, other buffers of which the NIC may be writing at the time.
//!
//! [`Work::Send`]: crate::endpoint::Work::Send
//! [`Work::Recv`]: crate::endpoint::Work::Recv

use crate::connection::transport::{Access, Region, Sge, Transport};
use crate::message::MESSAGE_LEN;

/// Fewest buffers a chunk is grown by.
const CHUNK_MIN: usize = 32;

/// One message buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MsgBuf {
    chunk: u32,
    index: u32,
}

struct Chunk {
    base: *mut u8,
    bufs: usize,
    /// Deregistered before the memory is freed.
    region: Option<Region>,
}

impl Drop for Chunk {
    fn drop(&mut self) {
        drop(self.region.take());
        // SAFETY: `base` came from `Box::into_raw` of a `bufs * MESSAGE_LEN`
        // slice in `grow`, and is freed once, here, after its registration.
        drop(unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                self.base,
                self.bufs * MESSAGE_LEN,
            ))
        });
    }
}

/// The endpoint's message buffers.
#[derive(Default)]
pub(crate) struct MsgPool {
    chunks: Vec<Chunk>,
    free: Vec<MsgBuf>,
}

// SAFETY: the raw chunk pointers are owned by the pool, which lives on the
// endpoint thread; it is moved there once, before any buffer is in use.
unsafe impl Send for MsgPool {}

impl MsgPool {
    /// Buffers in the pool, free or not.
    #[cfg(test)]
    pub(crate) fn total(&self) -> usize {
        self.chunks.iter().map(|c| c.bufs).sum()
    }

    #[cfg(test)]
    pub(crate) fn free(&self) -> usize {
        self.free.len()
    }

    /// Make sure at least `n` buffers are free, registering a new chunk with
    /// `transport` if not.
    pub(crate) fn reserve(
        &mut self,
        transport: &mut dyn Transport,
        n: usize,
    ) -> Result<(), String> {
        if self.free.len() >= n {
            return Ok(());
        }
        let bufs = (n - self.free.len()).max(CHUNK_MIN);
        let base = Box::into_raw(vec![0u8; bufs * MESSAGE_LEN].into_boxed_slice()) as *mut u8;
        let mut chunk = Chunk {
            base,
            bufs,
            region: None,
        };
        // SAFETY: the chunk owns the memory until its `Drop`, which
        // deregisters first; and the pool is dropped only after every link
        // that could hold one of its buffers in a posted request is drained.
        let region = unsafe { transport.register(base, bufs * MESSAGE_LEN, Access::LOCAL_WRITE) }
            .map_err(|e| format!("registering message buffers: {e}"))?;
        chunk.region = Some(region);
        let id = self.chunks.len() as u32;
        self.chunks.push(chunk);
        self.free.extend(
            (0..bufs as u32)
                .rev()
                .map(|index| MsgBuf { chunk: id, index }),
        );
        Ok(())
    }

    pub(crate) fn take(&mut self) -> Option<MsgBuf> {
        self.free.pop()
    }

    pub(crate) fn give(&mut self, b: MsgBuf) {
        debug_assert!(!self.free.contains(&b), "message buffer freed twice");
        self.free.push(b);
    }

    fn addr(&self, b: MsgBuf) -> *mut u8 {
        let c = &self.chunks[b.chunk as usize];
        assert!((b.index as usize) < c.bufs);
        // SAFETY: inside the chunk's allocation, checked above.
        unsafe { c.base.add(b.index as usize * MESSAGE_LEN) }
    }

    /// The first `len` bytes of `b`, for a work request.
    pub(crate) fn sge(&self, b: MsgBuf, len: usize) -> Sge<'_> {
        let region = self.chunks[b.chunk as usize]
            .region
            .as_ref()
            .expect("chunk is registered");
        region
            .sge(self.addr(b) as u64, len.min(MESSAGE_LEN))
            .expect("a buffer lies inside its chunk")
    }

    /// Write into `b`.
    ///
    /// # Safety
    ///
    /// `b` must not be in any posted work request.
    pub(crate) unsafe fn bytes_mut(&mut self, b: MsgBuf) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.addr(b), MESSAGE_LEN)
    }

    /// The first `len` bytes of `b`, copied out.
    ///
    /// # Safety
    ///
    /// `b` must not be in any posted work request.
    pub(crate) unsafe fn read(&self, b: MsgBuf, len: usize) -> Vec<u8> {
        std::slice::from_raw_parts(self.addr(b), len.min(MESSAGE_LEN)).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::mock::{MockFabric, MockTransport};

    #[test]
    fn grows_in_chunks_and_recycles() {
        let mut t = MockTransport::new(MockFabric::new());
        let mut p = MsgPool::default();
        p.reserve(&mut t, 3).unwrap();
        assert_eq!(
            (p.total(), p.free()),
            (CHUNK_MIN, CHUNK_MIN),
            "at least a chunk"
        );
        let bufs: Vec<_> = (0..CHUNK_MIN).map(|_| p.take().unwrap()).collect();
        assert!(p.take().is_none());
        p.reserve(&mut t, 40).unwrap();
        assert_eq!(p.total(), CHUNK_MIN + 40);
        for b in bufs {
            p.give(b);
        }
        assert_eq!(p.free(), CHUNK_MIN + 40);
        p.reserve(&mut t, 10).unwrap();
        assert_eq!(p.total(), CHUNK_MIN + 40, "enough free: nothing registered");
    }

    #[test]
    fn buffers_are_distinct_and_writable() {
        let mut t = MockTransport::new(MockFabric::new());
        let mut p = MsgPool::default();
        p.reserve(&mut t, 2).unwrap();
        let (a, b) = (p.take().unwrap(), p.take().unwrap());
        // SAFETY: neither is posted.
        unsafe {
            p.bytes_mut(a)[..3].copy_from_slice(b"aaa");
            p.bytes_mut(b)[..3].copy_from_slice(b"bbb");
            assert_eq!(p.read(a, 3), b"aaa");
            assert_eq!(p.read(b, 3), b"bbb");
        }
        assert_ne!(p.sge(a, 8).addr(), p.sge(b, 8).addr());
        assert_eq!(
            p.sge(a, 99999).len() as usize,
            MESSAGE_LEN,
            "clamped to one buffer"
        );
    }
}
