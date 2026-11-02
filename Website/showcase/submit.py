import json, sys, urllib.request
from gen_jobs import jobs
API = "http://192.168.1.100:7300"
only = set(sys.argv[1:])
state = json.load(open("art/jobs.json")) if __import__("os").path.exists("art/jobs.json") else {}
for key, aspect, prompt in jobs():
    if only and key not in only: continue
    if key in state and not only: continue
    count = 2 if ("poster" in key or "backdrop" in key) else 1
    body = {"model": "zimage-turbo", "mode": "generate", "values": {"prompt": prompt, "aspect": aspect, "size": "2 MP", "count": count}, "refs": {}}
    req = urllib.request.Request(API + "/api/create", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    res = json.load(urllib.request.urlopen(req, timeout=30))
    state[key] = {"job": res.get("id"), "prompt": prompt}
    print(key, res.get("id"), flush=True)
    json.dump(state, open("art/jobs.json", "w"), indent=1)
