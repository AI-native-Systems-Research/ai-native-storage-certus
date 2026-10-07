#!/usr/bin/env python3
"""Closed-loop multi-turn chat driver for an llm-d deployment.

Drives many concurrent chat sessions at an OpenAI-compatible endpoint and
attributes the resulting KV-offload traffic to each GPU and each certus
instance.

Why this exists rather than the harnesses under benchmarks/kv-offload-replay:
those construct `vllm.LLM(...)` in-process, so they cannot exercise llm-d's
router at all. The admission model here is lifted from run_multiturn_async.py:
`--active-sessions N` is a CLOSED loop that keeps exactly N conversations in
flight and admits a new one only as a running one retires, rather than dumping
every conversation in at once.

Each session sends a unique preamble, so sessions do not share a prefix with
each other, and replays the whole accumulated history every turn, so a
session's prefix GROWS per turn -- turn N reloads turns 1..N-1. That growth is
what drives eviction and therefore offload; a fixed-size shared prefix (what
guidellm's synthetic generator produces) does not.

Attribution is by metric delta, scraped straight from each decode pod and each
certus instance, because the response carries no header naming the endpoint
that served it. Per-pod request counts therefore show how the router spread
the load; per-instance certus counters show what reached the offload tier.

Stdlib only -- aiohttp/httpx/openai are not installed on the target host. A
thread per active session gives the closed-loop semantics directly: each
worker owns one conversation from first turn to last.

Usage:
    ./multiturn_http.py --sessions 128 --active-sessions 48 --turns 6
    ./multiturn_http.py --dry-run          # resolve, calibrate, scrape, exit
"""

from __future__ import annotations

import argparse
import json
import random
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

# Counters worth reporting per decode pod. external_prefix_cache_* is the
# clearest "did this request reach certus" signal.
#
# NO store-side counter is listed, because vLLM 0.30 does not populate any of
# them with this connector: kv_offload_total_bytes_total, kv_offload_size_sum
# and kv_offload_size_count all read 0.0 while load_bytes_total is correct and
# ~22 GiB per instance is demonstrably resident. Stored bytes are therefore
# reported from the certus side, as the delta of certus_memory_tier_used_bytes,
# which does track. Do not re-add one of those three without checking it moves.
VLLM_COUNTERS = [
    "vllm:request_success_total",
    "vllm:prompt_tokens_total",
    "vllm:generation_tokens_total",
    "vllm:prefix_cache_hits_total",
    "vllm:prefix_cache_queries_total",
    "vllm:external_prefix_cache_hits_total",
    "vllm:external_prefix_cache_queries_total",
    "vllm:kv_offload_load_bytes_total",
    "vllm:kv_offload_load_size_count",
    "vllm:kv_offload_lookup_sync_delay_seconds_count",
]

CERTUS_COUNTERS = [
    "certus_populates_total",
    "certus_lookup_hits_total",
    "certus_lookup_hits_dram_total",
    "certus_lookup_hits_ssd_total",
    "certus_lookup_misses_total",
    "certus_remote_lookup_hits_total",
    "certus_remote_lookup_misses_total",
    "certus_evictions_total",
    "certus_store_backpressure_events_total",
    "certus_store_drops_on_full_total",
    "certus_memory_tier_used_bytes",
    "certus_memory_tier_free_bytes",
    "certus_evictions_blocked_by_pin_total",
    "certus_evictions_blocked_unpersisted_total",
]


def kubectl(*args: str) -> str:
    return subprocess.run(
        ["kubectl", *args], capture_output=True, text=True, check=True
    ).stdout.strip()


def scrape(url: str, names: list[str], timeout: float = 5.0) -> dict[str, float]:
    """Sum each counter across all label sets at a /metrics endpoint."""
    try:
        body = urllib.request.urlopen(url, timeout=timeout).read().decode()
    except Exception:
        return {}
    out: dict[str, float] = {}
    wanted = set(names)
    for line in body.splitlines():
        if not line or line[0] == "#":
            continue
        name = line.split("{", 1)[0].split(" ", 1)[0]
        if name in wanted:
            try:
                out[name] = out.get(name, 0.0) + float(line.rsplit(" ", 1)[1])
            except (ValueError, IndexError):
                pass
    return out


@dataclass
class Target:
    """A decode pod or a certus instance we scrape."""
    label: str
    node: str
    url: str
    before: dict[str, float] = field(default_factory=dict)
    mid: dict[str, float] = field(default_factory=dict)
    after: dict[str, float] = field(default_factory=dict)

    def delta(self, name: str) -> float:
        return self.after.get(name, 0.0) - self.before.get(name, 0.0)

    def delta_phase2(self, name: str) -> float:
        """Phase-2 only. Needs `mid`; falls back to the whole run without it."""
        base = self.mid or self.before
        return self.after.get(name, 0.0) - base.get(name, 0.0)


