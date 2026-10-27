#!/usr/bin/env bash
# One LongDocQA run against a freshly formatted certus-server, for comparing builds:
# a server binary and a connector checkout, measured on identical settings.
# See RUNBOOK-compare-builds.md for why each setting matters.
#
#   run_one.sh --server <certus-server binary> --connector <checkout with certus-shmq-connector/>
#              --out <results dir> [--label NAME]
#
# Environment (defaults in brackets):
#   GPU            CDI index of the GPU [0]. Use the GPU on the NVMe drives' NUMA node.
#   OUTPUT_LEN     tokens generated per request [1]. 1 = query round is cold KV load + prefill.
#   KV_LAYOUT      set VLLM_KV_CACHE_LAYOUT in vLLM (e.g. BLHNC) [unset]
#   CONNECTOR      certus = CertusConnector (requests BLHNC itself), stock = vLLM's
#                  OffloadingConnector [certus]
#   DRIVES         space-separated PCI addresses [0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0]
#   TIER           certus-server --memory-tier-size [12G]
#   MODEL          [NousResearch/Meta-Llama-3-8B] (RoPE x4 is applied for Llama-3, as run_bench.sh)
#   IMAGE          vLLM + connector image [localhost/certus-shmq-connector:vllm0.30.0]
#   CLIENT_IMAGE   [localhost/long-doc-qa:latest]
#   LOCK           hardware lock file shared with other benchmarks [/tmp/certus-evolve-latency.hardware.lock]
#
# Exit codes: 0 ok, 1 error, 3 certus-server came up degraded (drives still held by a
# previous server, or memory tier not on the GPU's NUMA node); retry after it is gone.
set -euo pipefail

server= connector= out= label=run
while [ $# -gt 0 ]; do
    case "$1" in
        --server) server=$(readlink -f "$2"); shift 2 ;;
        --connector) connector=$(readlink -f "$2"); shift 2 ;;
        --out) out=$2; shift 2 ;;
        --label) label=$2; shift 2 ;;
        -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done
[ -x "$server" ] || { echo "--server: not an executable: $server" >&2; exit 1; }
[ -d "$connector/certus-shmq-connector" ] || { echo "--connector: no certus-shmq-connector/ in $connector" >&2; exit 1; }
[ -n "$out" ] || { echo "--out is required" >&2; exit 1; }
mkdir -p "$out"; out=$(readlink -f "$out")

GPU=${GPU:-0}
OUTPUT_LEN=${OUTPUT_LEN:-1}
CONNECTOR=${CONNECTOR:-certus}
DRIVES=${DRIVES:-0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0}
TIER=${TIER:-12G}
MODEL=${MODEL:-NousResearch/Meta-Llama-3-8B}
IMAGE=${IMAGE:-localhost/certus-shmq-connector:vllm0.30.0}
CLIENT_IMAGE=${CLIENT_IMAGE:-localhost/long-doc-qa:latest}
LOCK=${LOCK:-/tmp/certus-evolve-latency.hardware.lock}
SHM=/dev/shm/certus-shmq
NAME=ldq-vllm-$label

exec 9>>"$LOCK"
flock -w 120 9 || { echo "hardware lock $LOCK is busy" >&2; exit 1; }

# Any certus-server still running holds the drives; a new server started while the
# previous one is exiting comes up degraded. Stop only a server started from $server
# (e.g. left by an aborted run of this script); anything else belongs to someone else.
servers_of() { pgrep -f "^(sudo -n env [^ ]+ )?$1 --" || true; }
stop_server() {
    for p in $(servers_of "$server"); do sudo -n kill -TERM "$p" 2>/dev/null || true; done
    for _ in $(seq 150); do [ -z "$(servers_of "$server")" ] && return 0; sleep 0.2; done
    for p in $(servers_of "$server"); do sudo -n kill -KILL "$p" 2>/dev/null || true; done
}
stop_server
if pgrep -af "certus-server" | grep -E -- "--(device-pci|drive-count) " | grep -vqF "$server"; then
    echo "another certus-server is running; not touching the drives" >&2; exit 1
fi
vfio=()
for d in $DRIVES; do
    g=$(basename "$(readlink -f /sys/bus/pci/devices/$d/iommu_group)")
    vfio+=("/dev/vfio/$g")
done
for _ in $(seq 120); do sudo -n fuser -s "${vfio[@]}" 2>/dev/null || break; sleep 0.5; done

gpu_bus=$(nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader -i "$GPU" 2>/dev/null \
    | tr 'A-F' 'a-f' | sed 's/^00000000:/0000:/')
gpu_numa=$(cat "/sys/bus/pci/devices/$gpu_bus/numa_node" 2>/dev/null || true)
drive_numa=$(cat /sys/bus/pci/devices/${DRIVES%% *}/numa_node 2>/dev/null || echo "?")
echo "[$label] GPU $GPU NUMA ${gpu_numa:-?}, drives NUMA $drive_numa"
if [ -n "${gpu_numa:-}" ] && [ "$gpu_numa" != "$drive_numa" ]; then
    echo "warning: GPU $GPU is not on the drives' NUMA node; loads cross the socket link" >&2
fi

server_pid= iostat_pid=
cleanup() {
    podman rm -f "$NAME" >/dev/null 2>&1 || true
    [ -n "$iostat_pid" ] && kill -TERM "$iostat_pid" 2>/dev/null || true
    [ -n "$server_pid" ] && stop_server
}
trap cleanup EXIT

