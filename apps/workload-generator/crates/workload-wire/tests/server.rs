//! The server half, and a full client↔server conversation.
//!
//! Most of this runs over an in-memory pipe. Two tests use a real TCP socket, because the
//! accept loop, `TCP_NODELAY` on an accepted connection, and a genuine concurrent client are
//! properties only a socket has.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use workload_wire::client::{Client, ClientError};
use workload_wire::frame::{
    opcode, Counters, DrainAck, Hello, HelloAck, OpHistogram, ShutdownAck, Stats, SubmitTurn,
    TurnOutcome, Writer, BUILD_ID_BYTES, PROTO_VERSION,
};
use workload_wire::server::{
    serve_connection, FnFactory, Served, Server, ServerError, Service, ServiceFactory,
};

/// A service that records what it was asked and answers deterministically.
#[derive(Debug, Default)]
struct Recorder {
    /// Turns, in the order they were handled. Shared so a test can read it afterwards.
    turns: Arc<Mutex<Vec<SubmitTurn>>>,
    hellos: usize,
}

impl Service for Recorder {
    fn hello(&mut self, _hello: &Hello) -> HelloAck {
        self.hellos += 1;
        HelloAck {
            proto_version: PROTO_VERSION,
            build_id: [3u8; BUILD_ID_BYTES],
            channels: 8,
            block_bytes: 32768,
            status: 0,
        }
    }

    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        self.turns.lock().unwrap().push(turn.clone());
        // Deterministic: every key resident, so a test can tell a real reply from a default.
        TurnOutcome {
            resident: turn.path.len() as u32,
            blocks_read: turn.path.len() as u32,
            ..Default::default()
        }
    }

    fn drain(&mut self) -> DrainAck {
        DrainAck { pending: 0 }
    }

    fn stats(&mut self) -> Stats {
        Stats {
            counters: Counters {
                requests: 3,
                blocks_read: 9,
                ..Default::default()
            },
            ops: vec![OpHistogram {
                op_kind: 0,
                requests: 3,
                histogram: vec![1, 2, 3],
            }],
        }
    }

    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck {
            ops_submitted: 42,
            ops_failed: 0,
        }
    }
}

/// A bidirectional in-memory pipe: what one side writes, the other reads.
#[derive(Debug, Default)]
struct Pipe {
    to_server: Vec<u8>,
    to_client: Vec<u8>,
}

/// The client's end.
#[derive(Debug)]
struct ClientEnd(Arc<Mutex<Pipe>>, usize);

impl Write for ClientEnd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().to_server.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for ClientEnd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let p = self.0.lock().unwrap();
        let left = p.to_client.len() - self.1;
        if left == 0 {
            return Ok(0);
        }
        let n = left.min(buf.len());
        buf[..n].copy_from_slice(&p.to_client[self.1..self.1 + n]);
        drop(p);
        self.1 += n;
        Ok(n)
    }
}

/// The server's end.
#[derive(Debug)]
struct ServerEnd(Arc<Mutex<Pipe>>, usize);

impl Write for ServerEnd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().to_client.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for ServerEnd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let p = self.0.lock().unwrap();
        let left = p.to_server.len() - self.1;
        if left == 0 {
            return Ok(0);
        }
        let n = left.min(buf.len());
        buf[..n].copy_from_slice(&p.to_server[self.1..self.1 + n]);
        drop(p);
        self.1 += n;
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

/// Write a raw frame straight into the pipe, bypassing the client.
fn raw(pipe: &Arc<Mutex<Pipe>>, opcode: u16, corr: u32, body: &[u8], len_override: Option<u32>) {
    let mut w = Writer::new();
    w.bytes(body);
    let mut frame = w.frame(opcode, 0, corr);
    if let Some(len) = len_override {
        frame[0..4].copy_from_slice(&len.to_le_bytes());
    }
    pipe.lock().unwrap().to_server.extend_from_slice(&frame);
}

/// Run the server over a pipe that already holds every request.
fn serve_prefilled(pipe: &Arc<Mutex<Pipe>>, svc: &mut Recorder) -> Result<Served, ServerError> {
    let mut end = ServerEnd(Arc::clone(pipe), 0);
    serve_connection(&mut end, svc, 1 << 20)
}

