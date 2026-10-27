#!/usr/bin/env python3
"""Check knowledge/serving/*.yaml and print every derived value.

Fails (exit 1) on: a reference to an unknown model, deployment or file; a model value
the formulas need that is missing; an observation value without a basis; or a stored
derived value (`stored_*`) that disagrees with the one computed from its inputs.

Usage: python3 knowledge/serving/check.py [--quiet]
"""
from __future__ import annotations

import sys
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
DTYPE_BYTES = {"fp32": 4, "bf16": 2, "fp16": 2, "fp8": 1, "fp8_e4m3": 1, "fp8_e5m2": 1}
BASES = {"measured", "configured", "computed"}

errors: list[str] = []


def err(msg: str) -> None:
    errors.append(msg)


def load(name: str) -> dict:
    data = yaml.safe_load((HERE / name).read_text()) or {}
    if data.get("schema_version") != 1:
        err(f"{name}: schema_version must be 1")
    return data


def head_dim(m: dict) -> int | None:
    hd = m.get("head_dim")
    if isinstance(hd, dict):
        hd = hd.get("value")
    return hd


def kv_bytes_per_token_per_layer(mid: str, m: dict, kind: str, dtype_bytes: int) -> int | None:
    """KV bytes one token adds to one layer of the given kind."""
    if kind in ("mamba", "linear"):
        return 0
    if m.get("attention") == "mla":
        mla = m.get("mla") or {}
        r, rope = mla.get("kv_lora_rank"), mla.get("qk_rope_head_dim")
        if r is None or rope is None:
            err(f"model {mid}: MLA needs mla.kv_lora_rank and mla.qk_rope_head_dim")
            return None
        return (r + rope) * dtype_bytes
    kvh, hd = m.get("num_kv_heads"), head_dim(m)
    if kvh is None or hd is None:
        err(f"model {mid}: needs num_kv_heads and head_dim")
        return None
    return 2 * kvh * hd * dtype_bytes


def model_kv(mid: str, m: dict, dtype_bytes: int) -> tuple[int | None, int | None, int]:
    """(per-layer bytes for full-attention layers, per-token bytes over all KV layers, KV layers)."""
    kinds = m.get("layer_kinds") or [{"kind": "full", "count": m.get("num_layers")}]
    if sum(k.get("count") or 0 for k in kinds) != m.get("num_layers"):
        err(f"model {mid}: layer_kinds counts do not add up to num_layers")
    per_layer, total, kv_layers = None, 0, 0
    for k in kinds:
        b = kv_bytes_per_token_per_layer(mid, m, k.get("kind", "full"), dtype_bytes)
        if b is None:
            return None, None, 0
        if b:
            kv_layers += k["count"]
            per_layer = b if per_layer is None else per_layer
            total += b * k["count"]
    return per_layer, total, kv_layers


BENCH = REPO / "benchmarks" / "certus-connector-bench" / "bench_connector_lifecycle.py"


def bench_presets() -> dict:
    """COLD_PRESETS from the connector benchmark, read from its source (importing it
    needs CUDA). Returns {} if the benchmark or the literal is missing."""
    import ast
    if not BENCH.exists():
        return {}
    for node in ast.walk(ast.parse(BENCH.read_text())):
        if isinstance(node, ast.Assign) and any(
                isinstance(t, ast.Name) and t.id == "COLD_PRESETS" for t in node.targets):
            return ast.literal_eval(node.value)
    return {}


def check_bench_presets(obs_by_id: dict, derived_by_dep: dict) -> None:
    """Each benchmark preset must name an observation and match it: block bytes and
    pieces per block (via the observation's deployment) and blocks per load job."""
    for name, preset in bench_presets().items():
        oid = preset.get("observation")
        o = obs_by_id.get(oid)
        if o is None:
            err(f"benchmark preset {name!r}: observation {oid!r} not in observations.yaml")
            continue
        row = derived_by_dep.get(o.get("deployment"))
        if row:
            _, _, ppb, _, dv = row
            if preset.get("pieces_per_block") != ppb:
                err(f"benchmark preset {name!r}: pieces_per_block {preset.get('pieces_per_block')} "
                    f"!= {ppb} for {o.get('deployment')}")
            if preset.get("block_bytes") != dv["block_bytes"]:
                err(f"benchmark preset {name!r}: block_bytes {preset.get('block_bytes')} "
                    f"!= {dv['block_bytes']} for {o.get('deployment')}")
        bpj = ((o.get("values") or {}).get("blocks_per_load_job") or {}).get("value")
        req, nb = preset.get("requests"), preset.get("num_blocks")
        if bpj is not None and req and nb is not None and nb // req != bpj:
            err(f"benchmark preset {name!r}: num_blocks/requests {nb // req} != "
                f"observation's blocks_per_load_job {bpj}")


