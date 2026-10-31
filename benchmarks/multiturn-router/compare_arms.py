import sys
import json, collections, statistics as st
def arm(D):
    s=json.load(open(D+'/summary.json'))
    turns=[json.loads(l) for l in open(D+'/turns.jsonl')]
    errs=sum(1 for t in turns if 'error' in t)
    dec=collections.Counter(json.loads(l).get('outcome') for l in open(D+'/p2p.log'))
    tot={}
    for l in open(D+'/prom.log'):
        if l.startswith('[prom] round'):
            for t in l.split(': ',1)[1].split():
                k,v=t.split('=',1); tot[k]=tot.get(k,0)+float(v)
    g=lambda k: int(tot.get(k,0))
    later=[t for t in turns if 'error' not in t and t['turn']>=1]
    return dict(wall=s['wall_s'], turns=s['turns'], errors=errs, pulled=dec.get('pulled',0),
        fetch_req=g('certus-node5-numa1.certus_fetch_requests'), fetched=g('certus-node5-numa1.certus_fetch_hits'),
        served_fetched=g('certus-node5-numa1.certus_lookup_hits_fetched'), misses=g('certus-node5-numa1.certus_fetch_misses'),
        ext_hits=g('prefill.external_prefix_cache_hits'), ext_q=g('prefill.external_prefix_cache_queries'),
        gpu_hits=g('prefill.prefix_cache_hits'), gpu_q=g('prefill.prefix_cache_queries'),
        ttft_p50=round(st.median(t['ttft_s'] for t in later),3), ttft_mean=round(st.mean(t['ttft_s'] for t in later),3),
        lat_p50=round(st.median(t['latency_s'] for t in later),2),
        per_turn={k:v['ttft_p50_s'] for k,v in s['per_turn'].items()},
        prefill_prompt_tokens=g('prefill.prompt_tokens'), prefill_gen=g('prefill.generation_tokens'))
a=arm(sys.argv[1]); b=arm(sys.argv[2])
for k in a: print(f"{k:22s} on={a[k]}   off={b[k]}")
