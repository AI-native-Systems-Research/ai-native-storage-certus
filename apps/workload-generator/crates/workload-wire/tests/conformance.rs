//! Wire conformance, from `contracts/node-agent-wire.md`'s own list.
//!
//! These are written before the client and the server (T064 before T065/T066), which is
//! the point: the contract states the frame layout and the refusal rules, so the expected
//! values exist before any code does. A test written afterwards tends to assert what the
//! encoder happens to produce.
//!
//! Everything here runs with **no agent, no server and no accelerator** — the format is
//! bytes, and a format that can only be checked against a live peer is a format that will
//! be checked rarely.

use workload_wire::frame::{
    opcode, submit_flags, Counters, DrainAck, Header, Hello, HelloAck, OpHistogram, ShutdownAck,
    Stats, SubmitTurn, TurnOutcome, WireError, BUILD_ID_BYTES, HEADER_BYTES, PROTO_VERSION,
};

/// A recognisable digest, so a shifted or truncated copy is visible rather than plausible.
fn build_id(seed: u8) -> [u8; BUILD_ID_BYTES] {
    let mut id = [0u8; BUILD_ID_BYTES];
    for (i, b) in id.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8);
    }
    id
}

/// A generous body bound for tests that are not about the bound.
const MAX_BODY: u32 = 1 << 20;

// ---------------------------------------------------------------------------
// 1. Round-trip encode/decode for every opcode, including a zero-key SubmitTurn.
// ---------------------------------------------------------------------------

#[test]
fn every_body_round_trips() {
    let hello = Hello {
        proto_version: PROTO_VERSION,
        build_id: build_id(1),
        mailbox: "/dev/shm/certus-shmq".to_string(),
    };
    assert_eq!(Hello::decode(&hello.encode()).unwrap(), hello);

    let ack = HelloAck {
        proto_version: PROTO_VERSION,
        build_id: build_id(2),
        channels: 8,
        block_bytes: 32768,
        status: 0,
    };
    assert_eq!(HelloAck::decode(&ack.encode()).unwrap(), ack);

    let turn = SubmitTurn {
        session: 0xDEAD_BEEF_CAFE_1234,
        flags: submit_flags::POLL_EVENTS,
        path: vec![1, 2, 3, u64::MAX, 0],
    };
    let back = SubmitTurn::decode(&turn.encode()).unwrap();
    assert_eq!(back, turn);
    assert!(back.polls_events());

    let outcome = TurnOutcome {
        resident: 7,
        pending: 1,
        missing: 3,
        granted: 2,
        blocks_read: 7,
        blocks_written: 2,
        elapsed_ns: 123_456_789,
    };
    assert_eq!(TurnOutcome::decode(&outcome.encode()).unwrap(), outcome);

    let stats = Stats {
        counters: Counters::default(),
        ops: vec![
            OpHistogram {
                op_kind: 0,
                requests: 100,
                histogram: vec![9, 8, 7],
            },
            OpHistogram {
                op_kind: 3,
                requests: 0,
                histogram: Vec::new(),
            },
        ],
    };
    assert_eq!(Stats::decode(&stats.encode()).unwrap(), stats);

    let shutdown = ShutdownAck {
        ops_submitted: 12_345,
        ops_failed: 6,
    };
    assert_eq!(ShutdownAck::decode(&shutdown.encode()).unwrap(), shutdown);

    let drain = DrainAck { pending: 4 };
    assert_eq!(DrainAck::decode(&drain.encode()).unwrap(), drain);
}

#[test]
fn a_zero_key_submit_turn_is_legal_and_round_trips() {
    // Explicitly in the contract's list. A turn with no path is not a malformed frame —
    // and encoding it as "absent" rather than "empty" is how an off-by-one in the count
    // field escapes notice.
    let turn = SubmitTurn {
        session: 42,
        flags: 0,
        path: Vec::new(),
    };
    let bytes = turn.encode();
    let back = SubmitTurn::decode(&bytes).unwrap();
    assert_eq!(back, turn);
    assert!(back.path.is_empty());
    assert!(!back.polls_events());
    // 8 session + 2 flags + 4 count, and nothing else.
    assert_eq!(bytes.len(), 14);
}

