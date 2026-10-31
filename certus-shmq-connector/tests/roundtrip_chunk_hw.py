#!/usr/bin/env python3
"""Byte-exact round trip through Certus with several GPU blocks per offload key (hardware).

Needs a running certus-server and a GPU; pytest does not collect it (no test_ prefix).

Stores random GPU blocks with blocks_per_chunk (BPC) GPU blocks per key, then loads
them into zeroed tensors as vLLM 0.30 would and compares every byte:
  aligned     whole chunks
  unaligned   loads starting 1..BPC-1 blocks into the first chunk (block_indices)
  short_tail  the last key holds fewer than BPC blocks
  ssd         flush + clear the memory tier, then aligned + unaligned loads
  pressure    store more keys than the memory tier holds, then load all
Each GPU block's pages start with its (key, block) stamp, so a block placed at the
wrong slot offset is caught even where random bytes repeat.

  roundtrip_chunk_hw.py --repo <checkout> [--bpc 4] [--layers 2] [--page 458752]
"""
from __future__ import annotations

import argparse
import importlib.util
import random
import sys
import time
from pathlib import Path


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--repo", required=True, type=Path)
    p.add_argument("--bpc", type=int, default=4, help="GPU blocks per key")
    p.add_argument("--layers", type=int, default=2, help="KV regions per GPU block")
    p.add_argument("--page", type=int, default=458752, help="bytes per region per GPU block")
    p.add_argument("--keys", type=int, default=64)
    p.add_argument("--pressure", type=int, default=0, help="keys stored in the pressure case (0: skip)")
    p.add_argument("--shm-path", default="/dev/shm/certus-shmq")
    p.add_argument("--cases", default="aligned,unaligned,short_tail,ssd,pressure",
                   help="comma-separated cases to run")
    p.add_argument("--no-compact", action="store_true",
                   help="force the per-region ops (no LookupBlocks/Chunks)")
    a = p.parse_args()

    sys.path[:0] = [str(a.repo / "certus-shmq-connector"), str(a.repo / "apps" / "python")]
    bench_path = a.repo / "benchmarks" / "certus-connector-bench" / "bench_connector_lifecycle.py"
    spec = importlib.util.spec_from_file_location("bench_connector_lifecycle", bench_path)
    bench = importlib.util.module_from_spec(spec)
    sys.modules["bench_connector_lifecycle"] = bench
    spec.loader.exec_module(bench)
    import torch
    from concurrent.futures import ThreadPoolExecutor
    from certus_shmq_connector.ring import OFF_OWNER, Ring
    from certus_shmq_connector.manager import ShmqCertusOffloadingManager
    from certus_shmq_connector.gpu import KvCacheIpc

    torch.cuda.set_device(0)
    bpc, block_bytes = a.bpc, a.layers * a.page
    ring = Ring(a.shm_path, ready_timeout=10.0, log=lambda m: None, claim_slot=0, claim_slots=1)
    for ch in range(ring._num_channels):
        if ring._rd_u32(ring._channel_base(ch) + OFF_OWNER) != 0:
            ring._wr_u32(ring._channel_base(ch) + OFF_OWNER, 0)
    ring._tls.channel = None
    manager = ShmqCertusOffloadingManager(ring, block_size_bytes=block_bytes * bpc, world_size=1)
    gpu_blocks = 1024

    def tensors(fill):
        out, keep = [], []
        for _ in range(a.layers):
            t = (torch.randint(0, 256, (gpu_blocks, a.page), dtype=torch.uint8, device="cuda:0")
                 if fill else torch.zeros((gpu_blocks, a.page), dtype=torch.uint8, device="cuda:0"))
            ptr = t.data_ptr()
            out.append(KvCacheIpc(handle_bytes=bench._get_cuda_ipc_handle(ptr), gpu_device_id=0,
                                  stride_bytes=a.page, base_delta=ptr - bench._alloc_base(ptr)))
            keep.append(t)
        return out, keep

    src_r, src = tensors(True)
    dst_r, dst = tensors(False)
    pool = ThreadPoolExecutor(max_workers=4)
    W = bench.worker_class()
    if a.no_compact:
        W._blocks_supported = False
        W._chunks_supported = False
    store_w = W(ring, src_r, block_bytes, pool, rank=0, world_size=1, blocks_per_chunk=bpc)
    load_w = W(ring, dst_r, block_bytes, pool, rank=0, world_size=1, blocks_per_chunk=bpc)
    if bpc > 1:
        load_w.allocate_scratch()  # as the connector does at startup

    class Spec:
        def __init__(self, ids, first):
            self.block_ids, self.group_sizes, self.block_indices = ids, [len(ids)], [first]

    job = [0]

    def run(worker, submit):
        job[0] += 1
        submit(job[0])
        while True:
            done = worker.get_finished()
            if done:
                for r in done:
                    if r.success is False:
                        raise RuntimeError(f"job {r.job_id} failed")
                return
            time.sleep(0.0002)

    def stamp(g, key_idx, j):
        v = torch.tensor(list((key_idx * 1000 + j).to_bytes(8, "little")), dtype=torch.uint8, device="cuda:0")
        for t in src:
            t[g, :8] = v

    seed = random.randrange(1 << 30)
    ok = True

    def case(name, n_keys, settle, tail_short=0, starts=(0,)):
        nonlocal ok
        keys = bench.make_content_keys(n_keys, seed=seed + len(name) * 7919 + n_keys)
        # GPU source block for (key i, sub-block j); scattered, like vLLM's free list.
        nblk = n_keys * bpc - tail_short
        ids = random.sample(range(gpu_blocks), min(nblk, gpu_blocks))
        ids = [ids[i % len(ids)] for i in range(nblk)]
        expect = {}
        # Keys per request: a load's GPU blocks (dst_ids 0..) must fit the tensors.
        step = max(1, min(16, gpu_blocks // bpc))
        for i in range(0, n_keys, step):
            ks = keys[i:i + step]
            # Stamp just before this batch's store: with more blocks than GPU slots a
            # slot is reused, and only the stamp written before its store is stored.
            for b in range(i * bpc, min((i + len(ks)) * bpc, nblk)):
                stamp(ids[b], b // bpc, b % bpc)
                expect[b] = [t[ids[b]].clone() for t in src] if nblk <= gpu_blocks else None
            torch.cuda.synchronize()
            manager.touch(ks)
            res = manager.prepare_store(ks)
            assert res is not None and list(res.keys_to_store) == list(ks), "store not fully reserved"
            blk = ids[i * bpc:(i + len(ks)) * bpc]
            run(store_w, lambda j: store_w.submit_store(j, Spec(blk, i * bpc), res.store_spec))
            manager.complete_store(ks, success=True)
        if settle:
            ring.flush_to_ssd()
            ring.clear_memory_tier()
        bad = checked = 0
        for start in starts:
            for t in dst:
                t.zero_()
            torch.cuda.synchronize()
            for i in range(0, n_keys, step):
                ks = keys[i:i + step]
                first = i * bpc + (start if i == 0 else 0)
                last = min((i + len(ks)) * bpc, nblk)
                want_blocks = list(range(first, last))
                assert all(manager.lookup(k) for k in ks), f"{name}: keys missing"
                manager.touch(ks)
                ls = manager.prepare_load(ks)
                dst_ids = list(range(len(want_blocks)))
                run(load_w, lambda j: load_w.submit_load(j, ls, Spec(dst_ids, first)))
                manager.complete_load(ks)
                torch.cuda.synchronize()
                for d, b in zip(dst_ids, want_blocks):
                    want = expect[b]
                    tag = torch.tensor(list(((b // bpc) * 1000 + b % bpc).to_bytes(8, "little")),
                                       dtype=torch.uint8, device="cuda:0")
                    for L in range(a.layers):
                        got = dst[L][d]
                        good = (torch.equal(got, want[L]) if want is not None else
                                torch.equal(got[:8], tag) and torch.equal(got[8:], src[L][ids[b], 8:]))
                        bad += not good
                    checked += 1
        status = "OK" if bad == 0 else "MISMATCH"
        ok &= bad == 0
        print(f"{name:10} {status}: {checked} GPU blocks checked over {n_keys} keys x {bpc}, "
              f"starts {list(starts)}, {bad} region pages differ", flush=True)
        bench._remove_keys(ring, keys, 1)

    run_cases = set(a.cases.split(","))
    if "aligned" in run_cases:
        case("aligned", a.keys, False)
    if "unaligned" in run_cases:
        case("unaligned", a.keys, False, starts=tuple(range(1, bpc)))
    if "short_tail" in run_cases:
        case("short_tail", a.keys, False, tail_short=bpc - 1)
    if "ssd" in run_cases:
        case("ssd", a.keys, True, starts=(0, bpc - 1))
    if a.pressure and "pressure" in run_cases:
        # A start is inside the first key, so 1 needs keys of 2+ blocks.
        case("pressure", a.pressure, False, starts=(0, 1) if bpc > 1 else (0,))
    pool.shutdown(wait=True)
    print("RESULT", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
