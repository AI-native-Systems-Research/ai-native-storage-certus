#!/usr/bin/env python3
"""Attribute each turn of a multiturn_router run to the pod the EPP picked, and
count session moves (a later turn on a different pod than the one before).

The gateway replaces client request ids and the EPP logs no request content,
so turns are matched to picks by time: the driver's send times and the EPP's
pick times, each sorted, paired in order. Valid only while no two sends fall
closer together than the router's own jitter: the gap is reported."""
import json,sys,collections,datetime
turns=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
picks=[]
for l in open(sys.argv[2]):
    try: d=json.loads(l)
    except ValueError: continue
    if 'TargetEndpoints' not in l: continue
    ts=datetime.datetime.fromisoformat(d['timestamp'][:26].rstrip('Z')).replace(tzinfo=datetime.timezone.utc).timestamp()
    picks.append((ts,d['result']['TargetEndpoints'][0]['Endpoint']['Name'][-5:]))
picks.sort(); sent=sorted(turns,key=lambda t:t['sent'])
assert len(picks)==len(sent), f"{len(picks)} picks for {len(sent)} turns"
lag=[p[0]-t['sent'] for p,t in zip(picks,sent)]
gaps=[b['sent']-a['sent'] for a,b in zip(sent,sent[1:])]
for p,t in zip(picks,sent): t['pod']=p[1]
moves=0; later=0; byturn=collections.Counter(); prev={}
for t in sorted(turns,key=lambda t:(t['session'],t['turn'])):
    if t['turn']>0:
        later+=1
        if prev.get(t['session']) not in (None,t['pod']): moves+=1; byturn[t['turn']]+=1
    prev[t['session']]=t['pod']
print(json.dumps(dict(turns=len(turns),later=later,moves=moves,moves_by_turn=dict(byturn),
    pods=collections.Counter(t['pod'] for t in turns),
    send_to_pick_s=[round(min(lag),3),round(max(lag),3)],min_send_gap_s=round(min(gaps),3))))
