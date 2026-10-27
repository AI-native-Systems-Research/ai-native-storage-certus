//! Channel reclaim across processes: a client that dies holding channels does
//! not leak them (see the module docs, "Channel claims").
//!
//! The dead clients are real processes (`shmq-echo hold`), killed with SIGKILL:
//! nothing runs on their way out, as for an OOM kill or a deleted pod.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test: a failed setup step is the failure"
)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shm_queue::{Client, Server};

const ECHO: &str = env!("CARGO_BIN_EXE_shmq-echo");

/// Held while spawning a holder, and by assertions about which channels are
/// locked. Between fork and exec a child holds copies of every descriptor in
/// this (multithreaded) test process, so a lock released in that window still
/// reads as held. That is a moment's false "busy", not a leak -- but it would
/// make the assertions below flaky.
static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp_path(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("certus-shmq-reclaim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// An echo server polled on a thread; `STUCK` (0xDEAD) is not answered until
/// `release_stuck` is set.
struct Echo {
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    release_stuck: Arc<AtomicBool>,
    poller: Option<std::thread::JoinHandle<()>>,
}

impl Echo {
    fn start(path: &std::path::Path, n: usize) -> Echo {
        let server = Arc::new(Server::create(path, n, 4096, 4096).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let release_stuck = Arc::new(AtomicBool::new(false));
        let (srv, st, rs) = (
            Arc::clone(&server),
            Arc::clone(&stop),
            Arc::clone(&release_stuck),
        );
        let poller = std::thread::spawn(move || {
            let mut last_seen = srv.seq_baseline();
            let mut held: Vec<(usize, u32)> = Vec::new();
            while !st.load(Ordering::Relaxed) {
                for (ch, seen) in last_seen.iter_mut().enumerate() {
                    if let Some(req) = srv.take_request(ch, seen) {
                        if req.opcode == 0xDEAD {
                            held.push((req.channel, req.seq));
                        } else {
                            srv.reply(req.channel, req.seq, req.opcode, &req.payload);
                        }
                    }
                }
                if rs.load(Ordering::Relaxed) {
                    for (ch, seq) in held.drain(..) {
                        srv.reply(ch, seq, 0xDEAD, b"late");
                    }
                }
                std::hint::spin_loop();
            }
        });
        Echo {
            server,
            stop,
            release_stuck,
            poller: Some(poller),
        }
    }
}

impl Drop for Echo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(p) = self.poller.take() {
            let _ = p.join();
        }
        let _ = &self.server;
    }
}

/// A `shmq-echo hold` process, `SIGKILL`ed when dropped -- also when a test
/// panics, so a failing test never leaves it running.
struct Holder(Option<Child>);

impl Drop for Holder {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill(); // SIGKILL: no exit handlers
            let _ = c.wait();
        }
    }
}

/// A `shmq-echo hold` process that has claimed `n` channels.
fn holder(path: &std::path::Path, n: usize, stuck: bool) -> (Holder, String) {
    let mut cmd = Command::new(ECHO);
    cmd.arg("hold").arg(path).arg(n.to_string());
    if stuck {
        cmd.arg("stuck");
    }
    let _spawning = SPAWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut child = cmd.stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let h = Holder(Some(child));
    assert!(line.starts_with("held"), "holder said {line:?}");
    (h, line)
}

fn kill(h: Holder) {
    drop(h);
}

fn echo_once(c: &Client, ch: usize, msg: &[u8]) -> Vec<u8> {
    let (status, resp) = c
        .request(
            ch,
            7,
            msg,
            100,
            Duration::from_millis(50),
            Duration::from_secs(5),
        )
        .expect("request answered");
    assert_eq!(status, 7);
    resp
}

#[test]
fn a_killed_clients_channels_are_reclaimed() {
    let path = tmp_path("killed");
    let _echo = Echo::start(&path, 4);
    let (child, _) = holder(&path, 4, false);
    let me = Client::attach(&path, Duration::from_secs(5)).unwrap();
    assert_eq!(me.claim_channel(), None, "all four held by a live process");
    kill(child);
    let mut got = Vec::new();
    while let Some(ch) = me.claim_channel() {
        assert_eq!(
            echo_once(&me, ch, b"after"),
            b"after",
            "a reclaimed channel serves"
        );
        got.push(ch);
    }
    got.sort_unstable();
    assert_eq!(got, vec![0, 1, 2, 3]);
    assert_eq!(me.reclaimed(), 4);
}

