//! The handshake, fail-closed, from `contracts/node-agent-wire.md` cases 3 and 4.
//!
//! What is being defended against is recorded in this repository's history: a stale remote
//! binary invalidated measurements, and it did so **silently** — the run completed and
//! produced numbers. So these tests are less about the happy path than about proving each
//! refusal actually fires and says which node it is about.

use workload_wire::frame::{Hello, HelloAck, BUILD_ID_BYTES, PROTO_VERSION};
use workload_wire::handshake::{
    self, abbreviate, answer, check_capacity, current, digest, hello, is_known, status, Refused,
    SOURCE_ID, UNKNOWN,
};

const NODE: &str = "node5";

/// An acceptance this build would produce for itself.
fn good_ack() -> HelloAck {
    HelloAck {
        proto_version: PROTO_VERSION,
        build_id: current(),
        channels: 8,
        block_bytes: 32768,
        status: status::OK,
    }
}

#[test]
fn a_matching_peer_is_accepted() {
    // The happy path exists to prove the refusals below are not simply always-on.
    assert!(
        is_known(),
        "this tree has no source identity, so the provenance tests cannot mean anything; \
         SOURCE_ID = {SOURCE_ID}"
    );
    handshake::verify(NODE, &good_ack()).expect("this build must accept itself");
    check_capacity(NODE, &good_ack(), 4, 32768).expect("4 lanes of 8 channels");
}

