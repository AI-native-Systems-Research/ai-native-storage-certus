//! The verbs transport (T053): the seam over librdmacm and libibverbs, through
//! the safe wrappers in [`crate::rdma`].
//!
//! # One device, one protection domain, for the endpoint's life (T069, D17)
//!
//! The transport binds its listening id at [`Transport::listen`], which the
//! endpoint calls before registering anything, and allocates the protection
//! domain on that id's device. Every region is registered into it, and it
//! outlives every queue pair, so an `rkey` already on the wire survives a
//! reconnect. A connect that resolves to another device fails: its regions
//! exist only here. The rigs run one NIC per instance, so one device is the
//! whole of it; registering every region on every device is the alternative
//! not taken.
//!
//! # Never blocking
//!
//! The endpoint thread owns the transport and does all its other work between
//! calls, so nothing here waits. The event channel is non-blocking, and an
//! outbound connect is a small state machine — resolve the address, resolve the
//! route, connect — advanced by the events [`poll_events`] reads.
//!
//! # Addresses
//!
//! A peer's host must be an IPv4 literal. There is no resolver (D16, D24), and
//! the rigs name instances by address.
//!
//! [`poll_events`]: Transport::poll_events

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use super::transport::{
    Access, Completion, LinkId, PostError, QpCaps, QpState, RdmaError, RecvRequest, Region,
    RequestId, SendRequest, Transport, TransportEvent, TransportId,
};
use super::PeerId;
use crate::ffi;
use crate::rdma::{
    CmId, CmIdToken, CompletionQueue, ConnParam, EventChannel, MemoryRegion, ProtectionDomain,
    RawSge, VerbsRecv, VerbsSend, WorkCompletion,
};

/// rdma_cm's address and route resolution timeouts.
const RESOLVE_TIMEOUT_MS: i32 = 2000;

/// The listener's backlog.
const BACKLOG: i32 = 64;

/// Transport retries before `RETRY_EXC_ERR`: the most RC allows.
const RETRY_COUNT: u8 = 7;

/// RNR retries: 7 is RC's "retry forever". The receive partition (FR-026) is
/// what keeps a receive posted for every reply, so an RNR stall means only
/// that one is about to be; failing on it instead would turn a moment's
/// back-pressure into a teardown.
const RNR_RETRY_COUNT: u8 = 7;

/// Where an outbound connect has got to.
enum Dialing {
    /// `rdma_resolve_addr` issued; the private data waits for `connect`.
    Address { private_data: Vec<u8>, caps: QpCaps },
    /// `rdma_resolve_route` issued.
    Route { private_data: Vec<u8>, caps: QpCaps },
    /// `rdma_connect` issued; waiting for ESTABLISHED, REJECTED or an error.
    Connecting,
    /// Established, or accepted: both ends may post.
    Up,
    /// Failed or rejected; waiting to be closed.
    Down,
}

/// One link: its id (which owns the QP), and its own completion queue.
struct VerbsLink {
    id: CmId,
    cq: Option<Arc<CompletionQueue>>,
    granted: QpCaps,
    state: Dialing,
    /// We connected, rather than accepted: only the initiator's ESTABLISHED
    /// carries the other end's private data.
    initiator: bool,
    /// The transport whose regions may be posted here.
    owner: TransportId,
    /// Scratch for `ibv_poll_cq`.
    wcs: Vec<WorkCompletion>,
}

impl VerbsLink {
    fn qp(&self) -> Result<&crate::rdma::QueuePair, RdmaError> {
        self.id.qp().ok_or(RdmaError::Invalid {
            op: "ibv_post",
            reason: "the link has no queue pair yet",
        })
    }

    /// The link failed to come up: force its QP, if it has one, to ERROR, as
    /// the seam promises for a rejected or failed connect.
    fn fail(&mut self) {
        self.state = Dialing::Down;
        if let Some(qp) = self.id.qp() {
            let _ = qp.to_error();
        }
    }

    /// Refuse a buffer registered with another transport: its `lkey` means
    /// nothing in this PD, or worse, names another region.
    fn check_owner(&self, r: &Region, op: &'static str) -> Result<(), RdmaError> {
        if r.owner() != self.owner {
            return Err(RdmaError::Invalid {
                op,
                reason: "buffer is registered with another transport",
            });
        }
        Ok(())
    }
}

