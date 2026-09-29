//! The pre-filled reusable payload buffer, and the handle-batch template over it.
//!
//! `LOOKUP` and `COPY_TO_STORE` are the two operations that name GPU memory: a load DMAs
//! into a device buffer, a store copies out of one. Neither takes a key list — both take a
//! **handle batch**, which is a table of CUDA IPC handles plus, per key, the regions
//! within them. That was discovered by running against a live server, which answered a
//! key-list `LOOKUP` with "truncated: need 64 bytes at offset 4, have 32".
//!
//! # What FR-038 forbids, and what it permits
//!
//! > No per-operation work may be proportional to payload size; block payloads MUST be
//! > pre-filled reusable buffers carrying at most a small identifying stamp.
//!
//! A generator that built 32 KiB per block per operation would spend its time in `memcpy`
//! and would report a throughput that fell as block size rose — a property of the
//! instrument, presented as a property of Certus. So the device allocation is filled
//! **once**, before the timed window, and reused for every operation.
//!
//! The same reasoning applies one level up, to the request bytes. A handle batch is
//! `76 + 26n` bytes of which only the keys vary, so [`HandleBatchTemplate`] pre-writes
//! every region reference at construction and an operation stamps **8 bytes per key plus
//! a 4-byte count**. Nothing else is built, ever.
//!
//! # The layout, verified against the dispatcher's own decoder
//!
//! From `decode_handle_batch` in `lib/shmq-dispatcher/src/translate.rs`:
//!
//! ```text
//! offset  0   n_handles   u32
//! offset  4   handle      [u8; 64]      -- one per n_handles, each followed by:
//! offset 68   gpu_device  i32
//! offset 72   n_entries   u32
//! offset 76   entries, 26 bytes each:
//!              +0   key         u64
//!              +8   nreg        u16
//!             then nreg regions of:
//!              +10  handle_idx  u32
//!              +14  offset      u64
//!              +22  size        u32
//! ```
//!
//! One allocation serves the whole run, so the table holds exactly one handle
//! (`n_handles == 1`) and every entry is a single region (`nreg == 1`) into it at its own
//! offset. `handle_idx` is therefore always 0 — and the dispatcher range-checks it, so a
//! wrong value is refused rather than silently reading the wrong memory.
//!
//! # Disjoint regions are a correctness requirement, not a tidiness one
//!
//! Lanes issue concurrently, and the server DMAs into these regions without coordinating
//! with us. If two in-flight operations named the same bytes, two DMAs would race and the
//! measurement would be of a system corrupting its own transfers. So each lane owns a
//! **slot** — a disjoint `batch_keys * block_bytes` span — and within a slot each key owns
//! one block. [`PayloadBuffer::slot_base`] is the only place that arithmetic lives.
//!
//! # Stamping is only half of it: a stamp nobody reads proves nothing
//!
//! Writing the key into a block says what the block *should* be. Reading it back after a load
//! says what Certus actually returned. Without the second half every block in the buffer is
//! interchangeable — the pre-fill is one repeated byte — so a cache that returned the wrong
//! block, or no block at all, would produce a run indistinguishable from a correct one.
//! [`PayloadBuffer::read_stamp`] closes that, and [`PayloadBuffer::verifying`] says whether a
//! run asked for it.
//!
//! Two caveats, both real:
//!
//! * Verification requires that **every** store of every key in the cache stamped it. A block
//!   put there by a run with stamping off holds the fill byte, so checking it would report a
//!   mismatch that is the *instrument's* fault. Verification therefore implies stamping and
//!   wants a cold cache; against a warm one from a non-stamping run it reports noise.
//! * It costs a device-to-host copy **per key**, on top of the stamp's host-to-device copy.
//!   That is per-key work on the critical path, so it is opt-in for the same reason stamping
//!   is (FR-070).
//!
//! It is independent of Certus's own `integrity-check` feature, and deliberately so: a check
//! that shares an implementation with the thing it checks shares its bugs.
//!
//! # The key stamp is off by default, and that is FR-070's doing
//!
//! FR-038 permits "a small identifying stamp", and stamping each block's first 8 bytes
//! with its key would let a load's data be checked for identity. But the stamp is a
//! synchronous host-to-device copy **per key**, and FR-070 requires that timing add no
//! per-key work, so switching it on puts a CUDA call per key on the critical path. It is
//! therefore opt-in, and [`PayloadBuffer::stamping`] reports whether a run used it so a
//! report can say so. Verification is worth paying for deliberately; it is not worth
//! paying for by accident.

