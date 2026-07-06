#!/usr/bin/env python3
"""run_multiturn_certus.py — multi-turn e2e benchmark with CertusOffloadingSpec.

Same workload as run_multiturn_offloading.py but using the Certus NVMe+DRAM
offloading backend instead of CPU offloading.
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    SUBSET_PATH = os.environ.get("DATASET_PATH",
                                   os.path.join(_here, "sharegpt_subset_5000.json"))
    if not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)

    NUM_CONVS = int(os.environ.get("NUM_CONVS", 500))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)

    # NVMe drives migrated to node 1 (GPU's NUMA node), bound to vfio-pci.
    # dram_cache_bytes draws from the 48 x 1G-hugepage pool (SPDK tier). The tier
    # must leave ~3 hugepages for DPDK's own EAL heap + per-drive DMA buffers, so
    # a single spdk_zmalloc maxes at ~44-45 GiB of a 48-page pool (48 GiB fails).
    # Requires DPDK RTE_MAX_MEM_MB_PER_LIST raised to 64G (single alloc > 32G).
    # Keep in sync with CERTUS_HUGEPAGES in configure-bench.sh.
    DRAM_CACHE_BYTES = int(os.environ.get("DRAM_CACHE_BYTES", 47244640256))  # 44 GiB
    # NVMe PCI addrs + NUMA node are env-overridable so the same script can bench
    # on either node. Defaults are node 1 (GPU's node, historical). To bench on
    # node 0 (everything-but-GPU on node 0, matching configure-bench.sh BENCH_NUMA=0):
    #   DATA_PCI_ADDRS=0000:61:00.0,0000:62:00.0,0000:63:00.0,0000:64:00.0
    #   METADATA_PCI_ADDR=0000:62:00.0  CERTUS_NUMA_NODE=0
    DATA_PCI_ADDRS = os.environ.get(
        "DATA_PCI_ADDRS",
        "0000:c1:00.0,0000:c2:00.0,0000:c3:00.0,0000:c4:00.0").split(",")
    METADATA_PCI_ADDR = os.environ.get("METADATA_PCI_ADDR", "0000:c2:00.0")
    CERTUS_NUMA_NODE = int(os.environ.get("CERTUS_NUMA_NODE", 1))
    print(f"[run] data_pci_addrs={DATA_PCI_ADDRS} metadata={METADATA_PCI_ADDR} "
          f"numa_node={CERTUS_NUMA_NODE}", file=sys.stderr)
    KV_CONFIG = {
        "kv_connector": "OffloadingConnector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "spec_name": "CertusOffloadingSpec",
            "spec_module_path": "certus_connector.spec",
            "data_pci_addrs": DATA_PCI_ADDRS,
            "metadata_pci_addr": METADATA_PCI_ADDR,
            "slab_size_bytes": 2097152,
            "dram_cache_bytes": DRAM_CACHE_BYTES,
            "io_queue_depth": 128,
            "numa_node": CERTUS_NUMA_NODE,
        },
    }

    def preflight():
        """Verify the box is configured for the Certus bench before we spend
        minutes loading a model into a mis-configured system. Fail fast and loud.
        Configure with: sudo ./tools/configure-bench.sh certus (then REBOOT for
        the 1G hugepage reservation). Skip with SKIP_PREFLIGHT=1.

        Mirrors the SharedStorage script's preflight, but checks the certus
        prerequisites: enough 1G hugepages on CERTUS_NUMA_NODE for the DRAM tier,
        every data drive bound to vfio-pci, and no stale claims (SPDK locks / a
        GPU-holding process) that would block SPDK startup."""
        if os.environ.get("SKIP_PREFLIGHT"):
            return
        errs = []

        # 1. Hugepages: the SPDK DRAM tier is a single spdk_zmalloc from the 1G
        #    pool on CERTUS_NUMA_NODE. DPDK keeps ~3 pages for its own EAL heap +
        #    per-drive DMA buffers, so we need dram_cache_bytes + overhead worth
        #    of pages, all resident on the bench node. Runtime allocation of 1G
        #    pages fails once RAM is fragmented — hence the reboot requirement.
        DPDK_OVERHEAD_PAGES = 3
        need_pages = -(-DRAM_CACHE_BYTES // (1024**3)) + DPDK_OVERHEAD_PAGES  # ceil
        node_hp_path = (f"/sys/devices/system/node/node{CERTUS_NUMA_NODE}"
                        f"/hugepages/hugepages-1048576kB/nr_hugepages")
        node_free_path = (f"/sys/devices/system/node/node{CERTUS_NUMA_NODE}"
                          f"/hugepages/hugepages-1048576kB/free_hugepages")
        try:
            with open(node_hp_path) as f:
                node_hp = int(f.read())
            with open(node_free_path) as f:
                node_free = int(f.read())
        except OSError:
            node_hp = node_free = 0
        if node_free < need_pages:
            errs.append(
                f"only {node_free} free 1G hugepages on node {CERTUS_NUMA_NODE} "
                f"({node_hp} total) — DRAM tier of {DRAM_CACHE_BYTES/1024**3:.0f} "
                f"GiB needs >= {need_pages}. Runtime alloc of 1G pages fails on a "
                f"fragmented box; set hugepages=48 boot param and REBOOT "
                f"(sudo ./tools/configure-bench.sh certus).")

        # 2. Every data drive (and the metadata drive) must be bound to vfio-pci
        #    for SPDK userspace access. If any is still on the nvme kernel driver
        #    (e.g. left in the SharedStorage RAID), SPDK can't claim it.
        want_vfio = set(DATA_PCI_ADDRS) | {METADATA_PCI_ADDR}
        for bdf in sorted(want_vfio):
            drv_link = f"/sys/bus/pci/devices/{bdf}/driver"
            drv = os.path.basename(os.readlink(drv_link)) if os.path.islink(drv_link) else "none"
            if drv != "vfio-pci":
                errs.append(f"{bdf} bound to '{drv}', need vfio-pci "
                            f"(run configure-bench.sh certus)")
            else:
                # NUMA sanity: the drive should live on the node we're pinning to.
                try:
                    with open(f"/sys/bus/pci/devices/{bdf}/numa_node") as f:
                        dev_numa = int(f.read())
                    if dev_numa != CERTUS_NUMA_NODE:
                        errs.append(f"{bdf} on NUMA node {dev_numa}, but "
                                    f"numa_node={CERTUS_NUMA_NODE} requested")
                except (OSError, ValueError):
                    pass

        # 3. Stale SPDK per-device lock files block the device claim (Permission
        #    denied) on the next startup if a prior run died without releasing.
        stale = [f"/var/tmp/spdk_pci_lock_{bdf}" for bdf in sorted(want_vfio)
                 if os.path.exists(f"/var/tmp/spdk_pci_lock_{bdf}")]
        if stale:
            errs.append(f"{len(stale)} stale SPDK lock file(s) — a dead run left "
                        f"them; remove with: sudo rm -f {' '.join(stale)}")

        if errs:
            sys.exit("[preflight] NOT ready for certus:\n  - " + "\n  - ".join(errs))
        print(f"[preflight] ok: {node_free} free 1G hugepages on node "
              f"{CERTUS_NUMA_NODE}, {len(want_vfio)} drives on vfio-pci, no stale "
              f"locks", file=sys.stderr, flush=True)

    preflight()

    with open(SUBSET_PATH) as f:
        all_data = json.load(f)
    convs = []
    for entry in all_data:
        if len(convs) >= NUM_CONVS:
            break
        turns = entry.get("conversations", [])
        human_turns = [t["value"] for t in turns if t.get("from") == "human"]
        if len(human_turns) >= 2:
            convs.append(human_turns)
    print(f"[run] loaded {len(convs)} conversations", file=sys.stderr)

    from vllm import LLM, SamplingParams

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        enforce_eager=True,
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=True,
    )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)
    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

    contexts = [""] * len(convs)
    alive = [True] * len(convs)
    next_turn = [0] * len(convs)

    rounds_done = 0
    total_generations = 0
    round_io = []  # (round, prompts, d_read_bytes, d_write_bytes, d_read_ops, d_write_ops)

    # --- Per-round SSD I/O accounting via the iostat file ---------------------
    # The CertusEngine with the real counters lives in the vLLM EngineCore worker
    # PROCESS, not here. A writer thread in that process publishes cumulative
    # (read_ops, read_bytes, read_latency_ns_sum, write_ops, write_bytes,
    # write_latency_ns_sum) to CERTUS_IOSTAT_FILE every 0.5s (requires
    # certus_native built --features rw-telemetry). We read that file around each
    # generate() for per-round deltas — the certus analogue of the SharedStorage
    # /sys/block capture.
    IOSTAT_FILE = os.environ.get("CERTUS_IOSTAT_FILE", "/tmp/certus_iostat.txt")

    def io_stats():
        # 6 fields: read_ops, read_bytes, read_lat_ns_sum, write_ops, write_bytes, write_lat_ns_sum
        try:
            with open(IOSTAT_FILE) as f:
                parts = f.read().split()
            if len(parts) >= 6:
                return tuple(int(x) for x in parts[:6])
        except (OSError, ValueError):
            pass
        return None

    def gib(n):
        return "n/a" if n is None else f"{n / (1024**3):.2f} GiB"

    def mean_us(lat_ns_delta, ops_delta):
        if lat_ns_delta is None or not ops_delta:
            return "n/a"
        return f"{lat_ns_delta / ops_delta / 1000:.1f}us"

    t_start = time.perf_counter()

    while True:
        active_idx = []
        active_prompts = []
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            human = conv[k]
            candidate = human if k == 0 else contexts[i] + "\n\n" + human
            if n_tokens(candidate) > PROMPT_BUDGET:
                alive[i] = False
                continue
            contexts[i] = candidate
            active_idx.append(i)
            active_prompts.append(candidate)

        if not active_prompts:
            break

        rounds_done += 1
        io0 = io_stats()
        round_start = time.perf_counter()
        outs = llm.generate(active_prompts, sp)
        round_elapsed = time.perf_counter() - round_start
        io1 = io_stats()
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)
        # Deltas: (read_ops, read_bytes, read_lat_ns, write_ops, write_bytes, write_lat_ns).
        if io0 is not None and io1 is not None:
            d_rops, d_rb, d_rlat, d_wops, d_wb, d_wlat = (io1[j] - io0[j] for j in range(6))
        else:
            d_rops = d_rb = d_rlat = d_wops = d_wb = d_wlat = None
        round_io.append((rounds_done, len(active_prompts),
                         d_rb, d_wb, d_rops, d_wops, d_rlat, d_wlat))
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
              f"ssd_read={gib(d_rb)} ssd_write={gib(d_wb)} "
              f"r_ops={d_rops} w_ops={d_wops} "
              f"r_lat={mean_us(d_rlat, d_rops)} w_lat={mean_us(d_wlat, d_wops)}",
              file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
    summary = {
        "elapsed_time": elapsed,
        "num_conversations": len(convs),
        "num_rounds": rounds_done,
        "total_generations": total_generations,
        "model": MODEL,
        "max_model_len": MAX_MODEL_LEN,
        "output_tokens": OUTPUT_TOKENS,
        "dram_cache_bytes": DRAM_CACHE_BYTES,
        "slab_size_bytes": 2097152,
    }
    with open(os.path.join(_here, "certus_multiturn_results.json"), "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
          f"rounds={rounds_done}", file=sys.stderr)

    # --- Per-round SSD I/O summary --------------------------------------------
    have_io = any(r[2] is not None for r in round_io)
    if have_io:
        tot_rb = sum(r[2] for r in round_io if r[2] is not None)
        tot_wb = sum(r[3] for r in round_io if r[3] is not None)
        tot_rops = sum(r[4] for r in round_io if r[4] is not None)
        tot_wops = sum(r[5] for r in round_io if r[5] is not None)
        tot_rlat = sum(r[6] for r in round_io if r[6] is not None)
        tot_wlat = sum(r[7] for r in round_io if r[7] is not None)
        print("\n[io] per-round SSD bytes + latency (certus engine, all drives):", file=sys.stderr)
        print(f"[io] {'round':>5} {'prompts':>7} {'ssd_read':>12} {'ssd_write':>12} "
              f"{'r_ops':>10} {'w_ops':>10} {'r_lat':>10} {'w_lat':>10}", file=sys.stderr)
        for rnd, npr, rb, wb, rops, wops, rlat, wlat in round_io:
            print(f"[io] {rnd:>5} {npr:>7} {gib(rb):>12} {gib(wb):>12} "
                  f"{rops:>10} {wops:>10} {mean_us(rlat, rops):>10} {mean_us(wlat, wops):>10}",
                  file=sys.stderr)
        print(f"[io] {'TOTAL':>5} {'':>7} {gib(tot_rb):>12} {gib(tot_wb):>12} "
              f"{tot_rops:>10} {tot_wops:>10} {mean_us(tot_rlat, tot_rops):>10} "
              f"{mean_us(tot_wlat, tot_wops):>10}", file=sys.stderr)
        io_path = os.path.join(_here, f"certus_round_io_{int(elapsed)}.json")
        with open(io_path, "w") as f:
            json.dump({"wall": elapsed, "rounds": [
                {"round": r, "prompts": n, "read_bytes": rb, "write_bytes": wb,
                 "read_ops": rops, "write_ops": wops,
                 "read_latency_ns_sum": rlat, "write_latency_ns_sum": wlat}
                for r, n, rb, wb, rops, wops, rlat, wlat in round_io],
                "total_read_bytes": tot_rb, "total_write_bytes": tot_wb,
                "total_read_ops": tot_rops, "total_write_ops": tot_wops,
                "total_read_latency_ns_sum": tot_rlat,
                "total_write_latency_ns_sum": tot_wlat}, f, indent=2)
        print(f"[io] saved {io_path}", file=sys.stderr)