/// Give `id` its queue pair and its own completion queue in `pd`.
fn make_qp(
    pd: &Arc<ProtectionDomain>,
    id: &mut CmId,
    caps: QpCaps,
    op: &'static str,
) -> Result<(Arc<CompletionQueue>, QpCaps), RdmaError> {
    if !pd.same_device(id) {
        return Err(RdmaError::Invalid {
            op,
            reason: "the peer is reached through another device than the one listened on; \
                     regions exist only there (one device per endpoint)",
        });
    }
    // One CQ per link, deep enough for everything both queues can hold, so it
    // cannot overrun.
    let cq = CompletionQueue::new(pd, (caps.max_send_wr + caps.max_recv_wr) as usize)?;
    let granted = id.create_qp(pd, &cq, &cq, caps)?.granted();
    Ok((cq, granted))
}

/// The verbs transport. See the module docs.
pub struct VerbsTransport {
    id: TransportId,
    channel: Arc<EventChannel>,
    listener: Option<CmId>,
    pd: Option<Arc<ProtectionDomain>>,
    links: HashMap<LinkId, VerbsLink>,
    /// Which link each id is, for matching events.
    by_token: HashMap<CmIdToken, LinkId>,
    /// Inbound requests not yet accepted or rejected.
    requests: HashMap<RequestId, CmId>,
    next: u64,
}

// SAFETY: every field is owned by the one endpoint thread that drives the
// transport; the raw handles inside are moved with it, never shared.
unsafe impl Send for VerbsTransport {}

/// What a registration keeps alive: the region, and through it the PD.
struct VerbsRegistration(#[allow(dead_code)] MemoryRegion);

impl VerbsTransport {
    pub fn new() -> Result<Self, RdmaError> {
        let channel = EventChannel::new()?;
        channel.set_nonblocking()?;
        Ok(VerbsTransport {
            id: TransportId::next(),
            channel,
            listener: None,
            pd: None,
            links: HashMap::new(),
            by_token: HashMap::new(),
            requests: HashMap::new(),
            next: 1,
        })
    }

    fn next_id(&mut self) -> u64 {
        let n = self.next;
        self.next += 1;
        n
    }

    fn pd(&self, op: &'static str) -> Result<&Arc<ProtectionDomain>, RdmaError> {
        self.pd.as_ref().ok_or(RdmaError::Invalid {
            op,
            reason: "listen first: the protection domain is made on the listening device",
        })
    }

    fn param(private_data: &[u8]) -> ConnParam<'_> {
        ConnParam {
            private_data,
            // Nothing in this design reads remotely.
            responder_resources: 0,
            initiator_depth: 0,
            retry_count: RETRY_COUNT,
            rnr_retry_count: RNR_RETRY_COUNT,
        }
    }

    /// Advance the outbound connect `link` on a resolution event.
    fn dial(&mut self, link: LinkId, kind: i32, out: &mut Vec<TransportEvent>) {
        let Some(pd) = self.pd.clone() else {
            return;
        };
        let Some(l) = self.links.get_mut(&link) else {
            return;
        };
        let step = match (std::mem::replace(&mut l.state, Dialing::Down), kind) {
            (Dialing::Address { private_data, caps }, ffi::RDMA_CM_EVENT_ADDR_RESOLVED) => {
                l.id.resolve_route(RESOLVE_TIMEOUT_MS)
                    .map(|()| l.state = Dialing::Route { private_data, caps })
                    .map_err(|e| format!("resolving the route: {e}"))
            }
            (Dialing::Route { private_data, caps }, ffi::RDMA_CM_EVENT_ROUTE_RESOLVED) => {
                make_qp(&pd, &mut l.id, caps, "rdma_create_qp")
                    .map_err(|e| e.to_string())
                    .and_then(|(cq, granted)| {
                        l.cq = Some(cq);
                        l.granted = granted;
                        l.id.connect(Self::param(&private_data))
                            .map_err(|e| format!("connecting: {e}"))
                    })
                    .map(|()| l.state = Dialing::Connecting)
            }
            (_, k) => Err(format!("unexpected connection-manager event {k}")),
        };
        if let Err(reason) = step {
            l.fail();
            out.push(TransportEvent::ConnectError { link, reason });
        }
    }
}

