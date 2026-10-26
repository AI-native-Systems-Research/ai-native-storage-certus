# Proposal: RoCE for the model-server pods (secondary network)

**Status**: proposed, not applied. **Date**: 2026-10-26. Shared-cluster change: needs the
cluster owner's approval.

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

One `NetworkAttachmentDefinition` (a `MacvlanNetwork` CR, which the operator renders into
one), in the instance's namespace:

```yaml
apiVersion: mellanox.com/v1alpha1
kind: MacvlanNetwork
metadata:
  name: roce
spec:
  networkNamespace: llm-d-quickstart
  master: ens3f0np0          # the RoCE port; the same name on node2 and node5
  mode: bridge
  mtu: 9000                  # the port's MTU
  ipam: |
    {"type": "whereabouts", "range": "10.0.0.0/24",
     "range_start": "10.0.0.200", "range_end": "10.0.0.239",
     "exclude": ["10.0.0.0/25"]}
```

- **macvlan, not ipvlan**: each pod gets its own MAC, which RoCE needs for its own GID;
  ipvlan shares the master's MAC.
- **The address range must be reserved**: `10.0.0.200-239` is a guess at an unused slice.
  The hosts hold `10.0.0.102` and `10.0.0.105` (and others for the remaining nodes); the
  range must not overlap any host, switch or other user of the subnet. **Needs
  confirmation from whoever owns the 10.0.0.0/24 fabric.**
- **The switch must accept extra MACs** on each host port (macvlan adds one per pod).
  Usually so on a lab fabric; needs checking, since RoCE traffic from an unknown MAC is
  silently dropped and would look like a NIXL hang.
- **PFC / ECN**: the RoCE port's QoS configuration applies to the macvlan's traffic, since
  it leaves through the same port; nothing to add, but worth verifying with a bandwidth
  test before relying on it.

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

1. Confirm the address range and the switch's MAC policy with the fabric owner.
2. Add `secondaryNetwork` to the NicClusterPolicy; wait for the Multus and whereabouts
   DaemonSets on node2 and node5.
3. Create the `MacvlanNetwork` in `llm-d-quickstart`.
4. One throwaway pod with the annotation and the RDMA resource on each node: check it has
   `net1` with an address from the range, a non-zero GID at index 3 on `mlx5_0`, and that
   `ib_write_bw` between the two pods reaches line rate.
5. Apply the pd overlay change (a separate profile, `pd-rdma`, so the TCP one stays as the
   baseline), roll out, and measure the decode pull against the 107 MB/s TCP baseline.

## Pre-existing problems to look at first

- One `rdma-shared-dp-ds` pod and one network-operator NFD worker are in CrashLoopBackOff
  (as of 2026-10-24), probably on the NotReady nodes (node1, node3, node7).
- The control plane (node0, `192.168.0.53`) went off the network on 2026-10-26; nothing in
  this proposal can be applied until it is back.

## Not proposed

- **SR-IOV** (a virtual function per pod): better isolation and no macvlan MAC concerns,
  but needs VFs configured in firmware and on the hosts, a bigger change; worth it only if
  macvlan's shared port proves a problem.
- Moving the transfer endpoint into the pods: the certus servers run on the host network
  already and own the cache; no reason to.