#[test]
fn a_different_source_identity_is_refused_and_the_node_is_named() {
    // Contract case 3. The message must carry both identities: an operator needs to know
    // which build the node is running, not merely that it is the wrong one.
    let ack = HelloAck {
        build_id: digest("some other tree"),
        ..good_ack()
    };
    let err = handshake::verify(NODE, &ack).expect_err("a foreign build must be refused");
    match &err {
        Refused::SourceIdentity { node, ours, theirs } => {
            assert_eq!(node, NODE);
            assert_ne!(ours, theirs);
            assert_eq!(ours, &abbreviate(&current()));
        }
        other => panic!("expected SourceIdentity, got {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains(NODE),
        "the refusal must name the node: {text}"
    );
    assert!(text.contains("FR-051"), "and cite why: {text}");
}

#[test]
fn a_different_protocol_version_is_refused_first() {
    // Contract case 4, and checked before anything else: if the versions differ, the rest of
    // the reply may not mean what it appears to, so reporting a build mismatch instead would
    // send an operator after the wrong problem.
    let ack = HelloAck {
        proto_version: PROTO_VERSION + 1,
        build_id: digest("also a different tree"),
        ..good_ack()
    };
    match handshake::verify(NODE, &ack) {
        Err(Refused::ProtocolVersion { node, ours, theirs }) => {
            assert_eq!(node, NODE);
            assert_eq!(ours, PROTO_VERSION);
            assert_eq!(theirs, PROTO_VERSION + 1);
        }
        other => panic!("expected ProtocolVersion first, got {other:?}"),
    }
}

#[test]
fn an_unknown_provenance_is_refused_rather_than_assumed() {
    // The fail-closed direction that matters. An agent that cannot say what it was built from
    // has not demonstrated anything, and FR-051 asks for a demonstration. Accepting it would
    // make the check vacuous for exactly the deployments most likely to be stale.
    let ack = HelloAck {
        build_id: digest(UNKNOWN),
        ..good_ack()
    };
    match handshake::verify(NODE, &ack) {
        Err(Refused::UnknownProvenance { node, ours }) => {
            assert_eq!(node, NODE);
            assert!(!ours, "it is the agent that could not answer");
        }
        other => panic!("expected UnknownProvenance, got {other:?}"),
    }
}

#[test]
fn a_peer_that_refuses_us_is_reported_on_its_own_terms() {
    // Both ends check, so a status we did not predict is worth surfacing as itself rather
    // than as a mystery.
    let ack = HelloAck {
        status: status::BUILD_MISMATCH,
        ..good_ack()
    };
    match handshake::verify(NODE, &ack) {
        Err(Refused::ByPeer { node, status: s }) => {
            assert_eq!(node, NODE);
            assert_eq!(s, status::BUILD_MISMATCH);
        }
        other => panic!("expected ByPeer, got {other:?}"),
    }
}

#[test]
fn the_agent_refuses_too_and_always_reports_its_own_identity() {
    // Checking on both ends is what makes a refusal legible: the generator can print both
    // identities only because the agent sends its own even while refusing.
    let ours = current();
    let stale = Hello {
        proto_version: PROTO_VERSION,
        build_id: digest("yesterday's tree"),
        mailbox: "/dev/shm/certus-shmq".to_string(),
    };
    let ack = answer(&stale, 8, 32768);
    assert_eq!(ack.status, status::BUILD_MISMATCH);
    assert_eq!(
        ack.build_id, ours,
        "the agent must report its own identity even when refusing, or the generator's \
         message cannot name both sides"
    );

    let old = Hello {
        proto_version: PROTO_VERSION - 1,
        build_id: ours,
        mailbox: String::new(),
    };
    assert_eq!(answer(&old, 8, 32768).status, status::PROTO_MISMATCH);

    let unknown = Hello {
        proto_version: PROTO_VERSION,
        build_id: digest(UNKNOWN),
        mailbox: String::new(),
    };
    assert_eq!(
        answer(&unknown, 8, 32768).status,
        status::UNKNOWN_PROVENANCE
    );

    let good = hello("/dev/shm/certus-shmq");
    assert_eq!(answer(&good, 8, 32768).status, status::OK);
}

#[test]
fn more_lanes_than_channels_is_refused_because_it_would_serialise_silently() {
    // The failure this prevents produces no error at all: the mailbox is depth-1 per channel,
    // so extra lanes queue behind each other and the run looks like a slow server.
    let err = check_capacity(NODE, &good_ack(), 9, 32768).expect_err("9 lanes of 8 channels");
    match &err {
        Refused::TooManyLanes {
            node,
            lanes,
            channels,
        } => {
            assert_eq!(node, NODE);
            assert_eq!((*lanes, *channels), (9, 8));
        }
        other => panic!("expected TooManyLanes, got {other:?}"),
    }
    assert!(err.to_string().contains("serialise"), "{err}");
    // Exactly at capacity is allowed.
    check_capacity(NODE, &good_ack(), 8, 32768).expect("8 lanes of 8 channels");
}

#[test]
fn a_block_size_disagreement_is_refused() {
    // A reservation in the wrong unit sizes the cache wrongly and nothing reports it.
    match check_capacity(NODE, &good_ack(), 4, 4096) {
        Err(Refused::BlockBytes { node, ours, theirs }) => {
            assert_eq!(node, NODE);
            assert_eq!((ours, theirs), (4096, 32768));
        }
        other => panic!("expected BlockBytes, got {other:?}"),
    }
}

#[test]
fn the_digest_moves_everywhere_when_the_identity_changes_by_one_byte() {
    // Four independently seeded chains, so a one-character difference in the source identity
    // is not confined to one word of the digest. A digest whose tail rarely moved would make
    // the abbreviated form in the refusal message useless.
    let a = digest("abc123");
    let b = digest("abc124");
    assert_ne!(a, b);
    let differing_words = a.chunks(8).zip(b.chunks(8)).filter(|(x, y)| x != y).count();
    assert_eq!(differing_words, 4, "every word must move");
    // And the abbreviated forms differ, which is what an operator actually compares.
    assert_ne!(abbreviate(&a), abbreviate(&b));
    assert_eq!(abbreviate(&a).len(), 16);
}

#[test]
fn the_digest_is_stable_for_one_identity() {
    // It has to be: two binaries built from one tree must agree, and they only obtain the
    // value through this crate.
    assert_eq!(digest("same"), digest("same"));
    assert_eq!(current(), digest(SOURCE_ID));
    assert_eq!(current().len(), BUILD_ID_BYTES);
}

#[test]
fn this_builds_identity_is_not_the_unknown_sentinel() {
    // If this fails, every provenance check in the suite is passing for the wrong reason.
    assert_ne!(current(), digest(UNKNOWN));
    assert!(!SOURCE_ID.is_empty());
}

#[test]
fn the_captured_identity_is_a_digest_of_the_sources_and_needs_no_repository() {
    // Printed so a reader can see the check is not passing on a placeholder. The identity is
    // a digest of this application's source files rather than anything from git, which is what
    // lets a build from a release tarball take part in a multi-node run at all — the git-based
    // version this replaced would have refused one, and would also have missed an untracked
    // file entirely.
    eprintln!("SOURCE_ID = {SOURCE_ID}");
    eprintln!("digest     = {}", abbreviate(&current()));
    assert!(
        SOURCE_ID.starts_with("src:"),
        "not a source digest: {SOURCE_ID}"
    );
    assert!(
        !SOURCE_ID.contains("diff:") && !SOURCE_ID.contains("status:"),
        "the identity still depends on git: {SOURCE_ID}"
    );
    // File count and total size travel with the fold, so a collision must match all three.
    let parts: Vec<&str> = SOURCE_ID.split(':').collect();
    assert_eq!(parts.len(), 4, "expected src:count:bytes:fold");
    let count: usize = parts[1].parse().expect("a file count");
    let bytes: usize = parts[2].parse().expect("a byte total");
    assert!(count > 20, "only {count} source files hashed");
    assert!(bytes > 100_000, "only {bytes} bytes hashed");
}
