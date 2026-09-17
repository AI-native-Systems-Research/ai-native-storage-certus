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

/// Attach to the local mailbox and claim `lanes` channels.
///
/// # Errors
///
/// If the mailbox cannot be attached, or `lanes` exceeds the node's channel count — refused
/// rather than clamped, because the mailbox is depth-1 per channel and over-subscription would
/// serialise silently behind a claimed channel, which reads as a slow server rather than as a
/// misconfigured run.
pub fn attach(shm_path: &str, lanes: usize) -> Result<(Arc<Client>, Vec<usize>), String> {
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
    let mut claimed = Vec::with_capacity(lanes);
    for _ in 0..lanes {
        match client.claim_channel() {
            Some(ch) => claimed.push(ch),
            None => {
                for ch in &claimed {
                    client.release_channel(*ch);
                }
                return Err(format!(
                    "could only claim {} of {lanes} channels; another client holds the rest",
                    claimed.len()
                ));
            }
        }
    }
    Ok((Arc::new(client), claimed))
}
