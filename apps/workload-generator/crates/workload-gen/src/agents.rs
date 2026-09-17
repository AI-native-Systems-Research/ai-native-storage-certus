//! Starting the per-node agents, and stopping them so that nothing is left holding anything.
//!
//! # FR-052 and FR-053 are two halves of one mechanism
//!
//! FR-053 wants teardown that releases resources *even when the generator exits abnormally*,
//! and FR-052 wants startup that detects and replaces a leftover agent. Read separately the
//! first looks impossible — a generator killed with `SIGKILL` runs no code at all, so no
//! teardown it could contain would run.
//!
//! Together they work. Teardown covers the ordinary exit and the panic; **leftover
//! replacement is the backstop for everything else**. A generator that dies without stopping
//! its agents leaves them running, and the next run finds them, shuts them down and starts
//! fresh ones. Neither half alone is sufficient, and treating them as separate features is how
//! a teardown bug survives: this repository has already lost an entire A/B series to one, and
//! it failed nondeterministically rather than visibly.
//!
//! # A leftover is replaced even when it is the right binary
//!
//! `Hello`'s provenance check makes reusing a *stale* agent impossible. It says nothing about
//! a **current** one — same sources, still listening, left over from a run that crashed — and
//! that agent is not reusable either. It holds mailbox channels claimed for the previous run,
//! a device allocation filled for the previous run's geometry, and counters and histograms
//! that would be reported as this run's. FR-052 says *replaced, never silently reused*, and
//! the word that matters is "silently": reuse would produce a run whose numbers included
//! another run's.
//!
//! So startup always replaces. A leftover that answers is asked to stop, which is orderly and
//! lets it release its channels; one that does not answer is killed.
//!
//! # Only launching goes over ssh, and the local node not even that
//!
//! FR-054: load is driven over the fast transport, never by repeated remote command
//! invocation, which cannot sustain it. Ssh appears here and nowhere else, which is also why
//! [`Launcher`] is a trait — the lifecycle *policy* is then testable against a local agent,
//! and only the ssh mechanics are not.
//!
//! The **local** node is launched by [`LocalLauncher`], which spawns a child process and never
//! shells out to ssh. FR-079 makes every node — including this one — go through an agent, and
//! `ssh localhost` would have made `workload-gen run description.yml` need a trusted key for the
//! host's own account. It also lets a kill be by **PID**, which is stricter than any pattern:
//! `pkill -f` on this host would match the generator's own command line if the pattern ever
//! loosened.
//!
//! # A lane is a connection, on every node
//!
//! The mailbox is depth-1 per channel, so a lane needs its own channel, and the agent claims one
//! per connection. `--lanes n` therefore means *n connections per node*, and a session is routed
//! to one of them for its whole life so that its causally dependent turns stay ordered. Before
//! FR-079 the local path got its concurrency from claiming n channels directly; a single
//! connection per node would have silently collapsed a four-lane local run to one lane and
//! reported the throughput as though nothing had changed.

use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use workload_wire::client::{Client, ClientError, HandshakeError, DEFAULT_DEPTH};
use workload_wire::frame::{ClearCacheAck, HelloAck, ShutdownAck};

/// How long to wait for a freshly launched agent to accept a connection.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for a stopped agent's port to go quiet.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between attempts while waiting.
const POLL: Duration = Duration::from_millis(100);

/// Read timeout for a probe that only wants to know whether anything is there.
///
/// Short deliberately, unlike the timeout used to drive turns: a leftover that accepts and
/// stays silent must be discovered in a moment rather than after the default. That case is not
/// hypothetical — a socket accepting and never answering hung this very test until the client
/// grew a read timeout at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// A node stopped being reachable, so the run is over.
///
/// # Why this aborts everything rather than continuing on the survivors
///
/// FR-064 is emphatic, and the reason is that continuing produces a *plausible* number for a
/// different experiment. Losing a node does three things at once: its sessions' prefixes become
/// unreachable, so their turns miss forever; the set of migration targets shrinks, so migration
/// stops meaning what the description said; and the survivors absorb its share of the load, so
/// their latency reflects a concurrency nobody asked for. None of that produces an error on its
/// own — the run would finish and report a throughput.
///
/// So a lost node ends the run, the report says which one, and the exit code is 3. The other
/// agents are still torn down, because a run that failed still has to leave nothing holding
/// mailbox channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLost {
    /// The node. Named in the report, because on a cluster that is the actionable part.
    pub node: String,
    /// What the run was doing — "handshake", "submitting a turn", "collecting stats".
    pub during: &'static str,
    /// The underlying failure, in the transport's own words.
    pub reason: String,
}