def main() -> int:
    quiet = "--quiet" in sys.argv
    models = load("models.yaml").get("models", {})
    deps = load("deployments.yaml").get("deployments", {})
    obs = load("observations.yaml").get("observations", [])
    obs_ids = {o.get("id") for o in obs}

    for mid, m in models.items():
        for f in ("hf_name", "revision", "source", "attention", "num_layers"):
            if m.get(f) in (None, ""):
                err(f"model {mid}: missing {f}")

    rows = []
    for did, d in deps.items():
        mid = d.get("model")
        if mid not in models:
            err(f"deployment {did}: unknown model {mid!r}")
            continue
        m = models[mid]
        dt = d.get("kv_dtype")
        if dt not in DTYPE_BYTES:
            err(f"deployment {did}: kv_dtype {dt!r} not one of {sorted(DTYPE_BYTES)}")
            continue
        tp = d.get("tensor_parallel") or 1
        obt = d.get("offload_block_tokens")
        if obt is None:
            err(f"deployment {did}: missing offload_block_tokens")
            continue
        per_layer, per_token, kv_layers = model_kv(mid, m, DTYPE_BYTES[dt])
        if per_token is None:
            continue
        lay = d.get("layout") or {}
        rule = lay.get("rule_source", "")
        if rule and not (REPO / rule.split()[0]).exists():
            err(f"deployment {did}: layout.rule_source file {rule.split()[0]} does not exist")
        ppb = (lay.get("pieces_per_block") or {})
        if ppb.get("basis") == "observed" and ppb.get("observation") not in obs_ids:
            err(f"deployment {did}: pieces_per_block cites unknown observation {ppb.get('observation')!r}")
        if ppb.get("basis") not in ("observed", "assumed"):
            err(f"deployment {did}: pieces_per_block.basis must be observed or assumed")
        block_bytes = obt * per_token // tp
        ppb_val = ppb.get("value")
        if isinstance(ppb_val, int) and ppb_val > 0:
            # One piece per tensor the block is spread over: per-layer for layer-outermost
            # layouts, the whole block for a packed (block-outermost) one.
            if block_bytes % ppb_val:
                err(f"deployment {did}: block bytes {block_bytes} not divisible by pieces_per_block {ppb_val}")
            piece_bytes = block_bytes // ppb_val
        else:
            piece_bytes = obt * per_layer // tp
        derived = {
            "kv_bytes_per_token": per_token,
            "layer_bytes": obt * per_layer // tp,
            "piece_bytes": piece_bytes,
            "block_bytes": block_bytes,
        }
        for key, val in derived.items():
            stored = lay.get(f"stored_{key}")
            if stored is not None and stored != val:
                err(f"deployment {did}: stored_{key} {stored} != computed {val}")
        if (obt * per_token) % tp:
            err(f"deployment {did}: block bytes not divisible by tensor_parallel {tp}")
        rows.append((did, kv_layers, ppb.get("value"), ppb.get("basis"), derived))

    derived_by_dep = {r[0]: r for r in rows}
    for o in obs:
        oid = o.get("id")
        if o.get("deployment") not in deps:
            err(f"observation {oid}: unknown deployment {o.get('deployment')!r}")
        # A measured layout value must agree with what its deployment derives.
        row = derived_by_dep.get(o.get("deployment"))
        vals = o.get("values") or {}
        if row:
            _, _, ppb_val, _, dv = row
            for key, want in (("regions_per_block", ppb_val), ("piece_bytes", dv["piece_bytes"]),
                              ("block_bytes", dv["block_bytes"])):
                got = (vals.get(key) or {}).get("value") if isinstance(vals.get(key), dict) else None
                if got is not None and want is not None and got != want:
                    err(f"observation {oid}: {key} {got} != deployment's {want}")
        for key, v in (o.get("values") or {}).items():
            if not isinstance(v, dict) or v.get("basis") not in BASES:
                err(f"observation {oid}: value {key} needs basis in {sorted(BASES)}")
            elif v["basis"] == "measured" and not v.get("evidence"):
                err(f"observation {oid}: measured value {key} needs evidence")
            elif v["basis"] == "computed" and not v.get("formula"):
                err(f"observation {oid}: computed value {key} needs formula")

    check_bench_presets({o.get("id"): o for o in obs}, derived_by_dep)

    if not quiet:
        w = max([len("deployment")] + [len(r[0]) for r in rows])
        print(f"{'deployment':{w}} {'KV layers':>9} {'pieces':>7} {'KV B/token':>11} "
              f"{'layer B':>9} {'piece B':>9} {'block B':>10}")
        for did, kvl, ppb, basis, dv in rows:
            print(f"{did:{w}} {kvl:9} {str(ppb) + ('' if basis == 'observed' else '?'):>7} "
                  f"{dv['kv_bytes_per_token']:11} {dv['layer_bytes']:9} {dv['piece_bytes']:9} "
                  f"{dv['block_bytes']:10}")
        print("(bytes per rank except 'KV B/token', which is the whole model at TP=1)")
        print("('?' = pieces_per_block assumed from the layout rule, not observed)")
    for e in errors:
        print("ERROR", e, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
