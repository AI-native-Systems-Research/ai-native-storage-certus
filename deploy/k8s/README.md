# Certus Kubernetes Deployment Guide

This guide documents the full procedure for setting up a bare-metal Kubernetes
cluster and deploying the Certus server container image.

## Cluster Overview

- **OS:** RHEL 9 on all nodes
- **Kubernetes:** kubeadm-managed cluster
- **Topology:** 1 dedicated control plane node, N worker nodes (the control plane node also runs workloads)
- **CNI:** Flannel
- **Hardware:** Each worker node has NVIDIA GPUs, Mellanox ConnectX NICs (InfiniBand/RoCE), and NVMe drives

---

## 1. Install Kubernetes with kubeadm

### 1.1 Prepare all nodes

Configure kubelet to tolerate swap (rather than disabling swap system-wide):

```bash
# Create kubelet config drop-in to allow swap
mkdir -p /var/lib/kubelet
cat > /var/lib/kubelet/config.yaml <<EOF
apiVersion: kubelet.config.k8s.io/v1beta1
kind: KubeletConfiguration
failSwapOn: false
EOF
```

When running `kubeadm init` or `kubeadm join` (sections 1.4 and 1.6), pass
`--ignore-preflight-errors=Swap` to suppress the swap preflight check.

Enable required kernel modules for Kubernetes networking:

```bash
cat > /etc/modules-load.d/k8s.conf <<EOF
overlay
br_netfilter
EOF

modprobe overlay
modprobe br_netfilter
```

Set sysctl parameters:

```bash
cat > /etc/sysctl.d/k8s.conf <<EOF
net.bridge.bridge-nf-call-iptables  = 1
net.bridge.bridge-nf-call-ip6tables = 1
net.ipv4.ip_forward                 = 1
EOF

sysctl --system
```

### 1.2 Install containerd

```bash
dnf install -y containerd
containerd config default > /etc/containerd/config.toml
```

Edit `/etc/containerd/config.toml` to enable the systemd cgroup driver:

```toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runc.options]
  SystemdCgroup = true
```

```bash
systemctl enable --now containerd
```

### 1.3 Install kubeadm, kubelet, kubectl

```bash
cat > /etc/yum.repos.d/kubernetes.repo <<EOF
[kubernetes]
name=Kubernetes
baseurl=https://pkgs.k8s.io/core:/stable:/v1.30/rpm/
enabled=1
gpgcheck=1
gpgkey=https://pkgs.k8s.io/core:/stable:/v1.30/rpm/repodata/repomd.xml.key
EOF

dnf install -y kubelet kubeadm kubectl
systemctl enable kubelet
```

### 1.4 Initialize the control plane

```bash
kubeadm init --pod-network-cidr=10.244.0.0/16 --ignore-preflight-errors=Swap
```

Set up kubeconfig for the admin user:

```bash
mkdir -p $HOME/.kube
cp /etc/kubernetes/admin.conf $HOME/.kube/config
chown $(id -u):$(id -g) $HOME/.kube/config
```

### 1.5 Install Flannel CNI

```bash
kubectl apply -f https://github.com/flannel-io/flannel/releases/latest/download/kube-flannel.yml
```

### 1.6 Join worker nodes

On the control plane, generate the join command:

```bash
kubeadm token create --print-join-command
```

Run the output on each worker node, adding the swap override:

```bash
kubeadm join <control-plane-ip>:6443 --token <token> --discovery-token-ca-cert-hash sha256:<hash> --ignore-preflight-errors=Swap
```

To allow scheduling on the control plane node (so it also serves as a worker):

```bash
kubectl taint nodes <control-plane-node> node-role.kubernetes.io/control-plane:NoSchedule-
```

---

## 2. Node Configuration (all worker nodes)

### 2.1 Kernel boot parameters

SPDK requires IOMMU pass-through mode and 1GiB hugepages. Set the kernel boot
parameters (requires reboot):

For **AMD** processors:

```bash
grubby --update-kernel=ALL --args="amd_iommu=on iommu=pt default_hugepagesz=1G hugepagesz=1G hugepages=4"
```

For **Intel** processors:

```bash
grubby --update-kernel=ALL --args="intel_iommu=on iommu=pt default_hugepagesz=1G hugepagesz=1G hugepages=4"
```

Reboot after applying.

### 2.2 Memlock limits

SPDK and VFIO require unlimited memlock. Add to `/etc/security/limits.conf`:

```
* hard memlock unlimited
* soft memlock unlimited
```

Log out and back in for the change to take effect.

### 2.3 Kernel module pre-loading

Create `/etc/modules-load.d/certus.conf`:

```
ib_core
ib_umad
ib_uverbs
mlx5_ib
vfio-pci
```

**Why this is needed:** The NVIDIA Network Operator runs in "pre-installed OFED"
mode and does not manage driver loading. The RDMA device plugin enumerates
`/dev/infiniband/` at startup — if these modules haven't loaded, it advertises
zero RDMA capacity and certus pods can't be scheduled. Containers cannot
trigger host-side modprobe. The `vfio-pci` module never auto-loads because no
hardware matches it by default; SPDK explicitly binds NVMe devices to it.

### 2.4 Install NVIDIA GPU driver

Install the NVIDIA GPU driver on each node with a GPU. Download the local repo
RPM from [NVIDIA Driver Downloads](https://www.nvidia.com/download/index.aspx)
(select RHEL 9, x86_64, and the desired driver branch):

```bash
# Install the local repo package (sets up a local yum repo on the node)
rpm -i nvidia-driver-local-repo-rhel9-<version>-1.0-1.x86_64.rpm

# Install the driver from the local repo
dnf install -y nvidia-driver nvidia-driver-cuda nvidia-kmod-common kmod-nvidia-latest-dkms
```

Verify the driver is loaded:

```bash
nvidia-smi
```

### 2.5 Install NVIDIA MOFED (Mellanox OFED)

Install the NVIDIA Mellanox OFED drivers on each node. These replace the inbox
kernel RDMA modules with NVIDIA's out-of-tree versions, providing better
performance and features for InfiniBand/RoCE.

Download from [NVIDIA Networking](https://network.nvidia.com/products/infiniband-drivers/linux/mlnx_ofed/)
and install per NVIDIA's instructions for RHEL 9.

Verify the installation:

```bash
modinfo mlx5_core | grep -E '^filename|^version'
# Expected: filename under /lib/modules/.../extra/, version like 25.x
```

### 2.6 Install NVIDIA Container Toolkit

The container toolkit enables GPU access from within containers by injecting
host CUDA libraries at container launch time.

```bash
dnf config-manager --add-repo https://nvidia.github.io/libnvidia-container/stable/rpm/nvidia-container-toolkit.repo
dnf install -y nvidia-container-toolkit
```

Configure containerd to use the nvidia runtime:

```bash
nvidia-ctk runtime configure --runtime=containerd
systemctl restart containerd
```

This registers an `nvidia` handler in `/etc/containerd/config.toml`.

---

## 3. Cluster-Level Setup

### 3.1 Deploy the NVIDIA RuntimeClass

The RuntimeClass tells Kubernetes to use the nvidia container runtime handler
for pods that specify `runtimeClassName: nvidia`. This handler injects host
GPU drivers and CUDA libraries into containers based on the
`NVIDIA_VISIBLE_DEVICES` and `NVIDIA_DRIVER_CAPABILITIES` environment variables
set in the container image.

```bash
kubectl apply -f deploy/k8s/prerequisites.yaml
```

### 3.2 Deploy the NVIDIA Network Operator

The Network Operator manages the RDMA shared device plugin, which exposes
Mellanox HCA devices as schedulable Kubernetes resources
(`rdma/rdma_shared_device_a`). In our configuration, it runs in
**pre-installed OFED mode** — it does NOT deploy MOFED driver containers;
it only manages the device plugin.

Install the operator via Helm:

```bash
helm repo add nvidia https://helm.ngc.nvidia.com/nvidia
helm repo update
helm install network-operator nvidia/network-operator \
  --namespace nvidia-network-operator \
  --create-namespace \
  --wait
```

Apply the NicClusterPolicy to configure the RDMA device plugin:

```bash
cat <<EOF | kubectl apply -f -
apiVersion: mellanox.com/v1alpha1
kind: NicClusterPolicy
metadata:
  name: nic-cluster-policy
spec:
  rdmaSharedDevicePlugin:
    image: k8s-rdma-shared-dev-plugin
    repository: ghcr.io/mellanox
    version: v1.5.3
    config: |
      {
        "configList": [
          {
            "resourceName": "rdma_shared_device_a",
            "rdmaHcaMax": 63,
            "selectors": {
              "ifNames": ["ens3f0np0"]
            }
          }
        ]
      }
EOF
```

**Note:** The `ifNames` selector (`ens3f0np0`) must match the RDMA-capable
network interface on your nodes. Adjust if your interface name differs.

Verify the operator is ready:

```bash
kubectl get nicclusterpolicy -o jsonpath='{.items[0].status.state}'
# Expected: ready
```

### 3.3 Install Node Feature Discovery (NFD)

NFD labels nodes with hardware capabilities. The certus DaemonSet uses the
label `feature.node.kubernetes.io/pci-15b3.present: "true"` (Mellanox PCI
vendor ID) as a node selector.

If NFD was not deployed by the Network Operator, install it separately:

```bash
kubectl apply -k https://github.com/kubernetes-sigs/node-feature-discovery/deployment/overlays/default?ref=v0.15.1
```

Verify labels are applied:

```bash
kubectl get nodes -o json | jq '.items[].metadata.labels | with_entries(select(.key | startswith("feature.node.kubernetes.io/pci")))'
```

---

## 4. Build and Deploy Certus

### 4.1 Set environment variables

The build script requires the container registry coordinates. These are kept
out of source control:

```bash
export CERTUS_REGISTRY="registry.example.com"   # Required: registry hostname
export CERTUS_REPO="your-docker-repo"           # Required: repository path
export CERTUS_IMAGE="certus"                    # Optional (default: certus)
export CERTUS_TAG="$(git rev-parse --short HEAD)"  # Optional (default: repo short SHA)
```

Deployment knobs, read by `render-manifests.sh` (see 4.1.2) rather than by the
image build — none of them affect the image:

```bash
export CERTUS_NAMESPACE="certus"                # Optional (default: certus)
export CERTUS_NODE_SELECTOR_KEY="certus.ai/worker"   # Optional
export CERTUS_NODE_SELECTOR_VALUE="true"        # Optional (default: true)
export CERTUS_RL_GROUP="foo"                    # Optional (unset = remote-lookup off; blank is an error)
```

`CERTUS_TAG` deliberately defaults to the repo's short SHA, not `latest`: see
4.1.3 for why a shared mutable tag breaks side-by-side deployments.

### 4.1.1 Turning remote-lookup on and off

The switch is a **ConfigMap**, not an image property and not a literal in the
workload spec. `CERTUS_RL_GROUP` at render time only *seeds* its default; the
pod spec references the key:

```yaml
env:
- name: CERTUS_RL_GROUP
  valueFrom:
    configMapKeyRef:
      name: certus-config
      key: rl-group
      optional: true
```

| `rl-group` | effect |
| --- | --- |
| key absent, or ConfigMap absent | each instance forms its own isolated single-node group — remote-lookup **off**. The default, and what this deployment did before the option existed. |
| a name, same across instances | they join that zyre group, so a local miss can be filled from a peer — **on**. |
| present but **blank** | **startup error.** Not a synonym for absent. |

The third row is deliberate. A blank value can only come from a broken template
substitution, a config pipeline that yielded nothing, or a hand-edit that cleared
the value but left the key — and in all of those the intent was to *join* a
cluster. Accepting blank as "off" would turn a remote-lookup-ON run into an OFF
run, and zero remote hits reads exactly like "remote-lookup did not help": the
one wrong answer that is hard to notice. So it fails loudly instead, and the
message says to omit the key to mean off on purpose.

Because "off" is expressed by *absence*, it rests on the pod spec's
`optional: true` rather than on how the server treats a blank value — so it holds
even against an older certus image.

Flip it live without re-rendering or rebuilding:

```bash
kubectl -n certus edit configmap certus-config        # set or clear rl-group
kubectl -n certus rollout restart daemonset -l app=certus-server
```

The restart is required: a ConfigMap consumed via `env` is injected at pod
creation, so it does not propagate to running pods. That is desirable here — you
want a deliberate restart between two measurement runs, not a rollout landing
mid-run.

The image is built `full-remote` either way, so it always carries zyre and the
RDMA initiator/responder. Flipping this changes cluster membership and nothing
else, so an A/B pair differs only in remote-lookup rather than in which binary
is running. Cross-instance fill only pays off when a parallel workload is spread
across nodes, NUMA domains and GPUs; for a single instance there are no peers
worth asking, which is why off is the default.

### 4.1.2 Rendering manifests without building an image

`deploy/k8s/render-manifests.sh` does only the template substitution:

```bash
CERTUS_REGISTRY=... CERTUS_REPO=... deploy/k8s/render-manifests.sh
```

`build-image-and-push.sh` calls it, so the two cannot drift. Use it directly to
re-stamp a namespace, node selector, image tag or rl-group default without
coming back through a ~30GB image build.

### 4.1.3 Running more than one deployment on a cluster

Each deployment gets its own namespace, its own value of the node-selector
label, and its own image — so two people can test their own certus builds side
by side:

```bash
# the nodes you are testing on
kubectl label node <your-node-a> <your-node-b> certus.ai/worker=foo --overwrite
CERTUS_NAMESPACE=certus-foo CERTUS_NODE_SELECTOR_VALUE=foo \
CERTUS_RL_GROUP=foo CERTUS_IMAGE=certus CERTUS_TAG=$(git rev-parse --short HEAD) \
  deploy/k8s/build-image-and-push.sh

# a colleague's nodes, their own build, their own group
kubectl label node <their-node-a> <their-node-b> certus.ai/worker=bar --overwrite
CERTUS_NAMESPACE=certus-bar CERTUS_NODE_SELECTOR_VALUE=bar \
CERTUS_RL_GROUP=bar CERTUS_IMAGE=certus CERTUS_TAG=their-sha \
  deploy/k8s/build-image-and-push.sh
```

`CERTUS_TAG` defaults to the repo's short SHA rather than `latest` precisely for
this case: two people pushing to one repo must not share a mutable tag, or one
overwrites the other and a node can pull the wrong build.

**Node sets must be disjoint**, and that is forced by hardware rather than
convention: the NVMe drives are bound to vfio exclusively, hugepages are
consumed per instance, and the shm mailbox paths (`/dev/shm/certus-shmq-numa*`)
and metrics ports (9400/9401) are fixed. Two deployments on one node would
collide on all four. Partitioning by the label *value* keeps that explicit —
a node carries exactly one deployment's value.

### 4.2 Build the container image

The build script must be run from the repository root:

```bash
# Build and push:
deploy/k8s/build-image-and-push.sh

# Build only (no push):
deploy/k8s/build-image-and-push.sh --no-push

# Build with a specific tag:
deploy/k8s/build-image-and-push.sh --tag v0.1.0
```

The script:
1. Verifies it's running from the repo root
2. Validates `CERTUS_REGISTRY` and `CERTUS_REPO` are set
3. Builds the container image using `deploy/k8s/Dockerfile`
4. Generates `deploy/k8s/certus-server.yaml` and `deploy/k8s/certus-server-numa.yaml`
   from their `.yaml.tpl` templates, substituting the registry/image values
5. Pushes the image (unless `--no-push` is specified)

### 4.3 Set up image pull credentials

If your registry requires authentication, create a pull secret:

```bash
kubectl create namespace certus

kubectl create secret docker-registry artifactory-creds \
  --namespace certus \
  --docker-server="${CERTUS_REGISTRY}" \
  --docker-username="<username>" \
  --docker-password="<password>"
```

### 4.4 Label nodes for certus

The certus DaemonSet uses a `certus.ai/worker: "true"` nodeSelector to
determine which nodes run certus pods. Label each desired node explicitly:

```bash
kubectl label node node2 certus.ai/worker=true
kubectl label node node7 certus.ai/worker=true
```

To remove a node from the certus pool:

```bash
kubectl label node <node-name> certus.ai/worker-
```

### 4.5 Format drives on first deploy

When certus is deployed to a node with unformatted NVMe drives, it will crash
with `invalid superblock magic` because no certus metadata exists on the drives.
The `--format` flag must be passed on the first run to initialize the on-disk
metadata structures.

**`--format` destroys any existing cached data.** Only use it on fresh/blank
drives or when intentionally wiping a node's cache.

To format, temporarily add `--format` to the container args in the generated
manifest before applying. For the NUMA deployment, edit
`deploy/k8s/certus-server-numa.yaml` and add `--format` to the `exec` line
in both DaemonSets:

```yaml
        args:
        - |
          ARGS=""
          for dev in $(cat /config/drives.txt | tr ',' ' '); do
            ARGS="$ARGS --device-pci $dev"
          done
          exec certus-server-yaml $ARGS --format --shm-path ${CERTUS_SHM_PATH} --channels 32 --memory-tier-size 4G
```

Apply the manifest with `--format`, verify the pods start successfully, then
remove `--format` and re-apply:

```bash
# Apply with --format (first time):
kubectl apply -f deploy/k8s/certus-server-numa.yaml

# Verify pods are Running:
kubectl -n certus get pods -o wide

# Remove --format from the yaml, then re-apply:
kubectl apply -f deploy/k8s/certus-server-numa.yaml
```

Alternatively, if the DaemonSet is already deployed and a new node is added,
patch just the new node's pods by temporarily editing the DaemonSet:

```bash
kubectl -n certus edit daemonset certus-server-numa0
# Add --format to the exec line, save
# Wait for pods on the new node to restart successfully
# Edit again to remove --format
```

> **Future improvement:** A `--format-if-needed` flag is planned at the Rust
> level that will auto-detect blank drives (missing superblock) and format them
> automatically while preserving existing formatted drives. This will eliminate
> the manual format step for new nodes.

### 4.6 Deploy certus-server

For a single-instance-per-node deployment:

```bash
kubectl apply -f deploy/k8s/certus-server.yaml
```

For a NUMA-aware deployment (one instance per NUMA node, two per physical node):

```bash
kubectl apply -f deploy/k8s/certus-server-numa.yaml
```

### 4.7 Verify the deployment

```bash
kubectl -n certus get pods -o wide
kubectl -n certus logs daemonset/certus-server
```

Check that RDMA resources are being consumed:

```bash
kubectl describe node <node-name> | grep rdma
```

---

## 5. Adding and Removing Worker Nodes

### 5.1 Adding a new worker node

Complete all steps in **Section 1.1–1.3** (prepare the node, install containerd,
install kubeadm/kubelet/kubectl) and **Section 2** (kernel params, memlock,
modules, GPU driver, MOFED, container toolkit) on the new node. Then:

**On the control plane**, generate a join token:

```bash
kubeadm token create --print-join-command
```

**On the new worker node**, run the join command:

```bash
kubeadm join <control-plane-ip>:6443 --token <token> --discovery-token-ca-cert-hash sha256:<hash> --ignore-preflight-errors=Swap
```

**Verify** the node has joined and is Ready:

```bash
kubectl get nodes
```

**Label the node** to include it in the certus pool:

```bash
kubectl label node <new-node> certus.ai/worker=true
```

**Format drives on the new node** if they have not been previously formatted
(see **Section 4.5**). Without this step, certus pods will crash with
`invalid superblock magic`.

The certus DaemonSet will schedule a pod on the new node. Verify:

```bash
kubectl -n certus get pods -o wide | grep <new-node>
```

If the RDMA device plugin does not pick up the new node's devices, restart the
plugin pod on that node:

```bash
kubectl -n nvidia-network-operator delete pod -l app=rdma-shared-dp --field-selector spec.nodeName=<new-node>
```

### 5.2 Removing a worker node

**Drain the node** (evicts all pods gracefully):

```bash
kubectl drain <node-name> --ignore-daemonsets --delete-emptydir-data
```

**Remove the node from the cluster:**

```bash
kubectl delete node <node-name>
```

**On the removed node**, reset its kubeadm state and clean up CNI:

```bash
kubeadm reset -f
rm -rf /etc/cni/net.d
iptables -F && iptables -t nat -F && iptables -t mangle -F && iptables -X

# Remove the Flannel CNI bridge — if left behind, re-joining will fail with
# "cni0 already has an IP address different from ..." because the bridge
# retains the old subnet assignment.
ip link set cni0 down
ip link delete cni0
```

The node can be re-added later by repeating the join procedure.

---

## Troubleshooting

**Pods stuck in Pending with "insufficient rdma" resources:**
- Verify the RDMA modules are loaded: `lsmod | grep ib_core`
- Check the device plugin pods: `kubectl get pods -A | grep rdma`
- Verify the NicClusterPolicy is ready: `kubectl get nicclusterpolicy`
- Ensure the interface name in the policy matches: `ip link | grep ens3f`

**Pods fail with "RuntimeClass not found":**
- Verify the nvidia RuntimeClass exists: `kubectl get runtimeclass nvidia`
- Verify containerd has the nvidia handler: `grep nvidia /etc/containerd/config.toml`

**SPDK fails to bind devices:**
- Check IOMMU is active: `dmesg | grep -i iommu`
- Check hugepages are allocated: `grep HugePages /proc/meminfo`
- Check vfio-pci is loaded: `lsmod | grep vfio`
- Check memlock limits: `ulimit -l` (should show "unlimited")