/// An IPv4 instance address, or why not.
fn socket_addr(peer: &PeerId, op: &'static str) -> Result<SocketAddrV4, RdmaError> {
    let ip: Ipv4Addr = peer.host().parse().map_err(|_| RdmaError::Invalid {
        op,
        reason: "the host must be an IPv4 literal (there is no resolver)",
    })?;
    Ok(SocketAddrV4::new(ip, peer.port()))
}

impl Transport for VerbsTransport {
    fn id(&self) -> TransportId {
        self.id
    }

    fn listen(&mut self, local: &PeerId) -> Result<(), RdmaError> {
        if self.listener.is_some() {
            return Err(RdmaError::Invalid {
                op: "rdma_listen",
                reason: "already listening",
            });
        }
        let mut id = CmId::new(&self.channel)?;
        id.bind(socket_addr(local, "rdma_bind_addr")?)?;
        if !id.has_device() {
            return Err(RdmaError::Invalid {
                op: "rdma_bind_addr",
                reason: "the local address is not on an RDMA device",
            });
        }
        id.listen(BACKLOG)?;
        self.pd = Some(ProtectionDomain::new(&id)?);
        self.listener = Some(id);
        Ok(())
    }

    unsafe fn register(
        &mut self,
        addr: *mut u8,
        len: usize,
        access: Access,
    ) -> Result<Region, RdmaError> {
        let pd = self.pd("ibv_reg_mr")?;
        // SAFETY: forwarded from this method's contract.
        let mr = MemoryRegion::register(pd, addr, len, access)?;
        let (a, l, lk, rk) = (mr.addr(), mr.len(), mr.lkey(), mr.rkey());
        Ok(Region::new(
            a,
            l,
            lk,
            rk,
            self.id,
            Box::new(VerbsRegistration(mr)),
        ))
    }