#[test]
fn the_header_is_twelve_little_endian_bytes_in_the_documented_order() {
    // The layout the contract fixes: len, opcode, flags, corr. A field in the wrong place
    // still decodes to *something*, which is why this asserts the bytes and not a
    // round trip.
    let h = Header {
        len: 0x0403_0201,
        opcode: 0x0605,
        flags: 0x0807,
        corr: 0x0C0B_0A09,
    };
    let bytes = h.encode();
    assert_eq!(bytes.len(), HEADER_BYTES);
    assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    // `u32::MAX` as the bound: this asserts the field order, and the byte pattern that
    // makes a misplaced field visible is itself a large `len`. The bound is tested
    // separately.
    assert_eq!(Header::decode(&bytes, u32::MAX).unwrap(), h);
}

#[test]
fn a_frame_carries_its_header_and_body_together() {
    let turn = SubmitTurn {
        session: 5,
        flags: 0,
        path: vec![11, 22],
    };
    let body = turn.encode();
    let frame = workload_wire::frame::Writer::new()
        .bytes(&body)
        .as_slice()
        .to_vec();
    assert_eq!(frame, body);

    let mut w = workload_wire::frame::Writer::new();
    w.bytes(&body);
    let whole = w.frame(opcode::SUBMIT_TURN, 0, 77);
    let header = Header::decode(&whole, MAX_BODY).unwrap();
    assert_eq!(header.opcode, opcode::SUBMIT_TURN);
    assert_eq!(header.corr, 77, "the correlation id must be echoable");
    assert_eq!(header.len as usize, body.len());
    assert_eq!(
        SubmitTurn::decode(&whole[HEADER_BYTES..]).unwrap(),
        turn,
        "the body must start exactly after the header"
    );
}

// ---------------------------------------------------------------------------
// 2. An oversized `len` is rejected without allocating.
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_len_is_refused_before_anything_is_sized_from_it() {
    // The whole point is that the refusal happens on the *declared* length, so no
    // allocation is ever attempted. A reader that trusted `len` would let any peer, or any
    // corrupted stream, ask for an arbitrary allocation.
    let h = Header {
        len: u32::MAX,
        opcode: opcode::SUBMIT_TURN,
        flags: 0,
        corr: 1,
    };
    let err = Header::decode(&h.encode(), 4096).unwrap_err();
    assert_eq!(
        err,
        WireError::TooLarge {
            len: u32::MAX,
            max: 4096
        }
    );
    // And the message names both figures, because a refusal that says only "too large"
    // cannot be acted on.
    let text = err.to_string();
    assert!(text.contains(&u32::MAX.to_string()), "got: {text}");
    assert!(text.contains("4096"), "got: {text}");

    // Exactly at the bound is legal; one over is not.
    let at = Header { len: 4096, ..h };
    assert!(Header::decode(&at.encode(), 4096).is_ok());
    let over = Header { len: 4097, ..h };
    assert!(Header::decode(&over.encode(), 4096).is_err());
}