use std::ffi::c_void;

use crate::cuda;

/// Bytes of the fixed handle-batch header: `n_handles`, one handle entry, `n_entries`.
pub const HEADER_BYTES: usize = 4 + cuda::IPC_HANDLE_BYTES + 4 + 4;

/// Bytes per single-region batch entry: `key`, `nreg`, `(handle_idx, offset, size)`.
pub const ENTRY_BYTES: usize = 8 + 2 + 4 + 8 + 4;

/// Byte offset of the `n_entries` field.
const N_ENTRIES_AT: usize = 4 + cuda::IPC_HANDLE_BYTES + 4;

/// Bytes of the optional per-block key stamp.
pub const KEY_STAMP_BYTES: usize = 8;

/// Byte written through the pre-fill, chosen only to be non-zero.
///
/// Zeroed device memory would make a "did the transfer happen" question unanswerable,
/// since a failed DMA also leaves zeroes.
const FILL_BYTE: u8 = 0xA5;

/// The request bytes for one lane's `LOOKUP`/`COPY_TO_STORE`, pre-built except the keys.
///
/// Construction writes the header and every region reference. Issuing an operation writes
/// only the keys and the count — see the module docs on FR-038.
#[derive(Debug, Clone)]
pub struct HandleBatchTemplate {
    bytes: Vec<u8>,
    max_keys: usize,
}

impl HandleBatchTemplate {
    /// Pre-build the template for one lane's slot.
    ///
    /// `slot_base` is the lane's byte offset within the allocation, and key `i` occupies
    /// `slot_base + i * block_bytes`. Every one of those offsets is written now, so that
    /// none is computed later.
    pub fn new(
        handle: &[u8; cuda::IPC_HANDLE_BYTES],
        gpu_device: i32,
        slot_base: u64,
        block_bytes: u32,
        max_keys: usize,
    ) -> Self {
        let mut bytes = vec![0u8; HEADER_BYTES + max_keys * ENTRY_BYTES];
        // Header: exactly one handle, so every region can index 0.
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[4..4 + cuda::IPC_HANDLE_BYTES].copy_from_slice(handle);
        let dev_at = 4 + cuda::IPC_HANDLE_BYTES;
        bytes[dev_at..dev_at + 4].copy_from_slice(&gpu_device.to_le_bytes());
        // `n_entries` is patched per operation; the rest of each entry never changes.
        for i in 0..max_keys {
            let e = HEADER_BYTES + i * ENTRY_BYTES;
            // Leave [e..e+8] — the key — for `stamp_keys`.
            bytes[e + 8..e + 10].copy_from_slice(&1u16.to_le_bytes()); // nreg
            bytes[e + 10..e + 14].copy_from_slice(&0u32.to_le_bytes()); // handle_idx
            let offset = slot_base + i as u64 * block_bytes as u64;
            bytes[e + 14..e + 22].copy_from_slice(&offset.to_le_bytes());
            bytes[e + 22..e + 26].copy_from_slice(&block_bytes.to_le_bytes());
        }
        Self { bytes, max_keys }
    }

    /// Keys this template can carry, which is the run's `--batch-keys`.
    pub fn max_keys(&self) -> usize {
        self.max_keys
    }