    fn connect(
        &mut self,
        peer: &PeerId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError> {
        if private_data.len() > super::transport::MAX_CONNECT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_connect",
                reason: "private_data exceeds 56 bytes",
            });
        }
        self.pd("rdma_connect")?;
        let dst = socket_addr(peer, "rdma_resolve_addr")?;
        let mut id = CmId::new(&self.channel)?;
        id.resolve_addr(dst, RESOLVE_TIMEOUT_MS)?;
        let link = LinkId(self.next_id());
        self.by_token.insert(id.token(), link);
        self.links.insert(
            link,
            VerbsLink {
                id,
                cq: None,
                granted: caps,
                state: Dialing::Address {
                    private_data: private_data.to_vec(),
                    caps,
                },
                initiator: true,
                owner: self.id,
                wcs: Vec::new(),
            },
        );
        Ok(link)
    }

    fn accept(
        &mut self,
        request: RequestId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError> {
        if private_data.len() > super::transport::MAX_ACCEPT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_accept",
                reason: "private_data exceeds 196 bytes",
            });
        }
        let mut id = self.requests.remove(&request).ok_or(RdmaError::Invalid {
            op: "rdma_accept",
            reason: "no such pending request",
        })?;
        let pd = Arc::clone(self.pd("rdma_accept")?);
        let (cq, granted) = match make_qp(&pd, &mut id, caps, "rdma_accept") {
            Ok(made) => made,
            Err(e) => {
                let _ = id.reject(&[]);
                return Err(e);
            }
        };
        id.accept(Self::param(private_data))?;
        let link = LinkId(self.next_id());
        self.by_token.insert(id.token(), link);
        self.links.insert(
            link,
            VerbsLink {
                id,
                cq: Some(cq),
                granted,
                // Receives may be posted at once; ESTABLISHED says sends may.
                state: Dialing::Connecting,
                initiator: false,
                owner: self.id,
                wcs: Vec::new(),
            },
        );
        Ok(link)
    }

    fn reject(&mut self, request: RequestId, private_data: &[u8]) -> Result<(), RdmaError> {
        if private_data.len() > super::transport::MAX_REJECT_PRIVATE_DATA {
            return Err(RdmaError::Invalid {
                op: "rdma_reject",
                reason: "private_data exceeds 148 bytes",
            });
        }
        let mut id = self.requests.remove(&request).ok_or(RdmaError::Invalid {
            op: "rdma_reject",
            reason: "no such pending request",
        })?;
        id.reject(private_data)
    }

    fn poll_events(&mut self, out: &mut Vec<TransportEvent>) {
        loop {
            let channel = Arc::clone(&self.channel);
            let ev = match channel.try_event() {
                Ok(Some(ev)) => ev,
                Ok(None) => return,
                Err(e) => {
                    // The channel itself failed: nothing more will arrive.
                    eprintln!("certus-transfer-endpoint: rdma_get_cm_event: {e}");
                    return;
                }
            };
            let kind = ev.kind();
            if kind == ffi::RDMA_CM_EVENT_CONNECT_REQUEST {
                let Ok((id, private_data)) = ev.into_connect_request() else {
                    unreachable!("checked the kind")
                };
                let request = RequestId(self.next_id());
                self.requests.insert(request, id);
                out.push(TransportEvent::ConnectRequest {
                    request,
                    private_data,
                });
                continue;
            }
            let token = ev.id_token();
            let private_data = ev.private_data();
            let status = ev.status();
            // Ack before acting: destroying an id waits for its events' acks.
            drop(ev);
            let Some(&link) = self.by_token.get(&token) else {
                continue; // the listener's own events, or a closed link's
            };
            match kind {
                ffi::RDMA_CM_EVENT_ADDR_RESOLVED | ffi::RDMA_CM_EVENT_ROUTE_RESOLVED => {
                    self.dial(link, kind, out)
                }
                ffi::RDMA_CM_EVENT_ESTABLISHED => {
                    if let Some(l) = self.links.get_mut(&link) {
                        // The initiator sees the acceptor's private data; the
                        // acceptor sees none (the seam's contract). librdmacm
                        // already gives the acceptor none (measured on mlx5);
                        // this keeps the contract if a provider ever does.
                        l.state = Dialing::Up;
                        let private_data = if l.initiator {
                            private_data
                        } else {
                            Vec::new()
                        };
                        out.push(TransportEvent::Established { link, private_data });
                    }
                }
                ffi::RDMA_CM_EVENT_REJECTED => {
                    if let Some(l) = self.links.get_mut(&link) {
                        l.fail();
                    }
                    out.push(TransportEvent::Rejected { link, private_data });
                }
                ffi::RDMA_CM_EVENT_DISCONNECTED => {
                    if let Some(l) = self.links.get_mut(&link) {
                        l.state = Dialing::Down;
                    }
                    out.push(TransportEvent::Disconnected { link });
                }
                ffi::RDMA_CM_EVENT_ADDR_ERROR
                | ffi::RDMA_CM_EVENT_ROUTE_ERROR
                | ffi::RDMA_CM_EVENT_CONNECT_ERROR
                | ffi::RDMA_CM_EVENT_UNREACHABLE => {
                    if let Some(l) = self.links.get_mut(&link) {
                        l.fail();
                    }
                    out.push(TransportEvent::ConnectError {
                        link,
                        reason: format!("connection-manager event {kind}, status {status}"),
                    });
                }
                _ => {} // e.g. TIMEWAIT_EXIT: nothing for the seam
            }
        }
    }

    fn link(&mut self, link: LinkId) -> Option<&mut dyn super::transport::Link> {
        self.links
            .get_mut(&link)
            .map(|l| l as &mut dyn super::transport::Link)
    }

    fn close(&mut self, link: LinkId) {
        if let Some(l) = self.links.remove(&link) {
            self.by_token.remove(&l.id.token());
            // Dropping the id destroys its QP, then the id (which also
            // disconnects it); the CQ goes when its last `Arc` does.
            drop(l);
        }
    }
}

impl super::transport::Link for VerbsLink {
    fn state(&self) -> Result<QpState, RdmaError> {
        match self.id.qp() {
            Some(qp) => qp.state(),
            None => Ok(QpState::Reset),
        }
    }

    fn granted(&self) -> QpCaps {
        self.granted
    }

    fn to_error(&mut self) -> Result<(), RdmaError> {
        match self.id.qp() {
            Some(qp) => qp.to_error(),
            None => Ok(()), // no QP: nothing can land
        }
    }

    fn disconnect(&mut self) -> Result<(), RdmaError> {
        self.id.disconnect()
    }

