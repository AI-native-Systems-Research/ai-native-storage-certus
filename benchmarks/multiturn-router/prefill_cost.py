#!/usr/bin/env python3
"""Prefill compute for N new tokens over a cached context: TTFT of max_tokens=1.
Usage: prefill_cost.py <prefill pod IP>  (talks to vLLM on :8000 directly)."""
import json,random,sys,time,urllib.request,statistics
url=f"http://{sys.argv[1]}:8000/v1/completions"
def ttft(ids):
    body={"model":"Qwen/Qwen3-8B","prompt":ids,"max_tokens":1,"temperature":0}
    r=urllib.request.Request(url,json.dumps(body).encode(),{"Content-Type":"application/json"})
    t=time.monotonic(); json.load(urllib.request.urlopen(r,timeout=120)); return time.monotonic()-t
rng=random.Random(int(time.time()))
tok=lambda n:[rng.randrange(1000,100000) for _ in range(n)]
ttft(tok(64))  # warm
print("context  new  -> ttft(s), median of 5")
for ctx in [86,1229,2371,3528]:
    for new in [1100,40]:
        xs=[]
        for _ in range(5):
            pre=tok(ctx); ttft(pre)            # put the context in prefill's GPU cache
            xs.append(ttft(pre+tok(new)))      # time only the new tokens
        print(f"{ctx:7d} {new:5d} -> {statistics.median(xs):.3f}")
