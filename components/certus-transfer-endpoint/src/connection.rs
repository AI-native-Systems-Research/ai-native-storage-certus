//! Peer identity, the symmetric connection table, credits, the `wr_id` slab and
//! the transport seam.
//!
//! A connection is symmetric: a queue pair may be inbound-accepted or
//! outbound-initiated, and **either** end may post work requests (FR-001,
//! research D5). That is the thing neither predecessor crate can do — the
//! responder accepts but never posts, the initiator connects but never accepts.
//!
//! Scaffolding beyond [`PeerId`]: the transport seam lands in T010, the mock in
//! T011, the table in T012, the `wr_id` slab in T013 and send-queue credits in
//! T014.

use std::fmt;

// Scaffolding note: the items below are exercised only by this module's unit
// tests until T012 builds the connection table on them and T006 maps
// `PeerIdError` onto `TransferEndpointError::InvalidEndpoint`. The `allow`s are
// deliberately per-item rather than module-wide, so code added here later is
// still checked — remove them in T012.

/// Why an instance address could not be parsed.
///
/// T006 maps this onto `TransferEndpointError::InvalidEndpoint`; it is separate
/// so peer parsing does not wait on the interface definition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PeerIdError {
    /// No `':'` separator, so no port was supplied. Never defaulted (FR-021).
    MissingPort(String),
    /// The text after the final `':'` is not a `u16`.
    MalformedPort(String),
    /// Port zero is not an address.
    ZeroPort(String),
    /// The host part is empty.
    EmptyHost(String),
    /// An unbracketed IPv6 literal: `"::1:4791"` cannot be split into host and
    /// port without guessing. Write `"[::1]:4791"`.
    AmbiguousIpv6(String),
}

impl fmt::Display for PeerIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPort(s) => write!(f, "expected \"host:port\", got {s:?} (no port)"),
            Self::MalformedPort(s) => write!(f, "port is not a number in {s:?}"),
            Self::ZeroPort(s) => write!(f, "port 0 is not an address: {s:?}"),
            Self::EmptyHost(s) => write!(f, "empty host in {s:?}"),
            Self::AmbiguousIpv6(s) => {
                write!(f, "unbracketed IPv6 in {s:?}; write \"[addr]:port\"")
            }
        }
    }
}

impl std::error::Error for PeerIdError {}

/// A peer **instance** address: host plus port.
///
/// Identity is per instance, not per host (FR-021, research D16). Several certus
/// instances run per node — one per NUMA domain in the current rigs — so a host
/// alone is not an address: keyed by host, two NUMA-pinned instances on one node
/// would collapse into a single connection and one of them would be
/// unreachable. Two instances may legitimately share an IP and differ only by
/// port, which is also the single-host loopback verification path.
///
/// A missing port is an error, never a default. The grammar matches
/// `remote-lookup-rdma-initiator`'s `parse_endpoint` rather than inventing a
/// second one, with unbracketed IPv6 additionally rejected as ambiguous.
///
/// `"10.0.0.102:4791"` and `"[::1]:4791"` parse; `"10.0.0.102"` does not. The
/// [`Display`](fmt::Display) form is the normalized string and is what the
/// connection table keys on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(dead_code)]
pub struct PeerId {
    host: String,
    port: u16,
}

#[allow(dead_code)]
impl PeerId {
    /// Parse and normalize a `"host:port"` instance address.
    ///
    /// Accepts `"10.0.0.102:4791"` and `"[::1]:4791"`. Rejects a missing port,
    /// a non-numeric port, port 0, an empty host, and an unbracketed IPv6
    /// literal.
    pub fn parse(s: &str) -> Result<Self, PeerIdError> {
        let text = s.trim();
        let (host, port) = text
            .rsplit_once(':')
            .ok_or_else(|| PeerIdError::MissingPort(s.to_string()))?;

        if host.is_empty() {
            return Err(PeerIdError::EmptyHost(s.to_string()));
        }
        // A bracketed literal is unambiguous; a bare one is not, because the
        // address itself contains the separator we just split on.
        if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
            return Err(PeerIdError::AmbiguousIpv6(s.to_string()));
        }

