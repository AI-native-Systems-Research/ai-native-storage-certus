//! Block-tier reads: reader threads over a [`BlockSource`], which the `spdk`
//! build backs with `IDispatcher::read_to_bounce` (an `spdk`-only interface,
//! so not linked here).
//!
//! The endpoint never addresses a block device (research D13). It hands a
//! pinned `BlockDevice { offset }` entry to its source, which reads the extent
//! into a buffer from the memory tier's bounce reserve — inside the pool this
//! endpoint already registered, so the write goes out from it like any
//! memory-tier hit (D15). Serving never promotes, so a peer cannot evict the
//! source's own working set (FR-012, D8).
//!
//! A read blocks for as long as the device takes — there is no device timeout,
//! by decision: a timed-out read is forgotten while the drive may still write
//! its buffer. So reads run on `block_readers` threads of their own, never the
//! endpoint thread, and the endpoint keeps posting memory-tier writes while
//! they run (FR-022). The pin of an entry being read stays on the endpoint
//! thread, in its read table, and is released when the read's result comes
//! back — before the write is posted (FR-011, D7).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use interfaces::{BounceBuffer, BounceError, CacheKey};

/// Where block-tier entries are read from: the dispatcher in an `spdk` build,
/// a test double in tests. Called from reader threads, never the endpoint's.
pub(crate) trait BlockSource: Send + Sync {
    /// Read `len` bytes of the pinned entry `key` at extent `offset` into a
    /// fresh bounce buffer. Blocks until the read completes.
    fn read(&self, key: CacheKey, offset: u64, len: u32) -> Result<BounceBuffer, BounceError>;
    /// Return a buffer once nothing can still read it.
    fn release(&self, buf: BounceBuffer);
}

/// The `spdk` build's source: the bound dispatcher.
#[cfg(feature = "spdk")]
pub(crate) struct DispatcherSource(pub(crate) Arc<dyn interfaces::IDispatcher + Send + Sync>);

#[cfg(feature = "spdk")]
impl BlockSource for DispatcherSource {
    fn read(&self, key: CacheKey, offset: u64, len: u32) -> Result<BounceBuffer, BounceError> {
        self.0.read_to_bounce(key, offset, len)
    }
    fn release(&self, buf: BounceBuffer) {
        self.0.release_bounce(buf)
    }
}

/// One read handed to a reader thread.
struct Job {
    id: u64,
    key: CacheKey,
    offset: u64,
    len: u32,
}

/// What a reader hands back: the job's id and the read's result.
pub(crate) type ReadDone = (u64, Result<BounceBuffer, BounceError>);

/// The reader threads, and the channels to and from them.
pub(crate) struct Readers {
    source: Arc<dyn BlockSource>,
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<ReadDone>,
    threads: Vec<JoinHandle<()>>,
    /// Reader threads, and how many are reading now.
    count: usize,
    busy: usize,
    /// A read answered `NoSpace` and no space has been freed since. The
    /// endpoint is the reserve's only user, so only its own buffer returning
    /// can make room: retrying sooner would only spin lookups.
    full: bool,
    /// Frees so far. A `NoSpace` is believed only if nothing was freed after
    /// its read was submitted: otherwise the last buffer can come back before
    /// the `NoSpace` it answers is collected, nothing clears `full` again, and
    /// every deferred entry waits forever.
    frees: u64,
}

impl Readers {
    /// Start `count` reader threads over `source`.
    pub(crate) fn start(source: Arc<dyn BlockSource>, count: usize) -> std::io::Result<Self> {
        assert!(count > 0, "configure refuses block_readers = 0");
        let (jobs, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let (tx, done) = mpsc::channel();
        let mut threads = Vec::with_capacity(count);
        for i in 0..count {
            let (rx, tx, source) = (Arc::clone(&rx), tx.clone(), Arc::clone(&source));
            threads.push(
                std::thread::Builder::new()
                    .name(format!("cte-block-reader-{i}"))
                    .spawn(move || loop {
                        // The lock is held only to take a job, not to read it.
                        let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = job else {
                            return; // the endpoint dropped its sender: shut down
                        };
                        // A panic here would strand the read's pin and buffer,
                        // and shutdown waits for every read, so it is fatal.
                        let r = crate::endpoint::fatal_on_panic("block-tier read", || {
                            source.read(job.key, job.offset, job.len)
                        });
                        if tx.send((job.id, r)).is_err() {
                            return;
                        }
                    })?,
            );
        }
        Ok(Readers {
            source,
            jobs: Some(jobs),
            done,
            threads,
            count,
            busy: 0,
            full: false,
            frees: 0,
        })
    }

    /// Reads that may be handed out now: free readers, or none while the
    /// reserve is known full.
    pub(crate) fn can_read(&self) -> usize {
        if self.full {
            0
        } else {
            self.count - self.busy
        }
    }

    /// The free count, to record when a read is submitted.
    pub(crate) fn frees(&self) -> u64 {
        self.frees
    }

    /// A read submitted at free count `at` found the reserve full. Believed
    /// only if nothing has been freed since.
    pub(crate) fn mark_full(&mut self, at: u64) {
        if at == self.frees {
            self.full = true;
        }
    }

    /// Bounce space was freed other than through [`release`](Self::release):
    /// a failed read's buffer, which the source frees itself.
    pub(crate) fn reserve_freed(&mut self) {
        self.full = false;
        self.frees += 1;
    }

    /// Reads handed out and not yet collected.
    pub(crate) fn busy(&self) -> usize {
        self.busy
    }

    /// Hand a read to a free reader. Only when [`can_read`](Self::can_read).
    pub(crate) fn submit(&mut self, id: u64, key: CacheKey, offset: u64, len: u32) {
        assert!(
            self.busy < self.count,
            "a read submitted with every reader busy"
        );
        self.busy += 1;
        self.jobs
            .as_ref()
            .expect("readers are stopped only at shutdown")
            .send(Job {
                id,
                key,
                offset,
                len,
            })
            .expect("reader threads outlive the endpoint's use of them");
    }

    /// A finished read, if one is ready.
    pub(crate) fn try_collect(&mut self) -> Option<ReadDone> {
        let r = self.done.try_recv().ok()?;
        self.busy -= 1;
        Some(r)
    }

    /// The next finished read, waiting for it however long the device takes
    /// (shutdown only: there is no device timeout).
    pub(crate) fn collect_blocking(&mut self) -> ReadDone {
        assert!(self.busy > 0, "waited for a read none was handed out for");
        let r = self
            .done
            .recv()
            .expect("a busy reader thread is alive until it reports");
        self.busy -= 1;
        r
    }

    /// Return a buffer to the source; there is room again.
    pub(crate) fn release(&mut self, buf: BounceBuffer) {
        self.source.release(buf);
        self.full = false;
        self.frees += 1;
    }

    /// Stop the threads. Every read must have been collected first.
    pub(crate) fn stop(&mut self) {
        assert_eq!(self.busy, 0, "readers stopped with reads outstanding");
        self.jobs = None;
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Readers {
    fn drop(&mut self) {
        // A table leaked by a failed shutdown may leave reads uncollected; the
        // threads then exit when their result has nowhere to go.
        self.jobs = None;
        if self.busy == 0 {
            for t in self.threads.drain(..) {
                let _ = t.join();
            }
        }
    }
}
