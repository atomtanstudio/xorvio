"""Writes the served artwork to art/out/<key>.jpg from the selected H3LIX images,
using each one's 2x upscale once it exists (an upscale lists its source in madeFrom)."""
import io, json, os, urllib.request
from PIL import Image

API = "http://192.168.1.100:7300"
os.makedirs("art/cache", exist_ok=True)
os.makedirs("art/out", exist_ok=True)
sel = json.load(open("art/selection.json"))
posters = json.load(open("art/poster_jobs.json"))
items = json.load(urllib.request.urlopen(API + "/api/library?limit=1000", timeout=60))["items"]
by_prompt = {it.get("prompt", ""): it for it in items if it.get("model", "").startswith("dzine")}

def asset(asset_id):
    return json.load(urllib.request.urlopen(API + f"/api/assets/{asset_id}", timeout=30))

# Upscales are the library items no generator made; map each source to its upscale.
upscale_of = {}
for it in items:
    if it.get("model") in ("zimage-turbo",) or it.get("model", "").startswith("dzine"):
        continue
    for parent in asset(it["id"]).get("madeFrom", []):
        upscale_of[parent if isinstance(parent, str) else parent.get("id")] = it["id"]

def original(asset_id):
    cached = [f for f in os.listdir("art/cache") if f.startswith(asset_id)]
    if cached:
        return Image.open("art/cache/" + cached[0])
    res = urllib.request.urlopen(API + f"/files/{asset_id}/original", timeout=120)
    data = res.read()
    ext = ".jpg" if res.headers.get_content_type() == "image/jpeg" else ".png"
    open(f"art/cache/{asset_id}{ext}", "wb").write(data)
    return Image.open(io.BytesIO(data))

def save(img, key, width, quality=90):
    img = img.convert("RGB")
    if img.width > width:
        img = img.resize((width, round(img.height * width / img.width)), Image.LANCZOS)
    img.save(f"art/out/{key}.jpg", quality=quality, optimize=True, progressive=True)

upscaled = 0
for key, v in sel.items():
    chosen = upscale_of.get(v["asset"], v["asset"])
    upscaled += chosen != v["asset"]
    save(original(chosen), key, 3840 if ("backdrop" in key or key.startswith("prog-")) else 1600)
pending = []
for key, v in posters.items():
    aid = v.get("asset") or (by_prompt.get(v.get("prompt", "")) or {}).get("id")
    if not aid:
        pending.append(key)
        continue
    save(original(aid), key, 1000)
print(f"wrote {len(sel)} images ({upscaled} upscaled) and {len(posters) - len(pending)} posters; pending: {pending}")
