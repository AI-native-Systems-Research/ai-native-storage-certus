# llm-d deployment (A30 cluster, optional certus KV offload)

Deploys [llm-d](https://github.com/llm-d/llm-d) on this cluster and, optionally,
points its vLLM modelservers at certus as a KV-cache offload tier.

**This repo holds the sources; the llm-d checkout is a disposable build area.**
Nothing here is pushed to the llm-d project — the tuning is site-specific (A30
memory sizing, this cluster's NUMA layout, our registry). `install.sh`
regenerates the overlay from llm-d's own `base` on every run, so a stale
hand-edit inside the checkout can never survive.

## Layout

```
deploy/llm-d/
  install.sh                  end-to-end deploy (was ~/llm-d-install.sh)
  a30/
    edit-patch.ex             ex script: retunes base/patch-vllm.yaml for A30
    edit-kustomization.ex     ex script: fixes the llm-d.ai/model label
  a30-certus/
    kustomization.yaml        overlay layered on ../a30 (apply-time)
    patch-certus.yaml         hostIPC, entrypoint, cache env, mailbox mount
    entrypoint-certus.sh      derives the mailbox from the GPU's NUMA domain
    Dockerfile                vLLM + certus-shmq-connector image
    build-image-and-push.sh   build/push that image
```

`a30/` and `a30-certus/` here are **sources**. `install.sh` materializes them
into `<llm-d>/guides/optimized-baseline/modelserver/gpu/vllm/` as `a30/` and
`a30-certus/`. The certus overlay consumes `../a30` as a kustomize resource, so
it must sit beside the generated overlay — which is why it is copied in rather
than applied from here.

Do not hand-edit the generated overlays. Change the `.ex` scripts (A30 tuning)
or `a30-certus/*.yaml` (certus wiring) in this repo instead.

## Usage

```bash
deploy/llm-d/install.sh                 # plain A30 deployment
WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
```

## What the A30 tuning does

llm-d's `base` targets Qwen3-32B on 2 GPUs. `edit-patch.ex` retunes it for one
A30: model `Qwen3-8B`, `--tensor-parallel-size=1`, `nvidia.com/gpu: 1`,
`replicas: 3`, halved cpu/memory, and three changes worth calling out:

- **`--max-model-len=16384`.** An A30 is 24Gi; ~15.3Gi of bf16 weights plus
  activations and CUDA graphs leave only ~4.11Gi for KV cache, while Qwen3-8B
  advertises a 40960 context needing 5.62Gi — vLLM refuses to start. 16384 needs
  ~2.25Gi, giving `Maximum concurrency ... 1.83x`.
- **`maxSurge: 0, maxUnavailable: 1`.** 3 replicas on exactly 3 GPUs leaves no
  spare for a surge pod. With the default 25% surge a rolling update deadlocks:
  the old ReplicaSet holds every GPU and never goes Ready, so it is never scaled
  down.
- **`HF_HOME` / `VLLM_CACHE_ROOT` into a `/home/llm-d-cache` hostPath.** The
  image has `HOME=/root` and sets no `HF_HOME`, so HuggingFace downloaded ~16Gi
  per pod onto the container writable layer — under containerd's root on
  `/home`, which is kubelet's imagefs. Two pods on one node crossed the
  `imagefs.available<15%` threshold and were evicted mid-download. The hostPath
  gives one shared copy per node that also survives pod restarts. (The base
  manifest's emptyDirs at `/.cache`, `/.triton`, `/.config` were dead — 0 bytes —
  for the same `HOME=/root` reason.)

The HF token secret is deliberately **not** created: Qwen3-8B is ungated and the
HF_TOKEN env block is commented out. The "unauthenticated requests to the HF Hub"
warning only costs download rate limit.

## certus offload prerequisites

1. **certus servers running on each GPU node** in shmq mode, one per NUMA
   domain, e.g. `--shm-path /dev/shm/certus-stress-n0 --channels 16`. Currently
   host processes, not pods; `entrypoint-certus.sh` takes `CERTUS_SHM_PREFIX`
   (default `certus-stress-n`) to match whatever naming is live. Under the
   `deploy/k8s` DaemonSet the prefix is `certus-shmq-numa`.
2. **An image pull secret** named `artifactory-creds` in the target namespace:
   ```bash
   kubectl create secret docker-registry artifactory-creds \
       --docker-server="${CERTUS_REGISTRY}" \
       --docker-username=<user> --docker-password=<api-key> \
       -n llm-d-quickstart
   ```
3. **The connector image** built and pushed:
   `CERTUS_REGISTRY=... CERTUS_REPO=... deploy/llm-d/a30-certus/build-image-and-push.sh`

## GPU / NUMA / mailbox mapping

`entrypoint-certus.sh` derives this at runtime, so one Deployment covers both
nodes: the NVIDIA device plugin exposes only the assigned GPU, so `nvidia-smi`
reports its real host PCI BDF and `/sys/bus/pci/devices/<bdf>/numa_node` gives
the domain.

Example topology -- a two-GPU node and a one-GPU node:

| Node         | GPU            | NUMA | Mailbox            |
|--------------|----------------|------|--------------------|
| `gpu-node-a` | `0000:41:00.0` | 0    | `certus-stress-n0` |
| `gpu-node-a` | `0000:a1:00.0` | 1    | `certus-stress-n1` |
| `gpu-node-b` | `0000:a1:00.0` | 1    | `certus-stress-n1` |

Each pod gets its own mailbox on its own host — none is shared — so `DP_SIZE=1`
and `DP_RANK=0` are correct throughout. Those only need changing if two engines
ever share one mailbox, in which case each must get a distinct `DP_RANK` and a
matching `DP_SIZE`: the connector's channel claim is a non-atomic owner-word
write, and safety comes from each process scanning a disjoint channel slice.
On `gpu-node-b` above, the NUMA-0 instance has no local GPU and serves only as
a remote-lookup peer.

`hostIPC: true` is required — the connector passes CUDA IPC handles and the
server DMAs GPU<->DRAM<->SSD out of band, so the pod must share the host IPC
namespace. Note that this grants access to all host shared memory.