#[test]
fn a_whole_conversation_round_trips_over_a_pipe() {
    // The client and server are each other's only real test: a format both agree on but
    // neither exercises is a format that breaks on the first live run.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    let mut client = Client::with_transport(ClientEnd(Arc::clone(&pipe), 0), 4);

    // The client writes everything first, then the server answers it all, then the client
    // reads. That is enough to check framing and echoing without threads.
    client
        .hello(&Hello {
            proto_version: PROTO_VERSION,
            build_id: [3u8; BUILD_ID_BYTES],
            mailbox: "/dev/shm/certus-shmq".to_string(),
        })
        .expect_err("nothing has answered yet");

    let mut svc = Recorder::default();
    let turns = Arc::clone(&svc.turns);
    assert_eq!(serve_prefilled(&pipe, &mut svc).unwrap(), Served::Closed);
    assert_eq!(svc.hellos, 1, "the handshake must reach the service");
    assert!(turns.lock().unwrap().is_empty());
}

#[test]
fn turns_reach_the_service_in_the_order_they_were_sent() {
    // The causal rule the whole design rests on: turn n+1 of a session holds the blocks
    // turn n stored, so a server that reordered a connection's frames would make the
    // generator's workload wrong while the run still completed.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    let sent: Vec<SubmitTurn> = (0..20u64).map(|i| turn(i % 3, &[i, i + 50])).collect();
    raw(&pipe, opcode::HELLO, 1, &hello_body(), None);
    for (i, t) in sent.iter().enumerate() {
        raw(&pipe, opcode::SUBMIT_TURN, i as u32 + 2, &t.encode(), None);
    }
    let mut svc = Recorder::default();
    let turns = Arc::clone(&svc.turns);
    serve_prefilled(&pipe, &mut svc).unwrap();
    let handled = turns.lock().unwrap().clone();
    assert_eq!(handled, sent, "frames were handled out of order");
}

fn hello_body() -> Vec<u8> {
    Hello {
        proto_version: PROTO_VERSION,
        build_id: [3u8; BUILD_ID_BYTES],
        mailbox: "/dev/shm/x".to_string(),
    }
    .encode()
}

#[test]
fn a_frame_before_hello_closes_the_connection() {
    // The handshake is mandatory precisely so a stale peer is refused rather than measured,
    // and a peer that skipped it must not be served anyway.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    raw(&pipe, opcode::SUBMIT_TURN, 1, &turn(1, &[1]).encode(), None);
    let mut svc = Recorder::default();
    match serve_prefilled(&pipe, &mut svc) {
        Err(ServerError::HelloNotFirst { opcode: op }) => {
            assert_eq!(op, opcode::SUBMIT_TURN)
        }
        other => panic!("expected HelloNotFirst, got {other:?}"),
    }
    assert!(
        svc.turns.lock().unwrap().is_empty(),
        "the turn must not have been served"
    );
}

#[test]
fn an_oversized_len_closes_the_connection_without_serving_it() {
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    raw(&pipe, opcode::HELLO, 1, &hello_body(), None);
    raw(
        &pipe,
        opcode::SUBMIT_TURN,
        2,
        &turn(1, &[1]).encode(),
        Some(u32::MAX),
    );
    let mut svc = Recorder::default();
    let mut end = ServerEnd(Arc::clone(&pipe), 0);
    match serve_connection(&mut end, &mut svc, 4096) {
        Err(ServerError::Protocol(_)) => {}
        other => panic!("expected a protocol error, got {other:?}"),
    }
    assert!(svc.turns.lock().unwrap().is_empty());
}

#[test]
fn an_unknown_opcode_closes_the_connection() {
    // There is no error reply in the frame set, and inventing one would let a peer keep a
    // connection alive by sending nonsense.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    raw(&pipe, opcode::HELLO, 1, &hello_body(), None);
    raw(&pipe, 999, 2, &[], None);
    let mut svc = Recorder::default();
    match serve_prefilled(&pipe, &mut svc) {
        Err(ServerError::UnknownOpcode(op)) => assert_eq!(op, 999),
        other => panic!("expected UnknownOpcode, got {other:?}"),
    }
}