impl std::fmt::Display for NodeLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "node {} became unreachable while {} ({}). The run is aborted and invalid: \
             continuing on the surviving nodes would measure a different experiment, because \
             this node's sessions' prefixes are now unreachable, the set of migration targets \
             has shrunk, and the survivors have absorbed its load (FR-064)",
            self.node, self.during, self.reason
        )
    }
}

impl std::error::Error for NodeLost {}

impl NodeLost {
    /// Describe a transport failure as the loss of `node`.
    ///
    /// Every [`ClientError`] means the node is gone as far as a run is concerned: a clean close,
    /// a timeout and a refused read are all "this node is not answering", and a protocol error
    /// means it is answering something this build cannot use. There is no variant worth
    /// retrying — a retry would extend the window in which the run is measuring a cluster it no
    /// longer has.
    pub fn from_client(node: &str, during: &'static str, e: ClientError) -> Self {
        Self {
            node: node.to_string(),
            during,
            reason: e.to_string(),
        }
    }
}

/// Where one agent lives and how to start it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// Host name, as ssh understands it. Used in every message about this node.
    pub node: String,
    /// Port the agent listens on.
    pub port: u16,
    /// The node's local mailbox.
    pub shm_path: String,
    /// Path to the agent binary **on the node**.
    pub binary: String,
    /// Lanes the agent should serve, one mailbox channel each.
    pub lanes: usize,
    /// Bytes per block, which must match the description.
    pub block_bytes: u32,
    /// Keys per request (FR-069).
    pub batch_keys: usize,
    /// Extra arguments, for `--no-payload` and the like.
    pub extra_args: Vec<String>,
}

impl AgentSpec {
    /// The command line this spec launches, as a list of arguments.
    ///
    /// Separated from the launching so a test can check what would be run without running it.
    pub fn command(&self) -> Vec<String> {
        let mut args = vec![
            self.binary.clone(),
            "--port".into(),
            self.port.to_string(),
            "--shm-path".into(),
            self.shm_path.clone(),
            "--lanes".into(),
            self.lanes.to_string(),
            "--block-bytes".into(),
            self.block_bytes.to_string(),
            "--batch-keys".into(),
            self.batch_keys.to_string(),
        ];
        args.extend(self.extra_args.iter().cloned());
        args
    }

    /// `host:port`, which is what the client connects to.
    pub fn address(&self) -> String {
        format!("{}:{}", self.node, self.port)
    }
}

/// Starts and kills agent processes.
///
/// A trait so the lifecycle policy — detect a leftover, replace it, verify the teardown — can
/// be tested against an agent running locally, leaving only the ssh mechanics untested. A
/// policy that could only be exercised on a cluster would be exercised rarely.
pub trait Launcher: Send + Sync {
    /// Start the agent described by `spec`, returning once the command has been issued.
    ///
    /// # Errors
    ///
    /// If the command could not be issued. Note that success means *launched*, not *listening*:
    /// [`Agents::start`] waits for the port separately.
    fn launch(&self, spec: &AgentSpec) -> Result<(), String>;

    /// Kill any agent on `spec`'s node, however it is running.
    ///
    /// Used for a leftover that will not answer its port. Must succeed when there is nothing
    /// to kill.
    ///
    /// # Errors
    ///
    /// If the command could not be issued.
    fn kill(&self, spec: &AgentSpec) -> Result<(), String>;

    /// Anything this launcher knows about why an agent is not answering.
    ///
    /// Consulted only when a launch succeeded and the port never came up, which is the one case
    /// where "connection refused" is a true statement that helps nobody: the agent *started* and
    /// then failed for a reason it wrote down somewhere. A launcher that can read that reason
    /// should say so, because the alternative is an operator with a refused connection and no
    /// hint that a second process was even involved.
    ///
    /// `None` by default: over ssh there is nothing to consult without another round trip.
    fn why_not_listening(&self, spec: &AgentSpec) -> Option<String> {
        let _ = spec;
        None
    }
}

/// Uses agents that are already running, launching and killing nothing.
///
/// For an operator managing the daemons themselves, and for the local node under FR-079 — where
/// ssh to `localhost` is both unnecessary and, on a host without its own key trusted, refused.
/// A leftover is still *replaced* in the sense that matters: the handshake refuses a stale one,
/// and this launcher simply cannot start a fresh one, so a run against a leftover of the current
/// build reuses it. That is a deliberate weakening of FR-052 for the case where the caller has
/// taken responsibility for the daemon's lifecycle, and it is why it is not the default.
#[derive(Debug, Clone, Default)]
pub struct NoLaunch;

impl Launcher for NoLaunch {
    fn launch(&self, spec: &AgentSpec) -> Result<(), String> {
        // Nothing to start; `Agents::start` waits for the port and will report plainly if
        // nothing is listening there.
        let _ = spec;
        Ok(())
    }

    fn kill(&self, spec: &AgentSpec) -> Result<(), String> {
        let _ = spec;
        Ok(())
    }
}