#[test]
fn a_declared_key_count_cannot_drive_an_allocation() {
    // The same hazard one level down: `SubmitTurn`'s count is a u32 from the peer, so
    // sizing a vector from it before checking the bytes present would let 14 bytes on the
    // wire ask for 32 GB of memory.
    let mut w = workload_wire::frame::Writer::new();
    w.u64(1).u16(0).u32(u32::MAX);
    let err = SubmitTurn::decode(w.as_slice()).unwrap_err();
    assert!(
        matches!(err, WireError::Truncated { .. }),
        "a huge count with no keys must be truncation, not an allocation: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 3 & 4. Hello is fail-closed on build_id and proto_version.
// ---------------------------------------------------------------------------

#[test]
fn hello_survives_a_round_trip_so_a_mismatch_is_detectable_rather_than_lost() {
    // The refusal itself belongs to the client and server (T067). What the *format* must
    // guarantee is that both fields arrive intact, since a digest mangled in transit would
    // fail the comparison for the wrong reason and send an operator hunting a stale binary
    // that does not exist.
    let mine = build_id(0xA0);
    let theirs = build_id(0xB0);
    assert_ne!(mine, theirs);

    let hello = Hello {
        proto_version: PROTO_VERSION,
        build_id: theirs,
        mailbox: "/dev/shm/x".to_string(),
    };
    let back = Hello::decode(&hello.encode()).unwrap();
    assert_eq!(back.build_id, theirs, "every one of 32 bytes must survive");
    assert_ne!(back.build_id, mine);

    let old = Hello {
        proto_version: PROTO_VERSION - 1,
        build_id: mine,
        mailbox: "/dev/shm/x".to_string(),
    };
    assert_eq!(
        Hello::decode(&old.encode()).unwrap().proto_version,
        PROTO_VERSION - 1
    );
}

#[test]
fn the_protocol_version_is_two_because_submit_became_per_turn() {
    // Pinned deliberately. Version 1 sent one operation per frame; version 2 sends a
    // turn's key path and the agent applies FR-072a's rule. An agent speaking version 1
    // would interpret a path as an operation's key list and issue something plausible, so
    // the version must not be bumped silently.
    assert_eq!(PROTO_VERSION, 2);
}

// ---------------------------------------------------------------------------
// 5. A truncated frame mid-body is an error, not a partial parse.
// ---------------------------------------------------------------------------

#[test]
fn every_body_truncated_at_every_length_is_an_error_never_a_partial_value() {
    // Exhaustive rather than illustrative: a decoder that stopped early would return a
    // value with the remaining fields left at zero, and a half-decoded `SubmitTurn` submits
    // a turn whose keys the generator never sent.
    let turn = SubmitTurn {
        session: 9,
        flags: 1,
        path: vec![1, 2, 3],
    };
    let full = turn.encode();
    for cut in 0..full.len() {
        assert!(
            SubmitTurn::decode(&full[..cut]).is_err(),
            "a body cut to {cut} of {} bytes parsed",
            full.len()
        );
    }
    assert!(SubmitTurn::decode(&full).is_ok());

    let hello = Hello {
        proto_version: PROTO_VERSION,
        build_id: build_id(7),
        mailbox: "/dev/shm/certus-shmq".to_string(),
    };
    let full = hello.encode();
    for cut in 0..full.len() {
        assert!(
            Hello::decode(&full[..cut]).is_err(),
            "a hello cut to {cut} parsed"
        );
    }

    let outcome = TurnOutcome {
        resident: 1,
        pending: 2,
        missing: 3,
        granted: 4,
        blocks_read: 5,
        blocks_written: 6,
        elapsed_ns: 7,
    };
    let full = outcome.encode();
    for cut in 0..full.len() {
        assert!(
            TurnOutcome::decode(&full[..cut]).is_err(),
            "an outcome cut to {cut} parsed"
        );
    }

    let stats = Stats {
        counters: Counters::default(),
        ops: vec![OpHistogram {
            op_kind: 2,
            requests: 5,
            histogram: vec![1, 2, 3, 4],
        }],
    };
    let full = stats.encode();
    for cut in 0..full.len() {
        assert!(
            Stats::decode(&full[..cut]).is_err(),
            "stats cut to {cut} parsed"
        );
    }
}

#[test]
fn a_truncated_header_is_an_error() {
    let h = Header {
        len: 0,
        opcode: opcode::DRAIN,
        flags: 0,
        corr: 1,
    };
    let full = h.encode();
    for cut in 0..HEADER_BYTES {
        assert!(
            Header::decode(&full[..cut], MAX_BODY).is_err(),
            "a header cut to {cut} parsed"
        );
    }
}

#[test]
fn a_non_utf8_mailbox_path_is_refused_rather_than_lossily_accepted() {
    // Attaching to a path that is not what the operator wrote would fail later and
    // somewhere else, which is the expensive kind of failure on a cluster node.
    let mut w = workload_wire::frame::Writer::new();
    w.u32(PROTO_VERSION)
        .bytes(&build_id(1))
        .u16(2)
        .bytes(&[0xFF, 0xFE]);
    assert!(matches!(
        Hello::decode(w.as_slice()),
        Err(WireError::Invalid(_))
    ));
}

// ---------------------------------------------------------------------------
// 7. Histograms merge; percentiles do not.
// ---------------------------------------------------------------------------

#[test]
fn the_stats_reply_carries_bytes_so_histograms_can_be_merged_not_percentiles() {
    // The reason `Stats` returns a serialized histogram rather than four numbers. The
    // format's job is only to carry the bytes intact; that they *merge* is asserted where
    // the merging happens. What is asserted here is that nothing in the frame forces a
    // lossy summary — an `OpHistogram` whose payload were four u64 percentiles would make
    // a correct multi-node merge impossible at the protocol level.
    let stats = Stats {
        counters: Counters::default(),
        ops: vec![
            OpHistogram {
                op_kind: 0,
                requests: 8_000,
                histogram: (0u8..=255).collect(),
            },
            OpHistogram {
                op_kind: 1,
                requests: 8_000,
                histogram: (0u8..=255).rev().collect(),
            },
        ],
    };
    let back = Stats::decode(&stats.encode()).unwrap();
    assert_eq!(back, stats, "histogram bytes must survive exactly");
    for op in &back.ops {
        assert_eq!(op.histogram.len(), 256);
        assert!(
            op.requests >= 100,
            "the request count must travel so a thin sample can be marked (FR-066a)"
        );
    }
}

#[test]
fn an_empty_stats_reply_is_legal() {
    // An agent that issued nothing has nothing to report, and that is not an error: a run
    // may end before a lane's first turn.
    let stats = Stats::default();
    assert_eq!(Stats::decode(&stats.encode()).unwrap(), stats);
    assert!(stats.ops.is_empty());
}

#[test]
fn counters_round_trip_and_sum_across_nodes() {
    // Counters are what make bandwidth aggregable where a percentile is not. They are
    // collected by whatever code talks to the mailbox — the generator's executor locally,
    // the agent's remotely — so that a local number and a remote number mean the same
    // thing, and they travel back for reporting only.
    let a = Counters {
        requests: 10,
        key_references: 100,
        check_resident: 60,
        check_pending: 1,
        check_miss: 39,
        lookup_hits: 60,
        lookup_misses: 0,
        reserves_attempted: 39,
        reserves_declined: 2,
        transfers_attempted: 37,
        transfers_declined: 0,
        commits_attempted: 37,
        commits_declined: 0,
        payload_mismatches: 0,
        payloads_verified: 60,
    };
    let stats = Stats {
        counters: a,
        ops: vec![OpHistogram {
            op_kind: 0,
            requests: 10,
            histogram: vec![1, 2, 3],
        }],
    };
    let back = Stats::decode(&stats.encode()).unwrap();
    assert_eq!(back, stats, "every counter must survive the wire");

    // Summing is exact, which is the property that lets bandwidth be totalled over nodes.
    let mut total = a;
    total.merge(&a);
    // Derived rather than stored, so they cannot disagree with the counters they come from.
    assert_eq!(total.blocks_read(), 120, "two nodes' lookup hits");
    assert_eq!(total.blocks_written(), 74, "accepted transfers only");
    assert_eq!(total.check_resident, 120);
    assert_eq!(total.reserves_declined, 4);
}

#[test]
fn every_counter_field_is_carried_rather_than_some() {
    // A field added to the struct but forgotten in the codec would silently read as zero on
    // the far side, and a bandwidth number computed from it would be too low with no sign
    // that anything was missing. Distinct values per field catch a mis-ordered codec too.
    let mut c = Counters::default();
    let fields: [&mut u64; 15] = [
        &mut c.requests,
        &mut c.key_references,
        &mut c.check_resident,
        &mut c.check_pending,
        &mut c.check_miss,
        &mut c.lookup_hits,
        &mut c.lookup_misses,
        &mut c.reserves_attempted,
        &mut c.reserves_declined,
        &mut c.transfers_attempted,
        &mut c.transfers_declined,
        &mut c.commits_attempted,
        &mut c.commits_declined,
        &mut c.payload_mismatches,
        &mut c.payloads_verified,
    ];
    for (i, f) in fields.into_iter().enumerate() {
        *f = 1000 + i as u64;
    }
    let stats = Stats {
        counters: c,
        ops: Vec::new(),
    };
    assert_eq!(Stats::decode(&stats.encode()).unwrap().counters, c);
}

#[test]
fn a_payload_mismatch_travels_because_it_is_not_a_cache_outcome() {
    // Every other counter records something Certus legitimately decided. This one records that
    // it returned the **wrong block**, which is a correctness failure, so it has to reach the
    // generator or a multi-node run could not report it at all.
    let a = Counters {
        lookup_hits: 100,
        payloads_verified: 100,
        payload_mismatches: 3,
        ..Default::default()
    };
    let back = Stats::decode(
        &Stats {
            counters: a,
            ops: Vec::new(),
        }
        .encode(),
    )
    .unwrap();
    assert_eq!(back.counters.payload_mismatches, 3);
    assert_eq!(back.counters.payloads_verified, 100);

    // And it sums, so a run over several nodes reports the total rather than the worst node's.
    let mut total = a;
    total.merge(&a);
    assert_eq!(total.payload_mismatches, 6);
    assert_eq!(total.payloads_verified, 200);
}
