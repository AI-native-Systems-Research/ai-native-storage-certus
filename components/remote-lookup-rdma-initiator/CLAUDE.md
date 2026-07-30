# Remote Lookup Initiator

Outbound RDMA "push" component: given a remote host endpoint and a batch of
`(key, remote-region)` pairs, it connects out, looks each key up in the local
memory tier, and RDMA-writes matching values into the remote host's memory. It
is the data-holding (server) side of a remote lookup, driven by the
`remote-lookup` component; the zyre control plane and the RDMA accept side live
in `remote-lookup`. See `info/DESIGN.md` and `README.md` for the current design.

- Interface: `IRemoteLookupRdmaInitiator` (`push_async` / `push` / `connect` /
  `disconnect` / `disconnect_all` / `set_local_peer_id`)
  with value types `RemoteRegion`, `PushStatus` and `PushCompletion`, defined in the
  `interfaces` crate. `connect` warms a connection ahead of a push (moves the
  multi-second cold connect off the caller's hot path).
- **Submission is asynchronous.** `push_async` enqueues and returns; a thread per
  remote host owns that host's queue pair and does both the posting and the reaping,
  then invokes the batch's completion callback. `push` is a blocking wrapper over it.
  Because the NIC keeps reading the local buffers after submission returns, whatever
  keeps them alive (a dispatch-map read pin, in practice) must be owned by the
  callback — see `info/DESIGN.md`.
- Receptacles: `logger: ILogger`, `memory_tier: IMemoryTier`.
- Not a workspace default member; built explicitly. SPDK-orthogonal (uses only
  `IMemoryTier`/`ILogger`). The real rdma-core transport is behind the `rdma`
  Cargo feature; without it the crate builds/unit-tests over an in-process mock
  transport with no rdma-core present.

> **Note:** the `specs/001-rdma-remote-lookup-rdma-initiator/` spec describes the old
> passive responder (listener/session/protobuf) design, which has been removed.
> That spec is stale — regenerate it via speckit before relying on it.
