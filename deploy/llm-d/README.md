# llm-d deployment (A30 cluster, optional certus KV offload)

Deploys [llm-d](https://github.com/llm-d/llm-d) on this cluster and, optionally,
points its vLLM modelservers at certus as a KV-cache offload tier.

**This repo holds the sources; the llm-d checkout is a disposable build area.**
Nothing here is pushed to the llm-d project — the tuning is site-specific (A30
memory sizing, this cluster's NUMA layout, our registry). The checkout is pinned
to `llm-d.ref` and re-materialized on every run, so a stale hand-edit inside it
cannot survive.

## Layout

```
deploy/llm-d/
  install.sh                  end-to-end deploy (was ~/llm-d-install.sh)
  llm-d.ref                   pinned upstream llm-d revision
  a30/
    kustomization.yaml        overlay on llm-d's ../base
    patch-a30.yaml            A30 retuning (replicas, args, resources, cache)
  publish-image.sh            build + push the modelserver image
  a30-certus/
    kustomization.yaml        overlay layered on ../a30 (apply-time)
    patch-certus.yaml         hostIPC, entrypoint, cache env, mailbox mount
    entrypoint-certus.sh      derives the mailbox from the GPU's NUMA domain
```

`a30/` and `a30-certus/` here are **sources**. `install.sh` copies them into
`<llm-d>/guides/optimized-baseline/modelserver/gpu/vllm/`, because `a30` consumes
llm-d's `../base` and `a30-certus` consumes `../a30` — kustomize will not resolve
a relative resource outside its own root, so the overlays have to live next to
what they extend. Do not hand-edit them there; change the sources here.

Both are ordinary kustomize overlays. An earlier version instead copied `base/*`
and replayed two `ex` scripts over it, which failed **silently** whenever
upstream changed a line a script matched on: the substitution did not fire and
the overlay stayed byte-identical to base, with nothing to notice. That happened
once already. Now upstream changes flow through, and structural breakage surfaces
at the `kubectl kustomize` step.

Two kustomize details worth knowing if you edit `patch-a30.yaml`:

- `env` merges by name, so dropping base's `HF_TOKEN` entry needs an explicit
  `$patch: delete` — omitting it is not enough.
- `$patch: replace` on a **list element** does not work for swapping a volume's
  source. It silently keeps base's `emptyDir` and drops the new `hostPath`, with
  no error. Hence base's `torch-compile-cache` is deleted and a
  differently-named `model-cache` added, with the `/.cache` mount repointed by
  its `mountPath` merge key.

## Upstream pinning

`llm-d.ref` records the llm-d revision the overlay is known to apply against;
`install.sh` fetches and checks it out detached. Upstream changes to llm-d's base
manifests therefore reach us only through an explicit bump of that file, never
silently on the next deploy. After a bump, re-run `install.sh` — the
`kubectl kustomize` step is where a base restructuring will show up.

## Usage

```bash
deploy/llm-d/install.sh                 # plain A30 deployment
WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
```

## What the A30 tuning does

llm-d's `base` targets Qwen3-32B on 2 GPUs. `patch-a30.yaml` retunes it for one
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
   `CERTUS_REGISTRY=... CERTUS_REPO=... deploy/llm-d/publish-image.sh`

   There is deliberately no second Dockerfile here: `publish-image.sh` builds
   `certus-shmq-connector/Dockerfile`, which is already proven by the benchmark
   flow and carries the offline-safe pip flags (`--no-build-isolation
   --no-index --no-deps`; without `--no-deps`, pip re-resolves torch's pins and
   fails on `nvidia-nccl-cu13` for CUDA-13 bases, i.e. vLLM >= 0.27). It is a
   benchmark-driver image, but the pod's `command`/`args` override its
   `ENTRYPOINT`, so serving is unaffected -- the only cost is ~25MB of baked
   datasets and some inert env vars.

### Where to build

The vLLM base is ~30GB extracted, so the build host needs real room *and* has to
stay above kubelet's eviction floor (`nodefs.available<10%`,
`imagefs.available<15%`). In practice that rules out a GPU node that is already
serving: between the model cache and the running decode pods it can sit within a
few GB of its floor, or already below it, and pulling 30GB there evicts those
pods mid-pull. Build somewhere that is not serving. `publish-image.sh` computes
the margin for the host it is run on and refuses if it does not fit. Note docker
group membership is per-host and per-login: a session predating the group change
will not have it -- use a fresh login.

Pulling on the GPU nodes is cheap even though the image is large: their
containerd stores already hold the base layers, so only the connector layer
transfers. That holds as long as the base tag still resolves to the index they
have -- `v0.30.0` and both nodes agree on
`sha256:8a69ffad015f138d7170c4ddc429e230a3bc1c1719f67e14324749df200a4b90`. If
that tag ever moves, a pull becomes a full ~30GB transfer into `/home`, which has
only ~7G of headroom above the imagefs floor. Check before bumping:
`docker buildx imagetools inspect docker.io/vllm/vllm-openai:v<ver>`

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
