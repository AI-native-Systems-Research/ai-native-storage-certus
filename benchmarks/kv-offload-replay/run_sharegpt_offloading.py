#!/usr/bin/env python3
"""
run_sharegpt_offloading.py — drive vllm bench throughput on ShareGPT with
TracingOffloadingConnector + TracingCPUOffloadingSpec, capturing:

  1. KVConnector-level trace (offloading_trace_<pid>.jsonl)
  2. OffloadingManager-level trace (offloading_mgr_<pid>.jsonl)
     ← lookup / touch / prepare_load / complete_load / prepare_store /
       complete_store / take_events

Backend is vLLM's built-in CPUOffloadingSpec (CPU RAM is the offload tier).
No Certus IO — the focus here is the scheduler/manager policy layer, which is
where the 6 functions live in the vLLM architecture.

Usage:
    PYTHONPATH=/home/bdh/kvconn-trace python run_sharegpt_offloading.py \
        --num-conversations 200 --num-prompts 200

  --num-conversations N    filters sharegpt_v3.json to the first N valid
                           multi-turn conversations (written to a temp
                           subset file) before handing it to vLLM.
  --num-prompts M          how many prompts vLLM samples from that pool
                           (vLLM's own flag; passthrough). Omit to let
                           vLLM use its default.
"""

if __name__ == "__main__":
    import json
    import os
    import sys

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    SHAREGPT_JSON = os.path.join(_here, "sharegpt_v3.json")
    if not os.path.exists(SHAREGPT_JSON):
        print(f"[run] missing {SHAREGPT_JSON}", file=sys.stderr)
        sys.exit(1)

    # Pull --num-conversations out of argv before handing the rest to vLLM.
    extra = list(sys.argv[1:])
    num_convs: int | None = None
    i = 0
    while i < len(extra):
        a = extra[i]
        if a == "--num-conversations":
            num_convs = int(extra[i + 1])
            del extra[i:i + 2]
            continue
        if a.startswith("--num-conversations="):
            num_convs = int(a.split("=", 1)[1])
            del extra[i]
            continue
        i += 1

    dataset_path = SHAREGPT_JSON
    if num_convs is not None:
        print(f"[run] filtering ShareGPT to first {num_convs} valid conversations",
              file=sys.stderr)
        with open(SHAREGPT_JSON) as fh:
            full = json.load(fh)
        subset: list[dict] = []
        for entry in full:
            convs = entry.get("conversations") or []
            # vLLM's sharegpt loader expects alternating human/gpt turns;
            # keep conversations with ≥1 human→gpt pair.
            has_pair = False
            for j in range(len(convs) - 1):
                if (convs[j].get("from") == "human"
                        and convs[j + 1].get("from") == "gpt"):
                    has_pair = True
                    break
            if not has_pair:
                continue
            subset.append(entry)
            if len(subset) >= num_convs:
                break
        if len(subset) < num_convs:
            print(f"[run] warning: only {len(subset)} conversations met the "
                  f"human→gpt requirement (requested {num_convs})",
                  file=sys.stderr)
        subset_path = os.path.join(_here, f"sharegpt_subset_{num_convs}.json")
        with open(subset_path, "w") as fh:
            json.dump(subset, fh)
        dataset_path = subset_path
        print(f"[run] wrote {len(subset)} conversations → {subset_path}",
              file=sys.stderr)

    KV_CONFIG = {
        "kv_connector": "TracingOffloadingConnector",
        "kv_connector_module_path": "tracing_offloading_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            # 4 GiB of host RAM for the offload tier
            "cpu_bytes_to_use": 4 * (1 << 30),
            # Point OffloadingSpecFactory at our tracing spec
            "spec_name": "TracingCPUOffloadingSpec",
            "spec_module_path": "tracing_offloading_manager",
            # Offloaded block size = same as GPU block size (no multiplier)
            "eviction_policy": "lru",
        },
    }

    DEFAULTS = [
        "--model",                  "NousResearch/Meta-Llama-3-8B",
        "--dataset-name",           "sharegpt",
        "--dataset-path",           dataset_path,
        "--num-prompts",            "50",
        "--max-model-len",          "4096",
        "--max-num-seqs",           "64",
        "--gpu-memory-utilization", "0.90",
        "--dtype",                  "float16",
        "--disable-log-stats",
        "--kv-transfer-config",     json.dumps(KV_CONFIG),
        "--output-json",            os.path.join(_here, "sharegpt_offloading_results.json"),
    ]

    # Clear stale trace files
    for f in os.listdir(_here):
        if (f.startswith("offloading_trace_")
                or f.startswith("offloading_mgr_")
                or f.startswith("offloading_handler_")) \
                and f.endswith(".jsonl"):
            os.remove(os.path.join(_here, f))

    sys.argv = ["vllm", "bench", "throughput"] + DEFAULTS + extra
    print("[run] command:\n ", " ".join(sys.argv), "\n", file=sys.stderr)

    exit_code = 0
    try:
        from vllm.entrypoints.cli.main import main
        main()
    except SystemExit as e:
        exit_code = e.code if isinstance(e.code, int) else 1
    except Exception as e:
        print(f"[run] bench failed: {e}", file=sys.stderr)
        exit_code = 1
    finally:
        traces = sorted(
            f for f in os.listdir(_here)
            if (f.startswith("offloading_trace_")
                or f.startswith("offloading_mgr_")
                or f.startswith("offloading_handler_"))
            and f.endswith(".jsonl")
        )
        if traces:
            print("\n[run] trace files:", file=sys.stderr)
            for f in traces:
                p = os.path.join(_here, f)
                lines = sum(1 for _ in open(p))
                print(f"   {f}  ({lines} lines)", file=sys.stderr)

    sys.exit(exit_code)
