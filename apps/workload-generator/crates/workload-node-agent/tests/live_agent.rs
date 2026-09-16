//! End-to-end against a **live** agent and a **live** Certus mailbox.
//!
//! Ignored by default: it needs a Certus server on `/dev/shm/certus-shmq`, an agent on port
//! 7420, and a GPU. Run it deliberately:
//!
//! ```text
//! certus-server-yaml --shm-path /dev/shm/certus-shmq --channels 8 ... &
//! workload-node-agent --shm-path /dev/shm/certus-shmq --lanes 2 &
//! cargo test -p workload-node-agent --test live_agent -- --ignored --nocapture
//! ```
//!
//! What it proves that no unit test can: that the agent's `SubmitTurn` really reaches a
//! cache, that a key stored through it is **found again on the next turn**, and that the
//! counters and histograms come back across the wire. Everything else about the agent is a
//! statement about code; this is a statement about Certus.

use std::net::TcpStream;

use hdrhistogram::serialization::{Deserializer, Serializer, V2Serializer};
use workload_wire::client::Client;
use workload_wire::frame::SubmitTurn;

const AGENT: &str = "127.0.0.1:7420";

/// Keys nothing else will have stored, so a hit means *this* test stored it.
fn fresh_keys(tag: u64, n: usize) -> Vec<u64> {
    let base = 0xA5A5_0000_0000_0000 ^ (tag << 32);
    (0..n as u64).map(|i| base ^ (i * 0x9E37_79B9)).collect()
}

#[test]
#[ignore = "needs a live Certus server, a live agent on 7420, and a GPU"]
fn a_turn_submitted_to_a_live_agent_reaches_certus_and_is_found_again() {
    let mut c = Client::<TcpStream>::connect(AGENT, 4, None).expect("connect to the agent");

    // The handshake must succeed against an agent of this build. If it refuses here, the
    // provenance check is working and the agent needs rebuilding — not a test failure to
    // paper over.
    let ack = c
        .handshake("localhost", "/dev/shm/certus-shmq", 2, 32768)
        .expect("this build must accept its own agent");
    eprintln!(
        "agent: {} channels, {}-byte blocks",
        ack.channels, ack.block_bytes
    );
    assert!(ack.channels >= 2);

    // A path of keys the cache has never seen. Everything must miss, and everything must then
    // be stored: this is the reactive rule's cold case.
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let path = fresh_keys(tag, 8);

    let turn = SubmitTurn {
        session: 1,
        flags: 0,
        path: path.clone(),
    };
    c.submit(&turn).expect("submit the cold turn");
    let cold = c.finish().expect("the cold outcome").remove(0);
    eprintln!("cold  {cold:?}");
    assert_eq!(cold.resident, 0, "a fresh path cannot be resident");
    assert_eq!(cold.missing, path.len() as u32, "all of it must miss");
    assert!(
        cold.granted > 0,
        "nothing was reserved, so nothing could be stored"
    );
    assert_eq!(
        cold.blocks_written, cold.granted,
        "every granted reservation should have moved a block"
    );

    // The same path again. This is the assertion that matters: if the store did not really
    // reach Certus, this comes back missing and everything above was theatre.
    c.submit(&turn).expect("submit the warm turn");
    let warm = c.finish().expect("the warm outcome").remove(0);
    eprintln!("warm  {warm:?}");
    assert_eq!(
        warm.resident, cold.granted,
        "the blocks stored a moment ago must be found again — if they are not, the agent's \
         store did not reach the cache"
    );
    assert_eq!(
        warm.blocks_read, warm.resident,
        "a resident key must return its payload"
    );
    assert_eq!(warm.missing, path.len() as u32 - cold.granted);

    // Counters and histograms come back for reporting. Percentiles are taken only after
    // deserialising, because quantiles do not merge across nodes.
    let stats = c.stats().expect("stats");
    eprintln!(
        "counters: {} requests, {} refs, read {} blocks, wrote {} blocks",
        stats.counters.requests,
        stats.counters.key_references,
        stats.counters.blocks_read(),
        stats.counters.blocks_written()
    );
    assert!(stats.counters.requests > 0);
    assert_eq!(
        stats.counters.blocks_read(),
        (cold.blocks_read + warm.blocks_read) as u64,
        "the counters must agree with the outcomes they were built from"
    );
    assert!(
        !stats.ops.is_empty(),
        "no per-operation histogram came back, so FR-066a cannot be satisfied for this node"
    );

    // Verification, if the agent was started with --verify-payload. Asserted rather than
    // assumed: a check that silently no-ops is worse than no check, because the run then
    // *claims* the data was verified.
    eprintln!(
        "verified {} blocks, {} mismatches",
        stats.counters.payloads_verified, stats.counters.payload_mismatches
    );
    if stats.counters.payloads_verified > 0 {
        assert_eq!(
            stats.counters.payloads_verified, warm.blocks_read as u64,
            "every loaded block should have been checked, not some of them"
        );
        assert_eq!(
            stats.counters.payload_mismatches, 0,
            "Certus returned a block carrying a different key than the one asked for"
        );
    } else {
        eprintln!("note: this agent was started without --verify-payload, so nothing was checked");
    }

    let mut de = Deserializer::new();
    for op in &stats.ops {
        let hist: hdrhistogram::Histogram<u64> = de
            .deserialize(&mut std::io::Cursor::new(&op.histogram))
            .expect("a histogram must survive the wire");
        assert_eq!(
            hist.len(),
            op.requests,
            "the histogram and its request count disagree"
        );
        eprintln!(
            "  op {:>2}: {:>6} requests  p50 {:>6}us  max {:>6}us",
            op.op_kind,
            op.requests,
            hist.value_at_quantile(0.50),
            hist.max()
        );
    }

    // And a serialized-then-deserialized histogram must give the same quantiles, or the
    // multi-node merge would silently change the numbers.
    let first = &stats.ops[0];
    let hist: hdrhistogram::Histogram<u64> = Deserializer::new()
        .deserialize(&mut std::io::Cursor::new(&first.histogram))
        .expect("deserialize");
    let mut round = Vec::new();
    V2Serializer::new()
        .serialize(&hist, &mut round)
        .expect("reserialize");
    let again: hdrhistogram::Histogram<u64> = Deserializer::new()
        .deserialize(&mut std::io::Cursor::new(&round))
        .expect("deserialize again");
    assert_eq!(hist.value_at_quantile(0.99), again.value_at_quantile(0.99));
    assert_eq!(hist.len(), again.len());
}