def discover(ns: str, certus_ns: str, certus_ports: list[int]):
    """Find the endpoint, the decode pods, and the certus instances."""
    base = None
    ip = kubectl("get", "svc", "quickstart-epp", "-n", ns,
                 "-o", "jsonpath={.spec.clusterIP}")
    if ip:
        base = f"http://{ip}"

    pods = []
    raw = kubectl(
        "get", "pods", "-n", ns, "-l", "llm-d.ai/role=decode",
        "-o", "jsonpath={range .items[*]}{.metadata.name} {.status.podIP} "
        "{.spec.nodeName} {.status.containerStatuses[0].ready}{\"\\n\"}{end}",
    )
    for line in raw.splitlines():
        parts = line.split()
        if len(parts) == 4 and parts[3] == "true":
            pods.append(Target(parts[0], parts[2], f"http://{parts[1]}:8000/metrics"))

    certus = []
    raw = kubectl(
        "get", "pods", "-n", certus_ns,
        "-o", "jsonpath={range .items[*]}{.metadata.name} {.spec.nodeName}"
        "{\"\\n\"}{end}",
    )
    seen_nodes = []
    for line in raw.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1] not in seen_nodes:
            seen_nodes.append(parts[1])
    # certus runs hostNetwork, so each instance is reachable on its node at a
    # per-NUMA port rather than on a pod IP.
    for node in seen_nodes:
        for i, port in enumerate(certus_ports):
            certus.append(Target(f"{node} numa{i}", node,
                                 f"http://{node}:{port}/metrics"))
    return base, pods, certus


def post_json(url: str, payload: dict, timeout: float):
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, json.load(r)


def count_tokens(base: str, model: str, text: str, timeout: float) -> int:
    status, body = post_json(f"{base}/tokenize",
                             {"model": model, "prompt": text}, timeout)
    return int(body.get("count", 0))


WORDS = ("alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo "
         "lima mike november oscar papa quebec romeo sierra tango uniform "
         "victor whiskey xray yankee zulu").split()


def filler(n_words: int, rng: random.Random) -> str:
    return " ".join(rng.choice(WORDS) for _ in range(n_words))