    /// Stamp `keys` into the template and return the request bytes.
    ///
    /// Writes `8 * keys.len() + 4` bytes and nothing else: the region references are
    /// already in place. The returned slice is a prefix, so a batch shorter than the
    /// maximum costs no more to send.
    ///
    /// # Panics
    ///
    /// If `keys.len()` exceeds [`HandleBatchTemplate::max_keys`]. The caller splits
    /// batches to `--batch-keys`, so a longer one is a caller defect, and silently
    /// truncating it would drop key references from a run that reported them as issued.
    pub fn stamp_keys(&mut self, keys: &[u64]) -> &[u8] {
        assert!(
            keys.len() <= self.max_keys,
            "batch of {} keys exceeds the template's {} — the caller must split to \
             --batch-keys, and truncating here would silently drop key references",
            keys.len(),
            self.max_keys
        );
        self.bytes[N_ENTRIES_AT..N_ENTRIES_AT + 4]
            .copy_from_slice(&(keys.len() as u32).to_le_bytes());
        for (i, key) in keys.iter().enumerate() {
            let e = HEADER_BYTES + i * ENTRY_BYTES;
            self.bytes[e..e + 8].copy_from_slice(&key.to_le_bytes());
        }
        &self.bytes[..HEADER_BYTES + keys.len() * ENTRY_BYTES]
    }
}

/// One CUDA allocation, filled once and shared by every lane.
///
/// Each lane owns a disjoint slot; see the module docs on why that is a correctness
/// requirement. Dropping it frees the allocation.
pub struct PayloadBuffer {
    base: *mut c_void,
    verify: bool,
    handle: [u8; cuda::IPC_HANDLE_BYTES],
    gpu_device: i32,
    block_bytes: u32,
    batch_keys: usize,
    slots: usize,
    stamp: bool,
}

// SAFETY: `base` is a device pointer. The host treats it as an opaque address — it is
// exported in an IPC handle and used in offset arithmetic, and is never dereferenced on
// the host except through `cudaMemcpy`, which is thread-safe. Lanes address disjoint
// slots, so concurrent use aliases nothing.
unsafe impl Send for PayloadBuffer {}
unsafe impl Sync for PayloadBuffer {}

impl PayloadBuffer {
    /// Allocate, export and **pre-fill** one buffer for a run.
    ///
    /// `slots` is the lane count. The fill happens here, before the timed window, so no
    /// operation ever pays for it (FR-038).
    ///
    /// # Errors
    ///
    /// If any CUDA call fails, with the call and its arguments in the message — a bare
    /// "cudaMalloc failed" tells an operator nothing about how much was asked for.
    pub fn new(
        slots: usize,
        batch_keys: usize,
        block_bytes: u32,
        gpu_device: i32,
        stamp: bool,
        verify: bool,
    ) -> Result<Self, String> {
        assert!(slots > 0 && batch_keys > 0, "a run needs at least one slot");
        let slot_bytes = batch_keys as u64 * block_bytes as u64;
        let total = slot_bytes * slots as u64;

        // SAFETY: plain CUDA runtime calls with valid out-pointers; `base` is checked
        // against the returned status before any use.
        let (base, handle) = unsafe {
            cuda::check(
                || format!("cudaSetDevice({gpu_device})"),
                cuda::cudaSetDevice(gpu_device),
            )?;
            let mut base: *mut c_void = std::ptr::null_mut();
            cuda::check(
                || {
                    format!(
                        "cudaMalloc({total} bytes = {} MiB for {slots} lanes x \
                         {batch_keys} keys x {block_bytes} bytes)",
                        total / (1024 * 1024)
                    )
                },
                cuda::cudaMalloc(&mut base, total as usize),
            )?;
            let mut raw = cuda::IpcMemHandle::default();
            if let Err(e) = cuda::check(
                || "cudaIpcGetMemHandle".to_string(),
                cuda::cudaIpcGetMemHandle(&mut raw, base),
            ) {
                cuda::cudaFree(base);
                return Err(e);
            }
            (base, raw.reserved)
        };

        let me = Self {
            base,
            // Verification without stamping would compare a key against the fill byte.
            verify: verify && stamp,
            handle,
            gpu_device,
            block_bytes,
            batch_keys,
            slots,
            stamp,
        };
        me.prefill()?;
        Ok(me)
    }

