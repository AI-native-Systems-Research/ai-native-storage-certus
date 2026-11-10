#!/bin/sh
# Does remote lookup serve anything? The measurement that has never been possible.
#
# The earlier arms could not answer this: their peer was an empty, undriven server, so
# it held nothing and could not serve by construction. Here ALL FOUR instances are
# driven in ONE RDMA group, so every cache fills, and a session that migrates looks for
# its prefix on the node it left -- which is the only condition under which a peer can
# serve. Migrations are what create the opportunity, so the run reports them.
#
# Cold-formatted first, so nothing carries over from a previous run.
set -eu
R=/home/scooter/ai-native-storage-certus
export LD_LIBRARY_PATH=$R/deps/zyre-build/lib:$R/deps/zyre-build/lib64:$R/deps/spdk-build/lib
GEN=$R/target/release/workload-gen

echo "══ 4 instances, one group (stress_a), all driven ══"
sh /tmp/stress-servers.sh stop >/dev/null 2>&1 || true
ssh node5 'sh /tmp/stress-servers.sh stop' >/dev/null 2>&1 || true
MT_EVICT=0.8 SBP=200 sh /tmp/stress-servers.sh start >/dev/null 2>&1
ssh node5 'MT_EVICT=0.8 SBP=200 sh /tmp/stress-servers.sh start' >/dev/null 2>&1
PROBE=lookup sh /tmp/stress-agents.sh >/dev/null 2>&1
ssh node5 'PROBE=lookup sh /tmp/stress-agents.sh' >/dev/null 2>&1

$GEN run /tmp/stress-2m.yml --until 10 --rate inf --seed 1 \
  --batch-keys 64 --probe lookup --no-launch \
  --report /tmp/remote-benefit.json \
  --instance 127.0.0.1:7420:/dev/shm/certus-stress-n0 \
  --instance 127.0.0.1:7421:/dev/shm/certus-stress-n1 \
  --instance node5:7420:/dev/shm/certus-stress-n0 \
  --instance node5:7421:/dev/shm/certus-stress-n1 \
  > /tmp/remote-benefit.log 2>&1 || true

echo "-- what the generator saw --"
grep -oE "lookup +[0-9]+ returned data, [0-9]+ did not \([0-9.]+% hit\)|migrations +[0-9]+" \
  /tmp/remote-benefit.log | sed 's/^/   /'

echo "-- per-instance counters (each node counts only what IT obtained from peers) --"
for pair in "node2 9400" "node2 9401"; do
  set -- $pair
  printf "   %s:%s " "$1" "$2"
  curl -s --max-time 10 "localhost:$2/metrics" \
    | grep -E "^certus_(lookup_hits|lookup_misses|remote_lookup_hits|remote_lookup_misses)_total" \
    | awk '{printf "%s=%s ", $1, $2}'
  echo
done
for port in 9400 9401; do
  printf "   node5:%s " "$port"
  ssh node5 "curl -s --max-time 10 localhost:$port/metrics \
    | grep -E '^certus_(lookup_hits|lookup_misses|remote_lookup_hits|remote_lookup_misses)_total' \
    | awk '{printf \"%s=%s \", \$1, \$2}'"
  echo
done
