#!/bin/bash
# entrypoint.sh — bring up a single-node 3FS cluster inside this container.
#
#   fdbserver → (first boot: configure + init-cluster) → mgmtd → meta → storage
#   → (first boot: root user, targets, chains, chain table) → hf3fs_fuse_main
#
# State lives under /data (host DATA_DIR, bind-mounted by run.sh):
#   /data/fdb      FoundationDB data       /data/storage  3FS storage targets
#   /data/log      all service logs        /data/state    token, params, init marker
#
# Single node ⇒ replication factor 1: NUM_CHAINS chains, one target each, all on
# disk 0 of storage node 10001. Upstream's data_placement tool refuses one node
# (deploy/README.md FAQ), so the chain table is generated here directly in the
# same id scheme upstream's gen_chain_table.py uses.
#
# Any service exiting stops the container (no restart loop hiding a crash).
set -euo pipefail

CLUSTER_ID="${CLUSTER_ID:-certus3fs}"
NETWORK="${NETWORK:-RDMA}"
NET_IFACE="${NET_IFACE:?NET_IFACE (RoCE/IB netdev, e.g. ens6f0np0) is required}"
IB_DEVICE="${IB_DEVICE:?IB_DEVICE (e.g. mlx5_0) is required}"
NUM_CHAINS="${NUM_CHAINS:-16}"
CHUNK_SIZE="${CHUNK_SIZE:-1048576}"
FDB_PORT="${FDB_PORT:-4500}"
MGMTD_PORT="${MGMTD_PORT:-8100}"     # 3FS defaults (8000/9000…) collide with vLLM :8000
OWNER_UID="${OWNER_UID:-0}"
OWNER_GID="${OWNER_GID:-0}"
MOUNTPOINT=/mnt/3fs
STARTUP_TIMEOUT="${STARTUP_TIMEOUT:-180}"

BIN=/opt/3fs/bin
ETC=/opt/3fs/etc
D=/data
STATE=$D/state
TOKEN_FILE=$STATE/token.txt
FDB_CLUSTER_FILE=$STATE/fdb.cluster

log() { echo "[3fs $(date +%T)] $*"; }
die() { echo "[3fs $(date +%T)] FATAL: $*" >&2; exit 1; }

mkdir -p "$D"/{fdb,storage,log/fdb,state} "$ETC" "$MOUNTPOINT"
ln -sfn "$D/log" /var/log/3fs

IP=$(ip -4 -o addr show dev "$NET_IFACE" 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1)
[[ -n "$IP" ]] || die "no IPv4 address on $NET_IFACE (need --network host and the right NET_IFACE)"
[[ -d "/sys/class/infiniband/$IB_DEVICE" ]] || die "IB device $IB_DEVICE not visible (need --privileged)"

# Ports: mgmtd/meta/storage each get <rdma-group port>, <core TCP port> = base+0..2, base+1000+0..2.
PORTS=$(cat <<JSON
{"mgmtd":   {"8000": $MGMTD_PORT,       "9000": $((MGMTD_PORT + 1000))},
 "meta":    {"8001": $((MGMTD_PORT+1)), "9001": $((MGMTD_PORT + 1001))},
 "storage": {"8000": $((MGMTD_PORT+2)), "9000": $((MGMTD_PORT + 1002))}}
JSON
)
# Addresses are always RDMA-typed; NETWORK=TCP only makes the clients force TCP
# (see render_configs.py). SCHEME still records the mode in state/params.
SCHEME=$([[ "${NETWORK^^}" == TCP ]] && echo TCP || echo RDMA)
MGMTD_ADDR="RDMA://${IP}:${MGMTD_PORT}"

# Everything that is baked into FDB at init-cluster time. Changing any of it later
# needs a wipe (upstream FAQ: mgmtd config changes require clearing FDB).
PARAMS="cluster=$CLUSTER_ID net=$SCHEME ip=$IP mgmtd_port=$MGMTD_PORT chains=$NUM_CHAINS chunk=$CHUNK_SIZE ref=$(cat /opt/3fs/HF3FS_REF)"
if [[ -f $STATE/initialized ]]; then
  if [[ "$(cat "$STATE/params")" != "$PARAMS" ]]; then
    die "cluster was initialized with different parameters:
    was: $(cat "$STATE/params")
    now: $PARAMS
  Run ./teardown.sh --wipe and start again (or restore the old settings)."
  fi
