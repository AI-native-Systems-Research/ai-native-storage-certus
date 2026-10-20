//! The in-process transport: several endpoints in one process, joined by a
//! [`MockFabric`].
//!
//! Compiled in every build — the default build *serves* over it (research
//! D14) — and modelled on reliable-connected verbs closely enough that the
//! engine's flow control is exercised for real, not stubbed:
//!
//! * **The send queue executes in order.** A send whose peer has no receive
//!   posted stalls, and everything posted after it waits behind it — the
//!   infinite RNR retry (`rnr_retry_count = 7`) the predecessors configure.
//!   That stall is FR-026's deadlock, so it must be reproducible here.
//! * **One-sided writes are checked** against the target's registrations: an
//!   unknown `rkey`, missing `REMOTE_WRITE` or an out-of-bounds address fails
//!   `REM_ACCESS_ERR` and moves both queue pairs to ERROR, as hardware does.
//!   A valid write copies bytes into the registered memory.
//! * **ERROR flushes** every queued work request, signaled or not, and a send
//!   towards a peer that is in ERROR or gone fails `RETRY_EXC_ERR`.
//! * **Capacity** is enforced at post time (`ENOMEM`), and a send-queue slot
//!   is held until a completion at or after it is polled — so an unsignaled
//!   request retires only behind a later signaled one, as on hardware.
//!
//! Local buffers are read, and receive buffers written, when the work request
//! *executes*, not when it is posted — that is when the NIC touches them.
//!
//! The fabric can record an event log (T011's ERROR-ordering piece, built for
//! T021): each queue pair forced to ERROR, each completion handed to a poller,
//! and each link closed, stamped from a sequence counter tests may share with
//! other instruments, so teardown-before-reclaim is assertable as an order.
//!
//! It also has one fault knob, `MockFabric::pause_sends`, which holds every
//! posted send in flight, so a test can tear a connection down with writes
//! genuinely outstanding.
//!
//! Not yet modelled (T011): a finite RNR retry, injected inbound messages,
//! `LOCAL_WRITE` enforcement on receive buffers, and timing — everything here
//! happens at post or poll.

// Consumed from T011/T012 onward.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::transport::{
    errno, wc, Access, Completed, Completion, CompletionError, Link, LinkId, PostError, QpCaps,
    QpState, RdmaError, RecvRequest, Region, RequestId, SendOp, SendRequest, Transport,
    TransportEvent, TransportId, MAX_ACCEPT_PRIVATE_DATA, MAX_CONNECT_PRIVATE_DATA,
    MAX_REJECT_PRIVATE_DATA,
};
use super::PeerId;

/// A shared in-process fabric. Clones refer to the same fabric.
#[derive(Clone, Default)]
pub struct MockFabric {
    inner: Arc<Mutex<Fabric>>,
}

/// One entry in a fabric's event log, from the point of view of the link's
/// owner.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogEvent {
    /// The queue pair entered ERROR: after this, nothing it carries lands.
    ToError(LinkId),
    /// A poller took this completion off the link.
    Completed(LinkId, u64),
    /// The link was destroyed.
    Closed(LinkId),
}

/// A sequence counter shared by every instrument in a test, so events they
/// record separately can be put in one order.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Sequence(Arc<std::sync::atomic::AtomicU64>);

#[cfg(test)]
impl Sequence {
    pub(crate) fn next(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl MockFabric {
    /// A fresh, isolated fabric. Tests should use one each.
    pub fn new() -> Self {
        Self::default()
    }

    /// The process-wide fabric the default build serves over.
    pub fn process() -> Self {
        static PROCESS: OnceLock<MockFabric> = OnceLock::new();
        PROCESS.get_or_init(MockFabric::new).clone()
    }

    /// Start recording the event log, stamped from `seq`.
    #[cfg(test)]
    pub(crate) fn record(&self, seq: &Sequence) {
        self.lock().log = Some((seq.clone(), Vec::new()));
    }

    /// Hold every posted send in flight (a fault knob): nothing executes
    /// until cleared, but forcing ERROR still flushes it.
    #[cfg(test)]
    pub(crate) fn pause_sends(&self, on: bool) {
        let mut f = self.lock();
        f.sends_paused = on;
        if !on {
            let links: Vec<_> = f.qps.keys().copied().collect();
            for l in links {
                f.progress(l);
            }
        }
    }

    /// The events recorded so far, with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn log(&self) -> Vec<(u64, LogEvent)> {
        self.lock()
            .log
            .as_ref()
            .map(|(_, l)| l.clone())
            .unwrap_or_default()
    }

    /// Queue pairs alive on the fabric, both ends counted.
    #[cfg(test)]
    pub(crate) fn qp_count(&self) -> usize {
        self.lock().qps.len()
    }

    fn lock(&self) -> MutexGuard<'_, Fabric> {
        // A panic while holding the lock leaves the fabric inconsistent;
        // carrying on would turn one failure into misleading others.
        self.inner.lock().expect("mock fabric poisoned")
    }
}

