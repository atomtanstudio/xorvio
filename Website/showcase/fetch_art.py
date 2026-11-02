"""Downloads finished artwork from H3LIX into art/raw/<key>-<n>.<ext>, matching jobs by prompt."""
import json, os, urllib.request
API = "http://192.168.1.100:7300"
state = json.load(open("art/jobs.json"))
posters = json.load(open("art/poster_jobs.json")) if os.path.exists("art/poster_jobs.json") else {}
wanted = {v["prompt"]: k for k, v in state.items() if "poster" not in k}
wanted.update({v["prompt"]: "gpt-" + k for k, v in posters.items() if "prompt" in v})
os.makedirs("art/raw", exist_ok=True)
items = json.load(urllib.request.urlopen(API + "/api/library?limit=700", timeout=60))["items"]
found = {}
for it in items:
    key = wanted.get(it.get("prompt", ""))
    if key:
        found.setdefault(key, []).append(it)
for k, v in posters.items():
    if "asset" in v:
        found["gpt-" + k] = [json.load(urllib.request.urlopen(API + f"/api/assets/{v['asset']}", timeout=30))]
new = 0
for key, its in found.items():
    for n, it in enumerate(sorted(its, key=lambda x: x["id"])):
        path = f"art/raw/{key}-{n+1}"
        if any(os.path.exists(path + e) for e in (".png", ".jpg", ".webp")): continue
        res = urllib.request.urlopen(API + it["url"], timeout=120)
        ext = {"image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp"}.get(res.headers.get_content_type(), ".png")
        open(path + ext, "wb").write(res.read()); new += 1
print(f"{len(found)}/{len(wanted) + 1} keys; {new} new files")
print("missing:", sorted(set(wanted.values()) - set(found)))
