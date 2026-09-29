//! The client half: pipelining, correlation matching, and the properties FR-072 needs.
//!
//! Almost all of this runs over an in-memory transport, because the protocol is bytes and a
//! client testable only against a live agent is a client tested rarely. The one exception is
//! `TCP_NODELAY`, which is a property of a real socket and is checked on a real socket.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};

use workload_wire::client::{
    Client, ClientError, DEFAULT_DEPTH, DEFAULT_READ_TIMEOUT, DEFAULT_WRITE_TIMEOUT,
};
use workload_wire::frame::{
    opcode, DrainAck, Header, Hello, HelloAck, ShutdownAck, Stats, SubmitTurn, TurnOutcome, Writer,
    BUILD_ID_BYTES, HEADER_BYTES, PROTO_VERSION,
};

/// A scripted transport: records what the client wrote, serves what it was given.
#[derive(Debug, Default)]
struct Mock {
    written: Vec<u8>,
    to_read: Vec<u8>,
    read_at: usize,
}

impl Mock {
    /// Queue a reply frame.
    fn reply(&mut self, opcode: u16, corr: u32, body: &[u8]) -> &mut Self {
        let mut w = Writer::new();
        w.bytes(body);
        self.to_read.extend_from_slice(&w.frame(opcode, 0, corr));
        self
    }

    /// The frames the client wrote, as `(opcode, corr, body)`.
    fn frames(&self) -> Vec<(u16, u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + HEADER_BYTES <= self.written.len() {
            let h = Header::decode(&self.written[at..], u32::MAX).expect("a header");
            let start = at + HEADER_BYTES;
            let end = start + h.len as usize;
            if end > self.written.len() {
                break;
            }
            out.push((h.opcode, h.corr, self.written[start..end].to_vec()));
            at = end;
        }
        out
    }
}

impl Write for Mock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Mock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.to_read.len() - self.read_at;
        if left == 0 {
            // A clean close, which the client must report as `Closed`.
            return Ok(0);
        }
        let n = left.min(buf.len());
        buf[..n].copy_from_slice(&self.to_read[self.read_at..self.read_at + n]);
        self.read_at += n;
        Ok(n)
    }
}

fn turn(session: u64, keys: &[u64]) -> SubmitTurn {
    SubmitTurn {
        session,
        flags: 0,
        path: keys.to_vec(),
    }
}

fn outcome(resident: u32) -> TurnOutcome {
    TurnOutcome {
        resident,
        ..Default::default()
    }
}

#[test]
fn the_window_holds_depth_frames_before_it_waits() {
    // Pipelining is the whole reason this client exists: a synchronous request per turn
    // caps at one turn per round trip however fast the node is.
    let mut mock = Mock::default();
    for corr in 1..=8u32 {
        mock.reply(opcode::SUBMIT_TURN, corr, &outcome(corr).encode());
    }
    let mut c = Client::with_transport(mock, 4);

    // Four submits fill the window and read nothing.
    for i in 0..4u64 {
        let (_, drained) = c.submit(&turn(i, &[i])).unwrap();
        assert!(drained.is_none(), "the window drained early at {i}");
    }
    assert_eq!(c.inflight(), 4, "four frames should be outstanding");

    // The fifth must drain one to make room, and returns that measurement rather than
    // discarding it.
    let (_, drained) = c.submit(&turn(4, &[4])).unwrap();
    assert!(drained.is_some(), "the fifth submit must drain one");
    assert_eq!(drained.unwrap().resident, 1, "the oldest reply comes first");
    assert_eq!(c.inflight(), 4, "the window stays full, not over-full");
}