#[derive(Default)]
struct Fabric {
    next_id: u64,
    listeners: HashMap<PeerId, TransportId>,
    events: HashMap<TransportId, Vec<TransportEvent>>,
    /// Inbound connects awaiting accept/reject: (listener, request) → link.
    requests: HashMap<(TransportId, RequestId), LinkId>,
    regions: HashMap<u32, RegionEntry>,
    qps: HashMap<LinkId, Qp>,
    #[cfg(test)]
    log: Option<(Sequence, Vec<(u64, LogEvent)>)>,
    /// While set, no send queue executes: posted work stays in flight until
    /// the knob is cleared or its queue pair is forced to ERROR.
    #[cfg(test)]
    sends_paused: bool,
}

struct RegionEntry {
    owner: TransportId,
    addr: u64,
    len: usize,
    access: Access,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Peer {
    /// Not yet connected.
    None,
    Connected(LinkId),
    /// Disconnected or closed; sends towards it fail.
    Gone,
}

struct Qp {
    owner: TransportId,
    state: QpState,
    caps: QpCaps,
    peer: Peer,
    /// Posted send-queue work requests not yet retired by a poll. Those not
    /// `done` have not executed; they run strictly in order.
    sq: VecDeque<SqWr>,
    rq: VecDeque<RqWr>,
    /// Completions, each tagged with whether it retires a send-queue slot.
    cq: VecDeque<(Completion, bool)>,
}

struct SqWr {
    wr_id: u64,
    signaled: bool,
    op: SendOp,
    addr: u64,
    len: u32,
    done: bool,
}

struct RqWr {
    wr_id: u64,
    addr: u64,
    len: u32,
}

fn verb(op: &'static str, errno: i32) -> RdmaError {
    RdmaError::Verb { op, errno }
}

fn failed(status: i32) -> CompletionError {
    CompletionError::Failed {
        status,
        vendor_err: 0,
    }
}

fn padded(data: &[u8], size: usize) -> Vec<u8> {
    let mut v = data.to_vec();
    v.resize(size, 0);
    v
}

impl Fabric {
    fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn event(&mut self, to: TransportId, ev: TransportEvent) {
        self.events.entry(to).or_default().push(ev);
    }

    fn qp(&mut self, link: LinkId) -> &mut Qp {
        self.qps
            .get_mut(&link)
            .expect("mock link has no queue pair")
    }

    fn new_qp(&mut self, owner: TransportId, caps: QpCaps) -> LinkId {
        let link = LinkId(self.next());
        self.qps.insert(
            link,
            Qp {
                owner,
                state: QpState::Init,
                caps,
                peer: Peer::None,
                sq: VecDeque::new(),
                rq: VecDeque::new(),
                cq: VecDeque::new(),
            },
        );
        link
    }

    /// Force `link` to ERROR, flushing everything queued on it, then let its
    /// peer discover that (its pending sends fail).
    #[cfg(test)]
    fn note(&mut self, ev: LogEvent) {
        if let Some((seq, log)) = self.log.as_mut() {
            log.push((seq.next(), ev));
        }
    }

    fn force_error(&mut self, link: LinkId) {
        #[cfg(test)]
        if self.qps.get(&link).is_some_and(|q| q.state != QpState::Err) {
            self.note(LogEvent::ToError(link));
        }
        let qp = self.qp(link);
        qp.state = QpState::Err;
        for wr in qp.sq.iter_mut().filter(|wr| !wr.done) {
            wr.done = true;
            qp.cq.push_back((
                Completion {
                    wr_id: wr.wr_id,
                    result: Err(CompletionError::Flushed),
                },
                true,
            ));
        }
        for wr in std::mem::take(&mut qp.rq) {
            qp.cq.push_back((
                Completion {
                    wr_id: wr.wr_id,
                    result: Err(CompletionError::Flushed),
                },
                false,
            ));
        }
        if let Peer::Connected(peer) = qp.peer {
            self.progress(peer);
        }
    }

    /// Fail the first unexecuted send-queue request with `status` and move the
    /// queue pair to ERROR (which flushes the rest).
    fn fail_head(&mut self, link: LinkId, status: i32) {
        let qp = self.qp(link);
        if let Some(wr) = qp.sq.iter_mut().find(|wr| !wr.done) {
            wr.done = true;
            let wr_id = wr.wr_id;
            qp.cq.push_back((
                Completion {
                    wr_id,
                    result: Err(failed(status)),
                },
                true,
            ));
        }
        self.force_error(link);
    }