#[test]
fn a_live_clients_channels_are_not_taken() {
    let path = tmp_path("live");
    let _echo = Echo::start(&path, 3);
    let (child, held) = holder(&path, 2, false);
    let me = Client::attach(&path, Duration::from_secs(5)).unwrap();
    let ch = me.claim_channel().expect("the free one");
    assert!(
        !held.contains(&ch.to_string()),
        "took a live client's channel {ch}: {held}"
    );
    assert_eq!(me.claim_channel(), None);
    assert_eq!(me.reclaimed(), 0);
    kill(child);
}

#[test]
fn a_released_channel_is_free_while_its_releaser_lives() {
    // release_channel must drop the lock itself, not leave it to the mapping's
    // close: the releasing client keeps its mapping open.
    let path = tmp_path("release");
    let _echo = Echo::start(&path, 1);
    let a = Client::attach(&path, Duration::from_secs(5)).unwrap();
    let b = Client::attach(&path, Duration::from_secs(5)).unwrap();
    let _no_forks = SPAWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let ch = a.claim_channel().unwrap();
    assert_eq!(b.claim_channel(), None);
    a.release_channel(ch);
    assert_eq!(b.claim_channel(), Some(ch), "released, but still locked");
    assert_eq!(b.reclaimed(), 0, "a clean release is not a takeover");
    assert_eq!(echo_once(&b, ch, b"b"), b"b");
}

#[test]
fn a_takeover_continues_the_dead_owners_sequence() {
    // The dead owner used the channel; a fresh client starts its counter at 0.
    // Without adopting the channel's sequence, its first request could carry a
    // seq the server has already seen, and be ignored.
    let path = tmp_path("seq");
    let _echo = Echo::start(&path, 1);
    {
        let (child, _) = holder(&path, 1, false);
        kill(child);
    }
    // One request, so the channel's last seq is 1: exactly where a fresh
    // client's own counter would put its first request. Without adopting it,
    // the server ignores that request (seq == last seen) and the client reads
    // the previous owner's reply as its own.
    let a = Client::attach(&path, Duration::from_secs(5)).unwrap();
    let ch = a.claim_channel().unwrap();
    echo_once(&a, ch, b"prev");
    drop(a); // the process lives, but its description closes: the lock goes
    let b = Client::attach(&path, Duration::from_secs(5)).unwrap();
    let ch = b.claim_channel().expect("released by drop");
    assert_eq!(echo_once(&b, ch, b"next"), b"next");
}

#[test]
fn a_takeover_waits_for_the_dead_owners_reply() {
    let path = tmp_path("stuck");
    let echo = Echo::start(&path, 1);
    let (child, _) = holder(&path, 1, true);
    kill(child);
    let me = Arc::new(Client::attach(&path, Duration::from_secs(5)).unwrap());
    let m = Arc::clone(&me);
    let claimer = std::thread::spawn(move || m.claim_channel());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !claimer.is_finished(),
        "claimed while the dead owner's request was unanswered"
    );
    echo.release_stuck.store(true, Ordering::Relaxed);
    let ch = claimer
        .join()
        .unwrap()
        .expect("claimed once the reply landed");
    // The stale reply ("late") must not be read as ours.
    assert_eq!(echo_once(&me, ch, b"mine"), b"mine");
}

#[test]
fn two_claimers_never_share_a_dead_channel() {
    let path = tmp_path("race");
    let _echo = Echo::start(&path, 4);
    for _ in 0..20 {
        let (child, _) = holder(&path, 4, false);
        kill(child);
        let a = Arc::new(Client::attach(&path, Duration::from_secs(5)).unwrap());
        let b = Arc::new(Client::attach(&path, Duration::from_secs(5)).unwrap());
        let take = |c: Arc<Client>| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                while let Some(ch) = c.claim_channel() {
                    v.push(ch);
                }
                (c, v)
            })
        };
        let (ta, tb) = (take(a), take(b));
        let ((_a, va), (_b, vb)) = (ta.join().unwrap(), tb.join().unwrap());
        let mut all: Vec<usize> = va.iter().chain(&vb).copied().collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3], "a={va:?} b={vb:?}");
    }
}

