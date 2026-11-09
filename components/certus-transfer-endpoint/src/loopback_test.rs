//! Single-host RDMA loopback integration tests (hardware-gated, `#[ignore]`d).
//!
//! Every test here is `#[ignore]`d so a default `cargo test` never requires a
//! NIC (constitution Principle VIII). Run them explicitly on an RDMA host:
//!
//! ```text
//! CERTUS_RDMA_TEST_IP=10.0.0.102 \
//!     cargo test -p certus-transfer-endpoint --features rdma -- --ignored
//! ```
//!
//! Two whole endpoints in one process, on two ports of the one address (D16),
//! over the build's own transport — verbs — through the interface only, as a
//! caller would use them. The seam-level verbs tests are in
//! `connection::verbs`.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use interfaces::{
    EndpointConfig, EntryOutcome, FetchResult, IDispatchMap, IMemoryTier, ITransferEndpoint,
    ITransferEndpointAdmin, LocalSlot, RemoteRegion, TransferEndpointError,
};

use crate::test_doubles::{LedgerMap, PoolTier};
use crate::CertusTransferEndpoint;

fn ip() -> String {
    std::env::var("CERTUS_RDMA_TEST_IP")
        .expect("set CERTUS_RDMA_TEST_IP to a local RDMA (RoCE) IPv4 address")
}

/// A fresh instance address per endpoint, so parallel tests cannot collide.
fn addr() -> String {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let port =
        43000 + (std::process::id() % 500) as u16 * 16 + NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{}:{port}", ip())
}

/// An endpoint over verbs, serving from `pool` through `map`.
struct Node {
    ep: Arc<CertusTransferEndpoint>,
    map: Arc<LedgerMap>,
    pool: Box<[u8]>,
    addr: String,
    disk: Option<Arc<crate::test_doubles::DiskSource>>,
}

impl Node {
    fn new(pool_len: usize, tweak: impl FnOnce(&mut EndpointConfig)) -> Self {
        Self::build(
            pool_len,
            tweak,
            None::<fn(&crate::test_doubles::DiskSource)>,
        )
    }

    /// A node whose block tier is a [`DiskSource`] allocating from the top half
    /// of its pool, prepared by `prep` before the node starts.
    fn with_source(pool_len: usize, prep: impl FnOnce(&crate::test_doubles::DiskSource)) -> Self {
        Self::build(pool_len, |_| {}, Some(prep))
    }

    fn disk(&self) -> Arc<crate::test_doubles::DiskSource> {
        Arc::clone(self.disk.as_ref().expect("a node with a source"))
    }

    /// A node at a fixed address, for the two-node test.
    /// `CERTUS_T056_SEND_CREDITS` overrides the send-credit default.
    fn at(addr: &str, pool_len: usize) -> Self {
        let addr = addr.to_string();
        let credits: Option<u32> = std::env::var("CERTUS_T056_SEND_CREDITS")
            .ok()
            .and_then(|s| s.parse().ok());
        Self::build_at(
            Some(addr),
            pool_len,
            |c| {
                if let Some(n) = credits {
                    c.send_credits = n;
                }
            },
            None::<fn(&crate::test_doubles::DiskSource)>,
        )
    }

    fn build<P: FnOnce(&crate::test_doubles::DiskSource)>(
        pool_len: usize,
        tweak: impl FnOnce(&mut EndpointConfig),
        prep: Option<P>,
    ) -> Self {
        Self::build_at(None, pool_len, tweak, prep)
    }