    /// Execute `link`'s send queue in order for as long as it can proceed.
    fn progress(&mut self, link: LinkId) {
        #[cfg(test)]
        if self.sends_paused {
            return;
        }
        loop {
            let qp = self.qp(link);
            if qp.state != QpState::Rts {
                return;
            }
            let Some(i) = qp.sq.iter().position(|wr| !wr.done) else {
                return;
            };
            let (op, addr, len, wr_id, signaled) = {
                let wr = &qp.sq[i];
                (wr.op, wr.addr, wr.len, wr.wr_id, wr.signaled)
            };
            let peer_link = qp.peer;
            let peer = match peer_link {
                Peer::Connected(p) if self.qps[&p].state != QpState::Err => p,
                // No responder: the requester's retries run out.
                _ => return self.fail_head(link, wc::RETRY_EXC_ERR),
            };
            let completed = match op {
                SendOp::RdmaWrite { remote_addr, rkey } => {
                    let target = self.qps[&peer].owner;
                    let ok = self.regions.get(&rkey).is_some_and(|r| {
                        r.owner == target
                            && r.access.contains(Access::REMOTE_WRITE)
                            && super::transport::check_range(
                                r.addr,
                                r.len,
                                remote_addr,
                                len as usize,
                            )
                            .is_ok()
                    });
                    if !ok {
                        // The responder detects the violation and NAKs; both
                        // ends of the connection go to ERROR.
                        self.fail_head(link, wc::REM_ACCESS_ERR);
                        self.force_error(peer);
                        return;
                    }
                    // SAFETY: the local buffer is valid until this request
                    // completes (`post_send`'s contract) and the target range
                    // lies inside a live registration (checked above), whose
                    // memory stays allocated per `register`'s contract. `copy`
                    // tolerates overlap, which loopback can produce.
                    unsafe {
                        std::ptr::copy(addr as *const u8, remote_addr as *mut u8, len as usize)
                    };
                    Completed::Write
                }
                SendOp::Send => {
                    let Some(recv) = self.qp(peer).rq.pop_front() else {
                        // RNR with infinite retry: stall, and so does
                        // everything queued behind this send.
                        return;
                    };
                    if len > recv.len {
                        let p = self.qp(peer);
                        p.cq.push_back((
                            Completion {
                                wr_id: recv.wr_id,
                                result: Err(failed(wc::LOC_LEN_ERR)),
                            },
                            false,
                        ));
                        self.force_error(peer);
                        return self.fail_head(link, wc::REM_INV_REQ_ERR);
                    }
                    // SAFETY: the send buffer is valid until completion, and
                    // the receive buffer is valid and untouched by anyone else
                    // until its completion (`post_send`/`post_recv`
                    // contracts); `len <= recv.len` was checked above.
                    unsafe {
                        std::ptr::copy(addr as *const u8, recv.addr as *mut u8, len as usize)
                    };
                    self.qp(peer).cq.push_back((
                        Completion {
                            wr_id: recv.wr_id,
                            result: Ok(Completed::Recv { byte_len: len }),
                        },
                        false,
                    ));
                    Completed::Send
                }
            };
            let qp = self.qp(link);
            qp.sq[i].done = true;
            if signaled {
                qp.cq.push_back((
                    Completion {
                        wr_id,
                        result: Ok(completed),
                    },
                    true,
                ));
            }
        }
    }

    /// Mark both ends gone and tell each owner, if they were connected.
    fn sever(&mut self, link: LinkId) {
        let Peer::Connected(peer) = self.qp(link).peer else {
            return;
        };
        self.qp(link).peer = Peer::Gone;
        let peer_owner = {
            let p = self.qp(peer);
            p.peer = Peer::Gone;
            p.owner
        };
        let owner = self.qp(link).owner;
        self.event(owner, TransportEvent::Disconnected { link });
        self.event(peer_owner, TransportEvent::Disconnected { link: peer });
        // The peer's pending sends now fail, as its retries would.
        self.progress(peer);
    }
}

/// Deregisters on drop.
struct MockRegistration {
    fabric: MockFabric,
    rkey: u32,
}

impl Drop for MockRegistration {
    fn drop(&mut self) {
        self.fabric.lock().regions.remove(&self.rkey);
    }
}

/// One endpoint's view of a [`MockFabric`].
pub struct MockTransport {
    fabric: MockFabric,
    id: TransportId,
    links: HashMap<LinkId, MockLink>,
}

impl MockTransport {
    pub fn new(fabric: MockFabric) -> Self {
        MockTransport {
            fabric,
            id: TransportId::next(),
            links: HashMap::new(),
        }
    }

    fn add_link(&mut self, link: LinkId) {
        self.links.insert(
            link,
            MockLink {
                fabric: self.fabric.clone(),
                link,
            },
        );
    }
}

impl Transport for MockTransport {
    fn id(&self) -> TransportId {
        self.id
    }

    fn listen(&mut self, local: &PeerId) -> Result<(), RdmaError> {
        let mut f = self.fabric.lock();
        match f.listeners.get(local) {
            Some(&owner) if owner != self.id => Err(verb("rdma_bind_addr", errno::EADDRINUSE)),
            _ => {
                f.listeners.insert(local.clone(), self.id);
                Ok(())
            }
        }
    }

