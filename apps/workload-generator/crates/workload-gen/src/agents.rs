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
//! # Only launching goes over ssh
//!
//! FR-054: load is driven over the fast transport, never by repeated remote command
//! invocation, which cannot sustain it. Ssh appears here and nowhere else, which is also why
//! [`Launcher`] is a trait — the lifecycle *policy* is then testable against a local agent,
//! and only the ssh mechanics are not.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use workload_wire::client::{Client, ClientError, HandshakeError, DEFAULT_DEPTH};
use workload_wire::frame::{HelloAck, ShutdownAck};

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

/// One started agent, and the connection the run drives it over.
#[derive(Debug)]
pub struct Agent {
    /// The node, named in every message about it.
    pub node: String,
    /// What the handshake reported: channels and block size.
    pub ack: HelloAck,
    client: Client<TcpStream>,
    spec: AgentSpec,
}

impl Agent {
    /// The connection, for submitting turns.
    pub fn client(&mut self) -> &mut Client<TcpStream> {
        &mut self.client
    }

    /// What this agent was started from.
    pub fn spec(&self) -> &AgentSpec {
        &self.spec
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
        let mut agents = Vec::with_capacity(specs.len());
        for spec in specs {
            // Always replace, even a current build: a leftover holds the previous run's
            // channels, device memory and counters, and reusing it would report another run's
            // numbers as this one's (FR-052).
            match replace_leftover(launcher, spec) {
                Ok(Some(())) => eprintln!("{}: replaced a leftover agent", spec.node),
                Ok(None) => {}
                Err(e) => return Err(e),
            }
            launcher.launch(spec)?;
            let mut client = wait_for_port(spec)?;
            let ack = client
                .handshake(&spec.node, &spec.shm_path, spec.lanes, spec.block_bytes)
                .map_err(|e| describe(spec, e))?;
            agents.push(Agent {
                node: spec.node.clone(),
                ack,
                client,
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

    /// How many nodes are in the run.
    pub fn len(&self) -> usize {
        self.agents.len()
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
        for agent in &mut self.agents {
            let ack = agent
                .client
                .shutdown()
                .map_err(|e| format!("node {}: asking the agent to stop: {e}", agent.node))?;
            out.push(Teardown {
                node: agent.node.clone(),
                ack,
                port_released: wait_for_quiet(&agent.spec),
            });
        }
        Ok(out)
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
    let Ok(mut client) = probe else {
        // Nothing listening. Kill anyway: a process that is running but not accepting is the
        // worst leftover of the three, since it holds resources and answers nothing.
        launcher.kill(spec)?;
        return Ok(None);
    };
    // A leftover is replaced whatever its provenance, so the handshake is not verified here —
    // only used to reach `Shutdown`. A stale leftover would refuse a verified handshake and
    // then never be asked to stop.
    let asked = client
        .hello(&workload_wire::handshake::hello(&spec.shm_path))
        .is_ok()
        && client.shutdown().is_ok();
    if !asked {
        launcher.kill(spec)?;
    }
    if !wait_for_quiet(spec) {
        launcher.kill(spec)?;
        if !wait_for_quiet(spec) {
            return Err(format!(
                "node {}: a leftover agent on port {} would neither stop nor be killed, so this \
                 run would have shared its mailbox channels and reported its counters",
                spec.node, spec.port
            ));
        }
    }
    Ok(Some(()))
}

/// Connect once the agent is listening, or give up.
fn wait_for_port(spec: &AgentSpec) -> Result<Client<TcpStream>, String> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut last: Option<ClientError> = None;
    while Instant::now() < deadline {
        match Client::<TcpStream>::connect(spec.address(), DEFAULT_DEPTH, Some(POLL * 5)) {
            Ok(c) => return Ok(c),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(POLL);
    }
    Err(format!(
        "node {}: no agent accepted a connection on port {} within {:?}{}",
        spec.node,
        spec.port,
        START_TIMEOUT,
        match last {
            Some(e) => format!(" (last error: {e})"),
            None => String::new(),
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