# (find, not `ls -A a b`: with two dirs ls prints "a:" headers even when both are empty)
elif [[ -n "$(find "$D/fdb" "$D/storage" "$STATE" -mindepth 1 -print -quit 2>/dev/null)" ]]; then
  die "found data under $D from a previous, unfinished init. Run ./teardown.sh --wipe first."
fi

CLUSTER_ID="$CLUSTER_ID" MGMTD_ADDR="$MGMTD_ADDR" FDB_CLUSTER_FILE="$FDB_CLUSTER_FILE" \
TOKEN_FILE="$TOKEN_FILE" MOUNTPOINT="$MOUNTPOINT" STORAGE_PATH="$D/storage" \
NET_IFACE="$NET_IFACE" IB_DEVICE="$IB_DEVICE" NETWORK="$SCHEME" PORTS="$PORTS" \
  python3 /opt/3fs/render_configs.py /opt/3fs/templates "$ETC"

# aio-max-nr: upstream requires it raised for storage. --privileged makes /proc/sys writable.
sysctl -w fs.aio-max-nr=67108864 >/dev/null || log "warning: could not raise fs.aio-max-nr"

adm() {
  if [[ -s $TOKEN_FILE ]]; then
    "$BIN/admin_cli" --cfg "$ETC/admin_cli.toml" --config.user_info.token "$(cat "$TOKEN_FILE")" "$@"
  else
    "$BIN/admin_cli" --cfg "$ETC/admin_cli.toml" "$@"
  fi
}

declare -A PIDS
start() { local name=$1; shift; log "starting $name"; "$@" >>"$D/log/$name.stdout" 2>&1 & PIDS[$name]=$!; }

wait_for() {  # wait_for <desc> <cmd...> — poll until cmd succeeds, or die
  local desc=$1 t=0; shift
  until "$@" >/dev/null 2>&1; do
    for n in "${!PIDS[@]}"; do
      kill -0 "${PIDS[$n]}" 2>/dev/null || die "$n exited while waiting for $desc — see $D/log/$n.stdout and $D/log/${n}*.log"
    done
    (( t++ >= STARTUP_TIMEOUT )) && die "timed out after ${STARTUP_TIMEOUT}s waiting for $desc"
    sleep 1
  done
  log "ready: $desc"
}
node_up() { adm "list-nodes" 2>/dev/null | grep -E "\b$1\b" | grep -qE 'PRIMARY_MGMTD|HEARTBEAT_CONNECTED'; }
fdb_up()  { fdbcli -C "$FDB_CLUSTER_FILE" --timeout 5 --exec 'status minimal' | grep -q 'available'; }
chains_ready()   { (( $(adm "list-chains" 2>/dev/null | grep -c 'SERVING') >= NUM_CHAINS )); }

shutdown() {
  local rc=${1:-0}
  log "shutting down"
  umount "$MOUNTPOINT" 2>/dev/null || umount -l "$MOUNTPOINT" 2>/dev/null || true
  for n in fuse storage meta mgmtd fdb; do
    [[ -n "${PIDS[$n]:-}" ]] && kill -TERM "${PIDS[$n]}" 2>/dev/null && wait "${PIDS[$n]}" 2>/dev/null || true
  done
  exit "$rc"
}
trap 'shutdown 0' TERM INT

# ── FoundationDB ──────────────────────────────────────────────────────────────
[[ -f $FDB_CLUSTER_FILE ]] || echo "certus3fs:certus3fs@127.0.0.1:${FDB_PORT}" > "$FDB_CLUSTER_FILE"
start fdb /usr/sbin/fdbserver --cluster-file "$FDB_CLUSTER_FILE" \
  --public-address "127.0.0.1:${FDB_PORT}" --listen-address public \
  --datadir "$D/fdb" --logdir "$D/log/fdb"
if [[ ! -f $STATE/fdb_configured ]]; then
  sleep 2
  fdbcli -C "$FDB_CLUSTER_FILE" --timeout 30 --exec 'configure new single ssd' \
    || die "fdbcli configure failed — see $D/log/fdb"
  touch "$STATE/fdb_configured"
fi
wait_for "FoundationDB" fdb_up