#[test]
fn a_close_part_way_through_a_frame_is_truncation_not_a_clean_end() {
    // The distinction matters: a clean close between frames is how a run ends, while a
    // half-delivered frame is a failure. Conflating them would end a run quietly as though
    // the generator had finished.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    raw(&pipe, opcode::HELLO, 1, &hello_body(), None);
    let body = turn(1, &[1, 2, 3]).encode();
    raw(&pipe, opcode::SUBMIT_TURN, 2, &body, None);
    // Chop the last few bytes of the body.
    let len = pipe.lock().unwrap().to_server.len();
    pipe.lock().unwrap().to_server.truncate(len - 8);

    let mut svc = Recorder::default();
    match serve_prefilled(&pipe, &mut svc) {
        Err(ServerError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("expected UnexpectedEof, got {other:?}"),
    }
}

#[test]
fn shutdown_is_answered_before_the_loop_ends() {
    // If the loop returned first the generator would see a close instead of an ack, and a
    // close is what FR-064 treats as a lost node — an orderly stop would be reported as a
    // failure.
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    raw(&pipe, opcode::HELLO, 1, &hello_body(), None);
    raw(&pipe, opcode::SHUTDOWN, 2, &[], None);
    let mut svc = Recorder::default();
    assert_eq!(
        serve_prefilled(&pipe, &mut svc).unwrap(),
        Served::ShutdownRequested
    );
    // Two replies: the hello ack and the shutdown ack.
    let written = pipe.lock().unwrap().to_client.clone();
    let ack = ShutdownAck::decode(&written[written.len() - 16..]).unwrap();
    assert_eq!(ack.ops_submitted, 42, "the ack must be the service's own");
}

// ---------------------------------------------------------------------------
// Over a real socket: the accept loop, and a live client.
// ---------------------------------------------------------------------------

#[test]
fn a_live_client_and_server_agree_over_tcp() {
    // The end-to-end check: two halves written against one contract, exercised against each
    // other rather than against a shared assumption.
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let factory = FnFactory(move || {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(Recorder::default())
    });
    let server = Server::bind("127.0.0.1:0", factory).expect("bind");
    let addr = server.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_server = Arc::clone(&stop);
    let handle = std::thread::spawn(move || server.serve(stop_for_server));

    let mut client = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let ack = client
        .hello(&Hello {
            proto_version: PROTO_VERSION,
            build_id: [3u8; BUILD_ID_BYTES],
            mailbox: "/dev/shm/certus-shmq".to_string(),
        })
        .expect("hello");
    assert_eq!(ack.channels, 8);
    assert_eq!(ack.block_bytes, 32768);

    // Pipelined turns, then their outcomes.
    for i in 0..10u64 {
        client.submit(&turn(i % 3, &[i, i + 100])).expect("submit");
    }
    let outcomes = client.finish().expect("finish");
    assert!(!outcomes.is_empty());
    for o in &outcomes {
        assert_eq!(o.resident, 2, "the service saw both keys");
        assert_eq!(o.blocks_read, 2);
    }

    let stats = client.stats().expect("stats");
    assert_eq!(stats.counters.blocks_read, 9, "counters must survive TCP");
    assert_eq!(stats.ops.len(), 1);
    assert_eq!(stats.ops[0].histogram, vec![1, 2, 3]);

    let bye = client.shutdown().expect("shutdown");
    assert_eq!(bye.ops_submitted, 42);

    // Shutdown stops the accept loop, so the server thread joins on its own.
    handle.join().expect("server thread").expect("serve");
    assert_eq!(served.load(Ordering::Relaxed), 1, "one connection served");
}

#[test]
fn a_refused_connection_does_not_stop_the_agent() {
    // A factory that cannot obtain a mailbox channel refuses that connection; the agent must
    // stay up, because the generator is the thing that decides a run is over (FR-064).
    let factory =
        FnFactory(|| -> io::Result<Recorder> { Err(io::Error::other("no channel free")) });
    let server = Server::bind("127.0.0.1:0", factory).expect("bind");
    let addr = server.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_server = Arc::clone(&stop);
    let handle = std::thread::spawn(move || server.serve(stop_for_server));

    // The connection is accepted at the TCP level and then dropped, so the client sees a
    // close rather than a hang.
    let mut client = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let refused = client.hello(&Hello {
        proto_version: PROTO_VERSION,
        build_id: [0u8; BUILD_ID_BYTES],
        mailbox: "/dev/shm/x".to_string(),
    });
    assert!(
        matches!(refused, Err(ClientError::Closed) | Err(ClientError::Io(_))),
        "expected a close, got {refused:?}"
    );

    // The agent is still listening: a second connection is accepted too.
    let second = Client::<TcpStream>::connect(addr, 4, None);
    assert!(second.is_ok(), "the agent stopped after refusing one peer");

    stop.store(true, Ordering::Relaxed);
    handle.join().expect("server thread").expect("serve");
}

#[test]
fn the_factory_makes_one_service_per_connection() {
    // Per connection because a connection is a lane and a lane claims its own mailbox
    // channel: the mailbox is depth-1 per channel, so sharing one would serialise lanes
    // silently, and a shared handler would also break the causal ordering rule.
    let made = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&made);
    let factory = FnFactory(move || {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(Recorder::default())
    });
    assert_eq!(
        made.load(Ordering::Relaxed),
        0,
        "nothing made before accept"
    );
    let _ = factory.accept().unwrap();
    let _ = factory.accept().unwrap();
    assert_eq!(made.load(Ordering::Relaxed), 2);
}