    /// Fill the allocation with a non-zero pattern, one copy per slot.
    ///
    /// Zeroes would make "did the transfer happen" unanswerable, since a failed DMA also
    /// leaves zeroes. One host buffer is reused across slots so this costs one
    /// `slot_bytes` host allocation rather than one of the whole device buffer.
    fn prefill(&self) -> Result<(), String> {
        let slot_bytes = self.slot_bytes() as usize;
        let host = vec![FILL_BYTE; slot_bytes];
        for slot in 0..self.slots {
            // SAFETY: the destination is `base + slot * slot_bytes` with `slot < slots`,
            // so the whole `slot_bytes` span lies inside the allocation. `host` is a
            // valid host buffer of exactly that length.
            unsafe {
                let dst = (self.base as *mut u8).add(slot * slot_bytes) as *mut c_void;
                cuda::check(
                    || format!("cudaMemcpy H2D pre-fill of slot {slot} ({slot_bytes} bytes)"),
                    cuda::cudaMemcpy(
                        dst,
                        host.as_ptr() as *const c_void,
                        slot_bytes,
                        cuda::MEMCPY_HOST_TO_DEVICE,
                    ),
                )?;
            }
        }
        Ok(())
    }

    /// Byte offset of one lane's slot within the allocation.
    ///
    /// The single place this arithmetic lives; two lanes sharing bytes would mean two
    /// concurrent DMAs racing over the same memory.
    pub fn slot_base(&self, slot: usize) -> u64 {
        assert!(slot < self.slots, "slot {slot} of {}", self.slots);
        slot as u64 * self.slot_bytes()
    }

    /// Bytes per slot.
    pub fn slot_bytes(&self) -> u64 {
        self.batch_keys as u64 * self.block_bytes as u64
    }

    /// Total device bytes held.
    pub fn total_bytes(&self) -> u64 {
        self.slot_bytes() * self.slots as u64
    }

    /// The pre-built request template for one lane.
    pub fn template(&self, slot: usize) -> HandleBatchTemplate {
        HandleBatchTemplate::new(
            &self.handle,
            self.gpu_device,
            self.slot_base(slot),
            self.block_bytes,
            self.batch_keys,
        )
    }

    /// Whether this run stamps keys into blocks; see the module docs on FR-070.
    pub fn stamping(&self) -> bool {
        self.stamp
    }

    /// Whether loads are checked against their keys.
    ///
    /// Implies [`PayloadBuffer::stamping`]: reading a stamp nobody wrote would compare a key
    /// against the fill byte and report a mismatch that is the instrument's own fault.
    pub fn verifying(&self) -> bool {
        self.verify
    }

    /// Read back the key stamped into a block.
    ///
    /// After a `LOOKUP` this is what the cache actually delivered, so comparing it with the
    /// key that was asked for is the difference between "bytes arrived" and "the right bytes
    /// arrived". One synchronous device-to-host copy of [`KEY_STAMP_BYTES`] per call.
    ///
    /// # Errors
    ///
    /// If the copy fails.
    pub fn read_stamp(&self, slot: usize, index: usize) -> Result<u64, String> {
        assert!(
            index < self.batch_keys,
            "key index {index} of {}",
            self.batch_keys
        );
        let at = self.slot_base(slot) + index as u64 * self.block_bytes as u64;
        let mut bytes = [0u8; KEY_STAMP_BYTES];
        // SAFETY: `at` is the start of block `index` in slot `slot`, both range-checked above,
        // so the 8-byte read lies inside the allocation. `bytes` is a valid host buffer of
        // exactly that length.
        unsafe {
            let src = (self.base as *const u8).add(at as usize) as *const c_void;
            cuda::check(
                || format!("cudaMemcpy D2H stamp read at {at}"),
                cuda::cudaMemcpy(
                    bytes.as_mut_ptr() as *mut c_void,
                    src,
                    KEY_STAMP_BYTES,
                    cuda::MEMCPY_DEVICE_TO_HOST,
                ),
            )?;
        }
        Ok(u64::from_le_bytes(bytes))
    }

