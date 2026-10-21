#!/bin/bash
# smoke.sh — prove the exported 3FS mount works for vLLM's HF3FSKVConnector path.
#
# Runs as YOU (rootless), inside the same client image and with the same mount/IPC
# flags run-serve-h3fs0-cc131k.sh uses, and drives the exact USRBIO calls the
# connector's hf3fs_client.py makes (register_fd, make_iovec/make_ioring over
# /dev/shm, batched writes then reads) — a plain `dd` would only exercise FUSE.
#
#   ./smoke.sh                     # 256 MiB, 1 MiB I/Os
#   SIZE_MB=4096 ./smoke.sh
set -euo pipefail

CLIENT_IMAGE="${CLIENT_IMAGE:-localhost/vllm-hf3fs-bench:vllm0.30.0}"
MOUNT="${MOUNT:-/mnt/3fs}"
SIZE_MB="${SIZE_MB:-256}"

findmnt -t fuse.hf3fs "$MOUNT" >/dev/null \
  || { echo "error: no fuse.hf3fs mount at $MOUNT — sudo ./run.sh first" >&2; exit 1; }

echo "[smoke] 1/2 FUSE path (host): write+read $SIZE_MB MiB"
f="$MOUNT/vllm/smoke.$$"
dd if=/dev/urandom of="$f" bs=1M count="$SIZE_MB" conv=fsync status=none
dd if="$f" of=/dev/null bs=1M status=progress 2>&1 | tail -1
rm -f "$f"

echo "[smoke] 2/2 USRBIO path (client container, as the connector does it)"
podman run --rm --pull=never --ipc host \
  -v "$MOUNT:$MOUNT" -e MOUNT="$MOUNT" -e SIZE_MB="$SIZE_MB" \
  --entrypoint python3 "$CLIENT_IMAGE" - <<'PY'
import os, time, multiprocessing.shared_memory as shmm
from hf3fs_fuse.io import make_iovec, make_ioring, register_fd, deregister_fd, extract_mount_point

mnt = os.environ["MOUNT"]; total = int(os.environ["SIZE_MB"]) << 20
bs, depth = 1 << 20, 64
path = f"{mnt}/vllm/usrbio-smoke.{os.getpid()}"
assert extract_mount_point(path) == mnt, f"extract_mount_point -> {extract_mount_point(path)!r}"

fd = os.open(path, os.O_RDWR | os.O_CREAT); register_fd(fd)
shm = shmm.SharedMemory(size=bs * depth, create=True)
iov = make_iovec(shm, mnt); shm.unlink()
ior_w = make_ioring(mnt, depth, for_read=False, timeout=1)
ior_r = make_ioring(mnt, depth, for_read=True, timeout=1)
pattern = os.urandom(bs * depth); shm.buf[:] = pattern

def run(ior, read):
    t = time.time(); off = 0
    while off < total:
        n = min(depth, (total - off) // bs)
        for i in range(n):
            ior.prepare(iov[i * bs:(i + 1) * bs], read, fd, off + i * bs)
        for r in ior.submit().wait(min_results=n):
            assert r.result == bs, f"{'read' if read else 'write'} returned {r.result}"
        off += n * bs
    return total / (time.time() - t) / 2**30

w = run(ior_w, False)
shm.buf[:] = b"\0" * len(shm.buf)
r = run(ior_r, True)
assert bytes(shm.buf) == pattern, "read-back mismatch"
deregister_fd(fd); os.close(fd); os.unlink(path)
print(f"[smoke] USRBIO OK: write {w:.2f} GiB/s, read {r:.2f} GiB/s, data verified ({total >> 20} MiB)")
PY
echo "[smoke] PASS"
