//! The `wr_id` slab: the per-request state a completion must release, keyed by
//! the work-request id the completion echoes.
//!
//! Principle IX requires a pin held across an asynchronous operation to be
//! "released by the completion reaper that owns its work-request id". The slab
//! makes that a property of the type rather than of the call sites: the state
//! goes in at [`Slab::insert`], and the only ways to get it back are
//! [`Slab::complete`] — with the completion that owns it — and, for a request
//! that was never posted, the posting helper's `unposted` list. There is no
//! third path, so there is no way to release twice.
//!
//! The slab is generic: what a slot carries (pin key, bounce lease, context and
//! entry index) is the serve path's type, not this module's.
//!
//! # Why a generation number
//!
//! A `wr_id` is the slot index in its low 32 bits and the slot's generation in
//! its high 32, bumped on every reuse. The predecessor's slab had no
//! generation and was safe only because "recovery destroys the completion queue
//! along with the queue pair". Here a link keeps being polled after it is
//! forced to ERROR (FR-023's drain), so a slot can be reused while a completion
//! for its previous occupant is still on the way; without the generation, that
//! late completion would release the *new* occupant's pin. With it, the late
//! completion is [`SlabError::UnknownWrId`] — a defect to report, never a
//! release.
//!
//! # One slab per link
//!
//! A link's sends and receives complete into one queue, and a failed
//! completion does not say which queue it came from, so ids must be unique
//! across both. The connection table therefore holds one slab per link, sized
//! to its granted send plus receive depth, and will not destroy a link whose
//! slab still holds anything: the completions — and with them every pin still
//! held — would be lost with the queue pair.

// Consumed by the serve path and credits from T014/T017 onward.
#![allow(dead_code)]

use std::fmt;
use std::time::{Duration, Instant};

use super::transport::{
    Completed, Completion, Link, QpState, RdmaError, RecvRequest, SendOp, SendRequest, Sge,
};

/// What a slot's work request was, checked against its successful completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrKind {
    Send,
    Write,
    Recv,
}

impl WrKind {
    fn of(c: Completed) -> Self {
        match c {
            Completed::Send => WrKind::Send,
            Completed::Write => WrKind::Write,
            Completed::Recv { .. } => WrKind::Recv,
        }
    }
}

/// A completion the slab cannot attribute. Every variant is a defect in this
/// crate or the device, not a runtime condition: the caller should fail loudly.
#[derive(Debug)]
pub enum SlabError<T> {
    /// No live slot owns this id: a duplicate, a stale generation, or an id
    /// this slab never issued.
    UnknownWrId(u64),
    /// The work request succeeded as a different kind than it was posted as.
    /// The slot is released and its state handed back, so the caller decides
    /// what the defect means for it.
    KindMismatch {
        wr_id: u64,
        posted: WrKind,
        completed: WrKind,
        state: T,
    },
}

impl<T> fmt::Display for SlabError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlabError::UnknownWrId(id) => write!(f, "completion for unknown wr_id 0x{id:x}"),
            SlabError::KindMismatch {
                wr_id,
                posted,
                completed,
                ..
            } => write!(
                f,
                "wr_id 0x{wr_id:x} was posted as {posted:?} but completed as {completed:?}"
            ),
        }
    }
}

struct Slot<T> {
    generation: u32,
    entry: Option<(WrKind, T)>,
}

/// Per-request state for one link's outstanding work requests.
pub struct Slab<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    outstanding: usize,
}

fn wr_id(index: u32, generation: u32) -> u64 {
    (u64::from(generation) << 32) | u64::from(index)
}

impl<T> Slab<T> {
    /// A slab of `capacity` slots — the link's granted send plus receive
    /// depth, so a full slab means a full queue pair.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = u32::try_from(capacity).expect("slab capacity exceeds u32");
        Slab {
            slots: (0..capacity)
                .map(|_| Slot {
                    generation: 0,
                    entry: None,
                })
                .collect(),
            free: (0..capacity).rev().collect(),
            outstanding: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Requests posted and not yet completed.
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }

    pub fn available(&self) -> usize {
        self.free.len()
    }

    /// Take a slot for a request about to be posted, returning its `wr_id`.
    /// When full, the state is handed back.
    pub fn insert(&mut self, kind: WrKind, state: T) -> Result<u64, T> {
        let Some(index) = self.free.pop() else {
            return Err(state);
        };
        let slot = &mut self.slots[index as usize];
        // Wraps after 2^32 reuses of one slot; a completion that stale is not
        // a concern this design has.
        slot.generation = slot.generation.wrapping_add(1);
        slot.entry = Some((kind, state));
        self.outstanding += 1;
        Ok(wr_id(index, slot.generation))
    }

    /// Release the slot `c` completes, handing back its state. This is the
    /// reaper's half of Principle IX. A failed completion is attributed by
    /// `wr_id` alone, since verbs leaves its opcode undefined.
    pub fn complete(&mut self, c: &Completion) -> Result<T, SlabError<T>> {
        let (index, generation) = (c.wr_id as u32, (c.wr_id >> 32) as u32);
        let Some((kind, state)) = self
            .slots
            .get_mut(index as usize)
            .filter(|s| s.generation == generation)
            .and_then(|s| s.entry.take())
        else {
            return Err(SlabError::UnknownWrId(c.wr_id));
        };
        self.free.push(index);
        self.outstanding -= 1;
        match c.result {
            Ok(done) if WrKind::of(done) != kind => Err(SlabError::KindMismatch {
                wr_id: c.wr_id,
                posted: kind,
                completed: WrKind::of(done),
                state,
            }),
            _ => Ok(state),
        }
    }

    /// Release a slot whose request was **never queued**. Private: only the
    /// posting helpers know which requests a partial post left unqueued, and
    /// abandoning a queued one would release its state twice.
    fn abandon(&mut self, wr_id: u64) -> T {
        let index = wr_id as u32;
        let slot = &mut self.slots[index as usize];
        debug_assert_eq!(slot.generation, (wr_id >> 32) as u32);
        let (_, state) = slot.entry.take().expect("abandoned an empty slot");
        self.free.push(index);
        self.outstanding -= 1;
        state
    }
}

impl<T> fmt::Debug for Slab<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slab")
            .field("capacity", &self.capacity())
            .field("outstanding", &self.outstanding)
            .finish()
    }
}

// --- posting -----------------------------------------------------------------

/// A send-queue request whose state is held until its completion. There is
/// no `signaled` field: a tracked request is always signaled, because an
/// unsignaled success never completes and its slot — and pin — would never
/// come back.
pub struct TrackedSend<'a, T> {
    pub op: SendOp,
    pub local: Sge<'a>,
    pub state: T,
}

pub struct TrackedRecv<'a, T> {
    pub buf: Sge<'a>,
    pub state: T,
}

/// A tracked post that stopped partway. Requests before `accepted` were queued
/// and their states will come back through [`Slab::complete`]; `unposted`
/// holds the states of the rest, in request order, for the caller to release
/// now. Each state comes back exactly once, by one route or the other.
#[derive(Debug)]
pub struct TrackedPostError<T> {
    pub accepted: usize,
    pub error: RdmaError,
    pub unposted: Vec<T>,
}

impl<T> fmt::Display for TrackedPostError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} after {} accepted, {} unposted",
            self.error,
            self.accepted,
            self.unposted.len()
        )
    }
}

fn kind_of(op: SendOp) -> WrKind {
    match op {
        SendOp::Send => WrKind::Send,
        SendOp::RdmaWrite { .. } => WrKind::Write,
    }
}

/// Fail a whole batch before anything is posted.
fn refuse<T>(states: Vec<T>, error: RdmaError) -> TrackedPostError<T> {
    TrackedPostError {
        accepted: 0,
        error,
        unposted: states,
    }
}

fn slab_full() -> RdmaError {
    RdmaError::Invalid {
        op: "post_tracked",
        reason: "wr_id slab has no room for the batch",
    }
}

