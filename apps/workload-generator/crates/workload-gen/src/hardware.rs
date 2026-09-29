//! Which Certus instances a run drives, and how fast (FR-081, FR-082).
//!
//! # A hostname is not an identity
//!
//! A machine routinely runs several Certus instances — one per NUMA domain, or one per NVMe
//! device as `deploy/multi-instance/` does, with mailboxes `/dev/shm/certus-shmq-0`, `-1`, … So
//! an instance is `(host, port, mailbox)`, and two entries may legitimately share a host.
//!
//! Before FR-081 a run addressed a *host*: one global `--shm-path` and one global
//! `--agent-port` applied to every `--node`, so `--node node5 --node node5` built two identical
//! specs. The second agent's startup then took the first for a leftover of a crashed run and
//! shut it down (FR-052), and the run drove one instance while reporting two. The deployment
//! was not awkward to express, it was unrepresentable.
//!
//! # "node" the concept, `--instance` the flag
//!
//! The model keeps the word *node*: Zyre already uses it for an instance regardless of machine,
//! and placement (FR-048), migration (FR-049) and loss (FR-064) are all written in those terms.
//! Only the flag is `--instance`, because on a command line the ambiguity with "machine" is
//! expensive — and it is the mistake that produced this module.
//!
//! # Two ways an instance list can be a lie
//!
//! Both are refused, and neither is noisy on its own:
//!
//! - **Two entries on one `(host, port)`** cannot both have an agent, so the second one's
//!   startup shuts the first down as a leftover. The run then drives one instance and reports
//!   two.
//! - **Two entries on one `(host, mailbox)`** are two agents on **one Certus instance**, so
//!   they are one cache wearing two names. The simulation would place sessions across what it
//!   believes are two independent caches, and a migration between them — which FR-048 intends
//!   as a genuine miss — would be served as a hit.
//!
//! The second is not in FR-081's wording, which names only the port. It is the same failure,
//! and it is quieter: nothing collides, nothing is shut down, and the run simply measures a
//! deployment other than the one described.
//!
//! # Examples
//!
//! ```
//! use workload_gen::hardware::{Instance, DEFAULT_MAILBOX, DEFAULT_PORT};
//!
//! // All four shapes, disambiguated by form rather than by position: a component that parses
//! // as a u16 is the port, one starting with `/` is the mailbox.
//! let bare: Instance = "node5".parse().unwrap();
//! assert_eq!((bare.port, bare.mailbox.as_str()), (DEFAULT_PORT, DEFAULT_MAILBOX));
//!
//! let ported: Instance = "node5:7001".parse().unwrap();
//! assert_eq!(ported.port, 7001);
//!
//! let boxed: Instance = "node5:/dev/shm/certus-shmq-1".parse().unwrap();
//! assert_eq!((boxed.port, boxed.mailbox.as_str()), (DEFAULT_PORT, "/dev/shm/certus-shmq-1"));
//!
//! let both: Instance = "node5:7001:/dev/shm/certus-shmq-1".parse().unwrap();
//! assert_eq!(both.label(), "node5:7001");
//! ```

use std::fmt;
use std::path::Path;

use serde::Deserialize;

/// Port an agent listens on when none was given.
pub const DEFAULT_PORT: u16 = 7420;

/// Mailbox an agent attaches to when none was given.
pub const DEFAULT_MAILBOX: &str = "/dev/shm/certus-shmq";

/// The hardware file read when `--hardware` was not given.
///
/// Relative, so it describes *this* checkout's cluster rather than a machine-wide one — the
/// deployment a run is aimed at belongs with the experiment, not with the host.
pub const DEFAULT_HARDWARE_FILE: &str = "cluster.yml";

/// The version this build understands.
pub const HARDWARE_VERSION: u32 = 1;

/// One Certus instance to drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// Host the agent runs on, as ssh and `connect` understand it.
    pub host: String,
    /// Port its agent listens on.
    pub port: u16,
    /// The Certus mailbox on that host, which its agent attaches to.
    pub mailbox: String,
}

impl Instance {
    /// This instance's identity in every message and every report row.
    ///
    /// `host:port`, which is unique within a run because duplicates are refused, and which is
    /// also what the client connects to. Two rows reading `node5` are not a report — the defect
    /// that produced FR-081 — and the mailbox is listed once in the report rather than repeated
    /// on every row.
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.host, self.port, self.mailbox)
    }
}

impl std::str::FromStr for Instance {
    type Err = String;