#[test]
fn depth_changes_nothing_about_what_is_submitted() {
    // FR-072 in its transport form: concurrency governs how operations are dispatched,
    // never what they are. If depth altered the frames, a run's workload would depend on a
    // tuning knob and a trace would stop being a function of description and seed.
    let turns: Vec<SubmitTurn> = (0..12u64)
        .map(|i| turn(i, &[i, i + 100, i + 200]))
        .collect();

    let submitted = |depth: usize| -> Vec<(u16, Vec<u8>)> {
        let mut mock = Mock::default();
        for corr in 1..=64u32 {
            mock.reply(opcode::SUBMIT_TURN, corr, &outcome(0).encode());
        }
        let mut c = Client::with_transport(mock, depth);
        for t in &turns {
            c.submit(t).unwrap();
        }
        c.finish().unwrap();
        // Correlation ids are transport bookkeeping and are excluded deliberately: what
        // must not vary is the opcode and body of each frame, in order.
        c.into_transport()
            .frames()
            .into_iter()
            .map(|(op, _, body)| (op, body))
            .collect()
    };

    let one = submitted(1);
    let eight = submitted(8);
    let deep = submitted(32);
    assert_eq!(one, eight, "depth 1 and 8 submitted different frames");
    assert_eq!(eight, deep, "depth 8 and 32 submitted different frames");
    assert_eq!(one.len(), turns.len());
    for (op, _) in &one {
        assert_eq!(*op, opcode::SUBMIT_TURN);
    }
}

#[test]
fn a_reply_is_matched_by_correlation_id_rather_than_by_arrival_order() {
    // The contract permits a multiplexed connection, so in-order replies must not become
    // an unstated assumption. Replies are queued here newest-first on purpose.
    let mut mock = Mock::default();
    mock.reply(opcode::SUBMIT_TURN, 3, &outcome(30).encode())
        .reply(opcode::SUBMIT_TURN, 1, &outcome(10).encode())
        .reply(opcode::SUBMIT_TURN, 2, &outcome(20).encode());
    let mut c = Client::with_transport(mock, 8);
    let (c1, _) = c.submit(&turn(1, &[1])).unwrap();
    let (c2, _) = c.submit(&turn(2, &[2])).unwrap();
    let (c3, _) = c.submit(&turn(3, &[3])).unwrap();
    assert_eq!((c1, c2, c3), (1, 2, 3));

    let (got, o) = c.recv_outcome().unwrap();
    assert_eq!(
        (got, o.resident),
        (3, 30),
        "an out-of-order reply must match"
    );
    assert_eq!(c.inflight(), 2, "only the answered id is retired");
    let (got, o) = c.recv_outcome().unwrap();
    assert_eq!((got, o.resident), (1, 10));
    let (got, o) = c.recv_outcome().unwrap();
    assert_eq!((got, o.resident), (2, 20));
    assert_eq!(c.inflight(), 0);
}

#[test]
fn a_correlation_id_that_was_never_sent_is_an_error() {
    // Accepting it would attribute one turn's outcome to another, and the aggregate would
    // still look reasonable.
    let mut mock = Mock::default();
    mock.reply(opcode::SUBMIT_TURN, 999, &outcome(1).encode());
    let mut c = Client::with_transport(mock, 4);
    c.submit(&turn(1, &[1])).unwrap();
    match c.recv_outcome() {
        Err(ClientError::Unexpected { corr }) => assert_eq!(corr, 999),
        other => panic!("expected an unexpected-correlation error, got {other:?}"),
    }
}

#[test]
fn a_reply_with_the_wrong_opcode_is_an_error_not_a_misread() {
    // Decoding a `DrainAck` as a `TurnOutcome` would succeed on length alone for some
    // bodies and produce plausible counters.
    let mut mock = Mock::default();
    mock.reply(opcode::DRAIN, 1, &DrainAck { pending: 7 }.encode());
    let mut c = Client::with_transport(mock, 4);
    c.submit(&turn(1, &[1])).unwrap();
    match c.recv_outcome() {
        Err(ClientError::Mismatched { want, got }) => {
            assert_eq!(want, opcode::SUBMIT_TURN);
            assert_eq!(got, opcode::DRAIN);
        }
        other => panic!("expected a mismatch, got {other:?}"),
    }
}

