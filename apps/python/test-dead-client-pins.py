#!/usr/bin/env python3
"""Pins a dead client held are released when its channel is claimed again (5A).

Against a real certus server started by hand (any profile with a memory tier),
with at least 2 channels. This process first claims channel 0 for itself (the
observer), so the child's claim lands on another channel. The child populates
and pins ``--keys`` blocks there and is SIGKILLed with the pins held. Then:

  1. CLEAR_ALL from the observer reports every one of them still pinned: the
     pins outlived the process (before 5A they stayed so for good);
  2. a new thread here claims the next free channel -- the dead child's -- and
     the claim's UNPIN-all releases them (``pins_released_on_claim``);
  3. CLEAR_ALL now leaves nothing pinned;
  4. an UNPIN on a channel that never pinned a key is refused.

Usage:
  python apps/python/test-dead-client-pins.py --shm /dev/shm/certus-pin5a --keys 16
"""

import argparse
import os
import signal
import subprocess
import sys
import textwrap
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from certus_shmq_helpers import connect  # noqa: E402

CHILD = textwrap.dedent("""
    import ctypes, sys, time
    sys.path.insert(0, {here!r})
    from certus_shmq_helpers import connect, single_region_entries
    cuda = ctypes.CDLL("libcudart.so")
    cuda.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
    cuda.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    size, keys = {size}, list(range({base}, {base} + {n}))
    ptr = ctypes.c_void_p()
    assert cuda.cudaMalloc(ctypes.byref(ptr), size * len(keys)) == 0
    h = (ctypes.c_ubyte * 64)()
    assert cuda.cudaIpcGetMemHandle(h, ptr) == 0
    r = connect({shm!r})
    entries = single_region_entries([(k, bytes(h), size, i * size) for i, k in enumerate(keys)], 0)
    assert all(r.populate(entries))
    assert all(r.pin(keys))
    print("pinned", r._tls.channel, flush=True)
    time.sleep(600)
""")


def on_new_thread(fn):
    """Run ``fn`` on a fresh thread (so on a freshly claimed channel), return
    its result; the thread then parks, keeping its channel claimed."""
    out, done, stop = [], threading.Event(), threading.Event()

    def run():
        out.append(fn())
        done.set()
        stop.wait()

    threading.Thread(target=run, daemon=True).start()
    done.wait()
    return out[0], stop


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--shm", required=True)
    p.add_argument("--keys", type=int, default=16)
    p.add_argument("--size", type=lambda s: int(s, 0), default=64 * 1024)
    p.add_argument("--key-base", type=lambda s: int(s, 0), default=0x5A00_0000_0000)
    a = p.parse_args()
    keys = list(range(a.key_base, a.key_base + a.keys))

    r = connect(a.shm)
    r.check_states([keys[0]])  # the observer: claims the lowest free channel
    mine = r._tls.channel

    child = subprocess.Popen(
        [sys.executable, "-c",
         CHILD.format(here=HERE, size=a.size, base=a.key_base, n=a.keys, shm=a.shm)],
        stdout=subprocess.PIPE, text=True)
    line = child.stdout.readline().split()
    assert line and line[0] == "pinned", f"child said {line!r}"
    dead_ch = int(line[1])
    assert dead_ch != mine
    child.send_signal(signal.SIGKILL)
    child.wait()
    print(f"child pinned {a.keys} blocks on channel {dead_ch}, then was SIGKILLed "
          f"(observer on channel {mine})")

    _, still, _ = r.clear_all()
    print(f"1. CLEAR_ALL, dead client's channel unclaimed: still_pinned={still}")
    assert still == a.keys, f"expected the {a.keys} leaked pins, got {still}"

    before = r.pins_released_on_claim
    ch, stop = on_new_thread(lambda: (r.check_states([keys[0]]), r._tls.channel)[1])
    released = r.pins_released_on_claim - before
    print(f"2. a new thread claimed channel {ch}: pins released on claim = {released}")
    assert ch == dead_ch and released == a.keys

    removed, still, _ = r.clear_all()
    print(f"3. CLEAR_ALL: removed={removed} still_pinned={still}")
    assert still == 0 and removed >= a.keys

    assert r.unpin([keys[0]]) == [False]
    print("4. UNPIN of a key this channel never pinned: refused")
    stop.set()
    print("PASS")


if __name__ == "__main__":
    main()