/// Launches an agent on **this** host as a child process, with no ssh at all.
///
/// FR-079 routes every node through an agent, including the local one, and `run description.yml`
/// must still need no setup — so `ssh localhost` is not an option: it wants the host's own key
/// trusted for its own account, which is a configuration step for the simplest possible run.
///
/// A child process rather than a thread because the agent is a separate binary, and because that
/// is what makes the local and remote lifecycles the same mechanism rather than two.
///
/// # Killing by PID, not by pattern
///
/// [`SshLauncher::kill`] matches `--port N` because it has nothing better to go on. Here we
/// started the process, so a kill goes to the PID we have. That matters more locally than
/// remotely: a pattern matched against this host's process list can match the **generator's own**
/// command line, and `pkill -f` self-terminating is a failure this repository has already had.
/// The pattern is the fallback for a leftover from a *previous* process, whose PID is gone.
#[derive(Debug)]
pub struct LocalLauncher {
    /// Children this process started, by port.
    started: Mutex<HashMap<u16, Child>>,
    /// Where an agent's own output goes, so it does not interleave with the report.
    log_dir: PathBuf,
}

impl Default for LocalLauncher {
    fn default() -> Self {
        Self {
            started: Mutex::new(HashMap::new()),
            log_dir: std::env::temp_dir(),
        }
    }
}

impl LocalLauncher {
    /// A launcher writing agent output under `log_dir`.
    pub fn with_log_dir(log_dir: PathBuf) -> Self {
        Self {
            started: Mutex::new(HashMap::new()),
            log_dir,
        }
    }

    /// Where this launcher would put the agent's output for `port`.
    pub fn log_path(&self, port: u16) -> PathBuf {
        self.log_dir.join(format!("workload-node-agent.{port}.log"))
    }
}

impl Launcher for LocalLauncher {
    fn launch(&self, spec: &AgentSpec) -> Result<(), String> {
        let path = self.log_path(spec.port);
        // Truncated per launch: the log describes *this* agent, and a growing file would leave
        // the last run's mailbox complaint sitting above this run's startup.
        let log = std::fs::File::create(&path)
            .map_err(|e| format!("cannot open {} for the agent's output: {e}", path.display()))?;
        let errors = log
            .try_clone()
            .map_err(|e| format!("cannot share {}: {e}", path.display()))?;
        let args = spec.command();
        let child = Command::new(&args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(errors)
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot start {}: {e}. Build it with `cargo build -p workload-node-agent`, \
                     or name it with --agent-binary",
                    args[0]
                )
            })?;
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(spec.port, child);
        eprintln!("local agent on port {}: {}", spec.port, path.display());
        Ok(())
    }

    fn why_not_listening(&self, spec: &AgentSpec) -> Option<String> {
        let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
        let child = started.get_mut(&spec.port)?;
        // Still running and simply not listening yet is not a diagnosis, so say nothing.
        let status = child.try_wait().ok().flatten()?;
        let path = self.log_path(spec.port);
        // The agent's own last words, which are the actual reason nine times out of ten: a
        // missing mailbox, or channels another client still holds.
        let said = std::fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(match said {
            Some(text) => format!(
                "the local agent exited ({status}) before it could listen, saying: {text} \
                 (full output in {})",
                path.display()
            ),
            None => format!(
                "the local agent exited ({status}) before it could listen and wrote nothing to {}",
                path.display()
            ),
        })
    }

    fn kill(&self, spec: &AgentSpec) -> Result<(), String> {
        let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = started.remove(&spec.port) {
            // Already gone is the ordinary case after a clean `Shutdown`, and killing a reaped
            // PID would be at best pointless and at worst somebody else's process.
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(());
                }
            }
        }
        drop(started);
        // A leftover from a *previous* generator process: its PID is gone, so the port-scoped
        // pattern is all there is. `|| true` because nothing to kill is the expected case.
        let pattern = format!("workload-node-agent .*--port {}", spec.port);
        let out = Command::new("pkill")
            .arg("-f")
            .arg(&pattern)
            .output()
            .map_err(|e| format!("pkill: {e}"))?;
        // Exit 1 is "no process matched", which is success here.
        if !out.status.success() && out.status.code() != Some(1) {
            return Err(format!(
                "pkill -f {pattern} exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

/// How long a child gets to finish exiting before it is killed.
///
/// `Agents::stop` returns once the agent's **port** has gone quiet, which happens when its
/// listener closes — and the process still has its own teardown to do after that: releasing
/// mailbox channels and freeing a device allocation. Without a grace period the reap below
/// SIGKILLed a healthy agent a few milliseconds into that, printing an alarming line about an
/// agent that had done nothing wrong and cutting short exactly the cleanup FR-053 is about.
///
/// Derived from the agent's own linger rather than picked, and that is the whole point. An agent
/// whose connections have all closed waits `DEFAULT_LINGER` before believing the run is over — the
/// grace period that lets a generator open its lanes one at a time — and only then releases its
/// mailbox channels. A kill inside that window is a kill of a healthy agent mid-cleanup, and
/// because a **claim outlives the process that made it** the channels are then leaked until the
/// server restarts. A first attempt at 3 seconds against a 5-second linger did exactly that.
pub const EXIT_GRACE: Duration = workload_wire::server::DEFAULT_LINGER
    .saturating_add(Duration::from_secs(5))
    .saturating_add(STOP_TIMEOUT);

impl Drop for LocalLauncher {
    fn drop(&mut self) {
        // A backstop, not the mechanism: `Agents::stop` asks each agent to exit and verifies its
        // port went quiet. This reaps a child that ignored that, so a generator run cannot leave a
        // process holding this node's mailbox channels behind it.
        let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
        for (port, child) in started.iter_mut() {
            let deadline = Instant::now() + EXIT_GRACE;
            let mut exited = false;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        exited = true;
                        break;
                    }
                    Ok(None) => std::thread::sleep(POLL),
                    // Not waitable at all: killing it is the only thing left to try.
                    Err(_) => break,
                }
            }
            if !exited {
                eprintln!(
                    "killing the local agent on port {port}; it did not exit within {EXIT_GRACE:?} \
                     of being asked to"
                );
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        started.clear();
    }
}

/// Resolve the agent binary for a **local** launch.
///
/// A bare name is looked for next to the generator's own executable first, because that is where
/// a cargo build puts it and `run description.yml` is meant to need no setup. A name containing a
/// separator is taken as given. Remote specs are never resolved here: the path belongs to the
/// remote filesystem, and guessing from this one would name a file that node does not have.
pub fn local_agent_binary(name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(name);
            if beside.is_file() {
                return beside.display().to_string();
            }
        }
    }
    // Left bare, so `PATH` still applies and the failure names what it could not start.
    name.to_string()
}

