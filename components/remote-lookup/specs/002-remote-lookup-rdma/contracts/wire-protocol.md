# Contract: Remote-Lookup Zyre Wire Protocol (v1)

The peer-to-peer control protocol `remote-lookup` instances speak over zyre. It is the external
contract between Certus nodes; the RDMA data path is out of band (one-sided writes into the
requester's responder-registered pool). All integers are **little-endian**.

## Framing

Every message begins with a 10-byte header:

```
[version: u8 = 1] [msg_type: u8] [op_id: u64]
```

- `version` — bumped on incompatible changes; a receiver MUST ignore a frame whose `version` it
  does not support (logged, not fatal).
- `msg_type` — 1..=4 below; **unknown types MUST be logged and ignored** (FR-018).
- `op_id` — operation correlation id, echoed by responders (FR-018/019).

`endpoint` sub-record (responder address): `[ip_len: u16][ip: utf8 bytes][port: u16]`.

## Messages

### 1 — KEY_QUERY  (SHOUT to the group)

```
header(type=1) [count: u32] [ (key: u64, size: u32) × count ]
```

The requester asks the group which peers hold each `(key, size)`. A batch larger than
`max_keys_per_query` is split across multiple KEY_QUERY frames under one `op_id` (FR-005). The wire
identity is the `(key, size)` tuple (FR-004): a peer holding `key` at a different size MUST answer
"not available", not a size-mismatch.

### 2 — KEY_RESPONSE  (WHISPER back to the requester)

```
header(type=2) [endpoint] [count: u32] [ (key: u64, size: u32, avail: u8) × count ]
```

`endpoint` is the *responder's* bound endpoint (so the requester can, in turn, be served — the
requester is the one that receives writes). `avail`: `0` not available, `1` in memory, `2` on disk.
A peer MUST classify per FR-015 (memory match with equal size ⇒ 1; block/disk match with equal size
⇒ 2; otherwise 0).

### 3 — RDMA_REQUEST  (WHISPER to a serving peer)

```
header(type=3) [endpoint] [rkey: u32] [count: u32] [ (key: u64, addr: u64, length: u32) × count ]
```

The requester asks a peer to RDMA-write the listed keys into its landing slots. `endpoint` is the
**requester's** responder endpoint (where the serving peer's initiator connects); `rkey` is the
requester's single pool-wide rkey; each `(addr, length)` is one landing slot (FR-006a/007). The
serving peer delegates to `IRemoteLookupRdmaInitiator::push`.

### 4 — RDMA_STATUS  (WHISPER back to the requester)

```
header(type=4) [count: u32] [ (key: u64, status: u8) × count ]
```

Sent by the serving peer **after** its `push` returns, mapping `PushStatus` (FR-016): `0` success,
`1` unable-to-connect, `2` key-no-longer-available (`KeyNotFound`/`SizeMismatch` fold to `2`
defensively). This status vector **is** the completion signal for the one-sided writes — there is no
`WRITE_WITH_IMM`.

## Ordering & safety invariants

- A serving peer sends RDMA_STATUS only after reaping its RDMA completions (the status is the
  completion signal).
- Non-success status ⇒ zero bytes written for that key (retry/failover idempotent).
- The requester frees a landing slot exposed to a peer only on (1) RDMA_STATUS received, or (2) the
  peer's zyre `Exit` **after** a responder `DisconnectAck` — never on a per-op timeout while the
  peer is still a live member (FR-014, SC-005). Late one-sided writes cannot land into a reclaimed
  slot because the QP is driven to ERROR before reclaim.
- Stale/`op_id`-unknown KEY_RESPONSE and RDMA_STATUS are discarded (FR-019).
- A node ignores any SHOUT whose sender uuid equals its own (defensive; zyre does not self-deliver)
  (FR-021).