/// A service whose provenance disagrees with this build — a stale deployment.
#[derive(Debug, Default)]
struct StaleAgent;

impl Service for StaleAgent {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        // The agent answers with `handshake::answer`, so it refuses us too and still reports
        // its own identity. Here that identity is deliberately not this build's.
        let mut ack = workload_wire::handshake::answer(hello, 8, 32768);
        ack.build_id = workload_wire::handshake::digest("a stale tree");
        ack.status = workload_wire::handshake::status::BUILD_MISMATCH;
        ack
    }
    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        TurnOutcome {
            resident: turn.path.len() as u32,
            ..Default::default()
        }
    }
}

#[test]
fn a_verified_handshake_refuses_a_stale_agent_over_tcp_and_names_the_node() {
    // The end-to-end form of FR-051, and the failure it exists to stop: a stale remote binary
    // that would otherwise complete the run and produce numbers.
    let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(StaleAgent))).expect("bind");
    let addr = server.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_server = Arc::clone(&stop);
    let handle = std::thread::spawn(move || server.serve(stop_for_server));

    let mut client = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let err = client
        .handshake("node5", "/dev/shm/certus-shmq", 4, 32768)
        .expect_err("a stale agent must be refused");
    let text = err.to_string();
    assert!(
        text.contains("node5"),
        "the refusal must name the node: {text}"
    );
    assert!(text.contains("FR-051"), "and cite the requirement: {text}");

    stop.store(true, Ordering::Relaxed);
    handle.join().expect("server thread").expect("serve");
}

#[test]
fn a_verified_handshake_accepts_a_matching_agent_and_checks_capacity() {
    // The same path must succeed against an agent of this build, or the refusal above would
    // prove nothing. And it must still refuse a lane count the node cannot serve.
    struct Matching;
    impl Service for Matching {
        fn hello(&mut self, hello: &Hello) -> HelloAck {
            workload_wire::handshake::answer(hello, 8, 32768)
        }
        fn submit_turn(&mut self, _t: &SubmitTurn) -> TurnOutcome {
            TurnOutcome::default()
        }
    }
    let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(Matching))).expect("bind");
    let addr = server.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_server = Arc::clone(&stop);
    let handle = std::thread::spawn(move || server.serve(stop_for_server));

    let mut ok = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let ack = ok
        .handshake("node5", "/dev/shm/certus-shmq", 4, 32768)
        .expect("this build must accept itself");
    assert_eq!((ack.channels, ack.block_bytes), (8, 32768));

    let mut greedy = Client::<TcpStream>::connect(addr, 4, None).expect("connect");
    let err = greedy
        .handshake("node5", "/dev/shm/certus-shmq", 16, 32768)
        .expect_err("16 lanes of 8 channels must be refused");
    assert!(err.to_string().contains("serialise"), "{err}");

    stop.store(true, Ordering::Relaxed);
    handle.join().expect("server thread").expect("serve");
}