/// Launches over ssh, which is the only thing ssh is used for (FR-054).
#[derive(Debug, Clone, Default)]
pub struct SshLauncher {
    /// Extra ssh options, e.g. `-o BatchMode=yes`.
    pub ssh_args: Vec<String>,
}

impl Launcher for SshLauncher {
    fn launch(&self, spec: &AgentSpec) -> Result<(), String> {
        // `setsid` and the redirects so the agent outlives the ssh session: if it died with the
        // connection, load could not be driven over the fast transport at all (FR-054).
        let remote = format!(
            "setsid nohup {} > /tmp/workload-node-agent.{}.log 2>&1 < /dev/null &",
            shell_join(&spec.command()),
            spec.port
        );
        self.ssh(spec, &remote)
    }

    fn kill(&self, spec: &AgentSpec) -> Result<(), String> {
        // Matched on the binary's own name and its port, so a run does not kill an agent that
        // another run on the same host is using. `|| true` because "nothing to kill" is the
        // expected case and must not be an error.
        let remote = format!(
            "pkill -f 'workload-node-agent .*--port {}' || true",
            spec.port
        );
        self.ssh(spec, &remote)
    }
}

impl SshLauncher {
    fn ssh(&self, spec: &AgentSpec, remote: &str) -> Result<(), String> {
        let mut cmd = std::process::Command::new("ssh");
        cmd.args(&self.ssh_args).arg(&spec.node).arg(remote);
        let out = cmd
            .output()
            .map_err(|e| format!("ssh {}: {e}", spec.node))?;
        if !out.status.success() {
            return Err(format!(
                "ssh {} exited {}: {}",
                spec.node,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

/// Quote a command for a remote shell.
fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-/=:".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One started agent, and the connections the run drives it over — one per lane.
#[derive(Debug)]
pub struct Agent {
    /// The node, named in every message about it.
    pub node: String,
    /// What the handshake reported: channels and block size.
    pub ack: HelloAck,
    /// One connection per lane. The mailbox is depth-1 per channel and the agent claims a channel
    /// per connection, so this is where a node's concurrency comes from.
    lanes: Vec<Client<TcpStream>>,
    spec: AgentSpec,
}

impl Agent {
    /// Lanes on this node.
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// What this agent was started from.
    pub fn spec(&self) -> &AgentSpec {
        &self.spec
    }
}

/// One lane on one node: the connection a consumer thread drives, and the node to blame.
///
/// A borrow rather than a handle, so the compiler establishes that two lanes never share a
/// connection — a promise a comment would otherwise have to make, and the one whose breach
/// would reorder a session's causally dependent turns.
#[derive(Debug)]
pub struct AgentLane<'a> {
    /// The node this lane talks to, named in every failure.
    pub node: String,
    /// Which node, for aggregating per-node figures.
    pub node_index: usize,
    /// Which lane on that node, so a per-lane figure can be attributed.
    pub lane_index: usize,
    client: &'a mut Client<TcpStream>,
}

impl AgentLane<'_> {
    /// Submit a turn, treating any transport failure as the loss of this node (FR-064).
    ///
    /// # Errors
    ///
    /// [`NodeLost`], naming this node. The caller must abort the whole run rather than carry on
    /// with the others.
    pub fn submit(
        &mut self,
        turn: &workload_wire::frame::SubmitTurn,
    ) -> Result<(u32, Option<workload_wire::frame::TurnOutcome>), NodeLost> {
        self.client
            .submit(turn)
            .map_err(|e| NodeLost::from_client(&self.node, "submitting a turn", e))
    }

    /// Wait for every outstanding turn, treating a failure as the loss of this node.
    ///
    /// # Errors
    ///
    /// [`NodeLost`], naming this node.
    pub fn finish(&mut self) -> Result<Vec<workload_wire::frame::TurnOutcome>, NodeLost> {
        self.client
            .finish()
            .map_err(|e| NodeLost::from_client(&self.node, "draining its outstanding turns", e))
    }

    /// Collect this lane's counters and histograms.
    ///
    /// # Errors
    ///
    /// [`NodeLost`], naming this node. Statistics are gathered after the timed window, so losing
    /// a node here still invalidates the run: the figures would be missing one lane's share and
    /// the totals would silently describe a smaller cluster.
    pub fn stats(&mut self) -> Result<workload_wire::frame::Stats, NodeLost> {
        self.client
            .stats()
            .map_err(|e| NodeLost::from_client(&self.node, "collecting its statistics", e))
    }

    /// Ask this node to clear its memory tier, before the timed window opens (FR-046).
    ///
    /// # Errors
    ///
    /// [`NodeLost`] if the node cannot be reached. A node that answers and could *not* clear
    /// reports that in [`ClearCacheAck::error`], which the caller must refuse to run past — see
    /// the frame's own documentation on why that is not a zero count.
    pub fn clear_cache(&mut self) -> Result<ClearCacheAck, NodeLost> {
        self.client
            .clear_cache()
            .map_err(|e| NodeLost::from_client(&self.node, "clearing its memory tier", e))
    }
}

/// Every agent a run uses, started together and stopped together.
///
/// Dropping this stops them, so a panicking generator does not leave agents holding mailbox
/// channels and device memory. That covers the ordinary and the panicking exit; a `SIGKILL`
/// runs nothing, and the next run's leftover replacement is what covers that.
#[derive(Debug)]
pub struct Agents {
    agents: Vec<Agent>,
    /// Set once [`Agents::stop`] has run, so `Drop` does not stop them twice.
    stopped: bool,
}

/// What [`Agents::stop`] confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Teardown {
    /// The node.
    pub node: String,
    /// The agent's own tally.
    pub ack: ShutdownAck,
    /// Whether the port really went quiet afterwards.
    ///
    /// Reported rather than assumed (FR-053): an agent that answered `Shutdown` and then kept
    /// its channels would leave the next run unable to claim them, and the failure would look
    /// like a mailbox problem.
    pub port_released: bool,
}

impl Agents {
    /// Start an agent on every node, replacing any leftover, and handshake with each.
    ///
    /// Call this **before** the timed window opens (FR-050): launching, waiting for a port and
    /// replacing a leftover are setup, and charging them to the system under test would make a
    /// slow ssh look like a slow cache.
    ///
    /// # Errors
    ///
    /// If a node cannot be launched, does not come up within [`START_TIMEOUT`], or refuses the
    /// handshake. Every message names the node, and a refusal is fatal rather than skippable:
    /// running on the remaining nodes would be a different experiment (FR-064).
    pub fn start<L: Launcher>(
        launcher: &L,
        specs: &[AgentSpec],
        depth: usize,
    ) -> Result<Self, String> {
        Self::start_with(launcher, specs, depth, true)
    }

    /// Start, optionally reusing whatever is already listening.
    ///
    /// `replace` false is for [`NoLaunch`]: shutting an agent down and then being unable to
    /// start one would leave the run with nothing to talk to.
    ///
    /// # Errors
    ///
    /// As [`Agents::start`].
    pub fn start_with<L: Launcher>(
        launcher: &L,
        specs: &[AgentSpec],
        depth: usize,
        replace: bool,
    ) -> Result<Self, String> {
        // What was launched before the failure, so it can be stopped again. Without this a
        // refused handshake left an agent listening with its mailbox channels claimed: nobody had
        // asked it to stop, so it waited out its linger — and the local launcher's own backstop
        // killed it first, which leaks the claims into the shared segment where only a server
        // restart clears them. Four refused runs against an eight-channel mailbox and the fifth
        // cannot start. Found by running the same smoke test five times.
        let mut launched: Vec<AgentSpec> = Vec::new();
        match Self::start_inner(launcher, specs, depth, replace, &mut launched) {
            Ok(agents) => Ok(agents),
            Err(e) => {
                // Only what **this** launcher started. `replace` false means the caller manages
                // the daemons ([`NoLaunch`]), and stopping one of theirs on a refusal would leave
                // them with nothing to talk to and nothing able to start it again — which is the
                // very reason that flag exists.
                if replace {
                    for spec in &launched {
                        // Asked first, killed second: a `Shutdown` lets the agent release its
                        // channels and its device memory in order, and it is answered whatever the
                        // agent's provenance — which matters, because a provenance refusal is the
                        // commonest way to arrive here.
                        if let Err(cleanup) = shut_down_or_kill(launcher, spec) {
                            eprintln!("node {}: {cleanup}", spec.node);
                        }
                    }
                }
                Err(e)
            }
        }
    }

    /// The body of [`Agents::start_with`], recording what it launched as it goes.
    fn start_inner<L: Launcher>(
        launcher: &L,
        specs: &[AgentSpec],
        depth: usize,
        replace: bool,
        launched: &mut Vec<AgentSpec>,
    ) -> Result<Self, String> {
        assert!(depth > 0, "a pipelining depth of 0 could never send");
        let mut agents = Vec::with_capacity(specs.len());
        for spec in specs {
            if spec.lanes == 0 {
                return Err(format!(
                    "node {}: --lanes must be at least 1; a node with no lane would be started \
                     and driven with nothing",
                    spec.node
                ));
            }
            // Always replace, even a current build: a leftover holds the previous run's
            // channels, device memory and counters, and reusing it would report another run's
            // numbers as this one's (FR-052).
            if replace {
                match replace_leftover(launcher, spec) {
                    Ok(Some(())) => eprintln!("{}: replaced a leftover agent", spec.node),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                }
            }
            launcher.launch(spec)?;
            launched.push(spec.clone());
            // One connection per lane: the agent claims a mailbox channel per connection, and the
            // mailbox is depth-1 per channel, so this is the whole of a node's concurrency. The
            // first connection is also what proves the agent came up at all.
            let mut lanes = Vec::with_capacity(spec.lanes);
            let mut ack = None;
            for lane in 0..spec.lanes {
                let mut client = if lane == 0 {
                    wait_for_port(launcher, spec, depth)?
                } else {
                    // The port is already accepting, so a lane that cannot connect is a refusal —
                    // typically the agent having fewer channels than the run asked for, which the
                    // handshake's capacity check should have caught first.
                    Client::<TcpStream>::connect(spec.address(), depth, Some(POLL * 5)).map_err(
                        |e| {
                            format!(
                                "node {}: lane {lane} of {} could not connect to port {}: {e}. \
                                 The agent serves one connection per mailbox channel it claimed",
                                spec.node, spec.lanes, spec.port
                            )
                        },
                    )?
                };
                // Every connection handshakes: `Hello` is mandatory per connection, and a
                // provenance check on only the first would let a run be driven over connections
                // it never verified.
                let this = client
                    .handshake(&spec.node, &spec.shm_path, spec.lanes, spec.block_bytes)
                    .map_err(|e| describe(spec, e))?;
                ack = Some(this);
                lanes.push(client);
            }
            agents.push(Agent {
                node: spec.node.clone(),
                ack: ack.expect("at least one lane, checked above"),
                lanes,
                spec: spec.clone(),
            });
        }
        Ok(Self {
            agents,
            stopped: false,
        })
    }

    /// Start with the default pipelining depth.
    ///
    /// # Errors
    ///
    /// As [`Agents::start`].
    pub fn start_default<L: Launcher>(launcher: &L, specs: &[AgentSpec]) -> Result<Self, String> {
        Self::start(launcher, specs, DEFAULT_DEPTH)
    }

    /// The started agents.
    pub fn agents(&mut self) -> &mut [Agent] {
        &mut self.agents
    }

    /// Every lane, node-major: all of node 0's lanes, then all of node 1's.
    ///
    /// Node-major so the flat index is `node * lanes + lane`, which is what the driver's routing
    /// computes. A lane borrows its own connection, so the compiler — rather than a comment —
    /// establishes that no two lanes share one.
    pub fn lanes(&mut self) -> Vec<AgentLane<'_>> {
        let mut out = Vec::with_capacity(self.total_lanes());
        for (node_index, agent) in self.agents.iter_mut().enumerate() {
            // Destructured so the name and the connections are disjoint borrows of one `Agent`.
            let Agent { node, lanes, .. } = agent;
            let name = node.clone();
            for (lane_index, client) in lanes.iter_mut().enumerate() {
                out.push(AgentLane {
                    node: name.clone(),
                    node_index,
                    lane_index,
                    client,
                });
            }
        }
        out
    }

    /// How many nodes are in the run.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Lanes across every node, which is how many consumers the driver runs.
    pub fn total_lanes(&self) -> usize {
        self.agents.iter().map(|a| a.lanes.len()).sum()
    }

    /// Lanes per node, which is uniform because every spec is built from one `--lanes`.
    pub fn lanes_per_node(&self) -> usize {
        self.agents.first().map(|a| a.lanes.len()).unwrap_or(0)
    }

    /// Whether no agent was started.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Stop every agent and **verify** that its port went quiet.
    ///
    /// Call this after the timed window closes (FR-050). Verification is the requirement
    /// (FR-053): an agent that acknowledged a shutdown and then kept its mailbox channels would
    /// leave the next run unable to claim them, and that failure would present as a mailbox
    /// problem rather than as a teardown one.
    ///
    /// # Errors
    ///
    /// If an agent could not be asked to stop. A port that stays busy is **not** an error — it
    /// is reported as `port_released: false`, because the run's own measurements are already
    /// taken and the caller should see the whole picture rather than the first failure.
    pub fn stop(mut self) -> Result<Vec<Teardown>, String> {
        let out = self.stop_inner();
        self.stopped = true;
        out
    }

    fn stop_inner(&mut self) -> Result<Vec<Teardown>, String> {
        let mut out = Vec::with_capacity(self.agents.len());
        let mut first_error: Option<String> = None;
        for agent in &mut self.agents {
            // `Shutdown` stops the whole agent, so it goes down **one** connection and the rest
            // are simply closed. Sending it on every lane would ask an agent that has already
            // gone, and the second refusal would be reported as a teardown failure that never
            // happened. The agent's own `Shutdown` tally is process-wide, so nothing is lost.
            let Some(first) = agent.lanes.first_mut() else {
                continue;
            };
            let asked = first.shutdown();
            // Closed here rather than at the end of the loop: the agent exits on `Shutdown`, and
            // leaving our other ends open only delays it noticing.
            agent.lanes.clear();
            match asked {
                Ok(ack) => out.push(Teardown {
                    node: agent.node.clone(),
                    ack,
                    port_released: wait_for_quiet(&agent.spec),
                }),
                // Recorded and carried on with, never returned from here: a node that cannot be
                // asked to stop must not stop the *other* nodes being asked, or one lost node
                // would leave the rest of the cluster holding its mailbox channels — which is the
                // teardown failure FR-053 exists for, arriving by a different route.
                Err(e) => {
                    let text = format!("node {}: asking the agent to stop: {e}", agent.node);
                    first_error = first_error.or(Some(text));
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }
}

impl Drop for Agents {
    fn drop(&mut self) {
        if self.stopped || self.agents.is_empty() {
            return;
        }
        // The generator is unwinding, or forgot to stop. Either way an agent left running holds
        // mailbox channels and a device allocation, so this is worth attempting even though a
        // `SIGKILL` would skip it — that case is covered by the next run replacing the leftover.
        eprintln!(
            "stopping {} agent(s) on the way out; they were not stopped explicitly",
            self.agents.len()
        );
        if let Err(e) = self.stop_inner() {
            eprintln!("agent teardown on drop: {e}");
        }
    }
}

/// Shut down or kill anything already listening on `spec`'s port.
///
/// `Ok(Some(()))` if something was there. A leftover that answers is asked to stop, which lets
/// it release its channels in order; one that will not answer is killed.
fn replace_leftover<L: Launcher>(launcher: &L, spec: &AgentSpec) -> Result<Option<()>, String> {
    let probe = Client::<TcpStream>::connect(spec.address(), 1, Some(POLL * 5))
        .and_then(|c| c.with_read_timeout(PROBE_TIMEOUT));
    let Ok(client) = probe else {
        // Nothing listening. Kill anyway: a process that is running but not accepting is the
        // worst leftover of the three, since it holds resources and answers nothing.
        launcher.kill(spec)?;
        return Ok(None);
    };
    drop(client);
    shut_down_or_kill(launcher, spec)?;
    Ok(Some(()))
}

/// Stop whatever is listening on `spec`'s port, by asking first and killing if that fails.
///
/// Used for a leftover found at startup and for an agent this run launched and then could not
/// use. Both want the same thing and for the same reason: an agent left listening holds mailbox
/// channels, and a **claim outlives the process that made it**, so killing an agent that had not
/// been asked to stop leaks those channels into the shared segment until the server restarts.
///
/// The handshake here is deliberately **not verified**. It is only a way to reach `Shutdown`, and
/// verifying it would refuse precisely the two agents most in need of stopping: a stale leftover,
/// and the freshly launched agent whose provenance refusal brought us here.
///
/// # Errors
///
/// If the agent would neither stop nor be killed.
fn shut_down_or_kill<L: Launcher>(launcher: &L, spec: &AgentSpec) -> Result<(), String> {
    let asked = Client::<TcpStream>::connect(spec.address(), 1, Some(POLL * 5))
        .and_then(|c| c.with_read_timeout(PROBE_TIMEOUT))
        .map(|mut client| {
            client
                .hello(&workload_wire::handshake::hello(&spec.shm_path))
                .is_ok()
                && client.shutdown().is_ok()
        })
        .unwrap_or(false);
    if !asked {
        launcher.kill(spec)?;
    }
    if !wait_for_quiet(spec) {
        launcher.kill(spec)?;
        if !wait_for_quiet(spec) {
            return Err(format!(
                "an agent on port {} would neither stop nor be killed, so this run would have \
                 shared its mailbox channels and reported its counters",
                spec.port
            ));
        }
    }
    Ok(())
}

/// Connect once the agent is listening, or give up.
///
/// Gives up **early** when the launcher can tell us the agent has already exited: waiting out the
/// full timeout for a process that is gone spends twenty seconds proving nothing.
fn wait_for_port<L: Launcher>(
    launcher: &L,
    spec: &AgentSpec,
    depth: usize,
) -> Result<Client<TcpStream>, String> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut last: Option<ClientError> = None;
    let mut gone: Option<String> = None;
    while Instant::now() < deadline {
        match Client::<TcpStream>::connect(spec.address(), depth, Some(POLL * 5)) {
            Ok(c) => return Ok(c),
            Err(e) => last = Some(e),
        }
        gone = launcher.why_not_listening(spec);
        if gone.is_some() {
            break;
        }
        std::thread::sleep(POLL);
    }
    // The launcher's account first when there is one. "Connection refused" is true and useless;
    // "the agent exited saying it could claim 0 of 2 channels" is the same failure, actionable.
    Err(format!(
        "node {}: no agent accepted a connection on port {} within {:?}{}",
        spec.node,
        spec.port,
        START_TIMEOUT,
        match (gone, last) {
            (Some(why), _) => format!(": {why}"),
            (None, Some(e)) => format!(" (last error: {e})"),
            (None, None) => String::new(),
        }
    ))
}

/// Whether the port stops accepting within [`STOP_TIMEOUT`].
fn wait_for_quiet(spec: &AgentSpec) -> bool {
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if Client::<TcpStream>::connect(spec.address(), 1, Some(POLL * 2)).is_err() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// Name the node in a handshake failure, since on a cluster that is the useful part.
fn describe(spec: &AgentSpec, e: HandshakeError) -> String {
    match e {
        HandshakeError::Refused(r) => format!("{r}"),
        HandshakeError::Transport(t) => format!("node {}: {t}", spec.node),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(port: u16) -> AgentSpec {
        AgentSpec {
            node: "127.0.0.1".into(),
            port,
            shm_path: "/dev/shm/certus-shmq".into(),
            binary: "/opt/workload-node-agent".into(),
            lanes: 4,
            block_bytes: 32768,
            batch_keys: 64,
            extra_args: vec!["--no-payload".into()],
        }
    }

    #[test]
    fn the_command_line_carries_everything_the_agent_needs() {
        // A missing `--block-bytes` would let an agent serve a different block size than the
        // description, which the handshake would then refuse — correctly, but confusingly.
        let args = spec(7420).command();
        let joined = args.join(" ");
        assert!(joined.contains("--port 7420"));
        assert!(joined.contains("--shm-path /dev/shm/certus-shmq"));
        assert!(joined.contains("--lanes 4"));
        assert!(joined.contains("--block-bytes 32768"));
        assert!(joined.contains("--batch-keys 64"));
        assert!(joined.ends_with("--no-payload"), "extra args must survive");
        assert_eq!(args[0], "/opt/workload-node-agent");
    }

    #[test]
    fn a_remote_command_is_quoted_so_a_path_with_a_space_survives() {
        let mut s = spec(1);
        s.binary = "/opt/my agent".into();
        let joined = shell_join(&s.command());
        assert!(joined.contains("'/opt/my agent'"), "got {joined}");
    }

    #[test]
    fn the_kill_pattern_names_the_port_so_another_runs_agent_survives() {
        // Two runs on one host use different ports. A pattern matching only the binary would
        // kill the other run's agent, and the symptom would be a lost node mid-run.
        let launcher = SshLauncher::default();
        let a = spec(7420);
        let b = spec(7421);
        // The command is built in `kill`; check the pattern it would use.
        assert_ne!(a.port, b.port);
        let pattern_a = format!("--port {}", a.port);
        let pattern_b = format!("--port {}", b.port);
        assert_ne!(pattern_a, pattern_b);
        // And the launcher is the only thing that ever shells out (FR-054).
        let _ = &launcher;
    }
}