    /// Parse `host[:port][:mailbox]`.
    ///
    /// Disambiguated by the form of each component rather than by its position, so all four
    /// shapes are writable without an empty middle field: a component that parses as a `u16` is
    /// the port, one starting with `/` is the mailbox. Everything after the mailbox's leading
    /// `/` is taken verbatim, so a path containing a colon survives.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with('[') {
            return Err(format!(
                "{s:?} looks like a bracketed IPv6 address, which this flag does not accept. \
                 The agent binds IPv4, so an IPv6 host would present as \"no agent accepted a \
                 connection\" with an agent plainly running"
            ));
        }
        let (host, rest) = match s.split_once(':') {
            Some((h, r)) => (h, Some(r)),
            None => (s, None),
        };
        if host.is_empty() {
            return Err(format!(
                "{s:?} has no host. The form is host[:port][:mailbox], for example \
                 node5:7001:/dev/shm/certus-shmq-1"
            ));
        }
        let mut out = Self {
            host: host.to_string(),
            port: DEFAULT_PORT,
            mailbox: DEFAULT_MAILBOX.to_string(),
        };
        let Some(rest) = rest else {
            return Ok(out);
        };
        if rest.starts_with('/') {
            out.mailbox = rest.to_string();
            return Ok(out);
        }
        let (port, mailbox) = match rest.split_once(':') {
            Some((p, m)) => (p, Some(m)),
            None => (rest, None),
        };
        out.port = port.parse().map_err(|_| {
            format!(
                "{port:?} in {s:?} is neither a port nor a mailbox. A port is a number up to \
                 65535 and a mailbox starts with `/`"
            )
        })?;
        if let Some(mailbox) = mailbox {
            if !mailbox.starts_with('/') {
                return Err(format!(
                    "{mailbox:?} in {s:?} is not a mailbox path; it must start with `/`"
                ));
            }
            out.mailbox = mailbox.to_string();
        }
        Ok(out)
    }
}

/// Refuse an instance list that cannot mean what it says.
///
/// # Errors
///
/// Naming **both** entries, because the useful half of the message is which pair collided.
pub fn check_distinct(instances: &[Instance]) -> Result<(), String> {
    for (i, a) in instances.iter().enumerate() {
        for (j, b) in instances.iter().enumerate().skip(i + 1) {
            if a.host == b.host && a.port == b.port {
                return Err(format!(
                    "instances {} and {} are both {}:{}. Two agents cannot share a port: the \
                     second one's startup would find the first, take it for a leftover of a \
                     crashed run and shut it down (FR-052), so the run would drive one instance \
                     and report two. Give each instance its own port",
                    i + 1,
                    j + 1,
                    a.host,
                    a.port
                ));
            }
            if a.host == b.host && a.mailbox == b.mailbox {
                return Err(format!(
                    "instances {} ({}) and {} ({}) name the same mailbox {} on {}. That is one \
                     Certus instance with two agents, so it is one cache under two names: \
                     sessions would be placed across what the simulation believes are two \
                     independent caches, and a migration between them would be served as a hit \
                     where FR-048 intends a miss. Give each instance its own mailbox",
                    i + 1,
                    a.label(),
                    j + 1,
                    b.label(),
                    a.mailbox,
                    a.host
                ));
            }
        }
    }
    Ok(())
}

/// One `instances:` entry, as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceEntry {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_mailbox")]
    mailbox: String,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_mailbox() -> String {
    DEFAULT_MAILBOX.to_string()
}

/// The hardware file, as written.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HardwareDoc {
    version: u32,
    #[serde(default)]
    instances: Vec<InstanceEntry>,
    #[serde(default)]
    rate: Option<f64>,
}

/// What a hardware file said, with its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct Hardware {
    /// The instances it listed, empty if it listed none.
    pub instances: Vec<Instance>,
    /// The rate it asked for, if any. Infinite means work-conserving (FR-080).
    pub rate: Option<f64>,
    /// Where it was read from, for the report.
    pub path: String,
    /// Digest of its bytes, so a report says which file this was rather than only its name.
    pub digest: String,
    /// Whether it was found by default rather than named with `--hardware`.
    ///
    /// An implicitly picked-up file changes a run's meaning without appearing in the command
    /// line, so it is announced on the error stream as well as recorded (FR-063).
    pub implicit: bool,
}