    /// Stamp key identity into a block, when stamping is on.
    ///
    /// A no-op when off, so the caller needs no branch of its own. When on it is one
    /// synchronous host-to-device copy of [`KEY_STAMP_BYTES`] **per key**, which is why it
    /// is opt-in.
    ///
    /// # Errors
    ///
    /// If the copy fails.
    pub fn stamp_key(&self, slot: usize, index: usize, key: u64) -> Result<(), String> {
        if !self.stamp {
            return Ok(());
        }
        assert!(
            index < self.batch_keys,
            "key index {index} of {}",
            self.batch_keys
        );
        let at = self.slot_base(slot) + index as u64 * self.block_bytes as u64;
        let bytes = key.to_le_bytes();
        // SAFETY: `at` is the start of block `index` in slot `slot`, both range-checked
        // above, and `block_bytes >= KEY_STAMP_BYTES` is checked by the caller's
        // description validation, so the 8-byte write lies inside the allocation.
        unsafe {
            let dst = (self.base as *mut u8).add(at as usize) as *mut c_void;
            cuda::check(
                || format!("cudaMemcpy H2D key stamp at {at}"),
                cuda::cudaMemcpy(
                    dst,
                    bytes.as_ptr() as *const c_void,
                    KEY_STAMP_BYTES,
                    cuda::MEMCPY_HOST_TO_DEVICE,
                ),
            )?;
        }
        Ok(())
    }
}

impl Drop for PayloadBuffer {
    fn drop(&mut self) {
        // SAFETY: `base` came from `cudaMalloc` in `new` and is freed exactly once, here.
        // A failure is not reported: there is nothing a caller could do at drop time, and
        // the process is exiting.
        unsafe {
            cuda::cudaFree(self.base);
        }
    }
}