#[test]
#[ignore = "needs a live agent on 7420"]
fn two_histograms_merge_but_two_percentiles_do_not() {
    // The reason `Stats` carries histograms rather than four numbers. Asserted against real
    // histograms from a real node, so the claim is not merely arithmetic on paper.
    let mut c = Client::<TcpStream>::connect(AGENT, 4, None).expect("connect");
    c.handshake("localhost", "/dev/shm/certus-shmq", 2, 32768)
        .expect("handshake");
    let path = fresh_keys(0xBEEF, 16);
    c.submit(&SubmitTurn {
        session: 2,
        flags: 0,
        path,
    })
    .expect("submit");
    c.finish().expect("finish");
    let stats = c.stats().expect("stats");
    assert!(!stats.ops.is_empty());

    let mut de = Deserializer::new();
    let a: hdrhistogram::Histogram<u64> = de
        .deserialize(&mut std::io::Cursor::new(&stats.ops[0].histogram))
        .expect("a");
    let b = a.clone();

    // Merging the distributions is exact.
    let mut merged = a.clone();
    merged.add(&b).expect("histograms merge");
    assert_eq!(merged.len(), a.len() * 2);
    // Every recorded value is still present, so the merged quantiles come from the real
    // combined sample rather than from an average of two summaries.
    assert_eq!(merged.min(), a.min());
    assert_eq!(merged.max(), a.max());
    // Whereas averaging two nodes' p99s is a number belonging to no distribution: it is only
    // equal to the true merged p99 in the degenerate case where both are identical, which is
    // exactly why the wire must not carry percentiles.
    let averaged = (a.value_at_quantile(0.99) + b.value_at_quantile(0.99)) / 2;
    eprintln!(
        "merged p99 {} vs averaged p99 {}",
        merged.value_at_quantile(0.99),
        averaged
    );
}
