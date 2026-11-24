"""Write report/kv-offload-comparison.html from report/data2.json: a self-contained page
(data, styles and charts inline; only the fonts come from Google Fonts)."""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))
D = json.load(open(os.path.join(HERE, "data2.json")))

# Display names. Each says what moves KV where, so a reader never has to decode
# an arm number. Order is the order of the story.
CFG = [
    ("pd_certus_fetch", "P/D · NIXL + Certus, with fetch",
     "Split prefill/decode. NIXL hands each turn's KV to decode; Certus stores it (DRAM + SSD) and prefill fetches a session's earlier answers from decode's Certus over RDMA."),
    ("pd_certus_nofetch", "P/D · NIXL + Certus, no fetch",
     "The same, with fetch turned off: prefill reads only its own Certus and recomputes each earlier answer."),
    ("pd_vllm_cpu_fs", "P/D · NIXL + vLLM CPU + filesystem",
     "vLLM's documented multi-tier setup: a CPU tier spilling to a filesystem tier (RAID 0 + XFS on the same NVMe drives). No fetch between pods."),
    ("pd_vllm_cpu_p2p", "P/D · NIXL + vLLM CPU + P2P",
     "llm-d's documented P2P setup: a CPU tier, and prefill pulls earlier answers from decode's CPU tier. No disk tier."),
    ("pd_vllm_p2p_fs_x", "P/D · NIXL + vLLM CPU + P2P + filesystem (experimental)",
     "Not a supported configuration. vLLM's P2P and filesystem tiers together, with its KV-cache events changed so the router sees disk-resident blocks as Certus's are seen. Built to compare the fetch paths on equal routing terms."),
    ("agg_certus_nofetch", "Aggregated · Certus, no fetch",
     "No P/D split: each of the four GPUs does both prompt and generation, with a NUMA-local Certus. A session that moves to another GPU recomputes its history."),
    ("agg_certus_fetch", "Aggregated · Certus, with fetch",
     "The same, but when the router moves a session, the new GPU fetches its history from the old GPU's Certus."),
]
NAME = {k: n for k, n, _ in CFG}
C = D["configs"]
P = D["probe"]

def s(x, n=2): return f"{x:.{n}f} s"
def pct(x): return f"{x * 100:.1f}%"
def gb(x): return f"{x:,.0f} GB"

# ---------- tables ----------
def overview_rows():
    out = []
    for k, n, _ in CFG:
        c = C[k]
        out.append(f"""<tr><td class="cfg">{n}</td><td class="r">{s(c['p50'])}</td><td class="r">{s(c['p90'])}</td>
<td class="r">{pct(c['cached'])}</td><td class="r">{c['pulls']}</td><td class="r">{c['tok_s']:.0f}</td><td class="r">{c['ok']} / 320</td></tr>""")
    return "\n".join(out)

def bw_rows():
    out = []
    for k, n, _ in CFG:
        c = C[k]
        src = "Certus (DRAM + SSD)" if "certus" in k else ("none" if k == "pd_vllm_cpu_p2p" else "vLLM CPU tier, filled from disk")
        out.append(f"""<tr><td class="cfg">{n}</td><td class="r">{gb(c['local_gb'])}</td><td class="r">{gb(c['peer_gb'])}</td>
<td class="r">{gb(c['recompute_gb'])}</td><td class="r">{c['gpu_load_gbs']:.1f} GB/s</td><td>{src}</td></tr>""")
    return "\n".join(out)

R = D["repeats"]
def rep_mean(v, key): xs = [r[key] for r in R if r["v"] == v]; return sum(xs) / len(xs)

def rep_rows():
    rows = [("Mean time to first token, turns under 5 s", "tm5", "s"), ("Turns taking 2–5 s (moved, recomputing)", "band25", "n"),
            ("Median time to first token", "med", "s"), ("Turns over 10 s (queued)", "over10", "n"), ("Mean time to first token, all turns", "mean", "s")]
    out = []
    for label, k, u in rows:
        x = D["repcmp"][k]
        f = (lambda v, sg=False: f"{'+' if sg and v > 0 else ''}{v:.3f} s") if u == "s" else (lambda v, sg=False: f"{'+' if sg and v > 0 else ''}{v:.1f}")
        sig = x["lo"] > 0 or x["hi"] < 0
        yes = "yes" if x["p"] <= 0.03 else "no"
        out.append(f'<tr><td>{label}</td><td class="r">{f(x["nofetch"])}</td><td class="r">{f(x["fetch"])}</td>'
                   f'<td class="r">{"<b>" if sig else ""}{f(x["d"], True)}{"</b>" if sig else ""}<br><span class="ci">{f(x["lo"], True)} to {f(x["hi"], True)}</span></td><td class="r">{yes}</td></tr>')
    return "\n".join(out)

def cmp(a, b):
    x = D["cmp"][f"{a}|{b}"]
    return x["d"], x["lo"], x["hi"]

def stat_line(a, b, note=""):
    d, lo, hi = cmp(a, b)
    sig = lo > 0 or hi < 0
    ca, cb = C[a]["p50"], C[b]["p50"]
    faster = NAME[a] if d < 0 else NAME[b]
    ratio = (cb / ca) if d < 0 else (ca / cb)
    verdict = f"<b>{faster.replace(' (experimental)', '')}</b> is {ratio:.2f}× faster" if sig else "<b>no significant difference</b>"
    return f"""<div class="stat"><span>Median TTFT <span class="num">{s(ca)}</span> vs <span class="num">{s(cb)}</span></span>
<span>Difference <span class="num">{d:+.2f} s</span> (95% CI <span class="num">{lo:+.2f} to {hi:+.2f} s</span>)</span><span>{verdict}{note}</span></div>"""

data_js = json.dumps({"C": {k: C[k] for k, _, _ in CFG}, "NAME": NAME, "R": R}, separators=(",", ":"))

