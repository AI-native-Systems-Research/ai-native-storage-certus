//! Mapping the plan onto Certus's mailbox: opcodes, payloads, and what is forbidden.
//!
//! This is the one place the plan's abstract operations become the shipped client's
//! actual protocol, so it is where FR-039 through FR-044 either hold or quietly do
//! not. Every opcode and payload layout below was read from
//! `lib/shmq-dispatcher/src/{wire,translate}.rs` rather than inferred.
//!
//! # The plan's kinds are not the mailbox's opcodes
//!
//! Three numbering schemes exist and they are deliberately different:
//!
//! | plan `OpKind` | mailbox opcode | payload |
//! | --- | --- | --- |
//! | `Check` | `CHECK` = 1 | `n:u32`, `key:u64 * n` |
//! | `Touch` | `TOUCH` = 2 | `promote:u8`, `n:u32`, `key:u64 * n` |
//! | `Load` | `LOOKUP` = 9 | `n:u32`, `key:u64 * n` |
//! | `Reserve` | `RESERVE` = 3 | `n:u32`, then `(key:u64, size:u32, session:u64) * n` |
//! | `Transfer` | `COPY_TO_STORE` = 4 | `n:u32`, `key:u64 * n` |
//! | `Commit` | `COMMIT_STORE` = 5 | `n:u32`, `key:u64 * n` |
//! | `Abort` | `ABORT_STORE` = 6 | `n:u32`, `key:u64 * n` |
//! | `PollEvents` | `TAKE_EVENTS` = 10 | `max:u32` |
//!
//! Keeping them separate is what lets each protocol version independently; collapsing
//! them would mean a change to one silently redefining the other.
//!
//! # `promote` is always zero, and that is a requirement not a default
//!
//! `TOUCH` and `LOOKUP` both accept a `promote` flag that makes the dispatcher call
//! `promote_to_memory_tier`. FR-043 forbids requesting promotion and FR-044 forbids
//! supplying the eviction policy with information the production client would not.
//! Promotion is a *decision*, and taking it on the client's behalf would mean measuring
//! a tiering policy this generator had partly written. So the flag is a named constant
//! set to zero, never a parameter.
//!
//! # Four opcodes must never be issued
//!
//! `PIN`, `UNPIN`, `REMOVE` and `CLEAR_MEMORY_TIER` (FR-043). The first three would let
//! the generator hold blocks the policy wanted to evict or evict blocks it wanted to
//! keep — measuring the generator's opinion rather than Certus's. `CLEAR_MEMORY_TIER` is
//! permitted exactly once, at startup, before the timed window opens.
//!
//! [`forbidden_opcode`] names them and [`OpStream::encode`] cannot produce them, since
//! nothing in [`OpKind`] maps to one. The test that matters is the one asserting a whole
//! run's issued opcodes contain none of them, because that is a property of the stream
//! rather than of any single call.

use shmq_dispatcher::wire::{self, op};
use workload_model::plan::{OpKind, Operation, OperationPlan};

/// `promote` value for `TOUCH` and `LOOKUP`.
///
/// Always zero. See the module docs: promotion is a decision, and taking it would make
/// this instrument measure a tiering policy it had partly authored.
pub const NO_PROMOTE: u8 = 0;

/// Maximum events to drain per `TAKE_EVENTS`.
///
/// Zero means "everything queued", which is what a client polling once per turn wants:
/// a cap would leave events to accumulate and turn a steady cost into a sawtooth.
pub const DRAIN_ALL_EVENTS: u32 = 0;

/// One encoded request, ready for the mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// The mailbox opcode.
    pub opcode: u32,
    /// The payload, in the layout `translate.rs` expects.
    pub payload: Vec<u8>,
}

/// Whether an opcode is one this generator must never issue (FR-043).
///
/// `CLEAR_MEMORY_TIER` is included: it is permitted exactly once at startup, which is
/// not part of the operation stream and is issued by its own path, outside the timed
/// window (FR-046).
pub fn forbidden_opcode(opcode: u32) -> Option<&'static str> {
    match opcode {
        op::PIN => Some("PIN: pinning would hold blocks the eviction policy wanted to evict"),
        op::UNPIN => Some("UNPIN: only meaningful after a pin, which is itself forbidden"),
        op::REMOVE => {
            Some("REMOVE: evicting on the policy's behalf measures our opinion, not Certus's")
        }
        op::CLEAR_MEMORY_TIER => {
            Some("CLEAR_MEMORY_TIER: permitted once at startup only, outside the timed window")
        }
        op::POPULATE => Some("POPULATE: not an operation the production client issues"),
        op::FLUSH_TO_SSD => {
            Some("FLUSH_TO_SSD: a tiering decision the production client does not make")
        }
        _ => None,
    }
}

/// Encodes plan operations as mailbox requests.
#[derive(Debug, Clone)]
pub struct OpStream {
    block_bytes: u32,
}