#[test]
fn a_synchronous_call_is_refused_while_replies_are_outstanding() {
    // Otherwise the next frame off the socket is a `TurnOutcome` and would be decoded as
    // whatever was asked for — a silent misread rather than an error.
    let mut mock = Mock::default();
    mock.reply(opcode::SUBMIT_TURN, 1, &outcome(1).encode())
        .reply(opcode::STATS, 2, &Stats::default().encode());
    let mut c = Client::with_transport(mock, 4);
    c.submit(&turn(1, &[1])).unwrap();
    assert!(
        c.stats().is_err(),
        "stats must refuse with a turn in flight"
    );
    assert!(c.drain().is_err());
    assert!(c.shutdown().is_err());
    assert!(
        c.clear_cache().is_err(),
        "clearing the cache must refuse with a turn in flight; it is setup, and a clear that \
         raced a turn already issued would evict a block the run had just stored"
    );
    // Once drained it is allowed.
    c.finish().unwrap();
    assert!(c.stats().is_ok());
}

#[test]
fn a_closed_connection_is_reported_as_closed_rather_than_as_an_io_error() {
    // A lost node must abort the run and be named (FR-064), so the distinction has to
    // survive to the caller instead of arriving as a generic read failure.
    let mut c = Client::with_transport(Mock::default(), 4);
    c.submit(&turn(1, &[1])).unwrap();
    match c.recv_outcome() {
        Err(ClientError::Closed) => {}
        other => panic!("expected Closed, got {other:?}"),
    }
}

#[test]
fn the_handshake_round_trips_over_the_wire() {
    let build_id = [7u8; BUILD_ID_BYTES];
    let ack = HelloAck {
        proto_version: PROTO_VERSION,
        build_id,
        channels: 8,
        block_bytes: 32768,
        status: 0,
    };
    let mut mock = Mock::default();
    mock.reply(opcode::HELLO, 1, &ack.encode());
    let mut c = Client::with_transport(mock, 4);
    let got = c
        .hello(&Hello {
            proto_version: PROTO_VERSION,
            build_id,
            mailbox: "/dev/shm/certus-shmq".to_string(),
        })
        .unwrap();
    assert_eq!(got, ack);
    assert_eq!(c.inflight(), 0, "a synchronous call must retire its id");
    // The mailbox path must reach the agent: it is what the agent attaches to.
    let sent = c.into_transport().frames();
    assert_eq!(sent[0].0, opcode::HELLO);
    assert_eq!(
        Hello::decode(&sent[0].2).unwrap().mailbox,
        "/dev/shm/certus-shmq"
    );
}

#[test]
fn shutdown_and_drain_and_stats_round_trip() {
    let mut mock = Mock::default();
    mock.reply(opcode::DRAIN, 1, &DrainAck { pending: 3 }.encode())
        .reply(opcode::STATS, 2, &Stats::default().encode())
        .reply(
            opcode::SHUTDOWN,
            3,
            &ShutdownAck {
                ops_submitted: 900,
                ops_failed: 1,
            }
            .encode(),
        );
    let mut c = Client::with_transport(mock, 4);
    assert_eq!(c.drain().unwrap().pending, 3);
    assert!(c.stats().unwrap().ops.is_empty());
    let bye = c.shutdown().unwrap();
    assert_eq!((bye.ops_submitted, bye.ops_failed), (900, 1));
}

#[test]
#[should_panic(expected = "pipelining depth of 0")]
fn a_zero_depth_is_refused_rather_than_promoted_to_one() {
    // Silently treating 0 as 1 would hide a caller's arithmetic error behind a working run.
    let _ = Client::with_transport(Mock::default(), 0);
}

#[test]
fn the_default_depth_is_the_contracts_arithmetic() {
    // 1M keys/s in 64-key batches at a 50 µs round trip needs 0.78 batches in flight, so 8
    // is an order of magnitude of headroom. The reason not to raise it is that there is
    // nothing to gain: "it would add queueing delay to the latency being measured" was
    // withdrawn, since the agent is the sole collector and wire queueing sits outside every
    // reported figure.
    assert_eq!(DEFAULT_DEPTH, 8);
}

