//! Attaching to the node's Certus mailbox and claiming its channels.
//!
//! One place, on one side of the wire. Before FR-079 the generator attached to a local mailbox and
//! the agent attached to its own, with the same refusals written once and used twice; now only the
//! agent attaches, and the generator's `--lanes` reaches this through the agent's command line.

use std::sync::Arc;
use std::time::Duration;

use shm_queue::Client;

/// How long [`attach`] waits for the server to publish its ready flag.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Channels claimed on the mailbox, released when this is dropped.
///
/// # Why this is a guard and not a plain `Vec`
///
/// A claim lives in the **shared segment**, so it outlives the process that made it: a claimed
/// channel whose claimant has exited is unusable until the server restarts. Before FR-079 the
/// generator claimed the local channels and released them itself, and the agent — long-lived,
/// started once per deployment — did not. Making every local run start and stop an agent turned
/// that into a leak of `--lanes` channels *per run*: four two-lane runs against an eight-channel
/// mailbox and the fifth is refused with "another client holds the rest", with no agent running
/// and nothing to kill.
///
/// Found exactly that way, by running the same smoke test five times.
///
/// Dropping releases, so an ordinary exit and a panic both release. A `SIGKILL` still leaks —
/// nothing a process can contain survives that — which is why the server's own reservation
/// reclaim, and ultimately a restart, remain the backstop.
pub struct Claim {
    client: Arc<Client>,
    channels: Vec<usize>,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("channels", &self.channels)
            .finish()
    }
}

impl Claim {
    /// The attached mailbox.
    pub fn client(&self) -> &Arc<Client> {
        &self.client
    }

    /// The channels claimed, in claim order.
    pub fn channels(&self) -> &[usize] {
        &self.channels
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        for channel in &self.channels {
            self.client.release_channel(*channel);
        }
    }
}

/// Attach to the local mailbox and claim `lanes` channels.
///
/// # Errors
///
/// If the mailbox cannot be attached, or `lanes` exceeds the node's channel count — refused
/// rather than clamped, because the mailbox is depth-1 per channel and over-subscription would
/// serialise silently behind a claimed channel, which reads as a slow server rather than as a
/// misconfigured run.
pub fn attach(shm_path: &str, lanes: usize) -> Result<Claim, String> {
    let client = Client::attach(shm_path, ATTACH_TIMEOUT)
        .map_err(|e| format!("attach shmq mailbox {shm_path}: {e}"))?;
    let channels = client.channel_count();
    if lanes == 0 {
        return Err("--lanes must be at least 1".to_string());
    }
    if lanes > channels {
        return Err(format!(
            "refusing to serve: {lanes} lanes against a node with {channels} channels. The \
             mailbox is depth-1 per channel, so the extra lanes would not add concurrency \
             — they would serialise behind a claimed channel and the run would report a \
             throughput for a concurrency it never had. Use --lanes {channels} or fewer"
        ));
    }
    let client = Arc::new(client);
    let mut claimed = Vec::with_capacity(lanes);
    for _ in 0..lanes {
        match client.claim_channel() {
            Some(ch) => claimed.push(ch),
            None => {
                // Built as a `Claim` purely so its `Drop` gives back the ones we did get: a
                // partial claim that stayed claimed would take the mailbox further from usable on
                // every failed attempt.
                let partial = Claim {
                    client: Arc::clone(&client),
                    channels: claimed,
                };
                return Err(format!(
                    "could only claim {} of {lanes} channels; another client holds the rest. \
                     Claims live in the shared segment, so an agent killed outright leaves \
                     them held until the server restarts",
                    partial.channels().len()
                ));
            }
        }
    }
    Ok(Claim {
        client,
        channels: claimed,
    })
}
