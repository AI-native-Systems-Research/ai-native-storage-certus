#!/bin/bash
# run-mha-iobound-stream-sweep.sh — MHA counterpart of run-gqa-iobound-stream-sweep.sh.
# Same concurrency sweep (one 2x2 {certus-shmq, cputier} x {no intervals, intervals}
# matrix per stream count, equal CPU_TIER_GB on both backends, per-count report.html +
# cross-sweep summary.md/.csv), but over run-mha-iobound-matrix.sh: Qwen1.5-7B-Chat
# (MULTI-HEAD attention, 512 KiB/token — 9x the GQA arm). Thin wrapper; every knob,
# prerequisite and the private-SPDK server build are documented in the GQA script.
#
# WORKING SET: at the default PREFIX_COUNT=64 the reused prefix KV is
#     64 x 8192 tok x 512 KiB/tok  =  ~256 GiB
# which exceeds any DRAM tier this host can pin, so reloads are SSD-bound — the
# regime the GQA sweep only reaches at PREFIX_COUNT~512. Lower PREFIX_COUNT (e.g. 16,
# ~64 GiB) for a DRAM-resident MHA run.
#
#   ./run-mha-iobound-stream-sweep.sh                                 # 200..1000 streams, 128G tier
#   STREAMS=1000 CPU_TIER_GB=64 ./run-mha-iobound-stream-sweep.sh     # one point, 64G tier
#   MAX_SECONDS=120 STREAMS=200 ./run-mha-iobound-stream-sweep.sh     # quick smoke
#   PREFLIGHT_ONLY=1 ./run-mha-iobound-stream-sweep.sh                # prerequisites only
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec env FAMILY=mha "${SCRIPT_DIR}/run-gqa-iobound-stream-sweep.sh" "$@"