    unsafe fn post_send(&mut self, reqs: &[SendRequest<'_>]) -> Result<(), PostError> {
        let qp = self
            .qp()
            .map_err(|error| PostError { accepted: 0, error })?;
        if reqs
            .iter()
            .any(|r| self.check_owner(r.local.region(), "ibv_post_send").is_err())
        {
            // Checked before posting anything, so nothing was queued.
            return Err(PostError {
                accepted: 0,
                error: RdmaError::Invalid {
                    op: "ibv_post_send",
                    reason: "buffer is registered with another transport",
                },
            });
        }
        let wrs: Vec<VerbsSend> = reqs
            .iter()
            .map(|r| VerbsSend {
                wr_id: r.wr_id,
                op: r.op,
                local: raw(r.local),
                signaled: r.signaled,
            })
            .collect();
        // SAFETY: forwarded from this method's contract.
        qp.post_send(&wrs)
    }

    unsafe fn post_recv(&mut self, reqs: &[RecvRequest<'_>]) -> Result<(), PostError> {
        let qp = self
            .qp()
            .map_err(|error| PostError { accepted: 0, error })?;
        if reqs
            .iter()
            .any(|r| self.check_owner(r.buf.region(), "ibv_post_recv").is_err())
        {
            return Err(PostError {
                accepted: 0,
                error: RdmaError::Invalid {
                    op: "ibv_post_recv",
                    reason: "buffer is registered with another transport",
                },
            });
        }
        let wrs: Vec<VerbsRecv> = reqs
            .iter()
            .map(|r| VerbsRecv {
                wr_id: r.wr_id,
                buf: raw(r.buf),
            })
            .collect();
        // SAFETY: forwarded from this method's contract.
        qp.post_recv(&wrs)
    }

    fn poll(&mut self, out: &mut Vec<Completion>, max: usize) -> Result<usize, RdmaError> {
        let Some(cq) = self.cq.as_ref() else {
            return Ok(0);
        };
        if self.wcs.len() < max {
            self.wcs.resize_with(max, WorkCompletion::default);
        }
        let n = cq.poll(&mut self.wcs[..max])?;
        out.extend(self.wcs[..n].iter().map(WorkCompletion::to_completion));
        Ok(n)
    }
}

/// The verbs form of a seam buffer. Its bounds were checked by `Region::sge`
/// and its `lkey` is its own region's.
fn raw(sge: super::transport::Sge<'_>) -> RawSge {
    RawSge {
        addr: sge.addr(),
        len: sge.len(),
        lkey: sge.lkey(),
    }
}

#[cfg(test)]
mod tests {
    //! Over the NIC: set `CERTUS_RDMA_TEST_IP` to a local RoCE address. Two
    //! transports in one process, on two ports of that address — D16's
    //! single-host loopback.
    use super::*;
    use crate::connection::transport::{Completed, CompletionError, SendOp};
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::time::{Duration, Instant};

    fn ip() -> String {
        std::env::var("CERTUS_RDMA_TEST_IP")
            .expect("set CERTUS_RDMA_TEST_IP to a local RDMA (RoCE) IPv4 address")
    }

    /// A port unlikely to collide between tests run in parallel.
    fn port() -> u16 {
        static NEXT: AtomicU16 = AtomicU16::new(0);
        41000 + (std::process::id() % 1000) as u16 * 16 + NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn peer(port: u16) -> PeerId {
        PeerId::parse(&format!("{}:{port}", ip())).unwrap()
    }

    const CAPS: QpCaps = QpCaps {
        max_send_wr: 16,
        max_recv_wr: 16,
    };

    fn listening() -> (VerbsTransport, PeerId) {
        let mut t = VerbsTransport::new().unwrap();
        let p = peer(port());
        t.listen(&p).unwrap();
        (t, p)
    }

    /// Poll both transports' events until `want` says stop.
    fn pump(
        a: &mut VerbsTransport,
        b: &mut VerbsTransport,
        mut want: impl FnMut(&[TransportEvent], &[TransportEvent]) -> bool,
    ) -> (Vec<TransportEvent>, Vec<TransportEvent>) {
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !want(&ea, &eb) {
            assert!(Instant::now() < deadline, "events: a {ea:?}, b {eb:?}");
            a.poll_events(&mut ea);
            b.poll_events(&mut eb);
            std::thread::sleep(Duration::from_millis(1));
        }
        (ea, eb)
    }

    fn request_of(evs: &[TransportEvent]) -> Option<(RequestId, Vec<u8>)> {
        evs.iter().find_map(|e| match e {
            TransportEvent::ConnectRequest {
                request,
                private_data,
            } => Some((*request, private_data.clone())),
            _ => None,
        })
    }

    fn established(evs: &[TransportEvent], link: LinkId) -> Option<Vec<u8>> {
        evs.iter().find_map(|e| match e {
            TransportEvent::Established {
                link: l,
                private_data,
            } if *l == link => Some(private_data.clone()),
            _ => None,
        })
    }

    /// a connects to b; both links up. Returns (a's link, b's link).
    fn connected(
        a: &mut VerbsTransport,
        b: &mut VerbsTransport,
        b_addr: &PeerId,
    ) -> (LinkId, LinkId) {
        let la = a.connect(b_addr, b"hello-id", CAPS).unwrap();
        let (_, eb) = pump(a, b, |_, eb| request_of(eb).is_some());
        let (req, data) = request_of(&eb).unwrap();
        assert_eq!(&data[..8], b"hello-id", "private data carried, zero-padded");
        let lb = b.accept(req, b"accepted", CAPS).unwrap();
        let (ea, eb) = pump(a, b, |ea, eb| {
            established(ea, la).is_some() && established(eb, lb).is_some()
        });
        assert_eq!(&established(&ea, la).unwrap()[..8], b"accepted");
        assert!(established(&eb, lb).unwrap().is_empty());
        (la, lb)
    }

    fn poll_until(t: &mut VerbsTransport, l: LinkId, n: usize) -> Vec<Completion> {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while out.len() < n {
            assert!(Instant::now() < deadline, "got {out:?}, wanted {n}");
            t.link(l).unwrap().poll(&mut out, 16).unwrap();
        }
        out
    }

    /// FR-001: the accepted and the initiated QP are equally able to post, a
    /// send lands in a posted receive, and a one-sided write lands in a
    /// registered region.
    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn both_ends_send_and_write() {
        let (mut a, _) = listening();
        let (mut b, b_addr) = listening();
        let (la, lb) = connected(&mut a, &mut b, &b_addr);
        assert_eq!(a.link(la).unwrap().state().unwrap(), QpState::Rts);
        let mut abuf = vec![0u8; 4096];
        let mut bbuf = vec![0u8; 4096];
        // SAFETY: both buffers outlive their regions, dropped at the end.
        let ra = unsafe { a.register(abuf.as_mut_ptr(), 4096, Access::LOCAL_WRITE) }.unwrap();
        let rb = unsafe {
            b.register(
                bbuf.as_mut_ptr(),
                4096,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        // b posts a receive; a sends into it, then writes past it.
        // SAFETY: buffers outlive the completions polled below.
        unsafe {
            b.link(lb)
                .unwrap()
                .post_recv(&[RecvRequest {
                    wr_id: 7,
                    buf: rb.sge(rb.addr(), 64).unwrap(),
                }])
                .unwrap();
        }
        abuf[..5].copy_from_slice(b"hello");
        abuf[100..105].copy_from_slice(b"wrote");
        // SAFETY: as above.
        unsafe {
            a.link(la)
                .unwrap()
                .post_send(&[
                    SendRequest {
                        wr_id: 1,
                        op: SendOp::Send,
                        local: ra.sge(ra.addr(), 5).unwrap(),
                        signaled: true,
                    },
                    SendRequest {
                        wr_id: 2,
                        op: SendOp::RdmaWrite {
                            remote_addr: rb.addr() + 1000,
                            rkey: rb.rkey(),
                        },
                        local: ra.sge(ra.addr() + 100, 5).unwrap(),
                        signaled: true,
                    },
                ])
                .unwrap();
        }
        let ca = poll_until(&mut a, la, 2);
        assert_eq!(
            ca.iter().map(|c| (c.wr_id, c.result)).collect::<Vec<_>>(),
            [(1, Ok(Completed::Send)), (2, Ok(Completed::Write))]
        );
        let cb = poll_until(&mut b, lb, 1);
        assert_eq!(cb[0].wr_id, 7);
        assert_eq!(cb[0].result, Ok(Completed::Recv { byte_len: 5 }));
        assert_eq!(&bbuf[..5], b"hello");
        assert_eq!(&bbuf[1000..1005], b"wrote");
        // And the accepted end sends back.
        // SAFETY: as above.
        unsafe {
            a.link(la)
                .unwrap()
                .post_recv(&[RecvRequest {
                    wr_id: 9,
                    buf: ra.sge(ra.addr() + 2048, 64).unwrap(),
                }])
                .unwrap();
            bbuf[2000..2003].copy_from_slice(b"ack");
            b.link(lb)
                .unwrap()
                .post_send(&[SendRequest {
                    wr_id: 3,
                    op: SendOp::Send,
                    local: rb.sge(rb.addr() + 2000, 3).unwrap(),
                    signaled: true,
                }])
                .unwrap();
        }
        poll_until(&mut b, lb, 1);
        assert_eq!(
            poll_until(&mut a, la, 1)[0].result,
            Ok(Completed::Recv { byte_len: 3 })
        );
        assert_eq!(&abuf[2048..2051], b"ack");
        a.close(la);
        b.close(lb);
        drop((ra, rb));
    }

    /// FR-023 on hardware: after ERROR every queued receive flushes, and a
    /// write the peer then attempts is refused — it cannot land.
    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn error_flushes_and_refuses_late_writes() {
        let (mut a, _) = listening();
        let (mut b, b_addr) = listening();
        let (la, lb) = connected(&mut a, &mut b, &b_addr);
        let mut abuf = vec![1u8; 64];
        let mut bbuf = vec![0u8; 4096];
        // SAFETY: buffers outlive the regions.
        let ra = unsafe { a.register(abuf.as_mut_ptr(), 64, Access::LOCAL_WRITE) }.unwrap();
        let rb = unsafe {
            b.register(
                bbuf.as_mut_ptr(),
                4096,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        // SAFETY: as above.
        unsafe {
            b.link(lb)
                .unwrap()
                .post_recv(&[
                    RecvRequest {
                        wr_id: 10,
                        buf: rb.sge(rb.addr(), 64).unwrap(),
                    },
                    RecvRequest {
                        wr_id: 11,
                        buf: rb.sge(rb.addr() + 64, 64).unwrap(),
                    },
                ])
                .unwrap();
        }
        b.link(lb).unwrap().to_error().unwrap();
        assert_eq!(b.link(lb).unwrap().state().unwrap(), QpState::Err);
        let mut flushed: Vec<_> = poll_until(&mut b, lb, 2)
            .into_iter()
            .map(|c| (c.wr_id, c.result))
            .collect();
        flushed.sort_by_key(|c| c.0);
        assert_eq!(
            flushed,
            [
                (10, Err(CompletionError::Flushed)),
                (11, Err(CompletionError::Flushed))
            ]
        );
        // SAFETY: as above.
        unsafe {
            a.link(la)
                .unwrap()
                .post_send(&[SendRequest {
                    wr_id: 4,
                    op: SendOp::RdmaWrite {
                        remote_addr: rb.addr() + 3000,
                        rkey: rb.rkey(),
                    },
                    local: ra.sge(ra.addr(), 64).unwrap(),
                    signaled: true,
                }])
                .unwrap();
        }
        // The responder drops it silently, so the writer learns only when its
        // retries run out: RETRY_EXC_ERR after ~14 s at rdma_cm's default ACK
        // timeout (measured on mlx5, 2026-10-20). Slow to report, but it never
        // lands — the property FR-023 needs.
        let t0 = Instant::now();
        let mut c = Vec::new();
        while c.is_empty() {
            assert!(t0.elapsed() < Duration::from_secs(60), "no completion");
            a.link(la).unwrap().poll(&mut c, 16).unwrap();
        }
        assert!(
            matches!(c[0].result, Err(CompletionError::Failed { status, .. }) if status == crate::connection::transport::wc::RETRY_EXC_ERR),
            "the late write was refused: {c:?}"
        );
        assert!(bbuf[3000..3064].iter().all(|&x| x == 0), "nothing landed");
        a.close(la);
        b.close(lb);
        drop((ra, rb));
    }

    /// D23's loser learns why: a reject carries private data, and the
    /// initiator's QP is in ERROR.
    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn a_reject_carries_its_reason() {
        let (mut a, _) = listening();
        let (mut b, b_addr) = listening();
        let la = a.connect(&b_addr, b"id", CAPS).unwrap();
        let (_, eb) = pump(&mut a, &mut b, |_, eb| request_of(eb).is_some());
        b.reject(request_of(&eb).unwrap().0, b"busy").unwrap();
        let (ea, _) = pump(&mut a, &mut b, |ea, _| {
            ea.iter()
                .any(|e| matches!(e, TransportEvent::Rejected { .. }))
        });
        let data = ea
            .iter()
            .find_map(|e| match e {
                TransportEvent::Rejected { link, private_data } if *link == la => {
                    Some(private_data.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(&data[..4], b"busy");
        assert_eq!(a.link(la).unwrap().state().unwrap(), QpState::Err);
        a.close(la);
    }

    /// SC-010 / D17 on hardware: a region registered before a connection
    /// keeps its rkey, valid on a second connection after the first is
    /// closed — no re-registration.
    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn a_registration_outlives_its_connections() {
        let (mut a, _) = listening();
        let (mut b, b_addr) = listening();
        let mut abuf = vec![0u8; 64];
        let mut bbuf = vec![0u8; 4096];
        // SAFETY: buffers outlive the regions.
        let ra = unsafe { a.register(abuf.as_mut_ptr(), 64, Access::LOCAL_WRITE) }.unwrap();
        let rb = unsafe {
            b.register(
                bbuf.as_mut_ptr(),
                4096,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        let rkey = rb.rkey();
        for round in 0..2u8 {
            let (la, lb) = connected(&mut a, &mut b, &b_addr);
            abuf[0] = round + 1;
            // SAFETY: as above.
            unsafe {
                a.link(la)
                    .unwrap()
                    .post_send(&[SendRequest {
                        wr_id: 1,
                        op: SendOp::RdmaWrite {
                            remote_addr: rb.addr() + round as u64,
                            rkey,
                        },
                        local: ra.sge(ra.addr(), 1).unwrap(),
                        signaled: true,
                    }])
                    .unwrap();
            }
            assert_eq!(
                poll_until(&mut a, la, 1)[0].result,
                Ok(Completed::Write),
                "round {round}"
            );
            assert_eq!(bbuf[round as usize], round + 1);
            a.close(la);
            b.close(lb);
        }
        drop((ra, rb));
    }

    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn a_connect_to_nobody_fails_as_an_event() {
        let (mut a, _) = listening();
        let (mut b, _) = listening();
        let la = a.connect(&peer(port()), b"", CAPS).unwrap();
        let (ea, _) = pump(&mut a, &mut b, |ea, _| {
            ea.iter().any(|e| {
                matches!(e, TransportEvent::ConnectError { link, .. } | TransportEvent::Rejected { link, .. } if *link == la)
            })
        });
        assert!(!ea.is_empty());
        a.close(la);
    }

    /// `poll` reaps as many completions as are ready, up to `max`, in one call
    /// — the reaper's batch (`REAP_BATCH`) depends on it.
    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn one_poll_reaps_a_burst() {
        let (mut a, _) = listening();
        let (mut b, b_addr) = listening();
        let (la, lb) = connected(&mut a, &mut b, &b_addr);
        let mut abuf = vec![0u8; 64];
        let mut bbuf = vec![0u8; 4096];
        // SAFETY: buffers outlive the regions.
        let ra = unsafe { a.register(abuf.as_mut_ptr(), 64, Access::LOCAL_WRITE) }.unwrap();
        let rb = unsafe {
            b.register(
                bbuf.as_mut_ptr(),
                4096,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        let writes: Vec<_> = (0..8u64)
            .map(|i| SendRequest {
                wr_id: i,
                op: SendOp::RdmaWrite {
                    remote_addr: rb.addr() + i * 64,
                    rkey: rb.rkey(),
                },
                local: ra.sge(ra.addr(), 64).unwrap(),
                signaled: true,
            })
            .collect();
        // SAFETY: as above.
        unsafe { a.link(la).unwrap().post_send(&writes) }.unwrap();
        std::thread::sleep(Duration::from_millis(50)); // all eight complete
        let mut out = Vec::new();
        let n = a.link(la).unwrap().poll(&mut out, 16).unwrap();
        assert_eq!(n, 8, "one poll took them all: {out:?}");
        let n = a.link(la).unwrap().poll(&mut out, 16).unwrap();
        assert_eq!(n, 0);
        a.close(la);
        b.close(lb);
        drop((ra, rb));
    }

    #[test]
    fn hosts_must_be_ipv4_literals() {
        let p = PeerId::parse("node5:7000").unwrap();
        assert!(matches!(
            socket_addr(&p, "t"),
            Err(RdmaError::Invalid { .. })
        ));
        let p = PeerId::parse("10.0.0.105:7000").unwrap();
        assert_eq!(
            socket_addr(&p, "t").unwrap(),
            "10.0.0.105:7000".parse().unwrap()
        );
    }
}
