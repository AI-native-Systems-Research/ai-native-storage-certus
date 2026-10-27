# Proposal: RoCE for the model-server pods (secondary network)

**Status**: applied 2026-10-26 (approved by the cluster owner), revised the same day from macvlan to ipvlan; manifests in `rdma-network/`. **Date**: 2026-10-26.

## The problem

The vLLM model-server pods cannot use the RoCE NICs. Three things follow:

1. **NIXL runs over TCP on the pod network.** The P/D overlay pins `UCX_TLS=tcp` because
   there is no usable RDMA device in the pod. Measured on node2/node5: a decode pod pulled
   826 MB of KV in 7.7 s, **107 MB/s**, for a 5 872-token prompt -- that transfer, not the
   model, is most of a P/D request's latency.
2. **A pod cannot see its node's RoCE address**, so it cannot say which certus instance
   holds its cache. Worked around for peer selection by the certus server publishing its
   name beside its mailbox (`<mailbox>.endpoint`); this proposal is not needed for that.
3. Anything else in a pod that wants RDMA (a future GPU-direct path, the endpoint itself
   if it ever moved into the pod) hits the same wall.

## Why the pods have no RDMA

On the host, port 1 of `mlx5_0` carries the RoCE v2 GID at index 3,
`::ffff:10.0.0.102`, bound to netdev `ens3f0np0`. A RoCE GID exists only in the network
namespace that owns its netdev. A model-server pod has its own namespace with `eth0`
(flannel) and `lo` only:

- it sees the `mlx5_*` devices in sysfs, because the RDMA subsystem runs in
  `netns shared` mode (`rdma system show`), but every GID it reads is zero, and
  `gid_attrs/ndevs/3` is `EINVAL`: the netdev it names is not in the pod;
- it has no `/dev/infiniband`: the pods do not request `rdma/rdma_shared_device_a`
  (63 allocatable per node, from the NVIDIA network operator's RDMA shared device plugin,
  already installed). The certus pods do, but run with `hostNetwork`, so they see
  `ens3f0np0` and its GIDs.

So two things are missing for a pod: **a netdev on the RoCE port in its namespace**, and
**the RDMA device files**.

## Options

### A. `hostNetwork: true` on the model-server pods

The pod uses the node's namespace: it sees `ens3f0np0`, its GIDs and its address.

- **For**: no cluster component; works at once.
- **Against**: the pods share the node's ports. Two decode pods on node2 would both want
  vLLM's port, the routing sidecar's :8000, the NIXL side channel (5600) and vLLM's
  internal ports; each needs distinct values, per pod, which a Deployment cannot express
  (one template, two replicas). It means one Deployment per GPU, or a StatefulSet with
  ports derived from the ordinal. Every pod also gets the node's whole network, including
  the management interface. The k8s README already notes this as the fallback.

### B. A secondary network on the RoCE port (recommended)

Each model-server pod gets a second interface, on the RoCE port, with its own address in
`10.0.0.0/24`, through Multus. The kernel gives that interface its own RoCE v2 GID, in the
pod's namespace, so UCX/NIXL see a working RDMA device.

- **For**: pods keep their own network namespace and ports; replicas need no per-pod
  config; the standard NVIDIA network-operator setup, so it is documented and supported.
- **Against**: cluster components to install and an address range to reserve; a shared
  change affecting every namespace that opts in (none does unless it asks for the network).

## Proposal B in detail

### Components

The NVIDIA network operator is installed (`nvidia-network-operator`), with a
`NicClusterPolicy` that enables only the RDMA shared device plugin. Add to the same policy:

| policy field | what it installs | why |
|---|---|---|
| `secondaryNetwork.multus` | Multus CNI | attaches extra interfaces to a pod |
| `secondaryNetwork.cniPlugins` | the reference CNI plugins (macvlan, ipvlan) | the interface itself |
| `secondaryNetwork.ipamPlugin` | whereabouts | cluster-wide address allocation from a range |

Flannel stays the primary network; nothing changes for pods that do not ask.

### The attachment

One `NetworkAttachmentDefinition`, `roce`, in the instance's namespace
(`rdma-network/roce-network.yaml`):

```json
{"type": "ipvlan", "master": "ens3f0np0", "mode": "l2", "mtu": 9000,
 "ipam": {"type": "whereabouts", "range": "10.0.0.160/27"}}
```

- **ipvlan, not macvlan** (revised 2026-10-26, see *Revision: macvlan -> ipvlan*). Each pod
  still gets its own address and its own RoCE v2 GID, but shares the port's MAC. A plain
  NetworkAttachmentDefinition: the network operator has a `MacvlanNetwork` CR but none for
  ipvlan.
- **The address range is `10.0.0.160/27`** (`.160`-`.191`), confirmed free with the
  cluster owner: the hosts hold `.100`-`.108` and a DPU holds `.201`.
- **A port takes ipvlan or macvlan children, never both** ("device or resource busy").
- **A host cannot reach its own pods' `net1` addresses** (ipvlan). Pod to pod works on
  the same host and across hosts; nothing uses host to own-pod.
- **PFC / ECN**: the RoCE port's QoS configuration applies to the pods' traffic, since
  it leaves through the same port; nothing to add.

### The pods

