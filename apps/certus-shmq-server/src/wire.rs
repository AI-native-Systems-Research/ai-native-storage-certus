//! On-the-wire framing for the certus-shmq control protocol.
//!
//! Every message is `opcode: u32` (carried in the shm control block, not the
//! payload) plus a compact **little-endian** blob (the payload). This mirrors
//! the gRPC batch messages but without protobuf. The Python client
//! (`certus-shmq-connector/ring.py`) reproduces this layout byte-for-byte, so
//! **any change here must be mirrored there**.
//!
//! All integers are little-endian and **unaligned** (fields are written and
//! read sequentially by explicit offset, so no padding is inserted). x86-64 and
//! Python `struct` both handle unaligned access transparently.
//!
//! # Opcodes and blob layouts
//!
//! ```text
//! Check       (1): req  { n:u32, [key:u64]*n }
//!                  resp { [exists:u8]*n }
//! Touch       (2): req  { promote:u8, n:u32, [key:u64]*n }
//!                  resp { [ok:u8]*n }
//! Reserve     (3): req  { n:u32, [key:u64, size:u32, session:u64]*n }
//!                  resp { [ok:u8]*n }
//! CopyToStore (4): req  = HandleBatch (see below)
//!                  resp { [ok:u8]*n }
//! CommitStore (5): req  { n:u32, [key:u64]*n }
//!                  resp { [ok:u8]*n }
//! AbortStore  (6): req  { n:u32, [key:u64]*n }
//!                  resp { [ok:u8]*n }
//! Pin         (7): req  { promote:u8, n:u32, [key:u64]*n }
//!                  resp { [ok:u8]*n }
//! Unpin       (8): req  { n:u32, [key:u64]*n }
//!                  resp { [ok:u8]*n }
//! Lookup      (9): req  = HandleBatch (see below)
//!                  resp { [ok:u8]*n }
//! TakeEvents (10): req  { max:u32 }
//!                  resp { n:u32, [key:u64, reason:u32]*n, dropped:u64 }
//! ```
//!
//! ## HandleBatch (CopyToStore / Lookup)
//!
//! The live path is **multi-region**: one KV block carries one 64-byte CUDA IPC
//! handle *per layer tensor*, and those handles are identical across blocks of a
//! region (only the per-block `offset` differs). So distinct handles are sent
//! **once** in a handle table and each block references them by index — 5-6x
//! smaller than inlining a handle per region per block.
//!
//! ```text
//! HandleBatch {
//!   n_handles:  u32,
//!   handles:    [ cuda_ipc_handle: u8[64], gpu_device_id: i32 ]  * n_handles,
//!   n_entries:  u32,
//!   entries:    [ key: u64,
//!                 nreg: u16,
//!                 regions: [ handle_idx: u32, offset: u64, size: u32 ] * nreg
//!               ] * n_entries,
//! }
//! ```
//!
//! # Response status
//!
//! The shm response control word carries a transport-level `status: u32`:
//! `0` = OK (payload is the per-op response blob above); non-zero = the request
//! could not be decoded/dispatched and the payload is a UTF-8 error string.

/// Wire opcodes. Kept in sync with `ring.py`'s `OP_*` constants.
pub mod op {
    pub const CHECK: u32 = 1;
    pub const TOUCH: u32 = 2;
    pub const RESERVE: u32 = 3;
    pub const COPY_TO_STORE: u32 = 4;
    pub const COMMIT_STORE: u32 = 5;
    pub const ABORT_STORE: u32 = 6;
    pub const PIN: u32 = 7;
    pub const UNPIN: u32 = 8;
    pub const LOOKUP: u32 = 9;
    pub const TAKE_EVENTS: u32 = 10;
}

/// Transport-level response status written to the response control word.
pub const STATUS_OK: u32 = 0;
/// Request could not be decoded or dispatched; payload is a UTF-8 error string.
pub const STATUS_ERROR: u32 = 1;

/// Size of a single handle-table entry: 64-byte handle + i32 device id.
/// Not used by the server decoder (it reads sequentially), but the Python
/// client uses the equivalent to size its request buffers and chunk oversize
/// batches — kept here as the authoritative definition.
#[allow(dead_code)]
pub const HANDLE_ENTRY_SIZE: usize = 64 + 4;
/// Size of a single per-block region reference: idx:u32 + offset:u64 + size:u32.
#[allow(dead_code)]
pub const REGION_SIZE: usize = 4 + 8 + 4;

/// Error decoding a request blob (truncated / malformed).
#[derive(Debug)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wire decode error: {}", self.0)
    }
}

impl std::error::Error for WireError {}

type WResult<T> = Result<T, WireError>;

/// Sequential little-endian reader over a request blob.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    #[inline]
    fn take(&mut self, n: usize) -> WResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| WireError("length overflow".into()))?;
        if end > self.buf.len() {
            return Err(WireError(format!(
                "truncated: need {n} bytes at offset {}, have {}",
                self.pos,
                self.buf.len() - self.pos
            )));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> WResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> WResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> WResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> WResult<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> WResult<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn handle(&mut self) -> WResult<[u8; 64]> {
        let b = self.take(64)?;
        let mut h = [0u8; 64];
        h.copy_from_slice(b);
        Ok(h)
    }
}

/// Sequential little-endian writer for a response blob.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn with_capacity(cap: usize) -> Self {
        Writer {
            buf: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    #[inline]
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    #[inline]
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_roundtrips_scalars() {
        let mut w = Writer::default();
        w.u8(0xAB);
        w.u32(0xDEAD_BEEF);
        w.u64(0x0102_0304_0506_0708);
        let bytes = w.into_bytes();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 0xAB);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.u64().unwrap(), 0x0102_0304_0506_0708);
    }

    #[test]
    fn reader_reports_truncation() {
        let bytes = [1u8, 2, 3];
        let mut r = Reader::new(&bytes);
        assert!(r.u32().is_err());
    }

    #[test]
    fn reader_parses_handle_batch_shape() {
        // n_handles=1, one handle (0x11*64, dev=-1), n_entries=1,
        // entry key=42, nreg=1, region(idx=0, offset=128, size=4096).
        let mut w = Writer::default();
        w.u32(1);
        for _ in 0..64 {
            w.u8(0x11);
        }
        w.u32((-1i32) as u32); // gpu_device_id
        w.u32(1); // n_entries
        w.u64(42); // key
                   // nreg as u16 (no Writer::u16 — emit two bytes)
        w.buf.extend_from_slice(&1u16.to_le_bytes());
        w.u32(0); // handle_idx
        w.u64(128); // offset
        w.u32(4096); // size
        let bytes = w.into_bytes();

        let mut r = Reader::new(&bytes);
        let n_handles = r.u32().unwrap();
        assert_eq!(n_handles, 1);
        let h = r.handle().unwrap();
        assert_eq!(h, [0x11u8; 64]);
        assert_eq!(r.i32().unwrap(), -1);
        let n_entries = r.u32().unwrap();
        assert_eq!(n_entries, 1);
        assert_eq!(r.u64().unwrap(), 42);
        assert_eq!(r.u16().unwrap(), 1);
        assert_eq!(r.u32().unwrap(), 0);
        assert_eq!(r.u64().unwrap(), 128);
        assert_eq!(r.u32().unwrap(), 4096);
    }
}