impl OpStream {
    /// An encoder for a description's block geometry.
    ///
    /// `block_bytes` is what `RESERVE` asks for per key — the cache holds bytes, and a
    /// reservation in the wrong unit would silently size the cache wrongly.
    pub fn new(block_bytes: u32) -> Self {
        Self { block_bytes }
    }

    /// Encode one plan operation.
    ///
    /// Returns `None` for [`OpKind::Abort`], which the planner never emits: whether a
    /// commit fails is a run-time outcome, so the executor chooses it and this
    /// translates it only when asked to.
    pub fn encode(&self, plan: &OperationPlan, op: &Operation) -> Option<Encoded> {
        let keys = plan.keys_of(op);
        Some(match op.kind() {
            OpKind::Check => Encoded {
                opcode: op::CHECK,
                payload: encode_keys(keys),
            },
            OpKind::Touch => Encoded {
                opcode: op::TOUCH,
                payload: encode_promote_and_keys(NO_PROMOTE, keys),
            },
            OpKind::Load => Encoded {
                opcode: op::LOOKUP,
                payload: encode_keys(keys),
            },
            OpKind::Reserve => Encoded {
                opcode: op::RESERVE,
                payload: encode_reserve(keys, self.block_bytes, op.session()),
            },
            OpKind::Transfer => Encoded {
                opcode: op::COPY_TO_STORE,
                payload: encode_keys(keys),
            },
            OpKind::Commit => Encoded {
                opcode: op::COMMIT_STORE,
                payload: encode_keys(keys),
            },
            OpKind::Abort => return None,
            OpKind::PollEvents => Encoded {
                opcode: op::TAKE_EVENTS,
                payload: DRAIN_ALL_EVENTS.to_le_bytes().to_vec(),
            },
        })
    }

    /// Encode the abort branch of a store, which only the executor can choose.
    pub fn encode_abort(&self, keys: &[u64]) -> Encoded {
        Encoded {
            opcode: op::ABORT_STORE,
            payload: encode_keys(keys),
        }
    }
}

fn encode_keys(keys: &[u64]) -> Vec<u8> {
    let mut w = wire::Writer::with_capacity(4 + keys.len() * 8);
    w.u32(keys.len() as u32);
    for k in keys {
        w.u64(*k);
    }
    w.into_bytes()
}

fn encode_promote_and_keys(promote: u8, keys: &[u64]) -> Vec<u8> {
    let mut w = wire::Writer::with_capacity(1 + 4 + keys.len() * 8);
    w.u8(promote);
    w.u32(keys.len() as u32);
    for k in keys {
        w.u64(*k);
    }
    w.into_bytes()
}

