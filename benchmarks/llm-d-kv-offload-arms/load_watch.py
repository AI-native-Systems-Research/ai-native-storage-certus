# Per-pod Certus load-rate watch for a running arm. Every 10 minutes, reads each
# model-server pod's vLLM kv_offload_load_bytes/time and logs the rate over the
# last window; WARNs when a pod that loaded >= 5 GB in the window ran < 10 GB/s.
import json, os, re, subprocess, sys, time, urllib.request
NS, PERIOD, LOG = os.environ.get("NS", "llm-d-quickstart"), 600, sys.argv[1]
def pods():
    out = subprocess.run(["kubectl", "-n", NS, "get", "pods", "-o", "json"], capture_output=True, text=True).stdout
    res = []
    for p in json.loads(out)["items"]:
        if p["metadata"]["name"].startswith(("optimized-baseline", "pd-disaggregation")) and p["status"].get("podIP"):
            args = p["spec"]["containers"][0].get("args", [])
            port = next((a.split("=", 1)[1] for a in args if a.startswith("--port=")), "8000")
            res.append((p["metadata"]["name"], p["spec"]["nodeName"].split(".")[0], p["status"]["podIP"], port))
    return res
def read(ip, port):
    m = urllib.request.urlopen(f"http://{ip}:{port}/metrics", timeout=10).read().decode()
    g = lambda k: sum(float(x) for x in re.findall(rf'^vllm:{k}(?:_total)?(?:{{[^}}]*}})? ([0-9.e+-]+)$', m, re.M))
    return g("kv_offload_load_bytes"), g("kv_offload_load_time")
def log(s):
    with open(LOG, "a") as f: f.write(time.strftime("%H:%M:%S", time.gmtime()) + " " + s + "\n")
prev = {}
while True:
    for name, node, ip, port in pods():
        try: b, t = read(ip, port)
        except Exception as e: log(f"load-watch {name} {node}: unreadable ({e})"); continue
        if name in prev:
            db, dt = b - prev[name][0], t - prev[name][1]
            rate = db / dt / 1e9 if dt > 0 else 0
            flag = "WARN slow load " if db >= 5e9 and rate < 10 else ""
            log(f"load-watch {flag}{node} {name[-5:]}: {db/1e9:6.1f} GB in {dt:6.1f} s = {rate:5.1f} GB/s")
        prev[name] = (b, t)
    time.sleep(PERIOD)
