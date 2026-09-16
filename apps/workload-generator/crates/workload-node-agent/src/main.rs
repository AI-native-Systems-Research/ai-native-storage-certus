//! The per-node daemon.
//!
//! Attaches to the local Certus mailbox and serves `SubmitTurn` frames from the generator by
//! applying FR-072a's rule against it — check the path, load what is resident, store what is
//! absent. Only **keys** cross the network; the payload is a pre-filled device buffer here.
//!
//! It is not a second generator. Which keys, which session and which virtual time all come
//! from the generator's single simulation core, and the rule it applies is
//! [`workload_gen::exec::TurnExecutor`] — the same code the local path runs, so the two cannot
//! drift (see `agent`).
//!
//! Exits non-zero if the local mailbox is absent, so a missing server is a startup failure
//! rather than a run that quietly measures nothing.

mod agent;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::Parser;
use workload_gen::live;
use workload_gen::payload::PayloadBuffer;
use workload_wire::handshake;
use workload_wire::server::Server;

use crate::agent::AgentFactory;

/// Exit codes. Non-zero for anything that means the node cannot serve a run.
mod exit {
    /// Clean stop, asked for by the generator.
    pub const OK: i32 = 0;
    /// The mailbox, the GPU or the port could not be obtained.
    pub const SETUP: i32 = 2;
}

/// The node agent's command line.
#[derive(Debug, Parser)]
#[command(
    name = "workload-node-agent",
    about = "Per-node daemon: applies the generator's turns to the local Certus mailbox"
)]
struct Cli {
    /// Port to listen on.
    #[arg(long, default_value_t = 7420, env = "WORKLOAD_AGENT_PORT")]
    port: u16,

    /// Address to bind. Defaults to every interface, since the generator is remote.
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    /// The local Certus mailbox.
    #[arg(long, default_value = "/dev/shm/certus-shmq")]
    shm_path: String,

    /// Connections to serve, each claiming its own mailbox channel.
    ///
    /// A connection is a lane. The mailbox is depth-1 per channel, so a lane needs its own:
    /// sharing one would serialise lanes while still reporting the concurrency asked for.
    #[arg(long, default_value_t = 4)]
    lanes: usize,

    /// Bytes per block, which must match the description the generator is running.
    #[arg(long, default_value_t = 32768)]
    block_bytes: u32,

    /// Keys per request (FR-069). Must be at least what the generator uses.
    #[arg(long, default_value_t = 64)]
    batch_keys: usize,

    /// GPU device for the payload buffer.
    #[arg(long, default_value_t = 0)]
    gpu_device: i32,

    /// Serve the control path only, issuing no data-moving operations.
    ///
    /// For a node with no accelerator. Runs against it are **partial** and the generator's
    /// report says so, because a throughput from a stream missing its loads and stores is not
    /// comparable with a complete run's.
    #[arg(long)]
    no_payload: bool,

    /// Stamp each stored block with its key. Costs a host-to-device copy per key.
    #[arg(long)]
    stamp_keys: bool,
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => std::process::exit(exit::OK),
        Err(e) => {
            eprintln!("workload-node-agent: {e}");
            std::process::exit(exit::SETUP);
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    // Provenance first, before anything expensive is set up: an agent that cannot describe the
    // sources it was built from will be refused by every generator that connects (FR-051), so
    // saying it here is more useful than discovering it one handshake later.
    if !handshake::is_known() {
        eprintln!(
            "warning: this agent cannot describe the sources it was built from, so every \
             generator will refuse it (FR-051). Build from a readable source tree, or set \
             WORKLOAD_SOURCE_ID deliberately."
        );
    }

    // The mailbox, and the channels this agent will hand out one per connection. Absent
    // mailbox is a startup failure: a daemon that came up without one would accept
    // connections and measure nothing.
    let (client, channels) = live::attach(&cli.shm_path, cli.lanes)?;
    eprintln!(
        "attached {} with {} channels; serving {} lanes",
        cli.shm_path,
        client.channel_count(),
        channels.len()
    );

    // One allocation for the whole run, filled before anything is served (FR-038). Without it
    // the two data-moving operations cannot be issued, which is a legitimate mode on a node
    // with no accelerator but never a silent fallback.
    let payload = if cli.no_payload {
        eprintln!("no payload buffer: LOOKUP and COPY_TO_STORE will be counted, not issued");
        None
    } else {
        Some(Arc::new(PayloadBuffer::new(
            channels.len(),
            cli.batch_keys,
            cli.block_bytes,
            cli.gpu_device,
            cli.stamp_keys,
        )?))
    };

    let factory = AgentFactory::new(
        Arc::clone(&client),
        channels,
        payload,
        cli.block_bytes,
        cli.batch_keys,
    );
    let addr = format!("{}:{}", cli.bind, cli.port);
    let server = Server::bind(&addr, factory).map_err(|e| format!("bind {addr}: {e}"))?;
    eprintln!("listening on {addr}; source id {}", handshake::SOURCE_ID);

    // The generator stops the agent with a `Shutdown` frame, which is why there is no
    // signal-driven loop here: teardown is part of the protocol so that it can be *verified*
    // rather than assumed (FR-053). Detecting and replacing a leftover agent from a crashed
    // run is T071's, and needs more than a signal handler — the channels it claimed and the
    // device memory it held have to be reclaimed too.
    let stop = Arc::new(AtomicBool::new(false));
    server.serve(stop).map_err(|e| format!("serve: {e}"))?;
    eprintln!("stopped");
    Ok(())
}