fn encode_reserve(keys: &[u64], size: u32, session: u64) -> Vec<u8> {
    let mut w = wire::Writer::with_capacity(4 + keys.len() * 20);
    w.u32(keys.len() as u32);
    for k in keys {
        w.u64(*k);
        w.u32(size);
        w.u64(session);
    }
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_model::description::WorkloadDescription;
    use workload_model::sim::Simulation;

    fn plan_of(seed: u64, span: f64) -> OperationPlan {
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 3}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 3}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 3}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let mut sim = Simulation::new(&d, seed).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(span, &mut |s, t| plan.record_turn(s, t));
        plan
    }

    #[test]
    fn every_kind_maps_to_the_opcode_read_from_the_dispatcher() {
        // The mapping is the whole point of this module, and a wrong entry would issue
        // a valid request that does the wrong thing — the dispatcher would accept it.
        let plan = plan_of(1, 40.0);
        let s = OpStream::new(32768);
        let mut seen = std::collections::BTreeMap::new();
        for op in plan.operations() {
            if let Some(e) = s.encode(&plan, op) {
                seen.insert(op.kind(), e.opcode);
            }
        }
        assert_eq!(seen[&OpKind::Check], op::CHECK);
        assert_eq!(seen[&OpKind::Touch], op::TOUCH);
        assert_eq!(seen[&OpKind::Load], op::LOOKUP);
        assert_eq!(seen[&OpKind::Reserve], op::RESERVE);
        assert_eq!(seen[&OpKind::Transfer], op::COPY_TO_STORE);
        assert_eq!(seen[&OpKind::Commit], op::COMMIT_STORE);
        assert_eq!(seen[&OpKind::PollEvents], op::TAKE_EVENTS);
        assert!(!seen.contains_key(&OpKind::Abort), "an abort was planned");
    }

    #[test]
    fn touch_never_requests_promotion() {
        // FR-043 and FR-044. `promote` is the first byte of a TOUCH payload, so this
        // reads the wire rather than trusting the constant.
        let plan = plan_of(2, 40.0);
        let s = OpStream::new(32768);
        let mut touches = 0;
        for op in plan.operations() {
            if op.kind() == OpKind::Touch {
                let e = s.encode(&plan, op).unwrap();
                assert_eq!(e.payload[0], 0, "TOUCH asked for promotion");
                touches += 1;
            }
        }
        assert!(touches > 0, "no TOUCH was issued, so nothing was tested");
    }

    #[test]
    fn a_whole_run_issues_no_forbidden_opcode() {
        // FR-043 as a property of the stream rather than of any one call. Checked
        // against the dispatcher's own opcode constants, so adding a forbidden
        // operation upstream cannot slip past by renaming.
        let plan = plan_of(3, 200.0);
        let s = OpStream::new(32768);
        let mut issued = 0;
        for op in plan.operations() {
            if let Some(e) = s.encode(&plan, op) {
                if let Some(why) = forbidden_opcode(e.opcode) {
                    panic!("issued a forbidden opcode {}: {why}", e.opcode);
                }
                issued += 1;
            }
        }
        assert!(issued > 50, "only {issued} operations, too few to test");
    }

    #[test]
    fn the_forbidden_list_names_every_opcode_it_should() {
        // A guard list that quietly lost an entry would be worse than none, so each is
        // asserted by its dispatcher constant.
        for opcode in [
            op::PIN,
            op::UNPIN,
            op::REMOVE,
            op::CLEAR_MEMORY_TIER,
            op::POPULATE,
            op::FLUSH_TO_SSD,
        ] {
            assert!(
                forbidden_opcode(opcode).is_some(),
                "opcode {opcode} is not on the forbidden list"
            );
        }
        // And the ones we do issue must not be on it.
        for opcode in [
            op::CHECK,
            op::TOUCH,
            op::RESERVE,
            op::COPY_TO_STORE,
            op::COMMIT_STORE,
            op::ABORT_STORE,
            op::LOOKUP,
            op::TAKE_EVENTS,
        ] {
            assert!(
                forbidden_opcode(opcode).is_none(),
                "opcode {opcode} is issued but marked forbidden"
            );
        }
    }

    #[test]
    fn a_key_payload_round_trips_through_the_dispatchers_own_reader() {
        // The encodings are only right if the dispatcher's reader agrees, so this
        // decodes with `wire::Reader` rather than with a hand-written parser.
        let keys = [7u64, 8, 0xDEAD_BEEF_CAFE_1234];
        let payload = encode_keys(&keys);
        let mut r = wire::Reader::new(&payload);
        assert_eq!(r.u32().unwrap() as usize, keys.len());
        for want in keys {
            assert_eq!(r.u64().unwrap(), want);
        }
    }

    #[test]
    fn a_reserve_payload_carries_size_and_session_per_key() {
        // The layout `op_reserve` reads: n, then (key, size, session) per entry. A
        // reservation in the wrong unit would size the cache wrongly and nothing would
        // report it.
        let plan = plan_of(4, 40.0);
        let s = OpStream::new(32768);
        let op = plan
            .operations()
            .iter()
            .find(|o| o.kind() == OpKind::Reserve)
            .expect("a reserve");
        let e = s.encode(&plan, op).unwrap();
        let expected_keys = plan.keys_of(op);
        let mut r = wire::Reader::new(&e.payload);
        assert_eq!(r.u32().unwrap() as usize, expected_keys.len());
        for want in expected_keys {
            assert_eq!(r.u64().unwrap(), *want);
            assert_eq!(r.u32().unwrap(), 32768, "size must be bytes per block");
            assert_eq!(r.u64().unwrap(), op.session(), "session must be the turn's");
        }
    }

    #[test]
    fn a_poll_drains_every_queued_event() {
        // A cap would let events accumulate and turn a steady per-turn cost into a
        // sawtooth, which would show up in latency percentiles as the generator's own
        // artifact.
        let plan = plan_of(5, 40.0);
        let s = OpStream::new(32768);
        let op = plan
            .operations()
            .iter()
            .find(|o| o.kind() == OpKind::PollEvents)
            .expect("a poll");
        let e = s.encode(&plan, op).unwrap();
        assert_eq!(e.opcode, op::TAKE_EVENTS);
        let mut r = wire::Reader::new(&e.payload);
        assert_eq!(r.u32().unwrap(), DRAIN_ALL_EVENTS);
    }

    #[test]
    fn abort_is_encodable_but_never_planned() {
        // The executor needs it for a failed commit; the planner must not emit it,
        // because which branch is taken is a run-time outcome.
        let plan = plan_of(6, 100.0);
        let s = OpStream::new(32768);
        assert!(plan.operations().iter().all(|o| o.kind() != OpKind::Abort));
        let e = s.encode_abort(&[1, 2]);
        assert_eq!(e.opcode, op::ABORT_STORE);
        let mut r = wire::Reader::new(&e.payload);
        assert_eq!(r.u32().unwrap(), 2);
    }
}