/// Read a hardware file.
///
/// `explicit` is `--hardware`'s argument. With none, [`DEFAULT_HARDWARE_FILE`] is read if it
/// exists and `Ok(None)` is returned if it does not — a missing default is the ordinary case,
/// while a named file that is missing is a mistake worth reporting.
///
/// # Errors
///
/// If a named file is missing or unreadable, if the YAML does not parse, if `version` is not
/// [`HARDWARE_VERSION`], or if any field is unknown. An unknown field is refused rather than
/// ignored so a typo is not silently a default — the same reasoning as FR-006.
pub fn read(explicit: Option<&Path>) -> Result<Option<Hardware>, String> {
    let (path, implicit) = match explicit {
        Some(p) => (p.to_path_buf(), false),
        None => {
            let p = Path::new(DEFAULT_HARDWARE_FILE);
            if !p.exists() {
                return Ok(None);
            }
            (p.to_path_buf(), true)
        }
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read the hardware file {}: {e}", path.display()))?;
    let doc: HardwareDoc = serde_yaml::from_str(&text)
        .map_err(|e| format!("cannot parse the hardware file {}: {e}", path.display()))?;
    if doc.version != HARDWARE_VERSION {
        return Err(format!(
            "the hardware file {} declares version {}, and this build understands {}. Refused \
             rather than guessed: a field this build does not know would otherwise be read as \
             a default, and the run would drive a deployment other than the one described",
            path.display(),
            doc.version,
            HARDWARE_VERSION
        ));
    }
    let instances: Vec<Instance> = doc
        .instances
        .into_iter()
        .map(|e| Instance {
            host: e.host,
            port: e.port,
            mailbox: e.mailbox,
        })
        .collect();
    Ok(Some(Hardware {
        instances,
        rate: doc.rate,
        digest: digest(&text),
        path: path.display().to_string(),
        implicit,
    }))
}

/// Non-cryptographic digest of a hardware file, in the same shape as the description's.
fn digest(text: &str) -> String {
    let mut acc = 0x9e37_79b9_7f4a_7c15u64;
    for b in text.as_bytes() {
        acc = workload_model::keys::splitmix64(acc ^ u64::from(*b));
    }
    format!("{acc:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Instance {
        s.parse().expect(s)
    }

    #[test]
    fn all_four_shapes_parse_without_an_empty_field() {
        assert_eq!(
            parse("node5"),
            Instance {
                host: "node5".into(),
                port: DEFAULT_PORT,
                mailbox: DEFAULT_MAILBOX.into()
            }
        );
        assert_eq!(parse("node5:7001").port, 7001);
        assert_eq!(parse("node5:7001").mailbox, DEFAULT_MAILBOX);
        assert_eq!(parse("node5:/dev/shm/x").mailbox, "/dev/shm/x");
        assert_eq!(parse("node5:/dev/shm/x").port, DEFAULT_PORT);
        let both = parse("node5:7001:/dev/shm/x");
        assert_eq!((both.port, both.mailbox.as_str()), (7001, "/dev/shm/x"));
    }

    #[test]
    fn a_mailbox_containing_a_colon_survives() {
        // Everything after the leading `/` is verbatim, so splitting cannot eat a path.
        assert_eq!(parse("node5:7001:/dev/shm/a:b").mailbox, "/dev/shm/a:b");
    }

    #[test]
    fn a_component_that_is_neither_a_port_nor_a_mailbox_is_refused() {
        let e = "node5:seven".parse::<Instance>().expect_err("refused");
        assert!(e.contains("neither a port nor a mailbox"), "{e}");
        let e = "node5:7001:shm".parse::<Instance>().expect_err("refused");
        assert!(e.contains("must start with `/`"), "{e}");
        let e = "".parse::<Instance>().expect_err("refused");
        assert!(e.contains("no host"), "{e}");
    }

    #[test]
    fn a_bracketed_ipv6_host_is_refused_with_the_reason() {
        // Rather than failing as an unparseable port, which sends the reader after the wrong
        // problem. The agent binds IPv4, which is also why the local default is 127.0.0.1.
        let e = "[::1]:7420".parse::<Instance>().expect_err("refused");
        assert!(e.contains("IPv6"), "{e}");
    }

    #[test]
    fn two_instances_on_one_port_are_refused_naming_both() {
        let list = vec![
            parse("node5:7001"),
            parse("node2:7001"),
            parse("node5:7001"),
        ];
        let e = check_distinct(&list).expect_err("refused");
        assert!(e.contains("instances 1 and 3"), "{e}");
        assert!(e.contains("node5:7001"), "{e}");
    }

    #[test]
    fn two_instances_on_one_mailbox_are_refused_as_one_cache_twice() {
        // Quieter than the port collision: nothing collides and nothing is shut down, the run
        // just measures a deployment other than the one described.
        let list = vec![
            parse("node5:7001:/dev/shm/certus-shmq-0"),
            parse("node5:7002:/dev/shm/certus-shmq-0"),
        ];
        let e = check_distinct(&list).expect_err("refused");
        assert!(e.contains("same mailbox"), "{e}");
        assert!(e.contains("one cache under two names"), "{e}");
    }

    #[test]
    fn one_host_with_two_instances_is_the_whole_point_and_is_accepted() {
        let list = vec![
            parse("node5:7001:/dev/shm/certus-shmq-0"),
            parse("node5:7002:/dev/shm/certus-shmq-1"),
        ];
        check_distinct(&list).expect("two instances on one host are the point of FR-081");
    }

    #[test]
    fn labels_distinguish_instances_that_share_a_host() {
        let a = parse("node5:7001:/dev/shm/certus-shmq-0");
        let b = parse("node5:7002:/dev/shm/certus-shmq-1");
        assert_ne!(a.label(), b.label(), "two rows must not read identically");
        assert_eq!(a.label(), "node5:7001");
    }
}