# ── mgmtd ─────────────────────────────────────────────────────────────────────
FIRST_BOOT=0
if [[ ! -f $STATE/initialized ]]; then
  FIRST_BOOT=1
  STRIPE=$(( NUM_CHAINS < 16 ? NUM_CHAINS : 16 ))
  log "init-cluster: chain table 1, chunk ${CHUNK_SIZE}, stripe ${STRIPE}"
  adm "init-cluster --mgmtd $ETC/mgmtd_main.toml 1 $CHUNK_SIZE $STRIPE"
fi
start mgmtd "$BIN/mgmtd_main" --launcher_cfg "$ETC/mgmtd_main_launcher.toml" --app-cfg "$ETC/mgmtd_main_app.toml"
wait_for "mgmtd primary at $MGMTD_ADDR" node_up MGMTD

# ── meta + storage (main configs live in mgmtd; re-upload every boot) ────────
adm "set-config --type META --file $ETC/meta_main.toml"
adm "set-config --type STORAGE --file $ETC/storage_main.toml"
adm "set-config --type FUSE --file $ETC/hf3fs_fuse_main.toml"
start meta "$BIN/meta_main" --launcher_cfg "$ETC/meta_main_launcher.toml" --app-cfg "$ETC/meta_main_app.toml"
start storage "$BIN/storage_main" --launcher_cfg "$ETC/storage_main_launcher.toml" --app-cfg "$ETC/storage_main_app.toml"
wait_for "meta node" node_up META
wait_for "storage node" node_up STORAGE

# ── first boot: root user, targets, chains, chain table ──────────────────────
if (( FIRST_BOOT )); then
  log "creating root admin user"
  adm "user-add --root --admin 0 root" | tee "$STATE/user-add.out"
  grep -oP 'Token\s+\K[^\s(]+' "$STATE/user-add.out" | head -1 > "$TOKEN_FILE"
  [[ -s $TOKEN_FILE ]] || die "could not parse token from user-add output ($STATE/user-add.out)"
  chmod 600 "$TOKEN_FILE"

  # Ids as upstream gen_chain_table.py computes them (target prefix 1, chain prefix 9, disk 0).
  CMDS=$STATE/create_target_cmd.txt CHAINS=$STATE/chains.csv TABLE=$STATE/chain_table.csv
  : > "$CMDS"; echo "ChainId,TargetId" > "$CHAINS"; echo "ChainId" > "$TABLE"
  for i in $(seq 1 "$NUM_CHAINS"); do
    tid=$(( ((1 * 1000000 + 10001) * 1000 + 1) * 100 + i ))
    chain=$(( (9 * 1000 + 1) * 100000 + i ))
    echo "create-target --node-id 10001 --disk-index 0 --target-id $tid --chain-id $chain --use-new-chunk-engine" >> "$CMDS"
    echo "$chain,$tid" >> "$CHAINS"
    echo "$chain" >> "$TABLE"
  done
  log "creating ${NUM_CHAINS} targets / chains (replication 1)"
  adm < "$CMDS"
  adm "upload-chains $CHAINS"
  adm "upload-chain-table --desc $CLUSTER_ID 1 $TABLE"
fi
wait_for "${NUM_CHAINS} chains SERVING" chains_ready

if (( FIRST_BOOT )); then
  echo "$PARAMS" > "$STATE/params"
  touch "$STATE/initialized"
  log "cluster initialized: $PARAMS"
fi

# ── FUSE client ───────────────────────────────────────────────────────────────
start fuse "$BIN/hf3fs_fuse_main" --launcher_cfg "$ETC/hf3fs_fuse_main_launcher.toml"
wait_for "fuse.hf3fs mounted at $MOUNTPOINT" findmnt -t fuse.hf3fs "$MOUNTPOINT"

# A writable home for vLLM's KV files (the connector creates hf3fs_vllm_data_file_<rank> in it).
mkdir -p "$MOUNTPOINT/vllm"
chown "$OWNER_UID:$OWNER_GID" "$MOUNTPOINT/vllm"
log "UP — 3FS mounted at $MOUNTPOINT (host side: run.sh MOUNT), KV dir $MOUNTPOINT/vllm, mgmtd $MGMTD_ADDR"

# Exit (non-zero) as soon as any service dies.
wait -n "${PIDS[@]}" || true
for n in "${!PIDS[@]}"; do
  kill -0 "${PIDS[$n]}" 2>/dev/null || log "service '$n' exited — see $D/log/$n.stdout"
done
shutdown 1
