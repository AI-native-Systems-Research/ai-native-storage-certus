# Design: reclaiming a dead client's shmq channel, and CLEAR_ALL

**Feature**: `001-rdma-cache-transfer` (found during T087) | **Date**: 2026-10-23 |
**Status**: built. Part 1: `lib/shm-queue`, `ring.py` (vLLM-kill hardware run pending). Part 2: `IDispatcher::clear_all`, shm-queue op 18

Two changes to the shared-memory control plane (`lib/shm-queue`, `lib/shmq-dispatcher`,
`certus-shmq-connector/ring.py`): a channel held by a process that died is returned to the
pool, and one op empties the whole cache, both tiers.

## Part 1: channel reclaim

### The defect

A client takes a channel by writing a non-zero **owner word** in the channel's request
control block (`OFF_OWNER = 12`) and holds it for the thread's life. It is cleared only
by `release_channel`, called on a clean exit (`atexit`, `close()`). A client that dies
otherwise -- `SIGKILL`, OOM kill, `podman rm -f`, a pod deleted, an engine-core crash --
leaves its words set, and nothing ever clears them: the server never touches owner words
(`ring.py:560`: "the server never touches the owner word"). Each such death leaks the
channels that process held. With 8 or 16 channels per mailbox, after a few deaths the
next engine finds none free and dies at its first request:

```
RingError: no free shmq channel in slot 0/1 range [0,8) of 8
```

Seen twice on node2 during T087 (8 channels, ~8 kills). Only restarting the server --
which recreates the mailbox -- cleared it. In k8s every vLLM pod restart against a
long-lived certus DaemonSet pod is such a death, so this is a production defect, not a
test artefact.

What is in the owner word today does not help: Python writes a hash of
`threading.get_ident()`, Rust `pid ^ tid | 1`, neither recoverable to a process. And a
PID alone would not be enough: client and server are in **different PID namespaces**
(both pods have `hostIPC: true`, neither `hostPID`), so the server cannot look a client
PID up, and PIDs are reused.

### What liveness signal is available across PID namespaces

The two processes share exactly one thing: the mailbox file, `/dev/shm/<name>` on the
host's tmpfs (mounted into both pods). So the signal has to live on that file. **Open
file description (OFD) locks** do it:

- `fcntl(F_OFD_SETLK)` on a byte range of the mailbox file. The lock belongs to the open
  file description and is **released by the kernel when the last descriptor to it
  closes** -- which happens, whatever the cause, when the process exits. No cooperation
  from the dying process.
- The kernel keys locks on the inode, not on any namespace; a lock taken in one pod is
  seen by `F_OFD_GETLK` in another mapping the same tmpfs file. (Verified needed: see
  *Checks before building*.)
- Unlike classic POSIX locks, OFD locks are not dropped when the process closes *some
  other* descriptor to the file, and are owned per description, so two threads with
  their own descriptions can hold disjoint ranges independently.