/// Allocate slots for `reqs`, post them as one batch, and on a partial post
/// hand back exactly the states that were not queued.
///
/// The whole batch must fit the slab, or nothing is posted; sizing batches is
/// the credits' job (T014).
///
/// # Safety
///
/// As [`Link::post_send`]: every local buffer must stay valid until its
/// completion is reaped or the link is forced to ERROR and drained.
pub unsafe fn post_send_tracked<T>(
    link: &mut dyn Link,
    slab: &mut Slab<T>,
    reqs: Vec<TrackedSend<'_, T>>,
) -> Result<(), TrackedPostError<T>> {
    if reqs.len() > slab.available() {
        return Err(refuse(
            reqs.into_iter().map(|r| r.state).collect(),
            slab_full(),
        ));
    }
    let mut posted = Vec::with_capacity(reqs.len());
    for r in reqs {
        let wr_id = slab
            .insert(kind_of(r.op), r.state)
            .unwrap_or_else(|_| unreachable!("room was checked"));
        posted.push(SendRequest {
            wr_id,
            op: r.op,
            local: r.local,
            signaled: true,
        });
    }
    link.post_send(&posted).map_err(|e| TrackedPostError {
        accepted: e.accepted,
        error: e.error,
        unposted: posted[e.accepted..]
            .iter()
            .map(|r| slab.abandon(r.wr_id))
            .collect(),
    })
}

/// [`post_send_tracked`] for receive buffers.
///
/// # Safety
///
/// As [`Link::post_recv`].
pub unsafe fn post_recv_tracked<T>(
    link: &mut dyn Link,
    slab: &mut Slab<T>,
    reqs: Vec<TrackedRecv<'_, T>>,
) -> Result<(), TrackedPostError<T>> {
    if reqs.len() > slab.available() {
        return Err(refuse(
            reqs.into_iter().map(|r| r.state).collect(),
            slab_full(),
        ));
    }
    let mut posted = Vec::with_capacity(reqs.len());
    for r in reqs {
        let wr_id = slab
            .insert(WrKind::Recv, r.state)
            .unwrap_or_else(|_| unreachable!("room was checked"));
        posted.push(RecvRequest { wr_id, buf: r.buf });
    }
    link.post_recv(&posted).map_err(|e| TrackedPostError {
        accepted: e.accepted,
        error: e.error,
        unposted: posted[e.accepted..]
            .iter()
            .map(|r| slab.abandon(r.wr_id))
            .collect(),
    })
}

// --- draining ----------------------------------------------------------------

/// Why [`drain`] did not empty the slab.
#[derive(Debug)]
pub enum DrainError<T> {
    /// The link is not in ERROR, so its queued requests need not complete and
    /// the drain could wait forever. Force ERROR first (FR-023).
    NotInError(QpState),
    /// The device stopped producing completions with requests outstanding:
    /// every queued request must complete after ERROR, so this is a stuck
    /// device or a defect, never a slow peer.
    Stuck {
        outstanding: usize,
    },
    Slab(SlabError<T>),
    Poll(RdmaError),
}

impl<T> fmt::Display for DrainError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrainError::NotInError(s) => write!(f, "cannot drain a link in state {s:?}"),
            DrainError::Stuck { outstanding } => {
                write!(f, "drain stalled with {outstanding} requests outstanding")
            }
            DrainError::Slab(e) => write!(f, "{e}"),
            DrainError::Poll(e) => write!(f, "{e}"),
        }
    }
}