#[test]
fn connect_sets_tcp_nodelay_on_a_real_socket() {
    // The one property that cannot be checked over an in-memory transport. Nagle would
    // withhold a small frame until the previous was acknowledged, adding latency to exactly
    // the messages whose latency is the measurement.
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().unwrap();
    let accepted = std::thread::spawn(move || listener.accept().map(|(s, _)| s));

    let client = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let _server = accepted.join().unwrap().expect("accept");
    assert!(
        client.transport().nodelay().unwrap(),
        "TCP_NODELAY must be set on connect"
    );
}

#[test]
fn connect_bounds_writing_as_well_as_reading() {
    // The window makes a blocking write unreachable in practice, so this is defence in depth
    // against the same failure the read timeout closed: an unbounded block is a run that
    // neither finishes nor fails, and cannot even be reported as invalid.
    let (client, _server) = connected(4);
    assert_eq!(
        client.transport().write_timeout().unwrap(),
        Some(DEFAULT_WRITE_TIMEOUT),
        "a write must be bounded, not only a read"
    );
    assert_eq!(
        client.transport().read_timeout().unwrap(),
        Some(DEFAULT_READ_TIMEOUT)
    );
}

/// A connected pair over loopback: the client, and the server end to script replies on.
///
/// Real sockets rather than the `Mock`, because non-blocking reads are a property of a socket
/// and an in-memory transport cannot exhibit them.
fn connected(depth: usize) -> (Client<TcpStream>, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().unwrap();
    let accepted = std::thread::spawn(move || listener.accept().map(|(s, _)| s).expect("accept"));
    let client = Client::<TcpStream>::connect(addr, depth, None).expect("connect");
    (client, accepted.join().unwrap())
}

/// One `SUBMIT_TURN` reply, as bytes on the wire.
fn reply_bytes(corr: u32, resident: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(&outcome(resident).encode());
    w.frame(opcode::SUBMIT_TURN, 0, corr)
}

#[test]
fn every_waiting_reply_is_reaped_at_once_rather_than_one_per_submission() {
    // The point of reaping: a full window drains exactly one reply per submit, so credits come
    // back at the rate they are spent and never faster.
    let (mut client, mut server) = connected(8);
    for i in 0..3u64 {
        client.submit(&turn(i, &[i])).expect("submit");
    }
    assert_eq!(client.inflight(), 3);

    for corr in 1..=3u32 {
        server.write_all(&reply_bytes(corr, corr)).expect("reply");
    }
    server.flush().expect("flush");

    // Poll rather than assume the bytes have crossed loopback: this asserts that *all three*
    // come back in one reap, not how fast the kernel is.
    let mut reaped = 0usize;
    for _ in 0..200 {
        reaped += client.reap_ready().expect("reap");
        if reaped == 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(reaped, 3, "all three waiting replies should come back");
    assert_eq!(client.inflight(), 0, "reaping must return the credits");
}

#[test]
fn reaping_an_empty_socket_returns_nothing_and_does_not_block() {
    let (mut client, _server) = connected(8);
    client.submit(&turn(1, &[1])).expect("submit");

    let started = std::time::Instant::now();
    assert_eq!(client.reap_ready().expect("reap"), 0, "nothing has arrived");
    assert_eq!(client.inflight(), 1, "the credit is still spent");
    // A hang guard, not a latency claim: the failure this catches is a *blocking* read, which
    // would sit here for the whole 30 s read timeout.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "reaping must not block; took {:?}",
        started.elapsed()
    );

    // And with nothing outstanding it must not read at all, or it would consume a frame
    // belonging to a later synchronous call.
    let (mut idle, _s) = connected(8);
    assert_eq!(idle.reap_ready().expect("reap"), 0);
}