- Python has them (`fcntl.F_OFD_SETLK`, present in the vLLM 0.30 image's Python 3.12 and
  in the host's).

### Proposal

**Each claimed channel is backed by an OFD write lock on one byte of the mailbox file, at
`lock_base + channel`. The owner word stays the claim; the lock is its proof of life. A
claimer that finds a channel taken but its lock free knows the owner is dead and takes
the channel over.**

The byte range lies **beyond the mapped region** (`lock_base = total_size`): a lock does
not need the bytes to exist, so the file is not grown and the layout does not change.

Claim (client), per channel in its slice:

1. Owner word 0 → CAS (Rust) / locked write (Python) to my id, as today. Then take the
   lock: `F_OFD_SETLK` (non-blocking) on byte `lock_base + ch`. It cannot fail for a
   channel whose word was 0 -- unless a dead owner's lock is still held by a descriptor
   it passed to a child; then release the word and move on.
2. Owner word non-zero → **probe**: `F_OFD_SETLK` on the byte.
   - **Refused**: the owner is alive. Next channel.
   - **Granted**: the owner is dead (its description closed). I now hold the lock, so
     no other prober can also win. Take the channel: write my id to the owner word, and
     **resync my sequence number** from the cell (below). Count it (`reclaimed`).

Release (client): clear the word, then unlock the byte. Order matters: a prober that
gets the lock after the unlock sees word 0 or a new owner's id, never a live-but-
unlocked channel.

The lock is the arbiter, not the word: two claimers probing the same dead channel race
for the lock, and exactly one wins. The word's existing CAS (Rust) and per-process lock
(Python) stay; the TP>1 slot partition stays (it bounds scanning, and keeps processes of
one engine apart).

#### Sequence numbers after a takeover

A client keeps a per-channel sequence counter in its own memory (`self._seqs`, Rust
`seqs`). A new owner of a used channel would start where *its* counter stands, while the
server's `last_seen` for the channel is the dead owner's last value: if equal, the
server would miss the next request; and a reply cell holding the dead owner's last
response could match. So on takeover the new owner sets its counter to
`max(request.seq, response.seq)` read from the cell. The next request is strictly newer
than anything the server has seen on that channel.

This is also what a client must do when it *first* claims a used channel today -- the
same hazard exists between successive clean owners of one channel in two processes
(each process starts its counters at 0). It is hidden today only because a process
usually reclaims channels the server has seen few requests on. Fold it into every claim.

#### A request in flight when the owner died

The dead owner may have published a request the server has not yet answered. The server
still processes it and writes the reply to the response cell; nobody reads it. Effects:

- **Idempotent ops** (CHECK, LOOKUP into a dead process's GPU memory -- the IPC handle is
  then invalid, the copy fails and the key is reported failed): harmless.
- **RESERVE / store**: the reservation is left unpublished; the existing stale-
  reservation reaper reclaims it by age (`pending_stores`). Harmless.
- **PIN**: a pin taken for a dead process was never released -- a leak independent of
  channel reclaim. **Closed 2026-10-27** by *Pins belong to the channel lease* below:
  the takeover's UNPIN-all releases it, the in-flight one included, since the claim
  waits for that PIN's reply first.

A new owner must not publish until the in-flight request has been answered, or the
server would see the new request's seq while still writing the old reply. So after
taking over, the new owner waits until `response.seq == request.seq` (bounded by the
usual deadline) before its first request. A server that died with the client is the
existing deadline case.

#### Server side

Nothing is required: the server never reads owner words or locks. Optional, and useful:
a gauge `certus_shmq_channels_claimed` (owner word non-zero) and `..._alive` (lock held,
by probing with `F_OFD_GETLK`) on `/metrics`, so a leak shows up before it bites. The
server's own descriptor to the file holds no locks, so it does not interfere.

#### Compatibility

- **An old client** (no locks) holding a channel looks dead to a new client: the probe is
  granted and the channel is stolen from under a live process. Mixed old and new clients
  on one mailbox must not happen. Bump `ABI_VERSION` 1 → 2: an old client refuses a new
  server at attach (`ring.py:484` already checks), and a new client refuses an old one.
  Server, connector image and any host-side scripts (`apps/python`) roll together --
  which the orphaned-mailbox rule already forces (README: "after any roll of the certus
  DaemonSets, restart the decode pods").
- **Layout** is unchanged; only the ABI number moves, to make the semantic change
  detectable.

### Alternatives considered

- **Heartbeat per channel** (client bumps a word; server or claimers reclaim on
  staleness): needs a timeout, misjudges a client stalled in a long GPU copy or a GC
  pause as dead -- stealing a live channel is a correctness bug, a leak only a capacity
  one -- and costs a store per request.
- **PID in the owner word + `kill(pid, 0)`**: PID namespaces differ between pods; PIDs
  are reused.
- **`pidfd`**: per-process handles cannot be passed between unrelated namespaces through
  the mapping.
- **More channels**: postpones the failure by a few restarts.
- **Server reclaims on its own** (e.g. on attach of a new generation): the server does
  not know which channels are dead, for the same reasons.

### Checks (run on node2, 2026-10-23)

1. **Visible across pods: yes.** A container (own PID namespace, `--ipc=host`, host
   `/dev/shm` mounted) took an OFD lock on byte 100 000 of a 4 KiB tmpfs file -- beyond
   its end, as proposed. A second container and the host were both refused the same byte
   (`EAGAIN`) and granted the next one.
2. **Released on death: yes,** on `SIGKILL` of the holder and on `podman rm -f` of its
   container.
3. **A forked child keeps the lock alive: yes, as feared.** The parent locked, forked,
   and was `SIGKILL`ed; the probe was still refused until the child died too. So a
   process that forks while holding a channel keeps it alive in its child. In vLLM 0.30
   at TP=1 only `EngineCore` maps the mailbox and holds descriptors to it (`/proc/*/fd`
   in the prefill pod: one process); the API server and the resource tracker hold none,
   and `EngineCore` has no children. At TP>1 each worker opens the mailbox itself
   (`_get_or_create_ring`). **The ring opens the file `O_CLOEXEC` and registers an
   `os.register_at_fork(after_in_child=...)` hook that closes the inherited descriptor
   and forgets the claims**, so a fork cannot keep a dead parent's channels alive (the
   child's own claims are its own, through its own open). `exec` is covered by
   `O_CLOEXEC`.

### Tests

- Two processes: A claims all channels; `SIGKILL` A; B claims one -- succeeds, and its
  first request is answered (sequence resync).
- A alive holds a channel: B's probe is refused, B claims another; A's request still
  answered.
- A dies with a request published and unanswered: B takes over, waits for the reply,
  then its own request is answered and A's reply is never delivered to B.
- Two claimers race for one dead channel: exactly one wins (repeat ×1000).
- ABI: an ABI-1 client against an ABI-2 server is refused at attach.
- A process forks while holding a channel and is killed; the child lives on: the
  channel is reclaimed (the after-fork hook closed the child's copy).
- Planted bugs: unlock before clearing the word; no sequence resync; no wait for an
  in-flight reply; no after-fork close -- each must fail a test.
- Hardware: the T087 case -- kill 10 vLLM containers against one 8-channel server, the
  11th starts and serves.

## Part 2: CLEAR_ALL

### The need

`CLEAR_MEMORY_TIER` empties DRAM but leaves every entry on SSD (it demotes what it can,
`try_evict_to_block`, and only removes what has no SSD copy). So a "cleared" server still
holds the blocks: CHECK reads them, a LOOKUP serves them from SSD, a FETCH refuses them as
already present. In T087 an "empty destination" therefore needed a server restart. Tests
and benchmarks need a cold cache without the restart (and without the orphaned-mailbox
restart of every engine that a server restart forces).

### Design

**Op `CLEAR_ALL` (18): remove every entry from both tiers, freeing their extents.**
Request empty; response `{removed:u64, still_pinned:u64, fetch_pending:u64}`. Behind it,
**`IDispatcher::clear_all() -> ClearAll { removed, still_pinned }`** (dispatcher spec 001
FR-063).

- **Listing every entry without a new map method.** The dispatch map has no key
  iterator, and `define_interface!` drops method bodies, so a new `IDispatchMap::keys`
  could not have a default: all 23 implementations in the workspace would need one. Two
  sources already cover every entry instead -- the memory tier's `oldest_keys` (every DRAM
  entry, with or without an SSD copy) and each drive's `IExtentManager::for_each_extent`
  (every SSD entry, the walk the dispatcher already does at start-up to rebuild the map).
  An entry is in at least one. So the only interface change is `IDispatcher::clear_all`
  and the `ClearAll` struct (4 implementations).
- **Removal** goes through `IDispatcher::remove`, which frees the slot, frees an SSD-only
  entry's extent, and refuses a pinned entry. It does **not** free the SSD copy of an
  entry still in DRAM (its lookup reports a memory-tier location, no offset), so
  `clear_all` frees those extents itself from the listing, once the entry is out of the
  map and nothing can reach them. Found by the first test run: the extents leaked.
- **Pinned entries** are retried for 5 s, 1 ms apart; what is still listed then is
  `still_pinned`, not an error, so a caller can tell "empty" from "nearly".
- **Eviction events**: `remove` reports nothing, so `clear_all` emits a `Removed` event
  per entry it removed; the connector forwards them to vLLM through `TAKE_EVENTS`.
- **Fetch marks and pending fetches**: the translator drops every `Fetched` mark (counted
  `fetched_unused`, so the attribution identity still closes); a fetch in flight keeps
  its slots until its completion (FR-017) and lands afterwards. `fetch_pending` says how
  many keys are still in flight, so a test can wait for zero.

Interface change: `IDispatcher::clear_all` -- in `components/interfaces`, so every
component's spec-sync stamp moves. Re-stamp last, once, with the rest of the branch.

### Tests

- Dispatcher (`repair_test_clear_all.rs`, one mock drive behind the real background
  writer): SSD-only and DRAM-with-SSD-copy entries are all removed, extents freed, one
  `Removed` event each; a DRAM-only entry (its write-through failed) is removed; a pinned
  entry is kept and reported, and goes with a second call after the unpin. Four planted
  bugs each caught: list memory-tier keys only; skip freeing the SSD copy's extent; emit no
  event; release the pin to force removal.
- Translator: marks dropped, the in-flight fetch reported and still landing, the identity
  `hits = lookup_hits_fetched + fetched_unused + marks_live` closing; a planted "keep the
  marks" caught.
- On a real server (`full-fs-block`, a file as the drive): 64 blocks populated, flushed,
  cleared from DRAM (still present: on SSD), then `CLEAR_ALL`: removed 64, 0 present, all
  64 accepted again by populate, 64 `Removed` events drained. A restart after it recovered
  0 extents; one before it, 64.

## Out of scope, found on the way

- **Pins held for a dead client were never released.** A PIN whose UNPIN never came
  held the entry forever (it could be neither evicted nor removed). **Closed 2026-10-27**:
  see *Pins belong to the channel lease*.
- **The server does not bump `generation` across restarts** (`create` always wrote 1).
  A client could not tell a restarted server from the old one, which is part of why the
  orphaned-mailbox case was silent. **Done 2026-10-27, still ABI 2** (unstable is 1, so
  no one has seen 2 without it): see *Server restart detection* below.

## Server restart detection (2026-10-27)

A restarted server recreates the mailbox as a new inode; a client still maps the old one,
where nobody polls, and used to wait out its full deadline (30 s in the connector), which
`kv_load_failure_policy=fail` turns into an engine death.

- **A generation per start.** `Server::create` writes `fresh_generation()`: wall-clock
  nanoseconds folded with the PID, never 0. It cannot be "previous + 1": the server
  unlinks the mailbox when it stops, so a restart finds nothing to read.
- **Probe only when the server is quiet.** While waiting for a reply, a client watches
  the header `heartbeat`. The poller never sleeps, so a heartbeat that stands still for
  `QUIET_BEFORE_PROBE` (0.5 s) means a dead or wedged server; the client then reads the
  header at the mailbox *path* (at most once per interval). A ready server there with a
  different generation means this mapping is orphaned: the request fails at once --
  Rust `ErrorKind::BrokenPipe`, Python `ServerRestartedError` (a `RingError`) -- saying
  the server restarted and the engine must reattach.
- **Not a replacement:** the same generation (a slow server), no file, or a file whose
  magic is not yet ready (a server still starting). Those keep waiting to the deadline.
- **No reconnect.** Pins, reservations and fetch marks died with the old server; a
  client that silently switched would hold handles to nothing. Failing fast, with the
  reason, is the whole change. The engine still restarts.

## Pins belong to the channel lease (2026-10-27)

A pin is held by the channel that took it, and ends with that channel's lease.

- **The server records each pin against its channel** (`ChannelPins` in
  `shmq-dispatcher`, counted: a key pinned twice on a channel is two pins). `dispatch`
  now takes the channel the request arrived on.
- **UNPIN is strict.** It releases only a pin recorded on its own channel. A key the
  channel does not hold a pin on is refused (`0`, counted `refused`), never released:
  otherwise a late UNPIN, or one from the wrong thread, would decrement another
  reader's pin -- the double release principle IX forbids.
- **UNPIN-all**: an UNPIN whose key count is `UNPIN_ALL` (`0xFFFFFFFF`, no keys
  follow; 0 already means "unpin nothing") releases every pin on its channel and
  answers the number released, `u64`. No new opcode.
- **The client sends UNPIN-all on every claim and before every release.** On a claim
  it releases what a previous owner left -- a dead one's, including a PIN in flight
  when it died, because the claim already waits for that reply -- before this owner's
  first request. That closes the takeover corner case: without the per-channel
  record, a new owner could not tell its predecessor's pins from its own. On a release
  it ends the lease's pins (best effort; the next claimer releases them regardless).
- **Pin and unpin on one thread.** A Python ring holds one channel per thread, so an
  UNPIN from another thread is refused. vLLM's offloading scheduler calls
  `prepare_load` (PIN) and `complete_load` (UNPIN) on the engine core's busy-loop
  thread, so the connector already does.
- **No reaper.** A dead client's pins last until something claims its channel. Under
  the per-claim UNPIN-all that is the next client on the mailbox to need a channel;
  only a mailbox nobody uses again keeps them.
- **Still ABI 2.** Unstable is ABI 1, so no one has run a 2 without this; deployed
  ABI-2 servers and connectors are replaced together.

Checked on node2 (`full-fs-block`, 8 channels): a child pinned 16 blocks and was
SIGKILLed; CLEAR_ALL then reported `still_pinned=16`; the next claim of that channel
released 16 (`pins_released_on_claim`), after which CLEAR_ALL removed 16 with 0 still
pinned (`apps/python/test-dead-client-pins.py`). With the claim's UNPIN-all removed,
the same test fails.

## Changes, by component

| where | change | size |
|---|---|---|
| `lib/shm-queue` | OFD lock per claim, probe-and-take-over, seq resync, wait for in-flight reply; ABI 2 | medium |
| `certus-shmq-connector/ring.py` | the same in Python (`fcntl.F_OFD_SETLK`); ABI 2 | medium |
| `lib/shmq-dispatcher` | `CLEAR_ALL` op; drop fetch marks | small |
| `components/interfaces` | `IDispatcher::clear_all`, `ClearAll` | small, restamps all |
| `components/dispatcher`, `components/dispatcher-p2p` | implement it | small |
| deploy | none; server and connector image roll together (ABI 2) | -- |
