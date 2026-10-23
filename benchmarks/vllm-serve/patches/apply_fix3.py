#!/usr/bin/env python3
# apply_fix3.py — in-place, version-independent installer for shmq scheduler fix#3.
#
# run-serve-certus-shmq.sh bind-mounts this file into the shmq client container
# and runs it once, INSIDE the container, right before `vllm serve` (env
# VLLM_FIX3=1, the default). It edits the image's own vendored
#   vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py
# in place, inserting ONLY the fix#3 clamp (see patches/PROVENANCE.md for the
# bug write-up). This replaces the old whole-file bind-mount, which was pinned to
# vLLM 0.26.0 and would drop a stale scheduler onto a drifted API on any other
# version. The patcher instead locates the file via importlib (so no hardcoded
# python3.12 site-packages path) and anchors on a code block that is byte-stable
# across 0.26–0.29, so it self-adapts to the installed vLLM. On 0.30+ the clamp
# is already folded into vLLM's storable_chunks() (min over num_allocated_chunks
# AND num_keyed_chunks), so the patcher detects that and no-ops — see PROVENANCE.md
# for the per-version coverage and the upstream-landing history.
#
# Contract: NEVER fatal to serving. Exits 0 when it applies the clamp, when the
# clamp is already present (marker), or when the running vLLM already carries the
# fix upstream (0.30+). If the anchor is absent/duplicated on a version that is
# NOT already fixed (an unrecognized layout) or the result would not compile, it
# leaves the file untouched, prints a WARNING, and exits non-zero — the caller
# runs `vllm serve` on the STOCK scheduler either way. Idempotent: re-running is a
# no-op.
import sys

MARKER = "certus-fix3"  # idempotency sentinel; present iff the clamp is installed

# Byte-stable anchor across vLLM 0.26/0.27/0.28: the store-path block that builds
# `offload_keys`, immediately before the strided `block_ids` slice + the fatal
# `assert len(offload_keys) == len(offload_block_ids)`. The `storable_chunks(...)`
# call just above it drifted between versions (arg count changed), so we anchor
# BELOW it, on lines that did not. Insert the clamp right before this block.
ANCHOR = (
    "                start_chunk_idx = group_state.next_stored_chunk_idx\n"
    "                if num_chunks <= start_chunk_idx:\n"
    "                    continue\n"
    "                offload_keys = group_state.offload_keys[start_chunk_idx:num_chunks]\n"
)

# The clamp (16-space indent, matching the anchor's scope). num_chunks,
# group_state and blocks_per_chunk are all already in scope at this point.
CLAMP = (
    "                # certus-fix3: clamp num_chunks to chunks backed by BOTH a\n"
    "                # key and GPU blocks. offload_keys advances every step but\n"
    "                # block_ids only grows on new allocations, so a finishing\n"
    "                # request can cross an unbacked chunk boundary; the strided\n"
    "                # block_ids slice below then comes up short and trips\n"
    "                # `assert len(offload_keys) == len(offload_block_ids)`,\n"
    "                # killing the engine (EngineDeadError). Dropping an unbacked\n"
    "                # trailing chunk is correctness-preserving (costs at most a\n"
    "                # future prefix-cache hit). See patches/PROVENANCE.md.\n"
    "                num_chunks = min(\n"
    "                    num_chunks,\n"
    "                    len(group_state.offload_keys),\n"
    "                    len(group_state.block_ids) // blocks_per_chunk,\n"
    "                )\n"
)


def main() -> int:
    try:
        import vllm
        from vllm.distributed.kv_transfer.kv_connector.v1.offloading import (
            scheduler as sched_mod,
        )
    except Exception as exc:  # noqa: BLE001 — any import failure => can't patch
        print(f"[fix3] WARNING: cannot import vLLM offloading scheduler ({exc}); "
              "serving on STOCK scheduler", file=sys.stderr)
        return 3

    path = sched_mod.__file__
    ver = getattr(vllm, "__version__", "?")
    try:
        src = open(path, encoding="utf-8").read()
    except OSError as exc:
        print(f"[fix3] WARNING: cannot read {path} ({exc}); STOCK scheduler",
              file=sys.stderr)
        return 3

    if MARKER in src:
        print(f"[fix3] already applied to vLLM {ver} scheduler ({path}) — no-op")
        return 0

    # Upstream already carries fix#3? vLLM 0.30 folded the clamp into
    #   storable_chunks() -> min(num_chunks, num_allocated_chunks, num_keyed_chunks)
    # where num_keyed_chunks == len(offload_keys). Once num_chunks is clamped to
    # BOTH the backing GPU blocks (num_allocated_chunks, added in 0.27) AND the
    # available keys (num_keyed_chunks, added in 0.30), the strided slice can no
    # longer come up short, so `assert len(offload_keys) == len(offload_block_ids)`
    # cannot fire and this patch is a NO-OP. Report that plainly rather than the
    # anchor-mismatch WARNING below (which wrongly implies a crash-prone scheduler:
    # the 0.30 stock scheduler is NOT crash-prone). The 4-line anchor is absent in
    # 0.30 anyway (the region was rewritten), so without this we'd fail safe with a
    # misleading message. See patches/PROVENANCE.md.
    if "num_keyed_chunks" in src:
        print(f"[fix3] vLLM {ver} already clamps num_chunks upstream "
              "(storable_chunks includes num_keyed_chunks); fix#3 is a no-op")
        return 0

    n = src.count(ANCHOR)
    if n != 1:
        print(f"[fix3] WARNING: anchor found {n} time(s) in vLLM {ver} scheduler "
              f"({path}); expected exactly 1. Unrecognized layout — leaving file "
              "untouched, serving on STOCK scheduler (see patches/PROVENANCE.md).",
              file=sys.stderr)
        return 3

    patched = src.replace(ANCHOR, CLAMP + ANCHOR, 1)

    # Never write something that won't import: compile in memory first.
    try:
        compile(patched, path, "exec")
    except SyntaxError as exc:
        print(f"[fix3] WARNING: patched scheduler would not compile ({exc}); "
              "leaving file untouched, serving on STOCK scheduler", file=sys.stderr)
        return 3

    # Atomic-ish replace within the container overlay.
    import os
    import tempfile

    d = os.path.dirname(path)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".scheduler.fix3.", suffix=".py")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(patched)
        os.replace(tmp, path)
    except OSError as exc:
        print(f"[fix3] WARNING: cannot write {path} ({exc}); STOCK scheduler",
              file=sys.stderr)
        try:
            os.unlink(tmp)
        except OSError:
            pass
        return 3

    print(f"[fix3] applied clamp to vLLM {ver} scheduler ({path})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