        let port: u16 = port
            .parse()
            .map_err(|_| PeerIdError::MalformedPort(s.to_string()))?;
        if port == 0 {
            return Err(PeerIdError::ZeroPort(s.to_string()));
        }

        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    /// The host part, without brackets stripped.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port part, always non-zero.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for PeerId {
    /// The normalized form, which is also the connection-table key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parses_host_and_port() {
        let p = PeerId::parse("10.0.0.102:4791").expect("should parse");
        assert_eq!(p.host(), "10.0.0.102");
        assert_eq!(p.port(), 4791);
        assert_eq!(p.to_string(), "10.0.0.102:4791");
    }

    /// FR-021: a host alone is not an address, and must not acquire a default
    /// port. Keyed by host, two instances on one node would collapse.
    #[test]
    fn host_without_port_is_rejected_not_defaulted() {
        assert_eq!(
            PeerId::parse("10.0.0.102"),
            Err(PeerIdError::MissingPort("10.0.0.102".into()))
        );
    }

    /// SC-009: two instances differing only by port are distinct peers and get
    /// distinct connection-table keys. This is the NUMA-sibling case.
    #[test]
    fn same_host_different_ports_are_distinct_keys() {
        let a = PeerId::parse("10.0.0.102:4791").unwrap();
        let b = PeerId::parse("10.0.0.102:4792").unwrap();
        assert_ne!(a, b);

        let mut table: HashMap<PeerId, &str> = HashMap::new();
        table.insert(a.clone(), "numa0");
        table.insert(b.clone(), "numa1");
        assert_eq!(table.len(), 2, "sibling instances must not share an entry");
        assert_eq!(table[&a], "numa0");
        assert_eq!(table[&b], "numa1");
    }

    /// Both ends may be the local host — the loopback verification path.
    #[test]
    fn loopback_peers_are_addressable() {
        let a = PeerId::parse("127.0.0.1:4791").unwrap();
        let b = PeerId::parse("127.0.0.1:4792").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.host(), b.host());
    }

    #[test]
    fn bracketed_ipv6_parses_and_bare_ipv6_is_rejected() {
        let p = PeerId::parse("[::1]:4791").expect("bracketed v6 should parse");
        assert_eq!(p.host(), "[::1]");
        assert_eq!(p.port(), 4791);

        assert_eq!(
            PeerId::parse("::1:4791"),
            Err(PeerIdError::AmbiguousIpv6("::1:4791".into())),
            "a bare v6 literal contains the separator, so it must not be guessed"
        );
    }

    #[test]
    fn malformed_zero_and_empty_are_rejected() {
        assert_eq!(
            PeerId::parse("10.0.0.102:http"),
            Err(PeerIdError::MalformedPort("10.0.0.102:http".into()))
        );
        assert_eq!(
            PeerId::parse("10.0.0.102:0"),
            Err(PeerIdError::ZeroPort("10.0.0.102:0".into()))
        );
        assert_eq!(
            PeerId::parse(":4791"),
            Err(PeerIdError::EmptyHost(":4791".into()))
        );
    }

    /// The normalized form round-trips, so it is stable as a key.
    #[test]
    fn normalized_form_round_trips() {
        for s in ["10.0.0.102:4791", "[::1]:4791", "host.example:1"] {
            let p = PeerId::parse(s).unwrap();
            assert_eq!(PeerId::parse(&p.to_string()).unwrap(), p);
        }
        // Surrounding whitespace normalizes away.
        assert_eq!(
            PeerId::parse("  10.0.0.102:4791  ").unwrap().to_string(),
            "10.0.0.102:4791"
        );
    }
}