/// Poll an ERROR link until every outstanding request has completed, moving
/// each completion and its released state onto `out`. Flushed completions are
/// ordinary here: they are how a forced-ERROR link accounts for its queue.
///
/// Gives up with [`DrainError::Stuck`] if `stall` passes with no completion
/// while requests remain.
pub fn drain<T>(
    link: &mut dyn Link,
    slab: &mut Slab<T>,
    out: &mut Vec<(Completion, T)>,
    stall: Duration,
) -> Result<(), DrainError<T>> {
    match link.state().map_err(DrainError::Poll)? {
        QpState::Err => {}
        other => return Err(DrainError::NotInError(other)),
    }
    let mut batch = Vec::new();
    let mut last_progress = Instant::now();
    while slab.outstanding() > 0 {
        batch.clear();
        let n = link
            .poll(&mut batch, slab.outstanding())
            .map_err(DrainError::Poll)?;
        if n == 0 {
            if last_progress.elapsed() >= stall {
                return Err(DrainError::Stuck {
                    outstanding: slab.outstanding(),
                });
            }
            std::thread::yield_now();
            continue;
        }
        last_progress = Instant::now();
        for c in batch.drain(..) {
            let state = slab.complete(&c).map_err(DrainError::Slab)?;
            out.push((c, state));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::mock::{MockFabric, MockTransport};
    use crate::connection::transport::{
        Access, CompletionError, LinkId, QpCaps, Transport, TransportEvent,
    };
    use crate::connection::PeerId;

    /// Two raw mock transports, connected; `a`'s link has room for `send`
    /// sends and `recv` receives.
    fn connected(send: u32, recv: u32) -> (MockTransport, MockTransport, LinkId, LinkId) {
        let f = MockFabric::new();
        let (mut a, mut b) = (MockTransport::new(f.clone()), MockTransport::new(f));
        let caps = QpCaps {
            max_send_wr: send,
            max_recv_wr: recv,
        };
        let at = PeerId::parse("10.0.0.2:1").unwrap();
        b.listen(&at).unwrap();
        let la = a.connect(&at, b"", caps).unwrap();
        let mut evs = Vec::new();
        b.poll_events(&mut evs);
        let TransportEvent::ConnectRequest { request, .. } = evs[0] else {
            panic!("{evs:?}");
        };
        let lb = b.accept(request, b"", caps).unwrap();
        (a, b, la, lb)
    }

    fn ok(wr_id: u64, c: Completed) -> Completion {
        Completion {
            wr_id,
            result: Ok(c),
        }
    }

    #[test]
    fn insert_complete_round_trip_and_capacity() {
        let mut s = Slab::with_capacity(2);
        let a = s.insert(WrKind::Write, "a").unwrap();
        let b = s.insert(WrKind::Recv, "b").unwrap();
        assert_ne!(a, b);
        assert_eq!(
            s.insert(WrKind::Send, "c"),
            Err("c"),
            "full hands the state back"
        );
        assert_eq!(s.outstanding(), 2);
        assert_eq!(
            s.complete(&ok(b, Completed::Recv { byte_len: 3 })).unwrap(),
            "b"
        );
        assert_eq!(s.complete(&ok(a, Completed::Write)).unwrap(), "a");
        assert_eq!((s.outstanding(), s.available()), (0, 2));
    }

    #[test]
    fn a_completion_releases_once() {
        let mut s = Slab::with_capacity(1);
        let a = s.insert(WrKind::Send, 1).unwrap();
        s.complete(&ok(a, Completed::Send)).unwrap();
        assert!(matches!(
            s.complete(&ok(a, Completed::Send)),
            Err(SlabError::UnknownWrId(id)) if id == a
        ));
    }

    /// The case the generation exists for: a slot reused while a completion
    /// for its previous occupant is still in flight. The late completion must
    /// not release the new occupant.
    #[test]
    fn late_completion_cannot_release_the_slots_next_occupant() {
        let mut s = Slab::with_capacity(1);
        let old = s.insert(WrKind::Write, "old").unwrap();
        s.complete(&ok(old, Completed::Write)).unwrap();
        let new = s.insert(WrKind::Write, "new").unwrap();
        assert_eq!(old as u32, new as u32, "same slot");
        assert_ne!(old, new, "different generation");
        assert!(matches!(
            s.complete(&ok(old, Completed::Write)),
            Err(SlabError::UnknownWrId(_))
        ));
        assert_eq!(s.outstanding(), 1, "the new occupant is still held");
        assert_eq!(s.complete(&ok(new, Completed::Write)).unwrap(), "new");
    }

    #[test]
    fn foreign_and_out_of_range_ids_are_unknown() {
        let mut s: Slab<()> = Slab::with_capacity(4);
        for id in [0, 3, 99, u64::MAX] {
            assert!(matches!(
                s.complete(&ok(id, Completed::Send)),
                Err(SlabError::UnknownWrId(_))
            ));
        }
    }

    #[test]
    fn kind_mismatch_is_reported_with_the_state() {
        let mut s = Slab::with_capacity(1);
        let a = s.insert(WrKind::Write, 7).unwrap();
        match s.complete(&ok(a, Completed::Send)) {
            Err(SlabError::KindMismatch {
                posted: WrKind::Write,
                completed: WrKind::Send,
                state: 7,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(s.outstanding(), 0, "the slot is released, not leaked");
    }

    /// A failed completion has no reliable opcode, so the kind is not checked.
    #[test]
    fn failed_completions_are_attributed_by_id_alone() {
        let mut s = Slab::with_capacity(2);
        let a = s.insert(WrKind::Recv, "a").unwrap();
        let b = s.insert(WrKind::Write, "b").unwrap();
        for (id, err) in [
            (a, CompletionError::Flushed),
            (
                b,
                CompletionError::Failed {
                    status: 10,
                    vendor_err: 0,
                },
            ),
        ] {
            assert!(s
                .complete(&Completion {
                    wr_id: id,
                    result: Err(err)
                })
                .is_ok());
        }
        assert_eq!(s.outstanding(), 0);
    }

    /// Item 6's hazard: a partial post must hand back exactly the unqueued
    /// states, and the queued ones must still come back through `complete` —
    /// each state once, by one route or the other.
    #[test]
    fn partial_post_returns_exactly_the_unqueued_states() {
        let (mut a, _b, la, _lb) = connected(2, 2);
        let mut buf = [0u8; 8];
        // SAFETY: `buf` outlives the region and every completion below.
        let r = unsafe { a.register(buf.as_mut_ptr(), 8, Access::LOCAL_WRITE) }.unwrap();
        let mut slab = Slab::with_capacity(8); // roomier than the link's queue
        let link = a.link(la).unwrap();
        // SAFETY: as above.
        let err = unsafe {
            post_recv_tracked(
                link,
                &mut slab,
                (1..=4)
                    .map(|state| TrackedRecv {
                        buf: r.sge(r.addr(), 8).unwrap(),
                        state,
                    })
                    .collect(),
            )
        }
        .unwrap_err();
        assert_eq!(err.accepted, 2, "max_recv_wr is 2");
        assert_eq!(err.unposted, [3, 4]);
        assert_eq!(slab.outstanding(), 2, "only the queued two are held");

        link.to_error().unwrap();
        let mut out = Vec::new();
        drain(link, &mut slab, &mut out, Duration::from_secs(1)).unwrap();
        let mut back: Vec<_> = out.iter().map(|(_, s)| *s).collect();
        back.sort_unstable();
        assert_eq!(back, [1, 2], "the queued two, and only those");
        assert_eq!(slab.outstanding(), 0);
        drop(r);
    }

    /// The send-side twin: the send queue fills partway through a batch.
    #[test]
    fn partial_send_post_returns_exactly_the_unqueued_states() {
        let (mut a, mut b, la, _lb) = connected(2, 2);
        let mut src = [0u8; 8];
        let mut land = [0u8; 64];
        // SAFETY: both buffers outlive the regions and the drain below.
        let rs = unsafe { a.register(src.as_mut_ptr(), 8, Access::LOCAL_WRITE) }.unwrap();
        let rl = unsafe {
            b.register(
                land.as_mut_ptr(),
                64,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        let mut slab = Slab::with_capacity(8);
        let link = a.link(la).unwrap();
        let write = |state| TrackedSend {
            op: SendOp::RdmaWrite {
                remote_addr: rl.addr(),
                rkey: rl.rkey(),
            },
            local: rs.sge(rs.addr(), 8).unwrap(),
            state,
        };
        // SAFETY: as above.
        let err = unsafe { post_send_tracked(link, &mut slab, (1..=4).map(write).collect()) }
            .unwrap_err();
        assert_eq!(err.accepted, 2, "max_send_wr is 2");
        assert_eq!(err.unposted, [3, 4]);
        let mut c = Vec::new();
        link.poll(&mut c, 8).unwrap();
        let mut back: Vec<_> = c.iter().map(|c| slab.complete(c).unwrap()).collect();
        back.sort_unstable();
        assert_eq!(back, [1, 2], "the queued two, and only those");
        assert_eq!(slab.outstanding(), 0);
        drop((rs, rl));
    }

    #[test]
    fn a_batch_larger_than_the_slab_posts_nothing() {
        let (mut a, _b, la, _lb) = connected(8, 8);
        let mut buf = [0u8; 8];
        // SAFETY: nothing is posted.
        let r = unsafe { a.register(buf.as_mut_ptr(), 8, Access::LOCAL_WRITE) }.unwrap();
        let mut slab = Slab::with_capacity(1);
        // SAFETY: nothing is posted.
        let err = unsafe {
            post_recv_tracked(
                a.link(la).unwrap(),
                &mut slab,
                (1..=2)
                    .map(|state| TrackedRecv {
                        buf: r.sge(r.addr(), 8).unwrap(),
                        state,
                    })
                    .collect(),
            )
        }
        .unwrap_err();
        assert_eq!((err.accepted, err.unposted), (0, vec![1, 2]));
        assert_eq!(slab.outstanding(), 0);
        drop(r);
    }

    #[test]
    fn tracked_sends_are_always_signaled() {
        let (mut a, mut b, la, lb) = connected(4, 4);
        let mut src = [1u8; 8];
        let mut land = [0u8; 8];
        // SAFETY: both buffers outlive the regions and completions below.
        let rs = unsafe { a.register(src.as_mut_ptr(), 8, Access::LOCAL_WRITE) }.unwrap();
        let rl = unsafe {
            b.register(
                land.as_mut_ptr(),
                8,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        let mut slab = Slab::with_capacity(8);
        let link = a.link(la).unwrap();
        // SAFETY: as above.
        unsafe {
            post_send_tracked(
                link,
                &mut slab,
                vec![TrackedSend {
                    op: SendOp::RdmaWrite {
                        remote_addr: rl.addr(),
                        rkey: rl.rkey(),
                    },
                    local: rs.sge(rs.addr(), 8).unwrap(),
                    state: "pin",
                }],
            )
        }
        .unwrap();
        // A successful unsignaled write would never appear here.
        let mut c = Vec::new();
        link.poll(&mut c, 8).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(slab.complete(&c[0]).unwrap(), "pin");
        drop((rs, rl));
        let _ = lb;
        assert_eq!(land, [1; 8]);
    }

    #[test]
    fn drain_refuses_a_link_not_in_error() {
        let (mut a, _b, la, _lb) = connected(2, 2);
        let mut slab: Slab<()> = Slab::with_capacity(4);
        let err = drain(
            a.link(la).unwrap(),
            &mut slab,
            &mut Vec::new(),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(matches!(err, DrainError::NotInError(QpState::Rts)), "{err}");
    }

    /// A slot the device will never complete — here, one the slab issued but
    /// nothing posted — must end the drain loudly, not hang it.
    #[test]
    fn drain_reports_a_stall_instead_of_hanging() {
        let (mut a, _b, la, _lb) = connected(2, 2);
        let mut slab = Slab::with_capacity(4);
        slab.insert(WrKind::Write, "never posted").unwrap();
        let link = a.link(la).unwrap();
        link.to_error().unwrap();
        let err = drain(link, &mut slab, &mut Vec::new(), Duration::from_millis(10)).unwrap_err();
        assert!(matches!(err, DrainError::Stuck { outstanding: 1 }), "{err}");
    }
}