    fn build_at<P: FnOnce(&crate::test_doubles::DiskSource)>(
        at: Option<String>,
        pool_len: usize,
        tweak: impl FnOnce(&mut EndpointConfig),
        prep: Option<P>,
    ) -> Self {
        let mut pool = vec![0u8; pool_len].into_boxed_slice();
        let ep = CertusTransferEndpoint::create();
        let map = Arc::new(LedgerMap::new());
        ep.dispatch_map
            .connect(Arc::clone(&map) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        ep.memory_tier
            .connect(Arc::new(PoolTier {
                base: pool.as_mut_ptr(),
                len: pool.len(),
            }) as Arc<dyn IMemoryTier + Send + Sync>)
            .unwrap();
        let addr = at.unwrap_or_else(addr);
        let mut config = EndpointConfig::new(&addr);
        tweak(&mut config);
        ep.configure(config).unwrap();
        let disk = prep.map(|prep| {
            let half = pool_len / 2;
            let disk = Arc::new(crate::test_doubles::DiskSource::new(
                pool.as_ptr() as u64 + half as u64,
                (half / 256).min(8),
                256,
            ));
            prep(&disk);
            ep.use_block_source(Arc::clone(&disk) as Arc<dyn crate::bounce::BlockSource>);
            disk
        });
        ep.initialize().expect("initialize over verbs");
        Node {
            ep,
            map,
            pool,
            addr,
            disk,
        }
    }

    fn pool_addr(&self, off: usize) -> u64 {
        self.pool.as_ptr() as u64 + off as u64
    }

    /// The `len` bytes at `off` into the pool, as a fetch lands in them: a
    /// destination lands in its own pool.
    fn slot(&self, off: usize, len: u32) -> LocalSlot {
        assert!(off + len as usize <= self.pool.len(), "inside the pool");
        LocalSlot {
            addr: self.pool_addr(off),
            length: len,
        }
    }

    /// The same bytes as a push names them, with the pool's rkey.
    fn region(&self, off: usize, len: u32) -> RemoteRegion {
        let rkey = self.ep.landing_region().expect("running").rkey;
        RemoteRegion {
            addr: self.slot(off, len).addr,
            rkey,
            length: len,
        }
    }

    /// The pool's bytes, read back once the writes checked have completed.
    fn bytes(&self) -> &[u8] {
        &self.pool
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.ep.shutdown();
    }
}

fn wait(rx: &mpsc::Receiver<FetchResult>) -> FetchResult {
    rx.recv_timeout(Duration::from_secs(20))
        .expect("on_complete fired")
}

/// T054: a full fetch over the NIC — request, the source's writes, its reply,
/// and the caller seeing each value land exactly where it named, in its own
/// pool — then a chunked one (SC-007 on hardware) into the same pool.
#[test]
#[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
fn loopback_fetch_round_trip() {
    let mut dest = Node::new(1 << 20, |_| {});
    let mut src = Node::new(1 << 20, |_| {});
    for (i, b) in src.pool.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    // Three hits of different sizes and a miss.
    src.map.memory(10, src.pool_addr(0), 4096);
    src.map.memory(11, src.pool_addr(8192), 65536);
    src.map.memory(13, src.pool_addr(200_000), 100);
    let items = [
        (10, dest.slot(0, 4096)),
        (11, dest.slot(4096, 65536)),
        (12, dest.slot(70_000, 64)),
        (13, dest.slot(80_000, 100)),
    ];
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(&src.addr, &items, Box::new(move |r| tx.send(r).unwrap()))
        .unwrap();
    use EntryOutcome::*;
    assert_eq!(
        wait(&rx).unwrap(),
        [HitMemoryTier, HitMemoryTier, Miss, HitMemoryTier]
    );
    let slots = dest.bytes();
    assert_eq!(&slots[..4096], &src.pool[..4096]);
    assert_eq!(&slots[4096..4096 + 65536], &src.pool[8192..8192 + 65536]);
    assert!(
        slots[70_000..70_064].iter().all(|&b| b == 0),
        "nothing for the miss"
    );
    assert_eq!(&slots[80_000..80_100], &src.pool[200_000..200_100]);
    assert_eq!(src.map.held(), 0, "every pin released");

    // 700 one-KiB entries: three chunks against the peer's 340.
    dest.pool.fill(0);
    for k in 0..700u64 {
        src.map
            .memory(1000 + k, src.pool_addr(k as usize * 1024), 1024);
    }
    let items: Vec<(u64, LocalSlot)> = (0..700usize)
        .map(|k| (1000 + k as u64, dest.slot(k * 1024, 1024)))
        .collect();
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(&src.addr, &items, Box::new(move |r| tx.send(r).unwrap()))
        .unwrap();
    assert_eq!(wait(&rx).unwrap(), vec![HitMemoryTier; 700]);
    assert_eq!(&dest.bytes()[..700 * 1024], &src.pool[..700 * 1024]);
    let s = dest.ep.stats();
    assert_eq!(s.chunks_sent, 1 + 3);
    assert_eq!(src.map.held(), 0);
}

/// T055, SC-002 on hardware: the legacy push writes a source's entries into a
/// peer's pool — the initiator's
/// `loopback_push_writes_into_remote_buffer`, through this endpoint. Many
/// writes are in flight at once, each to its own slot, so a window that
/// mismatched sources to destinations would corrupt the comparison.
#[test]
#[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
fn loopback_push_writes_into_remote_buffer() {
    const WRITES: usize = 64;
    const PIECE: usize = 16 * 1024;
    let mut src = Node::new(WRITES * PIECE, |c| c.send_credits = 16);
    let dst = Node::new(WRITES * PIECE, |_| {});
    for (i, b) in src.pool.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    for k in 0..WRITES {
        src.map
            .memory(k as u64, src.pool_addr(k * PIECE), PIECE as u32);
    }
    // Written back to front, so a slot error cannot hide behind identity.
    let items: Vec<(u64, RemoteRegion)> = (0..WRITES)
        .map(|k| {
            let slot = WRITES - 1 - k;
            (k as u64, dst.region(slot * PIECE, PIECE as u32))
        })
        .collect();
    let out = src.ep.push(&dst.addr, &items).unwrap();
    assert_eq!(out, vec![EntryOutcome::HitMemoryTier; WRITES]);
    let landing = dst.bytes();
    for k in 0..WRITES {
        let slot = WRITES - 1 - k;
        assert_eq!(
            &landing[slot * PIECE..(slot + 1) * PIECE],
            &src.pool[k * PIECE..(k + 1) * PIECE],
            "entry {k} in slot {slot}"
        );
    }
    assert!(src.map.peak_held() <= 16, "SC-004 on hardware");
    assert_eq!(src.map.held(), 0);
}

/// FR-023 / SC-010 end to end on hardware: a source that is alive, connected
/// and silent — its block-tier read held — trips the requester's deadline,
/// which forces the queue pair to ERROR before failing the fetch; when the
/// read is released the source's late write finds no queue pair to land
/// through, and the next fetch reconnects and lands in the same pool.
#[test]
#[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
fn loopback_deadline_tears_down_then_reconnects() {
    let dest = Node::new(4096, |c| {
        c.fetch_deadline = Some(Duration::from_millis(300))
    });
    let src = Node::with_source(4096, |disk| {
        disk.put(0x100, b"too late");
        disk.hold_reads();
    });
    src.map.block_at(1, 0x100);
    src.map.memory(2, src.pool_addr(0), 8);
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(
            &src.addr,
            &[(1, dest.slot(0, 8))],
            Box::new(move |r| tx.send(r).unwrap()),
        )
        .unwrap();
    let r = wait(&rx);
    let Err(TransferEndpointError::Deadline(why)) = &r else {
        panic!("expected Deadline, got {r:?}");
    };
    assert!(why.contains("queue pair Rts"), "{why}");
    assert_eq!(dest.ep.stats().deadline_expiries, 1);
    src.disk().release_reads();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(dest.bytes()[..8], [0u8; 8], "the late write did not land");
    // The same pool and rkey, over a fresh connection.
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(
            &src.addr,
            &[(2, dest.slot(8, 8))],
            Box::new(move |r| tx.send(r).unwrap()),
        )
        .unwrap();
    assert_eq!(wait(&rx).unwrap(), [EntryOutcome::HitMemoryTier]);
}

/// A source that shuts down drops its connection; a fetch outstanding on it
/// fails with `Transport`, and a fetch to a fresh source lands in the same
/// pool.
#[test]
#[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
fn loopback_lost_source_then_another() {
    let dest = Node::new(4096, |_| {});
    let src = Node::with_source(4096, |disk| disk.hold_reads());
    src.map.block_at(1, 0x100);
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(
            &src.addr,
            &[(1, dest.slot(0, 8))],
            Box::new(move |r| tx.send(r).unwrap()),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let disk = src.disk();
    let stopper = std::thread::spawn(move || drop(src));
    std::thread::sleep(Duration::from_millis(100));
    disk.release_reads(); // shutdown waits for the read
    stopper.join().unwrap();
    assert!(
        matches!(wait(&rx), Err(TransferEndpointError::Transport(_))),
        "the connection was lost"
    );
    let src2 = Node::new(4096, |_| {});
    src2.map.memory(2, src2.pool_addr(0), 8);
    let (tx, rx) = mpsc::channel();
    dest.ep
        .fetch(
            &src2.addr,
            &[(2, dest.slot(16, 8))],
            Box::new(move |r| tx.send(r).unwrap()),
        )
        .unwrap();
    assert_eq!(wait(&rx).unwrap(), [EntryOutcome::HitMemoryTier]);
}

/// T056: two nodes across the RoCE fabric. Run the `source` half on one node
/// and the `dest` half on the other, each pinned to its NIC's NUMA node:
///
/// ```text
/// # on the source node
/// CERTUS_T056_ROLE=source CERTUS_T056_LOCAL=10.0.0.105:47100 CERTUS_RDMA_TEST_IP=10.0.0.105 \
///   numactl -N0 -m0 cargo test --release -p certus-transfer-endpoint --features rdma \
///   two_node -- --ignored --nocapture
/// # on the destination node
/// CERTUS_T056_ROLE=dest CERTUS_T056_LOCAL=10.0.0.102:47100 CERTUS_T056_PEER=10.0.0.105:47100 \
///   CERTUS_RDMA_TEST_IP=10.0.0.102 numactl -N0 -m0 cargo test ... two_node -- --ignored --nocapture
/// ```
///
/// The source serves `T056_KEYS` 64 KiB entries, entry `k` filled with a
/// pattern only it has, for `CERTUS_T056_SECS` (default 60). The destination
/// fetches them in one chunked request several times, checks every byte, and
/// prints one `T056` line per round with its throughput.
#[test]
#[ignore = "two nodes; see the doc comment"]
fn two_node() {
    const T056_KEYS: usize = 1536;
    const ENTRY: usize = 64 * 1024;
    let Ok(role) = std::env::var("CERTUS_T056_ROLE") else {
        // Not a two-node run: `-- --ignored` on one RDMA box runs everything
        // else here, and this test has no peer to talk to.
        eprintln!("two_node: CERTUS_T056_ROLE unset; this test needs two nodes, skipping");
        return;
    };
    let local = std::env::var("CERTUS_T056_LOCAL").expect("CERTUS_T056_LOCAL=ip:port");
    let fill = |k: usize, i: usize| ((k * 131 + i * 7) % 251) as u8;
    match role.as_str() {
        "source" => {
            let secs: u64 = std::env::var("CERTUS_T056_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            let src = Node::at(&local, T056_KEYS * ENTRY);
            // SAFETY of the cast: the pool is ours and nothing reads it yet.
            let pool = src.pool.as_ptr() as *mut u8;
            for k in 0..T056_KEYS {
                for i in 0..ENTRY {
                    // SAFETY: inside the pool, before any peer reads it.
                    unsafe { *pool.add(k * ENTRY + i) = fill(k, i) };
                }
                src.map
                    .memory(k as u64, src.pool_addr(k * ENTRY), ENTRY as u32);
            }
            println!("T056 source {local}: serving {T056_KEYS} x {ENTRY} B for {secs} s");
            std::thread::sleep(Duration::from_secs(secs));
            let s = src.ep.stats();
            println!(
                "T056 source done: hits {} misses {} failed {} protocol_faults {} pins held {}",
                s.hit_memory_tier,
                s.miss,
                s.failed,
                s.protocol_faults,
                src.map.held()
            );
            assert_eq!(src.map.held(), 0);
        }
        "dest" => {
            let peer = std::env::var("CERTUS_T056_PEER").expect("CERTUS_T056_PEER=ip:port");
            let mut dest = Node::at(&local, T056_KEYS * ENTRY);
            let items: Vec<(u64, LocalSlot)> = (0..T056_KEYS)
                .map(|k| (k as u64, dest.slot(k * ENTRY, ENTRY as u32)))
                .collect();
            // Round-trip probe, for the credit derivation (research D10):
            // medians of many rounds, each a whole fetch from call to callback.
            // A 1-entry hit is one message plus one write; a 340-entry fetch
            // of keys the source lacks (offset past T056_KEYS) is one message
            // and its full per-entry lookup cost, with no writes; a 340-entry
            // hit adds the writes.
            {
                let median = |items: &[(u64, LocalSlot)], want: EntryOutcome| {
                    let mut t = Vec::with_capacity(200);
                    for _ in 0..200 {
                        let t0 = std::time::Instant::now();
                        let (tx, rx) = mpsc::channel();
                        dest.ep
                            .fetch(&peer, items, Box::new(move |r| tx.send(r).unwrap()))
                            .unwrap();
                        let out = wait(&rx).unwrap();
                        t.push(t0.elapsed());
                        assert!(out.iter().all(|o| *o == want));
                    }
                    t.sort();
                    (t[t.len() / 2], t[t.len() * 99 / 100])
                };
                let one_hit = median(&items[..1], EntryOutcome::HitMemoryTier);
                let miss: Vec<(u64, LocalSlot)> = items[..340]
                    .iter()
                    .map(|(k, r)| (*k + 1_000_000, *r))
                    .collect();
                let many_miss = median(&miss, EntryOutcome::Miss);
                let many_hit = median(&items[..340], EntryOutcome::HitMemoryTier);
                println!(
                    "T056 rtt: 1 hit {:?} (p99 {:?}); 340 miss {:?} (p99 {:?}); 340 hit {:?} (p99 {:?})",
                    one_hit.0, one_hit.1, many_miss.0, many_miss.1, many_hit.0, many_hit.1
                );
            }
            for round in 0..5 {
                dest.pool.fill(0);
                let t0 = std::time::Instant::now();
                let (tx, rx) = mpsc::channel();
                dest.ep
                    .fetch(&peer, &items, Box::new(move |r| tx.send(r).unwrap()))
                    .unwrap();
                let out = wait(&rx).unwrap_or_else(|e| panic!("round {round}: {e:?}"));
                let dt = t0.elapsed();
                assert_eq!(
                    out,
                    vec![EntryOutcome::HitMemoryTier; T056_KEYS],
                    "round {round}"
                );
                let slots = dest.bytes();
                for k in 0..T056_KEYS {
                    for i in (0..ENTRY).step_by(509) {
                        assert_eq!(
                            slots[k * ENTRY + i],
                            fill(k, i),
                            "round {round} key {k} byte {i}"
                        );
                    }
                }
                let bytes = (T056_KEYS * ENTRY) as f64;
                println!(
                    "T056 round {round}: {T056_KEYS} entries, {:.1} MiB in {:.1} ms = {:.2} GB/s",
                    bytes / (1 << 20) as f64,
                    dt.as_secs_f64() * 1e3,
                    bytes / dt.as_secs_f64() / 1e9
                );
            }
            let s = dest.ep.stats();
            println!(
                "T056 dest done: chunks_sent {} requests {}/{} deadline_expiries {}",
                s.chunks_sent, s.requests_completed, s.requests_attempted, s.deadline_expiries
            );
        }
        other => panic!("CERTUS_T056_ROLE={other}: want source or dest"),
    }
}