    unsafe fn register(
        &mut self,
        addr: *mut u8,
        len: usize,
        access: Access,
    ) -> Result<Region, RdmaError> {
        if addr.is_null() || len == 0 {
            return Err(RdmaError::Invalid {
                op: "ibv_reg_mr",
                reason: "empty or null region",
            });
        }
        if access.contains(Access::REMOTE_WRITE) && !access.contains(Access::LOCAL_WRITE) {
            return Err(verb("ibv_reg_mr", errno::EINVAL));
        }
        let mut f = self.fabric.lock();
        let key = f.next() as u32;
        f.regions.insert(
            key,
            RegionEntry {
                owner: self.id,
                addr: addr as u64,
                len,
                access,
            },
        );
        let registration = MockRegistration {
            fabric: self.fabric.clone(),
            rkey: key,
        };
        Ok(Region::new(
            addr as u64,
            len,
            key,
            key,
            self.id,
            Box::new(registration),
        ))
    }

    fn connect(
        &mut self,
        peer: &PeerId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError> {
        if private_data.len() > MAX_CONNECT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_connect",
                reason: "private_data exceeds 56 bytes",
            });
        }
        let link = {
            let mut f = self.fabric.lock();
            let link = f.new_qp(self.id, caps);
            match f.listeners.get(peer).copied() {
                None => f.event(
                    self.id,
                    TransportEvent::ConnectError {
                        link,
                        reason: format!("no listener at {peer}"),
                    },
                ),
                Some(listener) => {
                    let request = RequestId(f.next());
                    f.requests.insert((listener, request), link);
                    f.event(
                        listener,
                        TransportEvent::ConnectRequest {
                            request,
                            private_data: padded(private_data, MAX_CONNECT_PRIVATE_DATA),
                        },
                    );
                }
            }
            link
        };
        self.add_link(link);
        Ok(link)
    }

    fn accept(
        &mut self,
        request: RequestId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError> {
        if private_data.len() > MAX_ACCEPT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_accept",
                reason: "private_data exceeds 196 bytes",
            });
        }
        let link = {
            let mut f = self.fabric.lock();
            let initiator = f
                .requests
                .remove(&(self.id, request))
                .ok_or_else(|| verb("rdma_accept", errno::EINVAL))?;
            let link = f.new_qp(self.id, caps);
            f.qp(link).state = QpState::Rts;
            f.qp(link).peer = Peer::Connected(initiator);
            let init_owner = {
                let i = f.qp(initiator);
                i.state = QpState::Rts;
                i.peer = Peer::Connected(link);
                i.owner
            };
            f.event(
                init_owner,
                TransportEvent::Established {
                    link: initiator,
                    private_data: padded(private_data, MAX_ACCEPT_PRIVATE_DATA),
                },
            );
            f.event(
                self.id,
                TransportEvent::Established {
                    link,
                    private_data: Vec::new(),
                },
            );
            // Either side may have posted before the connection completed.
            f.progress(initiator);
            f.progress(link);
            link
        };
        self.add_link(link);
        Ok(link)
    }

    fn reject(&mut self, request: RequestId, private_data: &[u8]) -> Result<(), RdmaError> {
        if private_data.len() > MAX_REJECT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_reject",
                reason: "private_data exceeds 148 bytes",
            });
        }
        let mut f = self.fabric.lock();
        let initiator = f
            .requests
            .remove(&(self.id, request))
            .ok_or_else(|| verb("rdma_reject", errno::EINVAL))?;
        // librdmacm moves the initiator's QP to ERROR on REJECTED.
        f.force_error(initiator);
        let owner = f.qp(initiator).owner;
        f.event(
            owner,
            TransportEvent::Rejected {
                link: initiator,
                private_data: padded(private_data, MAX_REJECT_PRIVATE_DATA),
            },
        );
        Ok(())
    }

    fn poll_events(&mut self, out: &mut Vec<TransportEvent>) {
        if let Some(evs) = self.fabric.lock().events.get_mut(&self.id) {
            out.append(evs);
        }
    }

    fn link(&mut self, link: LinkId) -> Option<&mut dyn Link> {
        self.links.get_mut(&link).map(|l| l as &mut dyn Link)
    }

    fn close(&mut self, link: LinkId) {
        if self.links.remove(&link).is_none() {
            return;
        }
        let mut f = self.fabric.lock();
        f.sever(link);
        f.requests.retain(|_, l| *l != link);
        f.qps.remove(&link);
        #[cfg(test)]
        f.note(LogEvent::Closed(link));
    }
}

impl Drop for MockTransport {
    fn drop(&mut self) {
        let links: Vec<_> = self.links.keys().copied().collect();
        for link in links {
            self.close(link);
        }
        let mut f = self.fabric.lock();
        f.listeners.retain(|_, owner| *owner != self.id);
        f.events.remove(&self.id);
        // Connects aimed at us that we never answered: the initiators' CM
        // times out.
        let orphaned: Vec<_> = f
            .requests
            .iter()
            .filter(|((listener, _), _)| *listener == self.id)
            .map(|(k, l)| (*k, *l))
            .collect();
        for (key, initiator) in orphaned {
            f.requests.remove(&key);
            let owner = f.qp(initiator).owner;
            f.event(
                owner,
                TransportEvent::ConnectError {
                    link: initiator,
                    reason: "listener went away".into(),
                },
            );
        }
    }
}

