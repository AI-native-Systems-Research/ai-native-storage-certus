"""Pair vLLM P2P debug events across prefill (consumer) and decode (producer) logs
by kv_request_id, and report how long each hop of a fetch takes.

Usage: p2p_timeline.py prefill.log decode1.log [decode2.log ...]

Hops, in order, for one kv_request_id (one fetch from one peer):
  C.SEND      consumer sends LookupMsg              (client.py:405)
  P.RECV      producer receives it                  (server.py:434)
  P.SEND      producer replies LookupRespMsg        (server.py:648) -- after ALL keys resolve
  C.RECV      consumer receives the reply           (client.py:435)
  C.FETCH     consumer sends FetchMsg               (client.py:194 request_blocks)
  P.FETCH     producer receives the fetch           (server.py:348)
  P.WRITE     producer submits the NIXL write       (server.py:1077)
  P.FINAL     producer finalizes (write done)       (server.py:971)
Clocks: both nodes are NTP-synced; cross-node gaps carry that error (~ms).
"""
import re, sys, statistics as st, collections, datetime
PAT = {
    "C.SEND":  r"P2P LOOKUP client \S+: SEND LookupMsg kv_request_id=(\S+) keys=(\d+)",
    "C.RECV":  r"P2P LOOKUP client \S+: RECV LookupRespMsg kv_request_id=(\S+) keys=(\d+) hits=(\d+)",
    "C.FETCH": r"P2PSession \S+: request_blocks job_id=\d+ kv_request_id=(\S+) blocks=(\d+)",
    "P.RECV":  r"P2P LOOKUP server \S+: RECV LookupMsg kv_request_id=(\S+) round=\S+ keys=(\d+)",
    "P.SEND":  r"P2P LOOKUP server \S+: SEND LookupRespMsg kv_request_id=(\S+) keys=(\d+) hits=(\d+)",
    "P.FETCH": r"P2PSession \S+: fetch RECEIVED kv_request_id=(\S+) round=\S+ blocks=(\d+)",
    "P.WRITE": r"P2PSession \S+: NIXL write_blocks SUBMITTED kv_request_id=(\S+)",
    "P.FINAL": r"P2PSession \S+: finalize kv_request_id=(\S+) round=\S+ success=(\S+)",
}
TS = re.compile(r"(\d\d-\d\d \d\d:\d\d:\d\d\.\d{3})")
ev = collections.defaultdict(dict)   # rid -> event -> (t, extra)
def parse(path):
    for line in open(path, errors="replace"):
        if "P2P" not in line: continue
        m = TS.search(line)
        if not m: continue
        t = datetime.datetime.strptime("2026-" + m.group(1), "%Y-%m-%d %H:%M:%S.%f").timestamp()
        for name, p in PAT.items():
            mm = re.search(p, line)
            if mm:
                rid = mm.group(1).rstrip(",")
                ev[rid].setdefault(name, (t, mm.groups()[1:]))   # first occurrence
for p in sys.argv[1:]: parse(p)
HOPS = [("C.SEND", "P.RECV", "lookup in flight"), ("P.RECV", "P.SEND", "producer resolves all keys"),
        ("P.SEND", "C.RECV", "reply in flight"), ("C.RECV", "C.FETCH", "consumer turns reply into a fetch"),
        ("C.FETCH", "P.FETCH", "fetch in flight"), ("P.FETCH", "P.WRITE", "producer submits NIXL write"),
        ("P.WRITE", "P.FINAL", "NIXL write + producer notices"), ("C.SEND", "P.FINAL", "TOTAL lookup→data written")]
full = [r for r, e in ev.items() if all(h in e for h in ("C.SEND", "P.RECV", "P.SEND", "C.RECV"))]
print(f"fetches with lookup events on both sides: {len(full)} (of {len(ev)} ids seen)")
for a, b, label in HOPS:
    d = [(ev[r][b][0] - ev[r][a][0]) * 1000 for r in ev if a in ev[r] and b in ev[r]]
    if d:
        d.sort(); q = lambda p: d[min(len(d) - 1, int(p * len(d)))]
        print(f"  {label:36s} n={len(d):4d}  median {q(.5):8.1f} ms  p90 {q(.9):8.1f} ms  max {d[-1]:8.1f} ms")
keys = [int(ev[r]["C.SEND"][1][0]) for r in full]
if keys: print(f"  keys per lookup: median {st.median(keys):.0f}, max {max(keys)}")
