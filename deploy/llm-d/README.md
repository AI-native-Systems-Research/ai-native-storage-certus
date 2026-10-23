# llm-d deployment (per-GPU profiles, optional certus KV offload or P/D split)

Deploys [llm-d](https://github.com/llm-d/llm-d) and, optionally, points its vLLM
modelservers at certus as a KV-cache offload tier, or splits prefill from decode.

**This repo holds the sources; the llm-d checkout is a disposable build area.**
Nothing here is pushed to the llm-d project — the tuning is specific to our GPU
types and to the certus integration. The checkout is pinned
to `llm-d.ref` and re-materialized on every run, so a stale hand-edit inside it
cannot survive.

## Layout

```
deploy/llm-d/
  install.sh                  end-to-end deploy; hardware-independent
  llm-d.ref                   pinned upstream llm-d revision
  site.env.example            template for the private site file
  publish-image.sh            build + push the certus modelserver image
  bench/                      multi-turn benchmark driver
  profiles/
    a30/                      tuning for one NVIDIA A30 (24Gi) per engine
      agg/                    aggregated serving
        kustomization.yaml    overlay on llm-d's optimized-baseline base
        patch.yaml            model, args, resources, model cache
      certus/                 certus KV offload
        kustomization.yaml    overlay layered on ../agg
        patch.yaml            hostIPC, entrypoint, cache env, mailbox mount
        entrypoint-certus.sh  derives the mailbox from the GPU's NUMA domain
      pd/                     prefill/decode disaggregation
        kustomization.yaml    overlay on llm-d's pd-disaggregation base
        patch.yaml            prefill + decode engines, NIXL over TCP
```

## Profiles and the private site file

Configuration comes in two layers, split by what is safe to publish:

- **`profiles/<gpu>/` is committed.** It holds tuning for a GPU type: model,
  memory sizing, TP, KV transport. It knows nothing about any particular machine.
  Adding a GPU type means adding a profile with whichever roles (`agg`, `certus`,
  `pd`) it supports.
- **The site file is not committed.** It is the only place that names real
  infrastructure: which profile the cluster uses, replica counts (i.e. how many
  GPUs there are), node names for P/D placement, and the image registry.
  `install.sh` reads `${XDG_CONFIG_HOME:-~/.config}/certus/llm-d-site.env`, or
  `SITE_ENV=<path>`. Start from `site.env.example`. The default location is
  outside the repo, so the file cannot be committed by accident. It is sourced as
  bash and sets defaults only (`: "${VAR:=value}"`), so a variable set in the
  caller's environment still wins for a one-off run.

Replica counts are required. `install.sh` stops rather than defaulting them,
because a silent default of 1 would leave GPUs idle. They are stamped onto the
applied overlay's `replicas:` field, which is why the committed profiles carry a
placeholder of 1.

The overlays are **sources**. `install.sh` copies them into the llm-d checkout,
beside the base each one consumes: `agg` and `certus` into
`<llm-d>/guides/optimized-baseline/modelserver/gpu/vllm/`, and `pd` into
`<llm-d>/guides/pd-disaggregation/modelserver/gpu/vllm/`. kustomize will not
resolve a relative resource outside its own root, so an overlay has to sit next
to what it extends. `install.sh` refuses to overwrite a directory llm-d itself
ships under the same name.

The checkout is a scratch area. By default `install.sh` makes a fresh blobless
clone in a `mktemp -d /tmp/llm-d.XXXXXX` directory (~40MB, a few seconds) and
deletes it on exit. `KEEP_LLMD=1` keeps it and prints its path, so you can
inspect what was applied. `LLMD_DIR=<path>` reuses a persistent checkout
instead; it is reset to `llm-d.ref` on every run.

Both are ordinary kustomize overlays. An earlier version instead copied `base/*`
and replayed two `ex` scripts over it, which failed **silently** whenever
upstream changed a line a script matched on: the substitution did not fire and
the overlay stayed byte-identical to base, with nothing to notice. That happened
once already. Now upstream changes flow through, and structural breakage surfaces
at the `kubectl kustomize` step.

Two kustomize details worth knowing if you edit a profile's `patch.yaml`:

- `env` merges by name, so dropping base's `HF_TOKEN` entry needs an explicit
  `$patch: delete` — omitting it is not enough.
- `$patch: replace` on a **list element** does not work for swapping a volume's
  source. It silently keeps base's `emptyDir` and drops the new `hostPath`, with
  no error. Hence base's `torch-compile-cache` is deleted and a
  differently-named `model-cache` added, with the `/.cache` mount repointed by
  its `mountPath` merge key.

## Setting up a new cluster

### Prerequisites

- `kubectl` with cluster-admin on the target cluster, plus `helm` (v3.8+, for
  OCI charts) and `git` on the machine you run `install.sh` from. Outbound
  HTTPS to GitHub, ghcr.io and Hugging Face.
- NVIDIA GPU operator (or device plugin) installed, so nodes advertise
  `nvidia.com/gpu`.
- A profile under `profiles/` for your GPU type. If there isn't one, see
  [Adding a GPU profile](#adding-a-gpu-profile).
- For `WITH_CERTUS=1` only: see [certus offload prerequisites](#certus-offload-prerequisites).

### 1. Collect the values

```bash
kubectl get nodes -o custom-columns='NAME:.metadata.name,GPUS:.status.allocatable.nvidia\.com/gpu'
ls deploy/llm-d/profiles
```

The `GPUS` total is your replica budget: every engine in a profile takes one GPU.
Nodes showing `<none>` have no GPUs and are ignored.

### 2. Write the site file

```bash
mkdir -p ~/.config/certus
cp deploy/llm-d/site.env.example ~/.config/certus/llm-d-site.env
chmod 600 ~/.config/certus/llm-d-site.env
$EDITOR ~/.config/certus/llm-d-site.env
```

The required values ship empty, and `install.sh` refuses to run until they are
filled in:

| Variable | Needed for | Value |
|---|---|---|
| `PROFILE` | always | directory under `profiles/` for your GPU type |
| `AGG_REPLICAS` | default, `WITH_CERTUS=1` | total GPUs |
| `PD_PREFILL_REPLICAS`, `PD_DECODE_REPLICAS` | `PD=1` | split of the total GPUs |
| `PD_PREFILL_NODES`, `PD_DECODE_NODES` | `PD=1` | space-separated node names whose GPUs cover each role |
| `CERTUS_REGISTRY`, `CERTUS_REPO` | `WITH_CERTUS=1` | where `publish-image.sh` pushed the image |
| `NAMESPACE` | optional | default `llm-d-quickstart` |

The rules that tie them together:

- Prefill + decode replicas should equal the GPU total. Fewer leaves GPUs idle;
  more leaves pods `Pending`.
- Each role's nodes need at least as many GPUs as that role's replicas.
  Placement is per role, not per pod: decode pods go to any `pd-decode` node with
  a free GPU.
- A node may appear in both lists. Its GPUs are then shared first come, first
  served, so prefer disjoint lists when the GPU counts allow it.

Worked examples, with made-up node names:

<details>
<summary>One node, 4 GPUs</summary>

```bash
: "${PROFILE:=a30}"
: "${AGG_REPLICAS:=4}"
# PD: 1 prefill + 3 decode on the same node.
: "${PD_PREFILL_REPLICAS:=1}"
: "${PD_DECODE_REPLICAS:=3}"
: "${PD_PREFILL_NODES:=gpu-0}"
: "${PD_DECODE_NODES:=gpu-0}"
```

</details>

<details>
<summary>Two nodes, 2 GPUs each, prefill on one node and decode on the other</summary>

```bash
: "${PROFILE:=a30}"
: "${AGG_REPLICAS:=4}"
: "${PD_PREFILL_REPLICAS:=2}"
: "${PD_DECODE_REPLICAS:=2}"
: "${PD_PREFILL_NODES:=gpu-a}"
: "${PD_DECODE_NODES:=gpu-b}"
```

</details>

<details>
<summary>Three nodes, 1 + 2 + 2 GPUs, decode-heavy for long chats, with certus</summary>

```bash
: "${PROFILE:=a30}"
: "${AGG_REPLICAS:=5}"
: "${PD_PREFILL_REPLICAS:=1}"
: "${PD_DECODE_REPLICAS:=4}"
: "${PD_PREFILL_NODES:=gpu-small}"
: "${PD_DECODE_NODES:=gpu-big-1 gpu-big-2}"
: "${CERTUS_REGISTRY:=registry.example.com}"
: "${CERTUS_REPO:=my-team-docker-local}"
```

</details>

### 3. Check it, then deploy

```bash
DRY_RUN=1 PD=1 deploy/llm-d/install.sh   # or without PD=1 for aggregated
```

A dry run makes no changes to the cluster. It loads the site file, renders the
manifests against the pinned llm-d, and, if a cluster is reachable, checks
read-only that every named node exists and has the GPUs for its replicas. Then it
prints the plan: profile, images, node labels it would apply, any Deployments it
would delete, and the path of the rendered YAML. A typo'd node name is an error;
too few GPUs is a warning, since those pods would only sit `Pending`. With no
cluster reachable, the node and GPU checks are skipped, so you can still check a
site file on a laptop.

When the plan looks right, run the same command without `DRY_RUN=1`. The first
start downloads the model weights, so allow several minutes per node.

## Upstream pinning

`llm-d.ref` records the llm-d revision the overlay is known to apply against;
`install.sh` fetches and checks it out detached. Upstream changes to llm-d's base
manifests therefore reach us only through an explicit bump of that file, never
silently on the next deploy. After a bump, re-run `install.sh` — the
`kubectl kustomize` step is where a base restructuring will show up.

## Usage

```bash
deploy/llm-d/install.sh                 # aggregated serving
WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
PD=1 deploy/llm-d/install.sh            # prefill/decode disaggregation
DRY_RUN=1 PD=1 deploy/llm-d/install.sh  # show the plan; change nothing
```

Other knobs (see the header of `install.sh`): `SITE_ENV` for a site file
elsewhere, `KEEP_LLMD=1` to keep the scratch llm-d checkout, `LLMD_DIR` to reuse
one, and `LLMD_REF` to try another llm-d revision.

Switching between the aggregated and P/D layouts deletes the other layout's
Deployments first, because each is sized to every GPU. That is an outage for the
length of the switch.

## Prefill/decode disaggregation (`PD=1`)

`profiles/a30/pd` runs prefill and decode engines of one A30 each at TP=1, with
vLLM's `NixlConnector` handing KV from prefill to decode. The site file sets how
many of each (`PD_PREFILL_REPLICAS`, `PD_DECODE_REPLICAS`) and which nodes they
run on. It is a testbed
for P/D routing, not a tuned deployment.

- **Choosing the ratio.** For long multi-turn chat, favour decode. Once KV is
  handed over, every in-flight sequence lives on a decode GPU, and one A30 holds
  only ~30k tokens of KV, so decode capacity runs out first. For
  long-prompt/short-output traffic the ratio goes the other way.
- **Placement is by label, not node name.** Prefill selects
  `llm-d.ai/pd-prefill=true` and decode selects `llm-d.ai/pd-decode=true`.
  `install.sh` applies them from `PD_PREFILL_NODES` / `PD_DECODE_NODES`, or
  fails if neither the variables nor existing labels name a node. Set them in
  the site file.
- **Transport is UCX over TCP on the pod network, with a host staging buffer**
  (`kv_buffer_device: cpu`, `UCX_TLS=tcp`). It proves the handoff works but is
  too slow to measure it: a 4k-token prompt carries ~600MB of Qwen3-8B KV,
  which takes seconds at pod-network speeds. A RoCE NIC is not enough to do
  better from an ordinary pod. With the shared RDMA device plugin, a pod outside
  the host network namespace sees `/dev/infiniband` but an empty GID table, so
  RDMA needs a Multus side-network on the RoCE port or `hostNetwork`. GPU-direct
  transfers also need `nvidia-peermem` loaded on the host.
- **Request path.** The EPP's `pd-disaggregation` profile picks a prefill and a
  decode endpoint. The request goes to the decode pod's routing sidecar on :8000,
  which sends the prompt to prefill, then forwards to its local vLLM on :8200,
  which pulls the KV over NIXL. `llm_d_epp_pd_decision_total` on the EPP shows
  whether requests are really being split.
- **`peakPrefillThroughput` is upstream's value** (33821 tok/s, gpt-oss-120b on
  H200). It sets when the prefix-affinity filter gives way to load, so on A30s
  it is effectively "never". Re-measure with llm-d's
  `guides/recipes/router/calibration` before trusting any affinity behaviour.
- **Not combinable with `WITH_CERTUS` yet.** Both set `--kv-transfer-config`.
  Running both means wrapping them in vLLM's `MultiConnector`.
- **Restarting the prefill pod: restart decode too.** NIXL's decode side caches
  the prefill agent. With the vLLM 0.30 image a restarted prefill can segfault
  decode, or leave it failing every handoff for up to an hour (llm-d's
  `docs/operations/disaggregation/vllm.md`, "Stale NIXL Agent Cache"):
  ```bash
  kubectl delete pod -n llm-d-quickstart -l llm-d.ai/role=decode
  ```

## What the A30 tuning does

llm-d's `base` targets Qwen3-32B on 2 GPUs. `profiles/a30/agg/patch.yaml` retunes
it for one A30 per replica: model `Qwen3-8B`, `--tensor-parallel-size=1`,
`nvidia.com/gpu: 1`, halved cpu/memory, and three changes worth calling out:

- **`--max-model-len=16384`.** An A30 is 24Gi; ~15.3Gi of bf16 weights plus
  activations and CUDA graphs leave only ~4.11Gi for KV cache, while Qwen3-8B
  advertises a 40960 context needing 5.62Gi — vLLM refuses to start. 16384 needs
  ~2.25Gi, giving `Maximum concurrency ... 1.83x`.
- **`maxSurge: 0, maxUnavailable: 1`.** One replica per GPU leaves no spare for
  a surge pod. With the default 25% surge a rolling update deadlocks:
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

## Adding a GPU profile

A profile is safe to publish: it describes a GPU type, not a cluster. Start from
the closest existing one:

```bash
cp -r deploy/llm-d/profiles/a30 deploy/llm-d/profiles/<gpu>
```

Then edit `agg/patch.yaml` and `pd/patch.yaml`. Keep the two in step: P/D
hands KV from one engine to another, so both sides must run the same model and
context length.

- **Model and context length.** The rule is the one in
  [What the A30 tuning does](#what-the-a30-tuning-does): weights plus activations
  and CUDA graphs must leave enough GPU memory for one full `--max-model-len`
  sequence of KV, or vLLM refuses to start. Its startup log prints the KV
  capacity (`GPU KV cache size: N tokens`) and `Maximum concurrency`. Pick a
  length below N, and leave headroom if you want more than one request at a time.
- **One GPU per engine.** The replica arithmetic in the site file assumes every
  engine takes one GPU (`--tensor-parallel-size=1`, `nvidia.com/gpu: 1`). A
  model too big for one GPU needs TP>1 and a matching GPU request. Then each
  replica takes TP GPUs, so document that in the profile's comments for whoever
  writes the site file. For P/D, decode TP must be >= prefill TP.
- **cpu/memory requests**, sized to the host RAM a node can spare per GPU.
- **Gated models** need the `llm-d-hf-token` secret. Drop the
  `HF_TOKEN` `$patch: delete` entries and create the secret in the namespace.
- **`certus/`** depends on the certus connector, not on the GPU, so it usually
  carries over unchanged. Keep `TENSOR_PARALLEL_SIZE` in its patch equal to the
  TP in `agg`. Delete any role directory the profile does not support;
  `install.sh` reports a missing role clearly.

Check with `DRY_RUN=1 PROFILE=<gpu> deploy/llm-d/install.sh`, with and
without `PD=1`. The rendered YAML it prints is exactly what would be applied.
Only real infrastructure goes in the site file; keep node names, GPU counts and
registries out of the profile.

## certus offload prerequisites

1. **certus servers running on each GPU node**, one per NUMA domain. Deploy them
   from `deploy/k8s` (see that README): the DaemonSets publish
   `/dev/shm/certus-shmq-numa{0,1}`, which is what `CERTUS_SHM_PREFIX` defaults
   to here. Override it (to `certus-stress-n`, say) if you are pointing at
   hand-started host-mode servers instead. The mailbox naming is the one thing
   that must agree across the client and server deployments; a mismatch fails
   fast in the entrypoint rather than silently.
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

## Restarting certus orphans the running engines

**After any roll of the certus DaemonSets, restart the decode pods:**

```bash
kubectl delete pod -n llm-d-quickstart -l llm-d.ai/role=decode
```

The Deployment recreates them one at a time (`maxSurge: 0, maxUnavailable: 1`),
so this is a rolling restart, not an outage of every replica at once.

Why it is needed: a certus server recreates `/dev/shm/certus-shmq-numa<N>` as a
**new inode** when it starts. An engine that opened the previous one keeps
writing into the orphaned segment. The server then logs `shmq-flow taken 0`
while looking perfectly healthy, and the pod stays `Ready` with 0 restarts,
because nothing detects the break until a request actually exercises the
offload path. At that point the connector burns its full 30s deadline and
raises

```
certus_shmq_connector.ring.RingError: shmq request deadline exceeded after 30s
```

which `kv_load_failure_policy='fail'` (vLLM's default) turns into
`EngineDeadError` — killing the engine and restarting the pod.

So **readiness is not evidence that the mailbox is live.** Compare when each
side started — a decode pod is orphaned if a certus pod on its node started
*after* it:

```bash
kubectl get pods -n certus -o custom-columns=\
'POD:.metadata.name,NODE:.spec.nodeName,STARTED:.status.containerStatuses[0].state.running.startedAt'
kubectl get pods -n llm-d-quickstart -l llm-d.ai/role=decode -o custom-columns=\
'POD:.metadata.name,NODE:.spec.nodeName,STARTED:.status.containerStatuses[0].state.running.startedAt'
```

Do **not** use the mailbox's mtime for this. `/dev/shm/certus-shmq-numa<N>` is a
file-backed shared mapping, so its mtime advances on every write — it tracks
traffic, not creation, and on a busy server it is always a few seconds old no
matter when the segment was made. It will report a perfectly healthy engine as
orphaned.

The other reliable signal is on the server: `shmq-flow taken` stays at 0 for an
orphaned client no matter how much traffic its engine is serving.

```bash
kubectl logs -n certus <certus-pod> --tail=5 | grep shmq-flow
```

This is a known gap rather than a configuration error — the connector has no
reconnect path, and the shm header already carries the `generation` and
`heartbeat` fields a client would need to detect it. Fixing it properly belongs
in the connector; until then, order the restarts.

## GPU / NUMA / mailbox mapping

`entrypoint-certus.sh` derives this at runtime, so one Deployment covers both
nodes: the NVIDIA device plugin exposes only the assigned GPU, so `nvidia-smi`
reports its real host PCI BDF and `/sys/bus/pci/devices/<bdf>/numa_node` gives
the domain.

Example topology -- a two-GPU node and a one-GPU node:

| Node         | GPU            | NUMA | Mailbox             |
|--------------|----------------|------|---------------------|
| `gpu-node-a` | `0000:41:00.0` | 0    | `certus-shmq-numa0` |
| `gpu-node-a` | `0000:a1:00.0` | 1    | `certus-shmq-numa1` |
| `gpu-node-b` | `0000:a1:00.0` | 1    | `certus-shmq-numa1` |

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
