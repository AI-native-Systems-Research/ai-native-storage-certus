#!/usr/bin/env python3
"""Count session moves under aggregated serving: run multiturn_router's turns one
at a time (concurrency 1 within a measurement), and attribute each turn to the pod
whose vllm:request_success_total rose. Background sessions add load if --load N."""
import json,subprocess,sys,threading,time,urllib.request,re,random
sys.path.insert(0,'/home/scooter/ai-native-storage-certus/benchmarks/multiturn-router')
import multiturn_router as M
ns='llm-d-quickstart'
pods=json.loads(subprocess.check_output(['kubectl','get','pods','-n',ns,'-o','json']))['items']
pods=[(p['metadata']['name'][-5:],p['status']['podIP'],p['spec']['nodeName'].split('.')[0]) for p in pods if 'vllm-decode' in p['metadata']['name']]
R=subprocess.check_output(['kubectl','get','svc','-n',ns,'quickstart-epp','-o','jsonpath={.spec.clusterIP}']).decode()
RI=[p for p in json.loads(subprocess.check_output(['kubectl','get','pods','-n',ns,'-o','json']))['items'] if p['metadata']['name'].startswith('render')][0]['status']['podIP']
def counts():
    out={}
    for n,ip,_ in pods:
        t=urllib.request.urlopen(f'http://{ip}:8000/metrics',timeout=5).read().decode()
        out[n]=sum(float(m) for m in re.findall(r'^vllm:request_success_total\{[^}]*\} ([0-9.e+]+)',t,re.M))
    return out
tok=M.Tokenizer(f'http://{RI}:8000','Qwen/Qwen3-8B',60)
convs=json.load(open('/home/scooter/ai-native-storage-certus/data/sharegpt_12turn_450.json'))
random.Random(int(sys.argv[2]) if len(sys.argv)>2 else 7).shuffle(convs)
convs=[c for c in convs if len(M.human_turns(c))>=5]
nsess=int(sys.argv[1]) if len(sys.argv)>1 else 12
SYSTEM=("You are a helpful assistant. "*int(sys.argv[3])) if len(sys.argv)>3 else "You are a helpful assistant."
tag=f"[MOVES-{int(time.time())}] "
sep,cue=tok("\n\nUser: "),tok("\n\nAssistant:")
url=f'http://{R}/v1/completions'
log=[]
for i,c in enumerate(convs[:nsess]):
    prompt=tok(tag+f"s{i} "+SYSTEM)
    prev=None
    for turn,text in enumerate(M.human_turns(c)[:5]):
        prompt=prompt+sep+tok(text)+cue
        b=counts()
        ans,ttft,tot=M.stream(url,{"model":"Qwen/Qwen3-8B","prompt":prompt,"max_tokens":300,"temperature":0,"ignore_eos":True},300)
        time.sleep(0.3)
        a=counts()
        served=[n for n in a if a[n]>b[n]]
        pod=served[0] if len(served)==1 else ('?' if not served else '+'.join(served))
        rec=dict(session=i,turn=turn,pod=pod,moved=(prev is not None and pod!=prev),prompt=len(prompt),ttft=round(ttft or tot,3))
        log.append(rec); print(json.dumps(rec),flush=True)
        prev=pod; prompt=prompt+ans
later=[r for r in log if r['turn']>0]
print("SUMMARY", json.dumps(dict(pods={n:node for n,_,node in pods}, turns=len(log), later=len(later), moves=sum(r['moved'] for r in later), unattributed=sum(1 for r in log if len(r['pod'])!=5))))