#[test]
fn a_node_that_dies_is_seen_by_the_reap_rather_than_depth_turns_later() {
    // The approach this replaced: one blocking drain per submit. With a window of 8, a peer
    // that died is then noticed only when its turn reaches the front of the queue — under a
    // paced run, seconds later. The reap must report it now.
    let (mut client, server) = connected(8);
    client.submit(&turn(1, &[1])).expect("submit");
    drop(server);

    let mut seen = None;
    for _ in 0..200 {
        match client.reap_ready() {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(n) => panic!("a dead peer reaped {n} replies"),
            Err(e) => {
                seen = Some(e);
                break;
            }
        }
    }
    // Either shape is the node lost, and which one arrives is the kernel's business: a peer
    // dropped with a request still unread sends RST, so this is `ConnectionReset`, while one
    // that exits after reading sends FIN and gives `Closed`. Both must be errors, and neither
    // may be a quiet `Ok(0)` — that is the bug.
    assert!(
        matches!(seen, Some(ClientError::Closed | ClientError::Io(_))),
        "a dead peer must be reported, got {seen:?}"
    );
}

#[test]
fn a_peer_that_closes_cleanly_is_reaped_as_closed_rather_than_as_an_io_error() {
    // `Closed` is kept apart from an I/O error because FR-064 wants the lost node named, and an
    // agent that finished its request and then exited is the ordinary shutdown race rather than
    // a broken network.
    let (mut client, mut server) = connected(8);
    client.submit(&turn(1, &[1])).expect("submit");

    // Consume the request, so nothing is unread and the close is a FIN rather than a reset.
    let mut head = [0u8; HEADER_BYTES];
    server.read_exact(&mut head).expect("the request header");
    let h = Header::decode(&head, u32::MAX).expect("a header");
    let mut body = vec![0u8; h.len as usize];
    server.read_exact(&mut body).expect("the request body");
    drop(server);

    let mut seen = None;
    for _ in 0..200 {
        match client.reap_ready() {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(n) => panic!("a closed peer reaped {n} replies"),
            Err(e) => {
                seen = Some(e);
                break;
            }
        }
    }
    assert!(
        matches!(seen, Some(ClientError::Closed)),
        "a clean close must be reported as Closed, got {seen:?}"
    );
}

#[test]
fn a_frame_that_is_only_part_way_here_is_completed_rather_than_abandoned() {
    // Bytes cannot be pushed back onto a stream, so once any byte of a header is taken the
    // frame must be finished — blocking. Getting this wrong would desynchronise the stream and
    // surface much later as a correlation-id error.
    let (mut client, mut server) = connected(8);
    client.submit(&turn(1, &[1])).expect("submit");

    let bytes = reply_bytes(1, 7);
    let rest = bytes[3..].to_vec();
    server.write_all(&bytes[..3]).expect("partial header");
    server.flush().expect("flush");
    let finish = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        server.write_all(&rest).expect("the rest");
        server.flush().expect("flush");
        server
    });

    let mut reaped = 0usize;
    for _ in 0..200 {
        reaped += client.reap_ready().expect("reap");
        if reaped == 1 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _server = finish.join().unwrap();
    assert_eq!(reaped, 1, "the split frame must be reassembled");
    assert_eq!(client.inflight(), 0);
}

#[test]
fn a_reap_leaves_the_socket_blocking_so_a_later_wait_still_waits() {
    // The hazard in doing this with `set_nonblocking`: a socket left non-blocking turns every
    // later read into a spurious `WouldBlock`, which is indistinguishable from the read timeout
    // — so the run would report a lost node for a bookkeeping slip.
    let (mut client, mut server) = connected(8);
    client.submit(&turn(1, &[1])).expect("submit");
    assert_eq!(client.reap_ready().expect("reap"), 0, "nothing yet");

    // Now answer, and let the *blocking* path collect it. If the socket were still
    // non-blocking this would fail instead of waiting.
    let replying = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        server.write_all(&reply_bytes(1, 5)).expect("reply");
        server.flush().expect("flush");
        server
    });
    let rest = client.finish().expect("finish must block, not fail");
    let _server = replying.join().unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].resident, 5);
}