impl std::fmt::Debug for PayloadBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The handle is 64 opaque bytes and the pointer is a device address; neither is
        // useful in a log, and printing the handle would put IPC-openable bytes in one.
        f.debug_struct("PayloadBuffer")
            .field("slots", &self.slots)
            .field("batch_keys", &self.batch_keys)
            .field("block_bytes", &self.block_bytes)
            .field("total_bytes", &self.total_bytes())
            .field("gpu_device", &self.gpu_device)
            .field("stamping", &self.stamp)
            .field("verifying", &self.verify)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shmq_dispatcher::wire;

    /// Decode a handle batch the way `decode_handle_batch` does, with the dispatcher's own
    /// reader, and return `(handles, entries)`.
    #[allow(clippy::type_complexity)]
    fn decode(bytes: &[u8]) -> (Vec<([u8; 64], i32)>, Vec<(u64, Vec<(u32, u64, u32)>)>) {
        let mut r = wire::Reader::new(bytes);
        let n_handles = r.u32().unwrap() as usize;
        let mut handles = Vec::new();
        for _ in 0..n_handles {
            let h = r.handle().unwrap();
            let dev = r.i32().unwrap();
            handles.push((h, dev));
        }
        let n_entries = r.u32().unwrap() as usize;
        let mut entries = Vec::new();
        for _ in 0..n_entries {
            let key = r.u64().unwrap();
            let nreg = r.u16().unwrap() as usize;
            let mut regions = Vec::new();
            for _ in 0..nreg {
                let idx = r.u32().unwrap();
                let offset = r.u64().unwrap();
                let size = r.u32().unwrap();
                regions.push((idx, offset, size));
            }
            entries.push((key, regions));
        }
        (handles, entries)
    }

    fn template(slot_base: u64, block_bytes: u32, max_keys: usize) -> HandleBatchTemplate {
        let mut handle = [0u8; 64];
        // A recognisable pattern, so a truncated or shifted copy is visible.
        for (i, b) in handle.iter_mut().enumerate() {
            *b = i as u8;
        }
        HandleBatchTemplate::new(&handle, 3, slot_base, block_bytes, max_keys)
    }

    #[test]
    fn the_request_round_trips_through_the_dispatchers_own_reader() {
        // The layout is only right if the decoder agrees, so this reads with
        // `wire::Reader` in `decode_handle_batch`'s field order rather than with a
        // hand-written parser over my own assumptions.
        let mut t = template(0, 32768, 4);
        let bytes = t.stamp_keys(&[11, 22, 33]).to_vec();
        let (handles, entries) = decode(&bytes);
        assert_eq!(handles.len(), 1, "one allocation means one handle");
        assert_eq!(handles[0].1, 3, "the device ordinal must survive");
        assert_eq!(handles[0].0[7], 7, "the handle bytes must be verbatim");
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec![11, 22, 33]
        );
        for (i, (_, regions)) in entries.iter().enumerate() {
            assert_eq!(regions.len(), 1, "one region per key");
            assert_eq!(regions[0].0, 0, "handle_idx must index the only handle");
            assert_eq!(regions[0].1, i as u64 * 32768, "each key its own block");
            assert_eq!(regions[0].2, 32768, "size is bytes per block");
        }
    }

    #[test]
    fn the_wire_size_is_the_documented_seventy_six_plus_twenty_six_per_key() {
        // Independent of my own constants: 4 + 64 + 4 + 4 header, 8 + 2 + 4 + 8 + 4 entry.
        // remote-lookup-bench's encoder reserves exactly these, so a mismatch would mean
        // one of us is wrong about the protocol.
        assert_eq!(HEADER_BYTES, 76);
        assert_eq!(ENTRY_BYTES, 26);
        let mut t = template(0, 4096, 8);
        assert_eq!(t.stamp_keys(&[1]).len(), 76 + 26);
        assert_eq!(t.stamp_keys(&[1, 2, 3]).len(), 76 + 3 * 26);
    }

    #[test]
    fn a_short_batch_returns_a_prefix_and_leaves_no_stale_entries_visible() {
        // The template is reused, so a batch of 2 after a batch of 5 must not present the
        // older run's last three keys. `n_entries` is what bounds it, and the returned
        // slice must agree with it — a disagreement would have the server read keys the
        // generator never counted as issued.
        let mut t = template(0, 4096, 8);
        t.stamp_keys(&[1, 2, 3, 4, 5]);
        let bytes = t.stamp_keys(&[9, 8]).to_vec();
        let (_, entries) = decode(&bytes);
        assert_eq!(entries.len(), 2, "n_entries must bound the request");
        assert_eq!(bytes.len(), HEADER_BYTES + 2 * ENTRY_BYTES);
        assert_eq!(entries.iter().map(|e| e.0).collect::<Vec<_>>(), vec![9, 8]);
    }

    #[test]
    fn reusing_the_template_rewrites_every_key_rather_than_only_the_changed_ones() {
        // A stamp that skipped unchanged positions would leave a previous batch's key in
        // place and the server would touch a block this operation never named.
        let mut t = template(0, 4096, 4);
        t.stamp_keys(&[100, 200, 300]);
        let bytes = t.stamp_keys(&[7, 7, 7]).to_vec();
        let (_, entries) = decode(&bytes);
        assert!(entries.iter().all(|e| e.0 == 7), "stale keys survived");
    }

    #[test]
    fn lanes_address_disjoint_bytes_so_concurrent_dmas_cannot_alias() {
        // A correctness property, not tidiness: the server DMAs into these regions
        // without coordinating with us, so an overlap would be two transfers racing.
        let block = 4096u32;
        let keys = 4usize;
        let slot_bytes = keys as u64 * block as u64;
        let mut spans = Vec::new();
        for slot in 0..3u64 {
            let mut t = template(slot * slot_bytes, block, keys);
            let bytes = t.stamp_keys(&[1, 2, 3, 4]).to_vec();
            for (_, regions) in decode(&bytes).1 {
                let (_, off, size) = regions[0];
                spans.push((off, off + size as u64));
            }
        }
        spans.sort();
        for pair in spans.windows(2) {
            assert!(
                pair[0].1 <= pair[1].0,
                "regions {:?} and {:?} overlap",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(spans.len(), 12);
    }

    #[test]
    #[should_panic(expected = "exceeds the template's")]
    fn a_batch_longer_than_the_template_is_refused_rather_than_truncated() {
        // Truncating would drop key references from a run that had already counted them
        // as issued, so the report would overstate the work done.
        let mut t = template(0, 4096, 2);
        t.stamp_keys(&[1, 2, 3]);
    }

    #[test]
    // As above: asserting a constant is the guard, not a redundancy.
    #[allow(clippy::assertions_on_constants)]
    fn stamping_bytes_are_small_enough_to_be_the_stamp_fr_038_permits() {
        // FR-038 allows "at most a small identifying stamp". Eight bytes is one key; the
        // point of asserting it is that a future "improvement" to stamp more would be a
        // per-operation cost proportional to payload size, which is the thing forbidden.
        assert_eq!(KEY_STAMP_BYTES, 8);
        assert!(KEY_STAMP_BYTES < 64);
    }
}
