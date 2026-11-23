#!/bin/sh
# apply.sh — bake the vLLM 0.30.0 finished-KV-load scheduler fix into an image.
#
# Called from the vLLM image Dockerfiles (Dockerfile.offload, certus-shmq-connector/
# Dockerfile) at build time:
#     /tmp/vllm-fix-kvload-hol/apply.sh "${VLLM_VERSION}"
#
# Replaces vllm/v1/core/sched/scheduler.py with the vendored patched copy, but only
# when (a) VLLM_FIX_KVLOAD_HOL is 1 (default), (b) the base is vLLM 0.30.0, and
# (c) the installed file is byte-identical to stock 0.30.0 (md5 below). Any other
# installed file is refused with a build error rather than overwritten, because the
# vendored copy is a whole-file replacement valid for that exact source only.
# Other vLLM versions build unpatched with a notice. See PROVENANCE.md.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
VER="${1:?usage: apply.sh <vllm-version>}"
VER="${VER#v}"
STOCK_MD5=849cc72e032b672f310e32cfc42eaef8     # vLLM 0.30.0 vllm/v1/core/sched/scheduler.py
PATCHED_MD5=a01271366fcb7ec89237c795b8565151   # vendored scheduler.py (this dir)
TAG="[vllm-fix-kvload-hol]"

if [ "${VLLM_FIX_KVLOAD_HOL:-1}" != "1" ]; then
  echo "$TAG opted out (VLLM_FIX_KVLOAD_HOL=${VLLM_FIX_KVLOAD_HOL}); stock vLLM ${VER} scheduler"
  exit 0
fi
if [ "$VER" != "0.30.0" ]; then
  echo "$TAG not applied: fix is for vLLM 0.30.0 only, base is ${VER}"
  exit 0
fi

[ "$(md5sum "$HERE/scheduler.py" | cut -d' ' -f1)" = "$PATCHED_MD5" ] || {
  echo "$TAG ERROR: vendored scheduler.py does not match PATCHED_MD5; re-vendor it (PROVENANCE.md)" >&2; exit 1; }

PY=python3
TARGET=$($PY -c 'import os, vllm; print(os.path.join(os.path.dirname(vllm.__file__), "v1/core/sched/scheduler.py"))')
CUR=$(md5sum "$TARGET" | cut -d' ' -f1)
if [ "$CUR" = "$PATCHED_MD5" ]; then
  echo "$TAG already applied to ${TARGET}"
  exit 0
fi
if [ "$CUR" != "$STOCK_MD5" ]; then
  echo "$TAG ERROR: ${TARGET} md5 ${CUR} is not stock vLLM 0.30.0 (${STOCK_MD5}); refusing to overwrite" >&2
  exit 1
fi

cp "$HERE/scheduler.py" "$TARGET"
$PY -c "import py_compile, sys; py_compile.compile(sys.argv[1], doraise=True)" "$TARGET"
grep -q "_certus_front_finished_kv_loads" "$TARGET"
[ "$(md5sum "$TARGET" | cut -d' ' -f1)" = "$PATCHED_MD5" ]
echo "$TAG baked into ${TARGET} (finished async KV loads scheduled ahead of new admissions)"