In the pd overlay, for both roles:

```yaml
template:
  metadata:
    annotations:
      k8s.v1.cni.cncf.io/networks: roce
  spec:
    containers:
      - name: modelserver
        resources:
          limits:   {rdma/rdma_shared_device_a: 1}
          requests: {rdma/rdma_shared_device_a: 1}
```

and drop `UCX_TLS=tcp`, letting UCX choose RC verbs over the new interface. Keep
`kv_buffer_device: cpu` at first: it stages KV through host memory and needs no GPUDirect.
GPU-to-GPU (`kv_buffer_device: cuda`) additionally needs `nvidia-peermem` loaded on the
hosts -- **not checked yet**.

NIXL's side channel still uses the pod IP on flannel (`VLLM_NIXL_SIDE_CHANNEL_HOST`); only
the bulk transfer moves to RoCE. UCX picks the RDMA device by itself; if it picks the wrong
one, `UCX_NET_DEVICES=mlx5_0:1` pins it.

## Rollout

1. Confirm the address range with the fabric owner.
2. Add `secondaryNetwork` to the NicClusterPolicy; wait for the Multus and whereabouts
   DaemonSets on node2 and node5.
3. Apply `rdma-network/roce-network.yaml` (the `roce` NetworkAttachmentDefinition).
4. One throwaway pod with the annotation and the RDMA resource on each node: check it has
   `net1` with an address from the range, a non-zero GID at index 3 on `mlx5_0`, and that
   `ib_write_bw` between the two pods reaches line rate.
5. Apply the pd overlay change (a separate profile, `pd-rdma`, so the TCP one stays as the
   baseline), roll out, and measure the decode pull against the 107 MB/s TCP baseline.

## Applied (2026-10-26)

1. `rdma-network/nicclusterpolicy-secondary-network.yaml` merged into the NicClusterPolicy:
   Multus, CNI plugins and whereabouts DaemonSets came up on node2 and node5; a plain pod
   still gets only its flannel address.
2. `rdma-network/roce-network.yaml`: the `roce` network in `llm-d-quickstart`, first as a
   macvlan `MacvlanNetwork`, replaced by ipvlan the same day (below).
3. Verified with two throwaway pods (one per node, the `roce` annotation and
   `rdma/rdma_shared_device_a`, `IPC_LOCK`): each got `net1` with an address from the range
   and its own RoCE v2 GID on `mlx5_0` (the index varies per pod -- 5, 7, 9, 11 seen), and
   `ib_write_bw` pod to pod, node2 -> node5, 1 MiB x 4 QPs: **173 Gb/s**, line rate.

## Revision: macvlan -> ipvlan (2026-10-26)

**The failure.** Under macvlan, restarting decode alone (or prefill alone) broke every
NIXL handoff: decode's UCX sent its wireup to prefill over UD, never got an acknowledgement,
and after 32 s failed the endpoint (`UCX AM send failed ... Endpoint timeout`, then
`NIXL_ERR_REMOTE_DISCONNECT`). Restarting both roles together cured it until the next
single-role restart.

**What it was not.** Raw RDMA from the live, failing decode pod to the live prefill pod
ran at 168 Gb/s; every pod's ARP table held the right MAC; UCX picked the IPv4 RoCE v2
GID in every pod (debug log `using gid_index=... addr_family=2`), so
`UCX_IB_ROCE_LOCAL_SUBNET` changed nothing.

**What it was.** Whereabouts gives a restarted pod the lowest free address, which is its
predecessor's; macvlan gives it a new random MAC. Restarting decode onto the *same*
addresses failed 0/4; holding the old addresses with placeholder pods so decode came back
on *new* ones passed 4/4. So the surviving peer's UCX keeps a path to the old MAC for an
address it has seen (inferred from that experiment; not traced in UCX's source).

**The fix.** ipvlan: every pod on a host uses the port's MAC, so a reused address can
never name a stale one. Checked with two pods before switching: own GID per pod (index 5
and 7 seen), `ib_write_bw` 169 Gb/s, `ucp_am_bw` 17.0 GB/s -- the same as macvlan (173
Gb/s, 17.3 GB/s). The earlier belief that ipvlan gets no GID of its own was wrong: the
kernel adds a RoCE GID for each address on any netdev over the port.

**Checked after switching** (pd-certus-rdma, 2 decode on node2 + 1 prefill on node5):
three decode-only restarts and two prefill-only restarts, every one landing on its
predecessor's addresses (`.162`/`.163`, `.161`) -- the case that failed 0/4 under
macvlan -- each 4/4 requests ok through the router.

## Pre-existing problems to look at first

- One `rdma-shared-dp-ds` pod and one network-operator NFD worker are in CrashLoopBackOff
  (as of 2026-10-24), probably on the NotReady nodes (node1, node3, node7).
- The control plane (node0, `192.168.0.53`) went off the network on 2026-10-26; nothing in
  this proposal can be applied until it is back.

## Not proposed

- **SR-IOV** (a virtual function per pod): better isolation and a MAC per pod that the VF keeps,
  but needs VFs configured in firmware and on the hosts, a bigger change; worth it only if
  the shared port proves a problem.
- Moving the transfer endpoint into the pods: the certus servers run on the host network
  already and own the cache; no reason to.