#[test]
fn an_old_abi_client_is_refused() {
    let path = tmp_path("abi");
    let _echo = Echo::start(&path, 1);
    // A version-1 client reads abi_version at header offset 4 and must refuse.
    let bytes = std::fs::read(&path).unwrap();
    let abi = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    assert_eq!(abi, shm_queue::ABI_VERSION);
    assert_ne!(abi, 1, "claims changed meaning; the ABI number must say so");
}

#[test]
fn each_server_start_has_its_own_generation() {
    let path = tmp_path("gen");
    let mut seen = std::collections::HashSet::new();
    for _ in 0..8 {
        let s = Server::create(&path, 1, 4096, 4096).unwrap();
        let c = Client::attach(&path, Duration::from_secs(1)).unwrap();
        assert_ne!(c.generation(), 0);
        assert!(seen.insert(c.generation()), "a generation repeated");
        drop(c);
        drop(s);
    }
}

#[test]
fn a_client_of_a_replaced_server_fails_fast_and_says_so() {
    let path = tmp_path("replaced");
    let old = Server::create(&path, 1, 4096, 4096).unwrap();
    let c = Client::attach(&path, Duration::from_secs(1)).unwrap();
    let ch = c.claim_channel().unwrap();
    assert_eq!(c.server_replaced(), None, "same server: not replaced");

    // A restart: the old server's file is gone (its drop unlinks it) and a
    // new one stands at the path. The client still maps the old inode, where
    // nobody polls: its heartbeat stands still and no reply ever comes.
    drop(old);
    let new = Server::create(&path, 1, 4096, 4096).unwrap();
    let new_gen = Client::attach(&path, Duration::from_secs(1))
        .unwrap()
        .generation();
    assert_eq!(c.server_replaced(), Some(new_gen));

    let t = std::time::Instant::now();
    let err = c
        .request(
            ch,
            7,
            b"x",
            100,
            Duration::from_millis(50),
            Duration::from_secs(30),
        )
        .expect_err("an orphaned mailbox is never answered");
    let waited = t.elapsed();
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err}");
    assert!(err.to_string().contains("restarted"), "{err}");
    assert!(
        waited < Duration::from_secs(5),
        "took {waited:?}: should be about QUIET_BEFORE_PROBE, not the 30 s deadline"
    );
    drop(new);
}

#[test]
fn a_stopped_server_is_not_mistaken_for_a_replaced_one() {
    let path = tmp_path("stopped");
    let old = Server::create(&path, 1, 4096, 4096).unwrap();
    let c = Client::attach(&path, Duration::from_secs(1)).unwrap();
    let ch = c.claim_channel().unwrap();
    drop(old); // gone, nothing new: no generation to compare
    assert_eq!(c.server_replaced(), None);
    let err = c
        .request(
            ch,
            7,
            b"x",
            100,
            Duration::from_millis(50),
            Duration::from_secs(1),
        )
        .expect_err("nobody answers");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::TimedOut,
        "the plain deadline, {err}"
    );
}

#[test]
fn a_server_still_starting_at_the_path_is_not_a_replacement_yet() {
    let path = tmp_path("starting");
    let old = Server::create(&path, 1, 4096, 4096).unwrap();
    let c = Client::attach(&path, Duration::from_secs(1)).unwrap();
    drop(old);
    // What a starting server leaves before it publishes: a sized file whose
    // header holds some generation but not yet the ready magic.
    let mut hdr = vec![0u8; 64];
    hdr[4..8].copy_from_slice(&shm_queue::ABI_VERSION.to_le_bytes());
    hdr[8..12].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    std::fs::write(&path, &hdr).unwrap();
    assert_eq!(c.server_replaced(), None, "not ready: it may never come up");
    let _ = std::fs::remove_file(&path);
}