/// A link on a [`MockTransport`]; its state lives in the fabric.
pub struct MockLink {
    fabric: MockFabric,
    link: LinkId,
}

impl MockLink {
    fn check_owner(&self, f: &Fabric, region: &Region, op: &'static str) -> Result<(), RdmaError> {
        if region.owner() != f.qps[&self.link].owner {
            return Err(RdmaError::Invalid {
                op,
                reason: "buffer is registered with another transport",
            });
        }
        Ok(())
    }
}

impl Link for MockLink {
    fn state(&self) -> Result<QpState, RdmaError> {
        Ok(self.fabric.lock().qps[&self.link].state)
    }

    fn granted(&self) -> QpCaps {
        self.fabric.lock().qps[&self.link].caps
    }

    fn to_error(&mut self) -> Result<(), RdmaError> {
        self.fabric.lock().force_error(self.link);
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), RdmaError> {
        let mut f = self.fabric.lock();
        // librdmacm forces ERROR on InfiniBand/RoCE before disconnecting.
        f.force_error(self.link);
        f.sever(self.link);
        Ok(())
    }

    unsafe fn post_send(&mut self, reqs: &[SendRequest<'_>]) -> Result<(), PostError> {
        let mut f = self.fabric.lock();
        for (i, r) in reqs.iter().enumerate() {
            let stop = |error| Err(PostError { accepted: i, error });
            if let Err(e) = self.check_owner(&f, r.local.region(), "ibv_post_send") {
                return stop(e);
            }
            let qp = f.qp(self.link);
            match qp.state {
                // Posting on an ERROR queue pair succeeds and flushes at once.
                QpState::Err => {}
                QpState::Rts => {
                    if qp.sq.len() >= qp.caps.max_send_wr as usize {
                        drop(f);
                        return stop(verb("ibv_post_send", errno::ENOMEM));
                    }
                }
                _ => {
                    drop(f);
                    return stop(verb("ibv_post_send", errno::EINVAL));
                }
            }
            let flushed = qp.state == QpState::Err;
            qp.sq.push_back(SqWr {
                wr_id: r.wr_id,
                signaled: r.signaled,
                op: r.op,
                addr: r.local.addr(),
                len: r.local.len(),
                done: flushed,
            });
            if flushed {
                qp.cq.push_back((
                    Completion {
                        wr_id: r.wr_id,
                        result: Err(CompletionError::Flushed),
                    },
                    true,
                ));
            }
            f.progress(self.link);
        }
        Ok(())
    }

    unsafe fn post_recv(&mut self, reqs: &[RecvRequest<'_>]) -> Result<(), PostError> {
        let mut f = self.fabric.lock();
        for (i, r) in reqs.iter().enumerate() {
            if let Err(error) = self.check_owner(&f, r.buf.region(), "ibv_post_recv") {
                return Err(PostError { accepted: i, error });
            }
            let qp = f.qp(self.link);
            if qp.state == QpState::Err {
                qp.cq.push_back((
                    Completion {
                        wr_id: r.wr_id,
                        result: Err(CompletionError::Flushed),
                    },
                    false,
                ));
                continue;
            }
            if qp.rq.len() >= qp.caps.max_recv_wr as usize {
                return Err(PostError {
                    accepted: i,
                    error: verb("ibv_post_recv", errno::ENOMEM),
                });
            }
            qp.rq.push_back(RqWr {
                wr_id: r.wr_id,
                addr: r.buf.addr(),
                len: r.buf.len(),
            });
            // A peer stalled on RNR towards us may now proceed.
            if let Peer::Connected(peer) = qp.peer {
                f.progress(peer);
            }
        }
        Ok(())
    }

    fn poll(&mut self, out: &mut Vec<Completion>, max: usize) -> Result<usize, RdmaError> {
        let mut f = self.fabric.lock();
        let qp = f.qp(self.link);
        let mut n = 0;
        while n < max {
            let Some((c, from_sq)) = qp.cq.pop_front() else {
                break;
            };
            if from_sq {
                // Retire send-queue slots up to and including this request:
                // an unsignaled request is freed only by a later completion.
                while let Some(wr) = qp.sq.pop_front() {
                    if wr.wr_id == c.wr_id {
                        break;
                    }
                }
            }
            out.push(c);
            n += 1;
        }
        #[cfg(test)]
        for c in &out[out.len() - n..] {
            f.note(LogEvent::Completed(self.link, c.wr_id));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: QpCaps = QpCaps {
        max_send_wr: 4,
        max_recv_wr: 4,
    };

    fn peer(s: &str) -> PeerId {
        PeerId::parse(s).unwrap()
    }

    /// Two transports on one fabric, `a` connected to `b`.
    struct Pair {
        a: MockTransport,
        b: MockTransport,
        la: LinkId,
        lb: LinkId,
    }

    fn pair() -> Pair {
        let fabric = MockFabric::new();
        let mut a = MockTransport::new(fabric.clone());
        let mut b = MockTransport::new(fabric);
        b.listen(&peer("10.0.0.2:7000")).unwrap();
        let la = a.connect(&peer("10.0.0.2:7000"), b"from-a", CAPS).unwrap();

        let mut evs = Vec::new();
        b.poll_events(&mut evs);
        let [TransportEvent::ConnectRequest {
            request,
            private_data,
        }] = evs.as_slice()
        else {
            panic!("expected one connect request, got {evs:?}");
        };
        assert_eq!(&private_data[..6], b"from-a");
        assert_eq!(private_data.len(), MAX_CONNECT_PRIVATE_DATA, "zero-padded");
        let lb = b.accept(*request, b"from-b", CAPS).unwrap();

        evs.clear();
        a.poll_events(&mut evs);
        assert!(matches!(
            evs.as_slice(),
            [TransportEvent::Established { link, private_data }]
                if *link == la && private_data.starts_with(b"from-b")
        ));
        Pair { a, b, la, lb }
    }

    fn reg(t: &mut MockTransport, buf: &mut [u8], access: Access) -> Region {
        // SAFETY: every test keeps `buf` alive past the region and its links.
        unsafe { t.register(buf.as_mut_ptr(), buf.len(), access) }.unwrap()
    }

    fn poll(t: &mut MockTransport, l: LinkId) -> Vec<Completion> {
        let mut out = Vec::new();
        t.link(l).unwrap().poll(&mut out, 64).unwrap();
        out
    }

    const RW: Access = Access(Access::LOCAL_WRITE.0 | Access::REMOTE_WRITE.0);

    #[test]
    fn message_and_write_cross_the_seam() {
        let mut p = pair();
        let mut src = *b"hello, world....";
        let mut dst = [0u8; 16];
        let mut land = [0u8; 16];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        let rd = reg(&mut p.b, &mut dst, Access::LOCAL_WRITE);
        let rl = reg(&mut p.b, &mut land, RW);

        // SAFETY: the buffers outlive every completion polled below.
        unsafe {
            p.b.link(p.lb)
                .unwrap()
                .post_recv(&[RecvRequest {
                    wr_id: 7,
                    buf: rd.sge(rd.addr(), 16).unwrap(),
                }])
                .unwrap();
            p.a.link(p.la)
                .unwrap()
                .post_send(&[
                    SendRequest {
                        wr_id: 1,
                        op: SendOp::Send,
                        local: rs.sge(rs.addr(), 5).unwrap(),
                        signaled: true,
                    },
                    SendRequest {
                        wr_id: 2,
                        op: SendOp::RdmaWrite {
                            remote_addr: rl.addr() + 4,
                            rkey: rl.rkey(),
                        },
                        local: rs.sge(rs.addr() + 7, 5).unwrap(),
                        signaled: true,
                    },
                ])
                .unwrap();
        }
        let ca = poll(&mut p.a, p.la);
        assert_eq!(
            ca.iter().map(|c| (c.wr_id, c.result)).collect::<Vec<_>>(),
            [(1, Ok(Completed::Send)), (2, Ok(Completed::Write))]
        );
        assert_eq!(
            poll(&mut p.b, p.lb),
            [Completion {
                wr_id: 7,
                result: Ok(Completed::Recv { byte_len: 5 })
            }]
        );
        drop((rs, rd, rl));
        assert_eq!(&dst[..5], b"hello");
        assert_eq!(&land[4..9], b"world");
        assert_eq!(land[..4], [0; 4]);
    }

    /// A send with no receive posted stalls, and holds up what follows it —
    /// the RNR shape FR-026's partitioning exists to prevent.
    #[test]
    fn send_without_receive_stalls_and_blocks_later_work() {
        let mut p = pair();
        let mut src = [1u8; 8];
        let mut land = [0u8; 8];
        let mut dst = [0u8; 8];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        let rl = reg(&mut p.b, &mut land, RW);
        let rd = reg(&mut p.b, &mut dst, Access::LOCAL_WRITE);
        let write = SendRequest {
            wr_id: 2,
            op: SendOp::RdmaWrite {
                remote_addr: rl.addr(),
                rkey: rl.rkey(),
            },
            local: rs.sge(rs.addr(), 8).unwrap(),
            signaled: true,
        };
        let send = SendRequest {
            wr_id: 1,
            op: SendOp::Send,
            local: rs.sge(rs.addr(), 8).unwrap(),
            signaled: true,
        };
        // SAFETY: buffers outlive the completions polled below.
        unsafe { p.a.link(p.la).unwrap().post_send(&[send, write]) }.unwrap();
        assert!(poll(&mut p.a, p.la).is_empty(), "stalled on RNR");
        assert_eq!(land, [0; 8], "the write is queued behind the send");

        // SAFETY: as above.
        unsafe {
            p.b.link(p.lb).unwrap().post_recv(&[RecvRequest {
                wr_id: 9,
                buf: rd.sge(rd.addr(), 8).unwrap(),
            }])
        }
        .unwrap();
        assert_eq!(
            poll(&mut p.a, p.la).len(),
            2,
            "both proceed once a receive is posted"
        );
        drop((rs, rl, rd));
        assert_eq!(land, [1; 8]);
    }

    #[test]
    fn write_with_bad_rkey_fails_and_errors_both_ends() {
        let mut p = pair();
        let mut src = [1u8; 8];
        let mut land = [0u8; 8];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        let rl = reg(&mut p.b, &mut land, Access::LOCAL_WRITE); // no REMOTE_WRITE
        let w = |wr_id| SendRequest {
            wr_id,
            op: SendOp::RdmaWrite {
                remote_addr: rl.addr(),
                rkey: rl.rkey(),
            },
            local: rs.sge(rs.addr(), 8).unwrap(),
            signaled: false,
        };
        // SAFETY: buffers outlive the completions polled below.
        unsafe { p.a.link(p.la).unwrap().post_send(&[w(1), w(2)]) }.unwrap();
        let c = poll(&mut p.a, p.la);
        assert_eq!(c[0].wr_id, 1);
        assert_eq!(c[0].result, Err(failed(wc::REM_ACCESS_ERR)));
        assert_eq!(c[1].wr_id, 2);
        assert_eq!(
            c[1].result,
            Err(CompletionError::Flushed),
            "unsignaled still flushes"
        );
        assert_eq!(p.a.link(p.la).unwrap().state(), Ok(QpState::Err));
        assert_eq!(p.b.link(p.lb).unwrap().state(), Ok(QpState::Err));
        drop((rs, rl));
        assert_eq!(land, [0; 8], "nothing landed");
    }

    #[test]
    fn to_error_flushes_receives_and_fails_peer_sends() {
        let mut p = pair();
        let mut buf = [0u8; 8];
        let rb = reg(&mut p.b, &mut buf, Access::LOCAL_WRITE);
        let mut src = [0u8; 8];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        // SAFETY: buffers outlive the completions polled below.
        unsafe {
            p.b.link(p.lb).unwrap().post_recv(&[RecvRequest {
                wr_id: 5,
                buf: rb.sge(rb.addr(), 8).unwrap(),
            }])
        }
        .unwrap();
        p.b.link(p.lb).unwrap().to_error().unwrap();
        assert_eq!(
            poll(&mut p.b, p.lb),
            [Completion {
                wr_id: 5,
                result: Err(CompletionError::Flushed)
            }]
        );
        // SAFETY: as above.
        unsafe {
            p.a.link(p.la).unwrap().post_send(&[SendRequest {
                wr_id: 1,
                op: SendOp::Send,
                local: rs.sge(rs.addr(), 8).unwrap(),
                signaled: true,
            }])
        }
        .unwrap();
        assert_eq!(
            poll(&mut p.a, p.la)[0].result,
            Err(failed(wc::RETRY_EXC_ERR))
        );
    }

    /// An unsignaled request holds its slot until a later completion retires
    /// it — the reason T014's credits cannot simply count signaled requests.
    #[test]
    fn unsignaled_slots_retire_only_behind_a_signaled_completion() {
        let mut p = pair();
        let mut src = [0u8; 8];
        let mut land = [0u8; 64];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        let rl = reg(&mut p.b, &mut land, RW);
        let w = |wr_id, signaled| SendRequest {
            wr_id,
            op: SendOp::RdmaWrite {
                remote_addr: rl.addr(),
                rkey: rl.rkey(),
            },
            local: rs.sge(rs.addr(), 8).unwrap(),
            signaled,
        };
        let link = p.a.link(p.la).unwrap();
        // SAFETY: buffers outlive the completions polled below.
        unsafe { link.post_send(&[w(1, false), w(2, false), w(3, false), w(4, false)]) }.unwrap();
        // All four executed, yet the queue is full: nothing has retired them.
        // SAFETY: as above.
        let err = unsafe { link.post_send(&[w(5, true)]) }.unwrap_err();
        assert_eq!(err.accepted, 0);
        assert_eq!(err.error, verb("ibv_post_send", errno::ENOMEM));
        assert!(poll(&mut p.a, p.la).is_empty());
        drop((rs, rl));
    }

    #[test]
    fn capacity_and_partial_post() {
        let mut p = pair();
        let mut buf = [0u8; 64];
        let rb = reg(&mut p.b, &mut buf, Access::LOCAL_WRITE);
        let reqs: Vec<_> = (0..6u64)
            .map(|i| RecvRequest {
                wr_id: i,
                buf: rb.sge(rb.addr() + i * 8, 8).unwrap(),
            })
            .collect();
        // SAFETY: nothing is ever delivered into these buffers.
        let err = unsafe { p.b.link(p.lb).unwrap().post_recv(&reqs) }.unwrap_err();
        assert_eq!(err.accepted, 4, "max_recv_wr is 4");
        drop(rb);
    }

    #[test]
    fn reject_errors_initiator_and_carries_reason() {
        let fabric = MockFabric::new();
        let mut a = MockTransport::new(fabric.clone());
        let mut b = MockTransport::new(fabric);
        b.listen(&peer("10.0.0.2:7000")).unwrap();
        let la = a.connect(&peer("10.0.0.2:7000"), b"", CAPS).unwrap();
        let mut evs = Vec::new();
        b.poll_events(&mut evs);
        let TransportEvent::ConnectRequest { request, .. } = evs[0] else {
            panic!("{evs:?}");
        };
        b.reject(request, b"lost-tie").unwrap();
        evs.clear();
        a.poll_events(&mut evs);
        assert!(matches!(
            evs.as_slice(),
            [TransportEvent::Rejected { link, private_data }]
                if *link == la && private_data.starts_with(b"lost-tie")
                    && private_data.len() == MAX_REJECT_PRIVATE_DATA
        ));
        assert_eq!(a.link(la).unwrap().state(), Ok(QpState::Err));
        assert!(
            b.accept(request, b"", CAPS).is_err(),
            "a request is answered once"
        );
    }

    #[test]
    fn connect_to_nobody_and_listen_conflicts() {
        let fabric = MockFabric::new();
        let mut a = MockTransport::new(fabric.clone());
        let mut b = MockTransport::new(fabric);
        let la = a.connect(&peer("10.0.0.9:1"), b"", CAPS).unwrap();
        let mut evs = Vec::new();
        a.poll_events(&mut evs);
        assert!(
            matches!(evs.as_slice(), [TransportEvent::ConnectError { link, .. }] if *link == la)
        );

        a.listen(&peer("10.0.0.1:1")).unwrap();
        assert_eq!(
            b.listen(&peer("10.0.0.1:1")),
            Err(verb("rdma_bind_addr", errno::EADDRINUSE))
        );
        // Same host, other port: a NUMA sibling (FR-021).
        b.listen(&peer("10.0.0.1:2")).unwrap();
        assert!(
            a.connect(&peer("10.0.0.1:1"), &[0; 57], CAPS).is_err(),
            "56-byte limit"
        );
    }

    #[test]
    fn close_tells_the_peer_and_fails_its_sends() {
        let mut p = pair();
        let mut src = [0u8; 8];
        let rs = reg(&mut p.a, &mut src, Access::LOCAL_WRITE);
        p.b.close(p.lb);
        let mut evs = Vec::new();
        p.a.poll_events(&mut evs);
        assert_eq!(evs, [TransportEvent::Disconnected { link: p.la }]);
        // SAFETY: the send fails without touching remote memory.
        unsafe {
            p.a.link(p.la).unwrap().post_send(&[SendRequest {
                wr_id: 1,
                op: SendOp::Send,
                local: rs.sge(rs.addr(), 8).unwrap(),
                signaled: true,
            }])
        }
        .unwrap();
        assert_eq!(
            poll(&mut p.a, p.la)[0].result,
            Err(failed(wc::RETRY_EXC_ERR))
        );
    }

    #[test]
    fn foreign_buffer_rejected_and_deregistered_key_refused() {
        let mut p = pair();
        let mut src = [0u8; 8];
        let mut land = [0u8; 8];
        let foreign = reg(&mut p.b, &mut src, Access::LOCAL_WRITE);
        // SAFETY: rejected before use.
        let err = unsafe {
            p.a.link(p.la).unwrap().post_send(&[SendRequest {
                wr_id: 1,
                op: SendOp::Send,
                local: foreign.sge(foreign.addr(), 8).unwrap(),
                signaled: true,
            }])
        }
        .unwrap_err();
        assert_eq!(err.accepted, 0);

        let rl = reg(&mut p.b, &mut land, RW);
        let (addr, rkey) = (rl.addr(), rl.rkey());
        drop(rl);
        let mut mine = [7u8; 8];
        let rm = reg(&mut p.a, &mut mine, Access::LOCAL_WRITE);
        // SAFETY: the write is refused at the target, nothing lands.
        unsafe {
            p.a.link(p.la).unwrap().post_send(&[SendRequest {
                wr_id: 2,
                op: SendOp::RdmaWrite {
                    remote_addr: addr,
                    rkey,
                },
                local: rm.sge(rm.addr(), 8).unwrap(),
                signaled: true,
            }])
        }
        .unwrap();
        assert_eq!(
            poll(&mut p.a, p.la)[0].result,
            Err(failed(wc::REM_ACCESS_ERR))
        );
        drop((foreign, rm));
        assert_eq!(land, [0; 8]);
    }

    #[test]
    fn remote_write_requires_local_write() {
        let mut t = MockTransport::new(MockFabric::new());
        let mut buf = [0u8; 8];
        // SAFETY: rejected before registration.
        let r = unsafe { t.register(buf.as_mut_ptr(), 8, Access::REMOTE_WRITE) };
        assert_eq!(r.err(), Some(verb("ibv_reg_mr", errno::EINVAL)));
    }
}