html = f"""<title>KV Offload Comparison</title>
<meta name="claude:viewport-fit-sides" content="cover">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans+Condensed:wght@500;600&family=IBM+Plex+Sans:ital,wght@0,400;0,500;0,600;1,400&family=IBM+Plex+Mono:wght@400;500&display=swap">
<style>
:root {{
  --bg: #f7f8f9; --panel: #ffffff; --ink: #14171b; --ink-2: #4a5059; --ink-3: #6b727d;
  --rule: #dde1e6; --rule-2: #eceff2; --accent: #1f5f8b;
  --s1: #2a78d6; --s2: #eb6834; --s3: #1baf7a; --s4: #eda100; --s5: #e87ba4;
  --flag-bg: #fff6e5; --flag-ink: #6b4700; --key-bg: #eef4fa;
  --display: "IBM Plex Sans Condensed", "Arial Narrow", system-ui, sans-serif;
  --body: "IBM Plex Sans", system-ui, -apple-system, "Segoe UI", sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, "SFMono-Regular", Menlo, monospace;
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    --bg: #121519; --panel: #191d22; --ink: #eceef1; --ink-2: #b4bac3; --ink-3: #8f96a0;
    --rule: #2c323a; --rule-2: #222830; --accent: #7fb6e0;
    --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500; --s5: #d55181;
    --flag-bg: #2e2617; --flag-ink: #f0d39a; --key-bg: #1b2530;
    color-scheme: dark;
  }}
}}
:root[data-theme="dark"] {{
  --bg: #121519; --panel: #191d22; --ink: #eceef1; --ink-2: #b4bac3; --ink-3: #8f96a0;
  --rule: #2c323a; --rule-2: #222830; --accent: #7fb6e0;
  --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500; --s5: #d55181;
  --flag-bg: #2e2617; --flag-ink: #f0d39a; --key-bg: #1b2530;
  color-scheme: dark;
}}
* {{ box-sizing: border-box; }}
body {{ background: var(--bg); color: var(--ink); font-family: var(--body); font-size: 15.5px; line-height: 1.55; }}
.wrap {{ max-width: 960px; margin: 0 auto; padding-left: max(16px, env(safe-area-inset-left, 0px)); padding-right: max(16px, env(safe-area-inset-right, 0px)); padding-block: 40px 64px; display: grid; gap: 44px; }}
h1, h2, h3 {{ font-family: var(--display); text-wrap: balance; margin: 0; line-height: 1.15; }}
h1 {{ font-size: clamp(30px, 5vw, 42px); font-weight: 600; letter-spacing: -0.01em; }}
h2 {{ font-size: 25px; font-weight: 600; }}
h3 {{ font-size: 17px; font-weight: 600; }}
p {{ margin: 0; max-width: 70ch; }}
.eyebrow {{ font-family: var(--mono); font-size: 12px; letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-3); }}
section {{ display: grid; gap: 16px; min-width: 0; }}
header {{ display: grid; gap: 14px; }}
.lede {{ font-size: 17px; color: var(--ink-2); }}
.num {{ font-family: var(--mono); font-variant-numeric: tabular-nums; }}
.key {{ display: grid; gap: 10px; padding: 0; margin: 0; list-style: none; counter-reset: k; }}
.key li {{ display: grid; grid-template-columns: 2em 1fr; gap: 10px; padding: 14px 16px; background: var(--panel); border: 1px solid var(--rule); border-radius: 6px; counter-increment: k; }}
.key li::before {{ content: counter(k); font-family: var(--display); font-size: 22px; font-weight: 600; color: var(--accent); line-height: 1.1; }}
.key b {{ font-weight: 600; }}
.tbl-wrap {{ overflow-x: auto; border: 1px solid var(--rule); border-radius: 6px; background: var(--panel); }}
table {{ border-collapse: collapse; width: 100%; font-size: 14px; }}
th, td {{ text-align: left; padding: 9px 12px; border-bottom: 1px solid var(--rule-2); vertical-align: top; }}
th {{ font-weight: 500; color: var(--ink-2); font-size: 12.5px; vertical-align: bottom; }}
td.r, th.r {{ text-align: right; }}
td.r {{ font-family: var(--mono); font-variant-numeric: tabular-nums; white-space: nowrap; }}
td.cfg {{ font-weight: 500; min-width: 16em; }}
td .ci {{ color: var(--ink-3); font-size: 12px; }}
@media (max-width: 560px) {{ td.cfg {{ min-width: 9em; }} th, td {{ padding: 8px 9px; }} }}
tr:last-child td {{ border-bottom: 0; }}
tr.x td {{ color: var(--ink-2); font-style: italic; }}
.cmp {{ background: var(--panel); border: 1px solid var(--rule); border-radius: 8px; padding: 22px; display: grid; gap: 16px; min-width: 0; }}
.cmp-head {{ display: grid; gap: 6px; }}
.q {{ font-size: 16px; color: var(--ink-2); }}
.answer {{ font-size: 16.5px; }}
.stat {{ display: flex; flex-wrap: wrap; gap: 8px 22px; font-size: 14px; color: var(--ink-2); padding: 10px 0; border-top: 1px solid var(--rule-2); border-bottom: 1px solid var(--rule-2); }}
.stat .num {{ color: var(--ink); font-weight: 500; }}
.legend {{ display: flex; flex-wrap: wrap; gap: 6px 18px; font-size: 13px; color: var(--ink-2); }}
.legend span {{ display: inline-flex; align-items: center; gap: 7px; }}
.sw {{ width: 18px; height: 3px; border-radius: 2px; display: inline-block; }}
.chart {{ position: relative; min-width: 0; }}
.chart svg {{ width: 100%; height: auto; display: block; overflow: visible; }}
.chart text {{ font-family: var(--mono); font-size: 11px; fill: var(--ink-3); }}
.chart .lbl {{ fill: var(--ink-2); font-size: 11.5px; }}
.chart .ttl {{ fill: var(--ink); font-size: 13px; font-family: var(--body); font-weight: 600; }}
.chart .rowlbl {{ fill: var(--ink-2); font-size: 12px; font-family: var(--body); }}
.chart .axis {{ fill: var(--ink-2); font-size: 11.5px; font-family: var(--body); }}
.tip {{ position: absolute; pointer-events: none; background: var(--panel); border: 1px solid var(--rule); border-radius: 6px; padding: 8px 10px; font-size: 12.5px; box-shadow: 0 4px 14px rgba(0,0,0,0.12); white-space: nowrap; display: grid; gap: 3px; z-index: 2; }}
.tip .t {{ font-family: var(--mono); color: var(--ink-3); font-size: 11.5px; }}
.tip .row {{ display: flex; align-items: center; gap: 7px; font-family: var(--mono); font-variant-numeric: tabular-nums; }}
.note {{ font-size: 13.5px; color: var(--ink-2); }}
.flag {{ background: var(--flag-bg); color: var(--flag-ink); border-radius: 6px; padding: 12px 14px; font-size: 14px; }}
.explain {{ background: var(--key-bg); border-radius: 6px; padding: 14px 16px; display: grid; gap: 8px; font-size: 14.5px; }}
.two {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(270px, 1fr)); gap: 18px 26px; }}
dl.setup {{ display: grid; grid-template-columns: 11em 1fr; gap: 8px 16px; margin: 0; font-size: 14.5px; }}
dl.setup dt {{ color: var(--ink-3); }}
dl.setup dd {{ margin: 0; min-width: 0; }}
dl.terms {{ display: grid; grid-template-columns: 8.5em 1fr; gap: 6px 16px; margin: 0; font-size: 14.5px; }}
dl.terms dt {{ font-weight: 600; }}
dl.terms dd {{ margin: 0; min-width: 0; color: var(--ink-2); }}
ul.caveats {{ margin: 0; padding-left: 1.2em; display: grid; gap: 8px; max-width: 74ch; }}
footer {{ font-size: 13px; color: var(--ink-3); border-top: 1px solid var(--rule); padding-top: 18px; }}
:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
@media (max-width: 560px) {{
  dl.setup, dl.terms {{ grid-template-columns: 1fr; gap: 2px 0; }}
  dl.setup dd, dl.terms dd {{ margin-bottom: 8px; }}
  .cmp {{ padding: 16px; }}
}}
</style>

<div class="wrap">
  <header>
    <div class="eyebrow">llm-d · vLLM 0.30 · Qwen3-8B · 4 × NVIDIA A30 · long multi-turn chat</div>
    <h1>KV Offload Comparison</h1>
    <p class="lede">How quickly a long chat's next turn starts, when its history no longer fits in GPU memory: Certus against vLLM's own KV tiers, with prefill and decode on separate GPUs and with every GPU doing both.</p>
  </header>

  <section aria-labelledby="h-key">
    <h2 id="h-key">Key results</h2>
    <ol class="key">
      <li><span><b>With prefill and decode split, Certus starts the next turn 2.7× faster than vLLM's documented tiered setup</b> (0.64 s against 1.76 s median). Two things add up: Certus gets stored history from disk to the GPU faster, and it fetches each turn's answer back from the decode GPU so prefill does not recompute it.</span></li>
      <li><span><b>Fetching the answer back is worth 1.5×.</b> With Certus's fetch turned off, the next turn takes 0.99 s; prefill recomputes each earlier answer.</span></li>
      <li><span><b>vLLM's P2P fetch does not survive the memory pressure.</b> It pulls only from a peer's CPU memory, which the workload overflows, so it made no pulls and recomputed every prompt (2.88 s). Even modified to fetch from disk, so it pulls as often as Certus, it was 2.6× slower (1.64 s). Prefill keeps a fetched answer only in memory, which overflows, so each turn fetches every earlier answer again: four times the data Certus fetched. The source reads all of those blocks from disk before it replies, plus blocks prefill then loads from its own disk instead. Those extra reads come from our unsupported setup, about 200 ms of the gap, and changing the tier order does not remove them; the rest is how vLLM handles fetches.</span></li>
      <li><span><b>Without the split, the median is faster but the slow turns are far slower.</b> Every GPU doing both jobs starts most turns in 0.28 s, but about 24 of 280 turns wait over 10 s behind other sessions' long answers, against 10 with the split. When the router moves a conversation to another GPU, Certus's fetch lets the new GPU take its history instead of recomputing it: over four runs each, turns taking 2–5 s fall from 28.5 per run to 7.</span></li>
    </ol>
  </section>

  <section aria-labelledby="h-what">
    <h2 id="h-what">What is being measured</h2>
    <p>In a chat, each new turn's prompt is the whole conversation so far: the document, every earlier question, and every earlier answer. Before the model can write the first word of its reply it has to have the KV cache (the attention keys and values) for that entire prompt on the GPU. It can either <b>load</b> that KV from wherever it was stored after earlier turns, or <b>recompute</b> it. Time to first token (TTFT) is mostly that step.</p>
    <p>The workload is built so the stored KV cannot all stay close: 40 conversations of 8 turns, each a 4,000-token document followed by questions with 2,000-token answers, about 110 GiB of KV in all, against 32 GiB of memory tiers across the four GPUs: <b>3.4 times too much</b>. So most history has to come from SSD, from another GPU's store, or be recomputed, which is the case KV offload exists for.</p>
    <div class="explain">
      <dl class="terms">
        <dt>P/D split</dt><dd>One GPU does every prompt pass (<i>prefill</i>); three do the generation (<i>decode</i>). Each turn's KV moves from prefill to decode over <b>NIXL</b>, GPU to GPU over RDMA. Each new turn's answer is produced on a decode GPU, but the next turn's prompt is processed on the prefill GPU, so that answer has to come back or be recomputed.</dd>
        <dt>Aggregated</dt><dd>Each of the four GPUs does both. A conversation usually stays on one GPU, so its history is local, but the router may move it to another.</dd>
        <dt>Fetch</dt><dd>Moving stored KV for a turn's history from the GPU that has it to the GPU that needs it, instead of recomputing it. The router names the source.</dd>
        <dt>Certus</dt><dd>The KV store being evaluated: one instance per NUMA node, holding KV in DRAM and spilling to NVMe SSD, reached by vLLM through a connector. It fetches from a peer Certus over RDMA.</dd>
        <dt>vLLM tiers</dt><dd>vLLM's built-in alternative: a CPU-memory tier on each GPU's server, optionally spilling to a filesystem, optionally sharing with peers (P2P).</dd>
      </dl>
    </div>
  </section>

  <section aria-labelledby="h-cfg">
    <h2 id="h-cfg">The configurations</h2>
    <div class="tbl-wrap"><table>
      <thead><tr><th>Configuration</th><th>How a turn's history reaches the GPU</th></tr></thead>
      <tbody>{"".join(f'<tr class="{"x" if k == "pd_vllm_p2p_fs_x" else ""}"><td class="cfg">{n}</td><td>{d}</td></tr>' for k, n, d in CFG)}</tbody>
    </table></div>
    <p class="note">Every configuration gets the same memory: 7.97 GiB of memory tier per GPU, 37,184 tokens of GPU KV cache, and the same eight NVMe drives per server. The split configurations all put prefill on the same GPU.</p>
  </section>

  <section aria-labelledby="h-all">
    <h2 id="h-all">All configurations at a glance</h2>
    <div class="tbl-wrap"><table>
      <thead><tr><th>Configuration</th><th class="r">TTFT, median</th><th class="r">TTFT, 90th percentile</th><th class="r">Prompt KV loaded<br>instead of recomputed</th><th class="r">Turns fetched<br>from a peer</th><th class="r">Output tokens/s<br>(all sessions)</th><th class="r">Turns completed</th></tr></thead>
      <tbody>{overview_rows()}</tbody>
    </table></div>
    <p class="note">TTFT is measured by the client through llm-d's router, over the 280 follow-up turns (turns 2–8 of each conversation); a first turn has no history to reuse and takes about 0.65–0.9 s everywhere. Output throughput is about equal everywhere because generating the 2,000-token answers dominates the run.</p>
  </section>

  <section class="cmp" aria-labelledby="h-q1">
    <div class="cmp-head"><div class="eyebrow">Question 1</div><h2 id="h-q1">Does fetching the answer back help, in a P/D split?</h2></div>
    <p class="answer">Yes, by 1.5×. With fetch, prefill takes each earlier answer from the decode GPU's Certus over RDMA; without it, it recomputes them. Prefill loads 95% of the prompt from Certus instead of 79%, and recomputes 4.4 times less.</p>
    {stat_line("pd_certus_fetch", "pd_certus_nofetch")}
    <div class="legend" data-legend="pd_certus_fetch,pd_certus_nofetch"></div>
    <div class="chart" data-chart="pd_certus_fetch,pd_certus_nofetch"></div>
    <p class="note">The gap grows with the conversation: each later turn has more earlier answers to bring back.</p>
  </section>

  <section class="cmp" aria-labelledby="h-q2">
    <div class="cmp-head"><div class="eyebrow">Question 2</div><h2 id="h-q2">How does Certus compare with vLLM's own tiers?</h2></div>
    <p class="answer">Certus is 2.7× faster than vLLM's documented multi-tier setup, and 4.5× faster than llm-d's documented P2P setup, which fetches nothing under this load.</p>
    <h3>Against vLLM CPU + filesystem: two separate effects</h3>
    <div class="two">
      <div class="explain">
        <span><b>Storage path alone.</b> Neither side fetches, and both reuse exactly the same 79.5% of prompt KV, so only the path from storage to the GPU differs.</span>
        <span>{s(C['pd_certus_nofetch']['p50'])} against {s(C['pd_vllm_cpu_fs']['p50'])}: <b>Certus {C['pd_vllm_cpu_fs']['p50'] / C['pd_certus_nofetch']['p50']:.2f}× faster</b> (95% CI {cmp('pd_certus_nofetch','pd_vllm_cpu_fs')[1]:+.2f} to {cmp('pd_certus_nofetch','pd_vllm_cpu_fs')[2]:+.2f} s).</span>
      </div>
      <div class="explain">
        <span><b>Plus fetch.</b> Certus as deployed, against vLLM's documented setup.</span>
        <span>{s(C['pd_certus_fetch']['p50'])} against {s(C['pd_vllm_cpu_fs']['p50'])}: <b>Certus {C['pd_vllm_cpu_fs']['p50'] / C['pd_certus_fetch']['p50']:.2f}× faster</b> (95% CI {cmp('pd_certus_fetch','pd_vllm_cpu_fs')[1]:+.2f} to {cmp('pd_certus_fetch','pd_vllm_cpu_fs')[2]:+.2f} s).</span>
      </div>
    </div>
    <div class="legend" data-legend="pd_certus_fetch,pd_certus_nofetch,pd_vllm_cpu_fs,pd_vllm_cpu_p2p,pd_vllm_p2p_fs_x"></div>
    <div class="chart" data-chart="pd_certus_fetch,pd_certus_nofetch,pd_vllm_cpu_fs,pd_vllm_cpu_p2p,pd_vllm_p2p_fs_x"></div>
    <p class="note">The experimental configuration, vLLM's P2P made to fetch from disk too, is included for comparison; Question 3 covers it.</p>
    <h3>Against vLLM P2P</h3>
    <p>llm-d's P2P setup lets prefill pull an earlier answer from a decode GPU's CPU tier. Here the CPU tiers overflow, decode has evicted each answer before prefill asks, and the router named no source on any of 316 turns. With no disk tier under it, prefill recomputed every prompt.</p>
    {stat_line("pd_certus_fetch", "pd_vllm_cpu_p2p")}
  </section>

  <section class="cmp" aria-labelledby="h-q3">
    <div class="cmp-head"><div class="eyebrow">Question 3</div><h2 id="h-q3">If vLLM's P2P could fetch from disk too, would it match Certus?</h2></div>
    <p class="answer">No. Made to fetch exactly as often as Certus, it is still 2.6× slower, about as slow as vLLM's tiers with no fetch at all.</p>
    <p>To test this fairly we added vLLM's filesystem tier to its P2P setup and changed vLLM's KV-cache events so the router treats blocks on disk as fetchable, which is how it already sees Certus's SSD. vLLM accepts the combination but does not document it, so this is an experiment, not a supported configuration. It worked as intended: the router named a source on all 280 follow-up turns, the same as Certus, and prefill loaded 95.3% of the prompt instead of recomputing it.</p>
    {stat_line("pd_certus_fetch", "pd_vllm_p2p_fs_x")}
    <div class="legend" data-legend="pd_certus_fetch,pd_vllm_p2p_fs_x,pd_vllm_cpu_fs"></div>
    <div class="chart" data-chart="pd_certus_fetch,pd_vllm_p2p_fs_x,pd_vllm_cpu_fs"></div>
    <div class="explain">
      <b>Why equal fetching does not give equal speed</b>
      <span>Not the data path: both read a block from disk into host memory on the source, send it over RDMA, and copy it into the GPU on the requester. We reran this configuration with vLLM's P2P logging turned up and timed every step of all {P['n']} fetches. The rerun took the same time as the original (median {P['p50']:.2f} s), so the logging did not change the result. A fetch took a median of {P['total_ms']} ms. {P['resolve_ms']} ms of that was the source getting the blocks ready before it replied; the reply, the fetch request and the RDMA write together took {P['rest_ms']} ms. Three things make that wait long.</span>
      <span><b>1. vLLM fetches the same answers again and again.</b> When prefill fetches a block, vLLM puts it only in prefill's CPU memory, not on its disk: prefill wrote just {P['prefill_fs_write_gb']} GB to disk over the run, against {P['p2p_gb']} GB fetched. That memory overflows under this workload, so by the next turn the fetched answers are gone, and prefill fetches them again together with the newest one: 125 blocks on the second turn, 250 on the third, up to 875 on the eighth. Certus keeps a fetched block in prefill's own Certus, so each turn fetches only the newest answer: {P['certus_fetch_keys']:,} blocks over 280 fetches, about 125 each, {C['pd_certus_fetch']['fetch_gb']} GB in all against vLLM's {P['p2p_gb']} GB.</span>
      <span><b>2. The source reads every block from disk before it replies.</b> A conversation's next turn arrives about {P['gap_s']} s after its last answer. In that time each decode GPU stores {P['vllm_own_gb']}–{P['certus_in_gb']} GB of new answers, more than its {P['tier_gb']} GB memory tier holds, so in both systems the blocks a fetch wants have usually left memory for disk. vLLM's source found none of the {P['decode_on_disk']:,} blocks it was asked for in memory. It reads them into its CPU memory, at about {P['resolve_gbs']} GB/s, and replies only when all of them are in; the time grows with the number of blocks asked for, by about 0.78 ms per block. Those reads go through the same memory tier, another {P['vllm_peer_gb']} GB per GPU between turns, which pushes out more of its own answers. Certus's source sends each block as soon as it has it, reads blocks that are on SSD into separate buffers rather than into its memory tier, and is asked for only the newest answer.</span>
      <span><b>3. The source also reads blocks prefill does not take.</b> Prefill asks its P2P tier and its own disk about the same blocks at once, a median of {P['keys_med']} per fetch. Its own disk supplies the older part of the conversation, so prefill fetches only the earlier answers, a median of {P['fetched_med']}. The source has already read all of them: {P['unused']:,} blocks ({P['unused_gb']} GB) over the run that were never fetched, the same amount prefill read from its own disk ({P['prefill_fs_gb']} GB). At 0.78 ms per block that is about 200 ms of each wait.</span>
      <span><b>Putting the disk tier first does not help.</b> To test whether point 3 is only a matter of configuration, we reran with prefill's disk tier ahead of P2P. Nothing changed: prefill still asked about a median of {P['keys_med']} blocks per fetch, and the median turn took {P['swap']['p50']:.2f} s against {P['p50']:.2f} s ({P['swap']['d']:+.2f} s, 95% CI {P['swap']['lo']:+.2f} to {P['swap']['hi']:+.2f}). vLLM's disk tier answers a lookup a scheduler step later, and an unanswered tier does not stop the lookup from moving on, so in the first step every block goes to both tiers whatever the order.</span>
      <span><b>When the blocks are in memory, the same fetch is fast.</b> In a smaller run where the source still had the blocks in memory, it replied in a median of 4 ms and a whole fetch took 108 ms, mostly the RDMA write. So vLLM's P2P mechanism itself is not slow; what it has to read from disk, and how often, is.</span>
      <span><b>How much is our setup, and how much is vLLM?</b> Point 3 comes from our unsupported setup, P2P and a filesystem tier on the same engine: about 200 ms of the 1.0 s gap, and no tier order removes it. Points 1 and 2 are how vLLM handles fetched blocks and peer requests, and they account for most of the rest, together with vLLM's slower path from disk to GPU (Question 2). In vLLM's documented P2P setup the source serves only from memory, which under this workload has already lost the blocks, so that setup fetched nothing.</span>
    </div>
  </section>

  <section class="cmp" aria-labelledby="h-q4">
    <div class="cmp-head"><div class="eyebrow">Question 4</div><h2 id="h-q4">Is splitting prefill and decode worth it?</h2></div>
    <p class="answer">It depends on what matters. Without the split the typical turn starts 2.3× sooner; with it, slow turns are far rarer.</p>
    {stat_line("agg_certus_fetch", "pd_certus_fetch")}
    <div class="two">
      <div class="chart" data-dist="pd_certus_fetch" data-title="P/D · NIXL + Certus, with fetch"></div>
      <div class="chart" data-dist="agg_certus_fetch" data-title="Aggregated · Certus, with fetch"></div>
    </div>
    <p class="note">Each bar counts follow-up turns by how long they took to start. Aggregated serving has no prefill-to-decode hand-off, so most turns start in under half a second. But a GPU busy writing other sessions' 2,000-token answers makes a new turn wait: {C['agg_certus_fetch']['over10']} turns waited over 10 s, against {C['pd_certus_fetch']['over10']} with the split, where prefill has a GPU of its own. That wait is queueing in vLLM, not loading: its own counters show the same number of requests queued over 10 s.</p>
    <h3>What fetch adds when every GPU does both</h3>
    <p>About 30% of follow-up turns land on a different GPU from the conversation's previous turn, when the router balances load. Without fetch such a turn recomputes its whole history, typically 2–5 s; with fetch the new GPU takes the history from the old GPU's Certus. Both ran four times each, alternating, each from empty stores:</p>
    <div class="two">
      <div class="chart" data-strip="tm5" data-ylabel="mean TTFT of turns under 5 s (s)" data-title="Turns not stuck in a queue"></div>
      <div class="chart" data-strip="band25" data-ylabel="turns taking 2–5 s" data-title="Turns recomputing a moved history"></div>
    </div>
    <div class="tbl-wrap"><table>
      <thead><tr><th>Follow-up turns, average of four runs</th><th class="r">No fetch</th><th class="r">With fetch</th><th class="r">Difference<br>(95% interval)</th><th class="r">Every run<br>separated?</th></tr></thead>
      <tbody>{rep_rows()}</tbody>
    </table></div>
    <p class="note">Both columns are aggregated serving with Certus. "Every run separated" means every run on one side beat every run on the other, the strongest result four runs against four can give (p = 0.029). Each dot in the charts is one run; the bar is the mean of four. Fetch lowers the mean of the turns that are not stuck in a queue by about a quarter of a second, and empties most of the 2–5 s band. It costs about {D['repcmp']['med']['d'] * 1000:.0f} ms on the median turn, most likely the router's extra work with fetch on (it tokenizes and indexes every prompt to find a source). It does not change the turns over 10 s, which are queueing, or the mean over all turns, which those dominate.</p>
  </section>

  <section aria-labelledby="h-bw">
    <h2 id="h-bw">Where each turn's history came from</h2>
    <p>The same 521 GB of prompt KV has to reach the GPU over the run in every configuration. This splits it by source. Loading from a store is cheap; recomputing costs GPU time at the start of the turn.</p>
    <div class="legend" data-legend-src></div>
    <div class="chart" data-bars></div>
    <div class="tbl-wrap"><table>
      <thead><tr><th>Configuration</th><th class="r">Loaded from<br>the GPU's own store</th><th class="r">Fetched from<br>another GPU's store</th><th class="r">Recomputed<br>(KV equivalent)</th><th class="r">Store → GPU<br>copy rate</th><th>What the GPU reads from</th></tr></thead>
      <tbody>{bw_rows()}</tbody>
    </table></div>
    <p class="note">Copy rate is bytes loaded into the GPU divided by the time vLLM spent loading them, from vLLM's own counters. It is about 15–20 GB/s in every configuration that loads at all, so the final copy into the GPU is not what separates them. Two things do:</p>
    <ul class="caveats" style="font-size:13.5px;color:var(--ink-2)">
      <li><b>How much is recomputed.</b> Recomputing costs GPU time at the start of every turn. The configurations that fetch recompute 24–35 GB of KV over the run; those that do not, 107–116 GB; vLLM's P2P setup under overflow, all 511 GB.</li>
      <li><b>How fast the store behind the copy is.</b> vLLM loads into the GPU only from its CPU tier, so a block on disk is first read into CPU memory before the copy can start; vLLM's filesystem tier read at about {C['pd_vllm_cpu_fs']['disk_read_gbs']:.1f} GB/s per server here. Most of Certus's loads (about 416 GB of the 496 GB in the split configuration with fetch) came from its SSD, and its fetch moved 82 GB between servers over RDMA. For a fetch from a peer, Question 3 shows the wait for the peer to answer matters more than either.</li>
    </ul>
  </section>

  <section aria-labelledby="h-setup">
    <h2 id="h-setup">How the comparison was kept fair</h2>
    <dl class="setup">
      <dt>Hardware</dt><dd>Two servers, each with two NVIDIA A30 GPUs (24 GB) on separate NUMA nodes, eight NVMe drives and a RoCE network card.</dd>
      <dt>Memory</dt><dd>7.97 GiB of memory tier per GPU in every configuration (Certus's 8 GiB less its transfer buffer, and the same <span class="num">cpu_bytes_to_use</span> for vLLM), and the GPU KV cache fixed at 37,184 tokens on every pod.</dd>
      <dt>Disk</dt><dd>The same eight NVMe drives per server: driven directly by Certus (SPDK), or given to vLLM as one RAID 0 + XFS array per NUMA node.</dd>
      <dt>Placement</dt><dd>In every split configuration prefill runs on the same GPU and decode on the other three; the hand-off is NIXL over RDMA.</dd>
      <dt>Routing</dt><dd>llm-d's router at a pinned release, with the same placement rules for every split configuration and for both aggregated ones. Where fetch is on, the same rule picks a source: the GPU whose store holds at least 1,024 more tokens of the prompt than the GPU doing the work.</dd>
      <dt>Starting state</dt><dd>Every run starts from empty stores.</dd>
      <dt>Answers stored</dt><dd>Every configuration stores the answers it generates, not only prompts. An answer is the next turn's prompt; vLLM's default of storing prompts only would make every configuration recompute it.</dd>
      <dt>Software</dt><dd>vLLM 0.30.0 with the Certus connector in one image for every configuration. Certus built with CUDA 13.2, which lets it copy a block's 36 per-layer pieces to the GPU as one batched transfer.</dd>
    </dl>
  </section>

  <section aria-labelledby="h-cav">
    <h2 id="h-cav">Limits</h2>
    <ul class="caveats">
      <li>One run per configuration, except the two aggregated ones, which ran four times each on the same build as the rest. Other confidence intervals are bootstrapped over the 280 turns of one run, not across runs.</li>
      <li>The experimental vLLM P2P + filesystem configuration is not supported by vLLM or llm-d, and its routing change is our own patch. A version designed for fetching from disk could do better.</li>
      <li>The two split Certus configurations were rerun on a Certus build that fixes a startup race, which could leave a server with fewer than its four drives. The reruns match the earlier runs within noise: 0.64 s against 0.63 s with fetch, 0.99 s against 0.95 s without. The aggregated runs were checked for all four drives before each run.</li>
      <li>In the vLLM CPU + P2P configuration, one conversation ended after its fourth turn on an HTTP error from llm-d's routing sidecar, so it completed 315 of 320 turns.</li>
      <li>One model, one GPU type, one workload shape. Shorter answers or longer histories would load prefill more and could change the throughput picture, which here is set by generating 2,000-token answers.</li>
    </ul>
  </section>

  <footer>Measured on two servers with two NVIDIA A30 GPUs each, 30 October – 5 November 2026. Per-turn records, Prometheus scrapes and router decision logs for every run are kept with the run scripts.</footer>
</div>

<script>
const D = {data_js};
const C = D.C, NAME = D.NAME, R = D.R;
const NS = "http://www.w3.org/2000/svg";
// One colour and dash per configuration, the same in every chart.
const SERIES = {{ pd_certus_fetch: ["var(--s1)", "none"], pd_certus_nofetch: ["var(--s2)", "6 4"], pd_vllm_cpu_fs: ["var(--s3)", "2 3"],
  pd_vllm_cpu_p2p: ["var(--s4)", "8 3 2 3"], pd_vllm_p2p_fs_x: ["var(--s5)", "10 3"] }};
const col = k => SERIES[k][0], dash = k => SERIES[k][1];
function mk(tag, attrs, parent) {{ const e = document.createElementNS(NS, tag); for (const k in attrs) e.setAttribute(k, attrs[k]); parent && parent.appendChild(e); return e; }}
function niceMax(v) {{ const steps = [0.5, 1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]; return steps.find(t => t >= v * 1.05) || Math.ceil(v); }}
const short = k => NAME[k].replace(" (experimental)", "");
function yTitle(svg, text, x, y) {{ const t = mk("text", {{ x: 0, y: 0, "text-anchor": "middle", class: "axis", transform: `translate(${{x}} ${{y}}) rotate(-90)` }}, svg); t.textContent = text; }}
function xTitle(svg, text, x, y) {{ const t = mk("text", {{ x, y, "text-anchor": "middle", class: "axis" }}, svg); t.textContent = text; }}
function tipAt(host, svg, W, H, px, py, html) {{
  const tip = host.querySelector(".tip") || Object.assign(host.appendChild(document.createElement("div")), {{ className: "tip" }});
  tip.innerHTML = html; tip.hidden = false;
  const r = svg.getBoundingClientRect(), tx = px * r.width / W, ty = py * r.height / H;
  tip.style.left = (tx < r.width / 2 ? tx + 14 : tx - tip.offsetWidth - 14) + "px"; tip.style.top = Math.max(0, ty - 20) + "px";
  return tip;
}}

// Legends for line charts: colour + dash per series, in the chart's order.
for (const el of document.querySelectorAll("[data-legend]")) {{
  el.innerHTML = el.dataset.legend.split(",").map((k, i) => `<span><svg width="22" height="6" aria-hidden="true"><line x1="1" x2="21" y1="3" y2="3" stroke="${{col(k)}}" stroke-width="2.5" stroke-dasharray="${{dash(k)}}"/></svg>${{short(k)}}</span>`).join("");
}}

// Per-turn median TTFT: one line per configuration.
for (const host of document.querySelectorAll("[data-chart]")) {{
  const keys = host.dataset.chart.split(","), series = keys.map(k => C[k].per_turn);
  const narrow = (host.clientWidth || 680) < 560;
  const W = narrow ? 400 : 680, H = narrow ? 300 : 260, L = narrow ? 50 : 64, Rr = narrow ? 58 : 120, T = 12, B = 46;
  const ymax = niceMax(Math.max(...series.flat()));
  const x = i => L + i * (W - L - Rr) / 7, y = v => T + (H - T - B) * (1 - v / ymax);
  const svg = mk("svg", {{ viewBox: `0 0 ${{W}} ${{H}}`, role: "img", "aria-label": `Median time to first token by turn: ${{keys.map(short).join("; ")}}` }}, host);
  const step = ymax <= 1.5 ? 0.25 : ymax <= 3 ? 0.5 : 1;
  for (let v = 0; v <= ymax + 1e-9; v += step) {{
    mk("line", {{ x1: L, x2: W - Rr, y1: y(v), y2: y(v), stroke: "var(--rule-2)", "stroke-width": 1 }}, svg);
    const t = mk("text", {{ x: L - 8, y: y(v) + 4, "text-anchor": "end" }}, svg); t.textContent = v.toFixed(v % 1 ? 2 : 0);
  }}
  for (let i = 0; i < 8; i++) {{ const t = mk("text", {{ x: x(i), y: H - B + 18, "text-anchor": "middle" }}, svg); t.textContent = i + 1; }}
  xTitle(svg, "turn in the conversation", (L + W - Rr) / 2, H - 6);
  yTitle(svg, narrow ? "median TTFT (s)" : "median time to first token (s)", 14, (T + H - B) / 2);
  const cross = mk("line", {{ x1: 0, x2: 0, y1: T, y2: H - B, stroke: "var(--ink-3)", "stroke-width": 1, "stroke-dasharray": "2 3", opacity: 0 }}, svg);
  const ends = [];
  series.forEach((pts, si) => {{
    mk("path", {{ d: pts.map((v, i) => `${{i ? "L" : "M"}}${{x(i).toFixed(1)}},${{y(v).toFixed(1)}}`).join(""), fill: "none", stroke: col(keys[si]), "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round", "stroke-dasharray": dash(keys[si]) }}, svg);
    pts.forEach((v, i) => mk("circle", {{ cx: x(i), cy: y(v), r: 4, fill: col(keys[si]), stroke: "var(--panel)", "stroke-width": 2 }}, svg));
    ends.push({{ v: pts[7], si }});
  }});
  ends.sort((a, b) => a.v - b.v);
  let last = 1e9; const placed = [];
  for (const e of [...ends].reverse()) {{ let ly = y(e.v) + 4; if (last - ly < 14) ly = last - 14; last = ly; placed.push([e, ly]); }}
  for (const [e, ly] of placed) {{ const t = mk("text", {{ x: W - Rr + 10, y: ly, class: "lbl" }}, svg); t.textContent = `${{C[keys[e.si]].per_turn[7].toFixed(2)}} s`; t.setAttribute("fill", col(keys[e.si])); t.setAttribute("style", `fill:${{col(keys[e.si])}}`); }}
  const hit = mk("rect", {{ x: L - 10, y: T, width: W - L - Rr + 20, height: H - T - B, fill: "transparent" }}, svg);
  const move = ev => {{
    const r = svg.getBoundingClientRect(), px = (ev.clientX - r.left) * W / r.width;
    const i = Math.max(0, Math.min(7, Math.round((px - L) / ((W - L - Rr) / 7))));
    cross.setAttribute("x1", x(i)); cross.setAttribute("x2", x(i)); cross.setAttribute("opacity", 1);
    tipAt(host, svg, W, H, x(i), T + 10, `<div class="t">turn ${{i + 1}}${{i === 0 ? " · nothing to reuse" : ""}}</div>` + keys.map((k, si) => `<div class="row"><svg width="18" height="6"><line x1="1" x2="17" y1="3" y2="3" stroke="${{col(k)}}" stroke-width="2.5" stroke-dasharray="${{dash(k)}}"/></svg>${{series[si][i].toFixed(2)}} s · ${{short(k)}}</div>`).join(""));
  }};
  hit.addEventListener("pointermove", move); hit.addEventListener("pointerdown", move);
  hit.addEventListener("pointerleave", () => {{ const t = host.querySelector(".tip"); if (t) t.hidden = true; cross.setAttribute("opacity", 0); }});
}}

// TTFT distribution: follow-up turns per latency band, one bar chart per configuration.
const BANDS = ["< 0.5", "0.5–2", "2–5", "5–10", "> 10"];
const distMax = 280;
for (const host of document.querySelectorAll("[data-dist]")) {{
  const k = host.dataset.dist, v = C[k].dist;
  const W = 340, H = 250, L = 56, Rr = 10, T = 30, B = 46;
  const svg = mk("svg", {{ viewBox: `0 0 ${{W}} ${{H}}`, role: "img", "aria-label": `${{host.dataset.title}}: follow-up turns by time to first token, ${{BANDS.map((b, i) => b + " " + v[i]).join(", ")}}` }}, host);
  const t = mk("text", {{ x: L, y: 14, class: "ttl" }}, svg); t.textContent = host.dataset.title;
  const y = n => T + (H - T - B) * (1 - n / distMax);
  for (let n = 0; n <= distMax; n += 70) {{
    mk("line", {{ x1: L, x2: W - Rr, y1: y(n), y2: y(n), stroke: "var(--rule-2)", "stroke-width": 1 }}, svg);
    const tt = mk("text", {{ x: L - 8, y: y(n) + 4, "text-anchor": "end" }}, svg); tt.textContent = n;
  }}
  yTitle(svg, "follow-up turns", 14, (T + H - B) / 2);
  xTitle(svg, "time to first token (s)", (L + W - Rr) / 2, H - 4);
  const bw = (W - L - Rr) / BANDS.length;
  v.forEach((n, i) => {{
    const x0 = L + i * bw + 3, w = bw - 6, top = y(n), base = y(0);
    const h = base - top;
    if (h > 0) mk("path", {{ d: `M${{x0}},${{base}}V${{top + Math.min(4, h)}}Q${{x0}},${{top}} ${{x0 + Math.min(4, w / 2)}},${{top}}H${{x0 + w - Math.min(4, w / 2)}}Q${{x0 + w}},${{top}} ${{x0 + w}},${{top + Math.min(4, h)}}V${{base}}Z`, fill: i === 4 ? "var(--s2)" : "var(--s1)" }}, svg);
    const lab = mk("text", {{ x: x0 + w / 2, y: top - 5, "text-anchor": "middle", class: "lbl" }}, svg); lab.textContent = n;
    const bl = mk("text", {{ x: x0 + w / 2, y: H - B + 16, "text-anchor": "middle" }}, svg); bl.textContent = BANDS[i];
  }});
}}

// Aggregated repeats: one dot per run, the four-run mean as a bar.
const REPS = [["fetch", "with fetch", "var(--s1)"], ["nofetch", "no fetch", "var(--s2)"]];
for (const host of document.querySelectorAll("[data-strip]")) {{
  const k = host.dataset.strip, isS = k === "tm5";
  const W = 330, H = 250, L = 64, Rr = 16, T = 30, B = 46;
  const ymax = isS ? 0.8 : 40, step = isS ? 0.2 : 10;
  const y = v => T + (H - T - B) * (1 - v / ymax), gx = i => L + (W - L - Rr) * (i + 0.5) / 2;
  const svg = mk("svg", {{ viewBox: `0 0 ${{W}} ${{H}}`, role: "img", "aria-label": `${{host.dataset.title}}: ${{host.dataset.ylabel}}, four aggregated runs with and without fetch` }}, host);
  const tt = mk("text", {{ x: L, y: 14, class: "ttl" }}, svg); tt.textContent = host.dataset.title;
  for (let v = 0; v <= ymax + 1e-9; v += step) {{
    mk("line", {{ x1: L, x2: W - Rr, y1: y(v), y2: y(v), stroke: "var(--rule-2)", "stroke-width": 1 }}, svg);
    const t = mk("text", {{ x: L - 8, y: y(v) + 4, "text-anchor": "end" }}, svg); t.textContent = isS ? v.toFixed(1) : v;
  }}
  yTitle(svg, host.dataset.ylabel, 14, (T + H - B) / 2);
  xTitle(svg, "aggregated serving with Certus", (L + W - Rr) / 2, H - 4);
  REPS.forEach(([v, label, col], gi) => {{
    const runs = R.filter(r => r.v === v), cx = gx(gi), m = runs.reduce((a, r) => a + r[k], 0) / runs.length;
    const xl = mk("text", {{ x: cx, y: H - B + 16, "text-anchor": "middle", class: "rowlbl" }}, svg); xl.textContent = label;
    mk("line", {{ x1: cx - 28, x2: cx + 28, y1: y(m), y2: y(m), stroke: col, "stroke-width": 2, "stroke-linecap": "round" }}, svg);
    runs.forEach((r, i) => {{
      const dx = (i - 1.5) * 12;
      mk("circle", {{ cx: cx + dx, cy: y(r[k]), r: 5, fill: col, stroke: "var(--panel)", "stroke-width": 2 }}, svg);
      const hit = mk("circle", {{ cx: cx + dx, cy: y(r[k]), r: 11, fill: "transparent" }}, svg);
      const show = () => tipAt(host, svg, W, H, cx + dx, y(r[k]), `<div class="t">${{label}} · run ${{r.run}}</div><div class="row">${{isS ? r[k].toFixed(3) + " s" : r[k] + " turns"}}</div><div class="row">median ${{r.med.toFixed(3)}} s</div>`);
      hit.addEventListener("pointerenter", show); hit.addEventListener("pointerdown", show);
      hit.addEventListener("pointerleave", () => {{ const t = host.querySelector(".tip"); if (t) t.hidden = true; }});
    }});
    const ml = mk("text", {{ x: cx + 34, y: y(m) + 4, class: "lbl" }}, svg); ml.textContent = isS ? m.toFixed(2) + " s" : m.toFixed(1);
  }});
}}

// Where each turn's history came from: stacked horizontal bars, GB, one per configuration.
const SRC = [["local_gb", "loaded from the GPU's own store", "var(--s1)"], ["peer_gb", "fetched from another GPU's store", "var(--s3)"], ["recompute_gb", "recomputed (KV equivalent)", "var(--s2)"]];
document.querySelector("[data-legend-src]").innerHTML = SRC.map(([, l, c]) => `<span><i class="sw" style="background:${{c}};height:10px;width:14px"></i>${{l}}</span>`).join("");
{{
  const host = document.querySelector("[data-bars]"), keys = Object.keys(C);
  const narrow = (host.clientWidth || 760) < 560;
  const W = narrow ? 380 : 760, rowH = narrow ? 44 : 30, T = 10, L = narrow ? 12 : 380, Rr = narrow ? 12 : 20, B = 40, H = T + keys.length * rowH + B, xmax = 560;
  const x = v => L + (W - L - Rr) * v / xmax;
  const svg = mk("svg", {{ viewBox: `0 0 ${{W}} ${{H}}`, role: "img", "aria-label": "Prompt KV per configuration split into loaded from own store, fetched from a peer, and recomputed" }}, host);
  for (let v = 0; v <= xmax; v += 100) {{
    mk("line", {{ x1: x(v), x2: x(v), y1: T, y2: H - B, stroke: "var(--rule-2)", "stroke-width": 1 }}, svg);
    const t = mk("text", {{ x: x(v), y: H - B + 16, "text-anchor": "middle" }}, svg); t.textContent = v;
  }}
  xTitle(svg, "prompt KV over the whole run (GB)", (L + W - Rr) / 2, H - 6);
  keys.forEach((k, i) => {{
    const yy = T + i * rowH + (narrow ? 18 : 6), h = narrow ? 16 : rowH - 12;
    const lab = narrow ? mk("text", {{ x: L, y: yy - 5, class: "rowlbl" }}, svg) : mk("text", {{ x: L - 10, y: yy + h / 2 + 4, "text-anchor": "end", class: "rowlbl" }}, svg);
    lab.textContent = short(k);
    let acc = 0;
    SRC.forEach(([f, l, col]) => {{
      const v = C[k][f]; if (v <= 0) return;
      const r = mk("rect", {{ x: x(acc) + (acc ? 1 : 0), y: yy, width: Math.max(0, x(acc + v) - x(acc) - (acc ? 1 : 0)), height: h, fill: col }}, svg);
      const show = () => tipAt(host, svg, W, H, x(acc + v / 2), yy, `<div class="t">${{short(k)}}</div><div class="row">${{l}}: ${{v.toFixed(0)}} GB</div>`);
      const a0 = acc; r.addEventListener("pointerenter", () => tipAt(host, svg, W, H, x(a0 + v / 2), yy, `<div class="t">${{short(k)}}</div><div class="row">${{l}}: ${{v.toFixed(0)}} GB</div>`));
      r.addEventListener("pointerleave", () => {{ const t = host.querySelector(".tip"); if (t) t.hidden = true; }});
      acc += v;
    }});
  }});
}}
</script>
"""
# A standalone page: the document skeleton around the body built above.
head, body = html.replace('<meta name="claude:viewport-fit-sides" content="cover">\n', "").split("</style>\n", 1)
page = ('<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n'
        '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">\n'
        f"{head}</style>\n</head>\n<body>\n{body}\n</body>\n</html>\n")
open(os.path.join(HERE, "kv-offload-comparison.html"), "w").write(page)
print("kv-offload-comparison.html written", len(page))
