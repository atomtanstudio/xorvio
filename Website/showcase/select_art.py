"""Picks one variant per artwork key and queues 2x upscales of the large ones (backdrops, broadcast frames)."""
import json, sys, urllib.request
API = "http://192.168.1.100:7300"
FIRST = {"film-backdrop-1003", "film-backdrop-1007", "series-backdrop-2001", "series-backdrop-2006",
         "series-backdrop-2007", "series-backdrop-2008", "series-backdrop-2009"}   # otherwise the second variant
state = json.load(open("art/jobs.json"))
wanted = {v["prompt"]: k for k, v in state.items() if "poster" not in k}
items = json.load(urllib.request.urlopen(API + "/api/library?limit=800", timeout=60))["items"]
found = {}
for it in items:
    k = wanted.get(it.get("prompt", ""))
    if k and it.get("model") == "zimage-turbo":  # upscales carry their source's prompt
        found.setdefault(k, []).append(it)
sel = json.load(open("art/selection.json")) if __import__("os").path.exists("art/selection.json") else {}
for k, its in sorted(found.items()):
    its = sorted(its, key=lambda x: x["id"])
    pick = its[0] if (k in FIRST or len(its) == 1) else its[1]
    if sel.get(k, {}).get("asset") == pick["id"]: continue
    if k in sel and "upscale_job" not in sel[k] and "upscaled" not in sel[k] and sel[k].get("asset") == pick["id"]: continue
    sel[k] = {"asset": pick["id"]}
    if "backdrop" in k or k in ("prog-football", "prog-race", "prog-news", "prog-studio", "prog-concert", "prog-court", "prog-sky", "prog-travel", "prog-talk"):
        body = {"asset": pick["id"], "upscaler": "wonder-3.5", "values": {"scale": "2×", "detail": "Medium"}}
        r = json.load(urllib.request.urlopen(urllib.request.Request(API + "/api/upscale", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}), timeout=30))
        sel[k]["upscale_job"] = r.get("id")
print(len(sel), "selected;", sum(1 for v in sel.values() if "upscale_job" in v), "upscales queued")
json.dump(sel, open("art/selection.json", "w"), indent=1)