def calibrate(base: str, model: str, target_tokens: int, timeout: float,
              rng: random.Random) -> int:
    """Words needed to hit target_tokens, measured with the server's tokenizer.

    Approximating tokens from characters would make the sizing arithmetic
    (working set vs GPU KV vs DRAM tier) wrong by whatever the tokenizer's
    actual ratio is, so ask the server.
    """
    if target_tokens <= 0:
        return 0
    words = max(1, target_tokens // 2)
    for _ in range(8):
        got = count_tokens(base, model, filler(words, rng), timeout)
        if got == 0:
            return words
        if abs(got - target_tokens) <= max(2, target_tokens * 0.02):
            return words
        words = max(1, int(words * target_tokens / got))
    return words


@dataclass
class TurnResult:
    session: int
    turn: int
    latency: float
    ok: bool
    prompt_tokens: int = 0
    error: str = ""
    phase: int = 1


def run_session(idx: int, args, base: str, model: str, sys_words: int,
                turn_words: int, results: list, lock: threading.Lock,
                stop: threading.Event, histories: dict | None = None,
                resume: list | None = None, phase: int = 1) -> None:
    """Run one conversation. With `resume`, continue an earlier session's
    history instead of starting a new one, which is what makes the revisit
    phase re-request blocks the tier has since demoted to SSD."""
    rng = random.Random(args.seed + idx)
    # A per-session preamble keeps sessions from sharing a prefix with one
    # another, so the only reuse is within a session, across its turns.
    if resume is not None:
        messages = list(resume)
        turns = 1
    else:
        preamble = f"Session {idx} reference notes. " + filler(sys_words, rng)
        messages = [{"role": "system", "content": preamble}]
        turns = args.turns
    for turn in range(1, turns + 1):
        if stop.is_set():
            return
        messages.append({"role": "user",
                         "content": f"[turn {turn}] {filler(turn_words, rng)} "
                                    f"Summarise in one sentence."})
        payload = {"model": model, "messages": messages,
                   "max_tokens": args.output_tokens, "temperature": 0.0,
                   "stream": False}
        t0 = time.monotonic()
        try:
            _, body = post_json(f"{base}/v1/chat/completions", payload,
                                args.timeout)
            dt = time.monotonic() - t0
            reply = body["choices"][0]["message"].get("content") or ""
            usage = body.get("usage") or {}
            messages.append({"role": "assistant", "content": reply})
            with lock:
                results.append(TurnResult(idx, turn, dt, True,
                                          int(usage.get("prompt_tokens", 0)),
                                          phase=phase))
        except urllib.error.HTTPError as e:
            dt = time.monotonic() - t0
            detail = ""
            try:
                detail = e.read().decode()[:160]
            except Exception:
                pass
            with lock:
                results.append(TurnResult(idx, turn, dt, False,
                                          error=f"HTTP {e.code} {detail}",
                                          phase=phase))
            return
        except Exception as e:
            dt = time.monotonic() - t0
            with lock:
                results.append(TurnResult(idx, turn, dt, False,
                                          error=f"{type(e).__name__}: {e}",
                                          phase=phase))
            return
    if histories is not None:
        with lock:
            histories[idx] = messages


def pct(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    s = sorted(values)
    k = min(len(s) - 1, int(round((p / 100.0) * (len(s) - 1))))
    return s[k]


def gib(n: float) -> str:
    return f"{n / (1 << 30):.2f} GiB"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base-url", help="default: quickstart-epp ClusterIP")
    ap.add_argument("--namespace", default="llm-d-quickstart")
    ap.add_argument("--certus-namespace", default="certus")
    ap.add_argument("--certus-ports", default="9400,9401",
                    help="one metrics port per NUMA instance")
    ap.add_argument("--model", help="default: first id from /v1/models")
    ap.add_argument("--sessions", type=int, default=128,
                    help="total conversations")
    ap.add_argument("--active-sessions", type=int, default=48,
                    help="conversations in flight (closed loop)")
    ap.add_argument("--turns", type=int, default=6, help="turns per session")
    ap.add_argument("--system-tokens", type=int, default=1024,
                    help="per-session unique preamble")
    ap.add_argument("--turn-tokens", type=int, default=256,
                    help="per-turn user message")
    ap.add_argument("--output-tokens", type=int, default=128)
    ap.add_argument("--timeout", type=float, default=600.0)
    ap.add_argument("--seed", type=int, default=1234)
    ap.add_argument("--revisit", type=int, default=0, metavar="N",
                    help="after the main phase, send one more turn to the N "
                         "earliest sessions. By then later traffic has pushed "
                         "their blocks out of the DRAM tier, so those turns "
                         "read from SSD -- which a single pass cannot do, "
                         "because it never revisits retired content.")
    ap.add_argument("--json-output")
    ap.add_argument("--dry-run", action="store_true",
                    help="resolve, calibrate and scrape, then exit")
    args = ap.parse_args()

    ports = [int(p) for p in args.certus_ports.split(",") if p.strip()]
    base, pods, certus = discover(args.namespace, args.certus_namespace, ports)
    base = args.base_url or base
    if not base:
        print("could not resolve an endpoint; pass --base-url", file=sys.stderr)
        return 2

    model = args.model
    if not model:
        try:
            r = urllib.request.urlopen(f"{base}/v1/models", timeout=15)
            model = json.load(r)["data"][0]["id"]
        except Exception as e:
            print(f"could not read /v1/models from {base}: {e}", file=sys.stderr)
            return 2

    print(f"endpoint   {base}")
    print(f"model      {model}")
    print(f"decode     {len(pods)} ready pod(s)")
    for p in pods:
        print(f"             {p.label}  on {p.node}")
    print(f"certus     {len(certus)} instance(s)")

    rng = random.Random(args.seed)
    sys_words = calibrate(base, model, args.system_tokens, args.timeout, rng)
    turn_words = calibrate(base, model, args.turn_tokens, args.timeout, rng)
    print(f"calibrated preamble~{args.system_tokens}tok={sys_words}w  "
          f"turn~{args.turn_tokens}tok={turn_words}w")

    # Working set vs the boundaries that decide whether anything spills.
    first = args.system_tokens + args.turn_tokens + args.output_tokens
    per_turn = args.turn_tokens + args.output_tokens
    final_ctx = first + (args.turns - 1) * per_turn
    peak = args.active_sessions * final_ctx
    print(f"sizing     final context/session ~{final_ctx:,} tok; "
          f"{args.active_sessions} active -> ~{peak:,} tok in flight")
    print(f"           at 144 KiB/tok that is ~{gib(peak * 147456)}; "
          f"GPU KV is ~37,184 tok (5.11 GiB) per pod")

    for t in pods:
        t.before = scrape(t.url, VLLM_COUNTERS)
    for t in certus:
        t.before = scrape(t.url, CERTUS_COUNTERS)

    if args.dry_run:
        print("\ndry run: no traffic sent")
        for t in certus:
            print(f"  {t.label:28} used={gib(t.before.get('certus_memory_tier_used_bytes', 0))}"
                  f"  populates={t.before.get('certus_populates_total', 0):.0f}")
        return 0

    results: list[TurnResult] = []
    histories: dict[int, list] = {}
    lock = threading.Lock()
    stop = threading.Event()
    print(f"\nrunning {args.sessions} sessions x {args.turns} turns, "
          f"{args.active_sessions} in flight ...")
    t_start = time.monotonic()
    try:
        with ThreadPoolExecutor(max_workers=args.active_sessions) as ex:
            futs = [ex.submit(run_session, i, args, base, model, sys_words,
                              turn_words, results, lock, stop,
                              histories if i < args.revisit else None)
                    for i in range(args.sessions)]
            for f in futs:
                f.result()
    except KeyboardInterrupt:
        stop.set()
        print("interrupted; reporting what completed")
    elapsed = time.monotonic() - t_start

    # Phase 2. Snapshot first so the revisit's counters can be read on their
    # own -- SSD hits are a rounding error against phase 1's DRAM hits and
    # would be invisible in a whole-run delta.
    revisited = 0
    if args.revisit and histories and not stop.is_set():
        for tg in pods:
            tg.mid = scrape(tg.url, VLLM_COUNTERS)
        for tg in certus:
            tg.mid = scrape(tg.url, CERTUS_COUNTERS)
        idxs = sorted(histories)
        print(f"\nrevisiting {len(idxs)} session(s) whose blocks phase 1 "
              f"has since demoted ...")
        t2 = time.monotonic()
        with ThreadPoolExecutor(max_workers=min(args.active_sessions,
                                                len(idxs))) as ex:
            futs = [ex.submit(run_session, i, args, base, model, sys_words,
                              turn_words, results, lock, stop, None,
                              histories[i], 2) for i in idxs]
            for f in futs:
                f.result()
        revisited = len(idxs)
        print(f"revisit phase: {time.monotonic() - t2:.1f}s")

    for t in pods:
        t.after = scrape(t.url, VLLM_COUNTERS)
    for t in certus:
        t.after = scrape(t.url, CERTUS_COUNTERS)

    ok = [r for r in results if r.ok]
    bad = [r for r in results if not r.ok]
    lat = [r.latency for r in ok]
    print(f"\n=== {len(ok)} turns ok, {len(bad)} failed, {elapsed:.1f}s "
          f"({len(ok) / elapsed:.2f} turns/s) ===")
    if lat:
        print(f"latency  p50 {pct(lat,50):.2f}s  p90 {pct(lat,90):.2f}s  "
              f"p99 {pct(lat,99):.2f}s  max {max(lat):.2f}s")
    by_turn: dict[int, list[float]] = {}
    for r in ok:
        by_turn.setdefault(r.turn, []).append(r.latency)
    for turn in sorted(by_turn):
        v = by_turn[turn]
        print(f"  turn {turn}: n={len(v):4d}  p50 {pct(v,50):6.2f}s  "
              f"mean {statistics.fmean(v):6.2f}s")
    if bad:
        seen: dict[str, int] = {}
        for r in bad:
            key = r.error.split("\n")[0][:110]
            seen[key] = seen.get(key, 0) + 1
        print("failures:")
        for k, n in sorted(seen.items(), key=lambda kv: -kv[1])[:6]:
            print(f"  {n:4d} x {k}")

    print("\n=== per GPU (decode pod) ===")
    tot_req = sum(t.delta("vllm:request_success_total") for t in pods) or 1.0
    for t in pods:
        req = t.delta("vllm:request_success_total")
        print(f"  {t.label}")
        print(f"    node={t.node}  requests={req:.0f} ({100*req/tot_req:4.1f}%)")
        print(f"    gpu prefix hits  {t.delta('vllm:prefix_cache_hits_total'):>14,.0f}"
              f" / queries {t.delta('vllm:prefix_cache_queries_total'):>14,.0f}")
        print(f"    certus hits      {t.delta('vllm:external_prefix_cache_hits_total'):>14,.0f}"
              f" / queries {t.delta('vllm:external_prefix_cache_queries_total'):>14,.0f}")
        print(f"    offload loaded   {gib(t.delta('vllm:kv_offload_load_bytes_total')):>14}"
              f"   loads {t.delta('vllm:kv_offload_load_size_count'):.0f}"
              f"   (stores are not instrumented vLLM-side; see certus below)")

    print("\n=== per certus instance ===")
    for t in certus:
        if not t.after:
            print(f"  {t.label:28} unreachable")
            continue
        print(f"  {t.label}")
        used = t.after.get("certus_memory_tier_used_bytes", 0.0)
        free = t.after.get("certus_memory_tier_free_bytes", 0.0)
        cap = used + free
        grew = t.delta("certus_memory_tier_used_bytes")
        pctfull = (100.0 * used / cap) if cap else 0.0
        # used is a gauge over the DRAM tier only (used+free == --memory-tier-size)
        # and does NOT reset between runs, so its delta is this run's net store.
        # SSD is written through, so a low number here does not mean an empty drive.
        print(f"    dram tier {gib(used)} of {gib(cap)} ({pctfull:.0f}% full),"
              f" +{gib(grew)} this run")
        print(f"    evictions {t.delta('certus_evictions_total'):>10,.0f}"
              f"   blocked-by-pin {t.delta('certus_evictions_blocked_by_pin_total'):>8,.0f}"
              f"   blocked-unpersisted {t.delta('certus_evictions_blocked_unpersisted_total'):>8,.0f}")
        pops = t.delta("certus_populates_total")
        if pops:
            print(f"    populates {pops:>10,.0f}")
        print(f"    lookup hits {t.delta('certus_lookup_hits_total'):>8,.0f}"
              f" (dram {t.delta('certus_lookup_hits_dram_total'):,.0f},"
              f" ssd {t.delta('certus_lookup_hits_ssd_total'):,.0f})"
              f"   misses {t.delta('certus_lookup_misses_total'):,.0f}")
        print(f"    remote hits {t.delta('certus_remote_lookup_hits_total'):>8,.0f}"
              f"   remote misses {t.delta('certus_remote_lookup_misses_total'):,.0f}")
        bp = t.delta("certus_store_backpressure_events_total")
        dr = t.delta("certus_store_drops_on_full_total")
        if bp or dr:
            print(f"    backpressure {bp:,.0f}   drops-on-full {dr:,.0f}")

    if revisited:
        p2 = [r for r in results if r.phase == 2 and r.ok]
        p2lat = [r.latency for r in p2]
        print(f"\n=== revisit phase: {len(p2)} turns over {revisited} session(s) ===")
        if p2lat:
            print(f"latency  p50 {pct(p2lat,50):.2f}s  p90 {pct(p2lat,90):.2f}s")
        print("  per certus instance, THIS PHASE ONLY:")
        for tg in certus:
            if not tg.after:
                continue
            dram = tg.delta_phase2("certus_lookup_hits_dram_total")
            ssd = tg.delta_phase2("certus_lookup_hits_ssd_total")
            rem = tg.delta_phase2("certus_remote_lookup_hits_total")
            tot = dram + ssd + rem
            if not tot:
                continue
            print(f"    {tg.label}")
            print(f"      dram {dram:>8,.0f} ({100*dram/tot:4.1f}%)"
                  f"   ssd {ssd:>8,.0f} ({100*ssd/tot:4.1f}%)"
                  f"   remote {rem:>6,.0f} ({100*rem/tot:4.1f}%)")
        p2load = sum(tg.delta_phase2("vllm:kv_offload_load_bytes_total")
                     for tg in pods)
        print(f"  loaded this phase: {gib(p2load)}")

    if args.json_output:
        blob = {
            "config": vars(args), "model": model, "endpoint": base,
            "elapsed_s": elapsed,
            "turns_ok": len(ok), "turns_failed": len(bad),
            "revisited_sessions": revisited,
            "phase2": [{"label": tg.label,
                        "delta": {k: tg.delta_phase2(k) for k in CERTUS_COUNTERS}}
                       for tg in certus] if revisited else [],
            "latency": {"p50": pct(lat, 50), "p90": pct(lat, 90),
                        "p99": pct(lat, 99)} if lat else {},
            "pods": [{"label": t.label, "node": t.node,
                      "delta": {k: t.delta(k) for k in VLLM_COUNTERS}}
                     for t in pods],
            "certus": [{"label": t.label,
                        "delta": {k: t.delta(k) for k in CERTUS_COUNTERS},
                        "after": t.after} for t in certus],
        }
        with open(args.json_output, "w") as fh:
            json.dump(blob, fh, indent=2)
        print(f"\nwrote {args.json_output}")
    return 0 if not bad else 1


if __name__ == "__main__":
    sys.exit(main())