dev_args=(); for d in $DRIVES; do dev_args+=(--device-pci "$d"); done
sudo -n rm -f "$SHM"
started=$(date +%s)
setsid sudo -n env LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64 "$server" \
    "${dev_args[@]}" --memory-tier-size "$TIER" --shm-path "$SHM" --channels 32 --format \
    > "$out/certus.stdout.log" 2> "$out/certus.stderr.log" 9>&- &
server_pid=$!
for _ in $(seq 100); do
    [ -e "$SHM" ] && [ "$(stat -c %Z "$SHM")" -ge "$started" ] && break
    kill -0 "$server_pid" 2>/dev/null || { echo "certus-server exited; see $out/certus.stderr.log" >&2; exit 1; }
    sleep 0.2
done
[ -e "$SHM" ] || { echo "certus-server not ready" >&2; exit 1; }
sudo -n chmod 666 "$SHM"
for _ in $(seq 50); do grep -q "memory-tier: allocated" "$out/certus.stderr.log" && break; sleep 0.2; done
want_numa=${gpu_numa:-$drive_numa}
if grep -q "Device or resource busy" "$out/certus.stderr.log" \
   || ! grep -q "memory-tier: allocated from SPDK hugepages (NUMA node $want_numa)" "$out/certus.stderr.log"; then
    echo "certus-server started degraded (drives busy or memory tier not on NUMA node $want_numa)" >&2
    exit 3
fi

# SSD byte counters stay 0 unless the server was built with the rw-telemetry feature.
PYTHONPATH="$connector/certus-shmq-connector:$connector/apps/python" CERTUS_PY_HELPERS="$connector/apps/python" \
    python3 "$connector/tools/certus-iostat-poll.py" "$SHM" 1.0 \
    > "$out/iostat_samples.csv" 2> "$out/iostat.err" 9>&- &
iostat_pid=$!

if [ "$CONNECTOR" = certus ]; then
    conn='"kv_connector":"CertusConnector","kv_connector_module_path":"certus_shmq_connector.connector"'
else
    conn='"kv_connector":"OffloadingConnector"'
fi
kv_cfg="{${conn},\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"spec_name\":\"CertusShmqOffloadingSpec\",\"spec_module_path\":\"certus_shmq_connector.spec\",\"shm_path\":\"$SHM\",\"slab_size_bytes\":2097152}}"
hf=()
case "$MODEL" in *Llama-3-8B*) hf=(--hf-overrides '{"rope_scaling":{"rope_type":"linear","factor":4.0}}') ;; esac
podman rm -f "$NAME" >/dev/null 2>&1 || true
# The connector checkout is mounted over the image's editable install, so the build under
# test is the one vLLM imports (recorded in connector_identity.txt).
podman run -d --name "$NAME" --device "nvidia.com/gpu=$GPU" --network host --ipc=host \
    --entrypoint '["vllm","serve"]' \
    -v "$HOME/.cache/huggingface:/root/.cache/huggingface:z" \
    -v "$connector/certus-shmq-connector:/workspace/certus-shmq-connector:ro,z" \
    -e HF_HUB_OFFLINE=1 -e VLLM_ALLOW_LONG_MAX_MODEL_LEN=1 -e PYTHONHASHSEED=0 \
    ${KV_LAYOUT:+-e VLLM_KV_CACHE_LAYOUT=$KV_LAYOUT} \
    "$IMAGE" \
    --model "$MODEL" --dtype bfloat16 --load-format auto --tensor-parallel-size 1 \
    "${hf[@]}" --max-model-len 32768 --gpu-memory-utilization 0.90 --enable-prefix-caching \
    --enable-log-requests --kv-transfer-config "$kv_cfg" --enforce-eager --port 8000 >/dev/null

deadline=$((SECONDS + 900))
until curl -fsS --max-time 3 http://localhost:8000/v1/models >/dev/null 2>&1; do
    [ "$SECONDS" -ge "$deadline" ] && { podman logs --tail 40 "$NAME" >&2; exit 1; }
    podman ps --filter "name=$NAME" --filter status=running -q | grep -q . \
        || { podman logs --tail 60 "$NAME" >&2; exit 1; }
    sleep 3
done
podman exec "$NAME" python3 -c "import certus_shmq_connector.manager as m, hashlib; print(m.__file__, hashlib.sha256(open(m.__file__,'rb').read()).hexdigest())" \
    > "$out/connector_identity.txt"

podman run --rm --network host -v "$out:/workspace/results:z" \
    -e BASE_URL=http://localhost:8000/v1 -e MODEL="$MODEL" \
    -e NUM_DOCUMENTS=51 -e DOCUMENT_LENGTH=10000 -e OUTPUT_LEN="$OUTPUT_LEN" \
    -e REPEAT_COUNT=1 -e REPEAT_MODE=tile -e MAX_INFLIGHT_REQUESTS=4 \
    -e COMPLETIONS=1 -e JSON_OUTPUT=1 \
    "$CLIENT_IMAGE" > "$out/client.stdout.log" 2> "$out/client.stderr.log"

sleep 15    # let vLLM log its last KV Transfer metrics interval
curl -fsS --max-time 5 http://localhost:8000/metrics > "$out/metrics.txt" 2>/dev/null || true
podman logs "$NAME" > "$out/server.log" 2>&1 || true
echo "[$label] done: $out"
