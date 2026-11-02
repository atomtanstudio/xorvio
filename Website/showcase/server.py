"""A local Xtream Codes-style server for Xorvio's showcase screenshots.

Serves the fictional library in catalog.py: player_api.php (account, categories,
films, series, details), xmltv.php (a guide around the current time), artwork,
looping "live" HLS streams and long still-frame films. Everything is generated
from art/out, so the app shows realistic screens with nobody's real content.

    python3 server.py [port]      # default 8765; user "showcase", password "xorvio"
"""
import json, math, os, subprocess, sys, time, threading
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
from xml.sax.saxutils import escape

from catalog import *
import episodes as episode_data

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
HERE = os.path.dirname(os.path.abspath(__file__))
ART = os.path.join(HERE, "art", "out")
MEDIA = os.path.join(HERE, "art", "media")
BASE = f"http://127.0.0.1:{PORT}"
START = time.time()
LOCAL_TZ = datetime.now().astimezone().tzinfo
TZ_NAME = os.path.realpath("/etc/localtime").split("zoneinfo/")[-1] if "zoneinfo/" in os.path.realpath("/etc/localtime") else "UTC"
SEGMENT = 6  # seconds per live HLS segment
SEGMENTS = 5  # distinct segment files per stream

def img(name):
    return f"{BASE}/img/{name}.jpg"

def art_url(key):
    """Artwork for a programme art key ("film:1002", "series:2001", "football")."""
    if key is None:
        return None
    if key.startswith("film:"):
        return img(f"film-backdrop-{key[5:]}")
    if key.startswith("series:"):
        return img(f"series-backdrop-{key[7:]}")
    return img(f"prog-{key}")

def art_file(key):
    if key.startswith("film:"):
        return os.path.join(ART, f"film-backdrop-{key[5:]}.jpg")
    if key.startswith("series:"):
        return os.path.join(ART, f"series-backdrop-{key[7:]}.jpg")
    return os.path.join(ART, f"prog-{key}.jpg")

# ---------------------------------------------------------------- library

def added(days_ago):
    return int(START - days_ago * 86400)

def film_entry(i, f):
    return {
        "num": i + 1, "name": f"{f['title']} ({f['year']})", "title": f["title"], "year": str(f["year"]),
        "stream_type": "movie", "stream_id": f["id"], "stream_icon": img(f"film-poster-{f['id']}"),
        "rating": str(f["rating"]), "rating_5based": round(f["rating"] / 2, 1),
        "added": str(added(i * 3 + (0 if f["year"] >= 2025 else 40))), "category_id": f["category"],
        "container_extension": "mp4", "plot": f["plot"], "genre": f["genre"], "cast": ", ".join(f["cast"]),
        "director": f["director"], "releaseDate": f"{f['year']}-{(int(f['id']) % 12) + 1:02d}-{(int(f['id']) % 27) + 1:02d}",
        "backdrop_path": [img(f"film-backdrop-{f['id']}")], "mpaa_rating": f["age"],
    }

def series_entry(i, s):
    return {
        "num": i + 1, "name": s["title"], "series_id": s["id"], "cover": img(f"series-poster-{s['id']}"),
        "plot": s["plot"], "cast": ", ".join(s["cast"]), "director": ", ".join(s["creators"]), "genre": s["genre"],
        "releaseDate": f"{s['year']}-03-14", "last_modified": str(added(i * 2)), "rating": str(s["rating"]),
        "rating_5based": round(s["rating"] / 2, 1), "backdrop_path": [img(f"series-backdrop-{s['id']}")],
        "category_id": s["category"], "age": s["age"],
    }

def video_info(kind):
    if kind == "1080p":
        return {"width": 1920, "height": 1080, "codec_name": "h264", "color_transfer": "bt709"}
    return {"width": 3840, "height": 2160, "codec_name": "hevc", "color_transfer": "smpte2084" if kind == "4k-hdr" else "bt709"}

def vod_info(f):
    entry = film_entry(FILMS.index(f), f)
    info = dict(entry)
    info.update({
        "releasedate": entry["releaseDate"], "duration_secs": f["runtime"] * 60,
        "duration": f"{f['runtime'] // 60:02d}:{f['runtime'] % 60:02d}:00", "country": "United Kingdom" if int(f["id"]) % 3 == 0 else "United States",
        "tagline": f["tagline"], "o_name": f["title"], "video": video_info(f["video"]),
        "audio": {"channels": 6 if f["video"] != "1080p" else 2, "codec_name": "eac3"},
        "movie_image": img(f"film-poster-{f['id']}"),
    })
    return {"info": info, "movie_data": {"stream_id": f["id"], "name": f["title"], "container_extension": "mp4", "category_id": f["category"]}}

def series_info(s):
    seasons, episodes = [], {}
    stills = EPISODE_STILLS.get(s["id"], [])
    for season in range(1, s["seasons"] + 1):
        year = s["year"] + season - 1
        seasons.append({"season_number": season, "name": f"Season {season}", "air_date": f"{year}-03-14",
                        "episode_count": s["episodes"], "cover_big": img(f"series-poster-{s['id']}"),
                        "overview": s["plot"] if season == 1 else f"Season {season} of {s['title']}."})
        eps = []
        for n in range(s["episodes"]):
            still = img(f"still-{s['id']}-{(n + season - 1) % len(stills) + 1}") if stills else img(f"series-backdrop-{s['id']}")
            air = datetime(year, 3, 14) + timedelta(days=7 * n)
            eps.append({
                "id": f"{s['id']}{season:02d}{n + 1:02d}", "episode_num": n + 1, "season": season, "title": episode_data.title(s["id"], season, n + 1),
                "container_extension": "mp4",
                "info": {"plot": episode_data.plot(s["id"], season, n + 1), "duration_secs": (52 if "Comedy" not in s["genre"] else 28) * 60 + n * 37,
                         "movie_image": still, "air_date": air.strftime("%Y-%m-%d")},
            })
        episodes[str(season)] = eps
    info = series_entry(SERIES.index(s), s)
    info["country"] = "United Kingdom" if int(s["id"]) % 2 == 0 else "United States"
    return {"info": info, "seasons": seasons, "episodes": episodes}

# ---------------------------------------------------------------- guide

def featured_now(channel):
    """Programmes placed on air at screenshot time, keyed by channel id: (pool index, minutes already played)."""
    return {"201": (0, 38), "204": (1, 22), "202": (4, 51), "101": (4, 17), "601": (0, 24), "301": (1, 47),
            "401": (1, 12), "701": (0, 31), "801": (0, 9), "501": (2, 40)}.get(channel["id"])

def schedule(channel):
    pool = PROGRAMMES[channel["cat"]]
    now = datetime.fromtimestamp(START, LOCAL_TZ).replace(second=0, microsecond=0)
    feat = featured_now(channel)
    k = int(channel["id"]) % len(pool)
    offset = (int(channel["id"]) * 7) % 45
    if feat:
        k, offset = feat
    anchor = now - timedelta(minutes=offset)
    anchor = anchor.replace(minute=(anchor.minute // 5) * 5)
    slot = lambda m: int(math.ceil(m / 15.0) * 15)
    items = []
    t, i = anchor, k
    while t < now + timedelta(days=3):
        p = pool[i % len(pool)]
        d = slot(p[1])
        items.append((t, t + timedelta(minutes=d), p, i))
        t += timedelta(minutes=d)
        i += 1
    t, i = anchor, k - 1
    while t > now - timedelta(days=2):
        p = pool[i % len(pool)]
        d = slot(p[1])
        items.insert(0, (t - timedelta(minutes=d), t, p, i))
        t -= timedelta(minutes=d)
        i -= 1
    return items

def current_art(channel):
    now = datetime.fromtimestamp(time.time(), LOCAL_TZ)
    for start, stop, p, _ in schedule(channel):
        if start <= now < stop:
            return p[3]
    return None

def stream_key(channel):
    key = current_art(channel)
    if key and os.path.exists(art_file(key)):
        return key
    return {"news": "news", "sports": "football", "movies": "film:1002", "ent": "series:2001", "kids": "film:1008",
            "doc": "film:1015", "music": "concert", "life": "series:2009"}[channel["cat"]]

def xmltv():
    fmt = lambda d: d.strftime("%Y%m%d%H%M%S %z")
    out = ['<?xml version="1.0" encoding="UTF-8"?>', '<tv generator-info-name="xorvio-showcase">']
    for c in CHANNELS:
        out.append(f'<channel id="{c["id"]}.showcase"><display-name>{escape(c["name"])}</display-name>'
                   f'<icon src="{img("logo-" + c["id"])}"/></channel>')
    for c in CHANNELS:
        for start, stop, (title, _, desc, art, live), idx in schedule(c):
            out.append(f'<programme start="{fmt(start)}" stop="{fmt(stop)}" channel="{c["id"]}.showcase">')
            out.append(f'<title lang="en">{escape(title)}</title>')
            if art and art.startswith("series:"):
                sid = art[7:]
                s = next(x for x in SERIES if x["id"] == sid)
                ep = (idx % s["episodes"])
                out.append(f'<sub-title lang="en">{escape(episode_data.title(sid, 1, ep + 1))}</sub-title>')
                out.append(f'<episode-num system="onscreen">S1 E{ep + 1}</episode-num>')
            out.append(f'<desc lang="en">{escape(desc)}</desc>')
            out.append(f'<category lang="en">{escape(dict(CHANNEL_CATEGORIES)[c["cat"]])}</category>')
            if art_url(art):
                out.append(f'<icon src="{art_url(art)}"/>')
            if live:
                out.append("<live/>")
            out.append("</programme>")
    out.append("</tv>")
    return "\n".join(out)

# ---------------------------------------------------------------- media

lock = threading.Lock()

def ensure_live(key):
    """Five 6-second segments of a still frame, served as an endless live playlist."""
    folder = os.path.join(MEDIA, "live-" + key.replace(":", "-"))
    if os.path.exists(os.path.join(folder, f"seg{SEGMENTS - 1}.ts")):
        return folder
    with lock:
        os.makedirs(folder, exist_ok=True)
        for n in range(SEGMENTS):
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-loop", "1", "-i", art_file(key),
                            "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", str(SEGMENT),
                            "-vf", "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,format=yuv420p",
                            "-r", "25", "-c:v", "libx264", "-tune", "stillimage", "-preset", "veryfast", "-g", "50",
                            "-c:a", "aac", "-b:a", "64k", "-output_ts_offset", str(n * SEGMENT),
                            "-f", "mpegts", os.path.join(folder, f"seg{n}.ts")], check=True)
    return folder

def ensure_film(key, minutes):
    """A full-length film made of one still frame (tiny, but with a real duration for the scrubber)."""
    os.makedirs(MEDIA, exist_ok=True)
    path = os.path.join(MEDIA, f"film-{key.replace(':', '-')}-{minutes}.mp4")
    if os.path.exists(path):
        return path
    with lock:
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-loop", "1", "-framerate", "1", "-i", art_file(key),
                        "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", str(minutes * 60),
                        "-vf", "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,format=yuv420p",
                        "-c:v", "libx264", "-tune", "stillimage", "-preset", "veryfast", "-g", "600",
                        "-c:a", "aac", "-b:a", "16k", "-movflags", "+faststart", path], check=True)
    return path

def live_playlist(key):
    ensure_live(key)
    seq = int(time.time() // SEGMENT)
    lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEGMENT}", f"#EXT-X-MEDIA-SEQUENCE:{seq}"]
    for n in range(seq, seq + 4):
        if n % SEGMENTS == 0:
            lines.append("#EXT-X-DISCONTINUITY")
        lines += [f"#EXTINF:{SEGMENT}.0,", f"/seg/{key.replace(':', '-')}/{n % SEGMENTS}.ts"]
    return "\n".join(lines) + "\n"

# ---------------------------------------------------------------- HTTP

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        if os.environ.get("SHOWCASE_LOG"):
            sys.stderr.write("%s\n" % (fmt % args))

    def send(self, body, ctype="application/json", status=200):
        if isinstance(body, str):
            body = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def send_file(self, path, ctype):
        if not os.path.exists(path):
            return self.send("not found", "text/plain", 404)
        size = os.path.getsize(path)
        start, end = 0, size - 1
        rng = self.headers.get("Range")
        if rng and rng.startswith("bytes="):
            a, _, b = rng[6:].partition("-")
            start = int(a) if a else 0
            end = int(b) if b else size - 1
        self.send_response(206 if rng else 200)
        self.send_header("Content-Type", ctype)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        if rng:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if self.command == "HEAD":
            return
        with open(path, "rb") as fh:
            fh.seek(start)
            remaining = end - start + 1
            while remaining > 0:
                chunk = fh.read(min(1 << 20, remaining))
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return
                remaining -= len(chunk)

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        url = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(url.query).items()}
        parts = [p for p in url.path.split("/") if p]
        if url.path.endswith("player_api.php"):
            return self.api(q)
        if url.path.endswith("xmltv.php"):
            return self.send(xmltv(), "application/xml")
        if parts and parts[0] == "img" and len(parts) == 2:
            return self.send_file(os.path.join(ART, parts[1]), "image/png" if parts[1].startswith("logo-") else "image/jpeg")
        if parts and parts[0] == "seg" and len(parts) == 3:
            key = parts[1].replace("-", ":", 1) if parts[1].startswith(("film-", "series-")) else parts[1]
            return self.send_file(os.path.join(ensure_live(key), f"seg{parts[2].split('.')[0]}.ts"), "video/mp2t")
        if parts and parts[0] in ("live", "timeshift") and parts[-1].endswith((".m3u8", ".ts")):
            cid = parts[-1].split(".")[0]
            channel = next((c for c in CHANNELS if c["id"] == cid), CHANNELS[0])
            return self.send(live_playlist(stream_key(channel)), "application/vnd.apple.mpegurl")
        if parts and parts[0] == "movie" and len(parts) == 4:
            fid = parts[3].split(".")[0]
            f = next((x for x in FILMS if x["id"] == fid), FILMS[0])
            return self.send_file(ensure_film("film:" + f["id"], f["runtime"]), "video/mp4")
        if parts and parts[0] == "series" and len(parts) == 4:
            eid = parts[3].split(".")[0]
            s = next((x for x in SERIES if eid.startswith(x["id"])), SERIES[0])
            n = int(eid[-2:]) if eid[-2:].isdigit() else 1
            stills = EPISODE_STILLS.get(s["id"])
            key = f"still-{s['id']}-{(n - 1) % len(stills) + 1}" if stills else None
            if key and os.path.exists(os.path.join(ART, key + ".jpg")):
                return self.send_file(ensure_film(key, 52), "video/mp4")
            return self.send_file(ensure_film("series:" + s["id"], 52), "video/mp4")
        return self.send("not found", "text/plain", 404)

    def api(self, q):
        if q.get("username") != "showcase" or q.get("password") != "xorvio":
            return self.send(json.dumps({"user_info": {"auth": 0}}))
        action = q.get("action")
        if not action:
            return self.send(json.dumps({
                "user_info": {"username": "showcase", "auth": 1, "status": "Active", "exp_date": str(int(START + 300 * 86400)),
                              "is_trial": "0", "active_cons": "0", "max_connections": "4", "allowed_output_formats": ["m3u8", "ts"]},
                "server_info": {"url": "127.0.0.1", "port": str(PORT), "server_protocol": "http",
                                "timezone": TZ_NAME,
                                "timestamp_now": int(time.time())},
            }))
        if action == "get_live_categories":
            return self.send(json.dumps([{"category_id": k, "category_name": v, "parent_id": 0} for k, v in CHANNEL_CATEGORIES]))
        if action == "get_vod_categories":
            return self.send(json.dumps([{"category_id": k, "category_name": v, "parent_id": 0} for k, v in FILM_CATEGORIES]))
        if action == "get_series_categories":
            return self.send(json.dumps([{"category_id": k, "category_name": v, "parent_id": 0} for k, v in SERIES_CATEGORIES]))
        if action == "get_live_streams":
            return self.send(json.dumps([{
                "num": c["num"], "name": c["name"], "stream_type": "live", "stream_id": c["id"], "stream_icon": img("logo-" + c["id"]),
                "epg_channel_id": c["id"] + ".showcase", "added": str(added(30)), "category_id": c["cat"],
                "tv_archive": 1 if c["cat"] in ("news", "sports", "ent", "doc") else 0, "tv_archive_duration": 7,
            } for c in CHANNELS]))
        if action == "get_vod_streams":
            return self.send(json.dumps([film_entry(i, f) for i, f in enumerate(FILMS)]))
        if action == "get_series":
            return self.send(json.dumps([series_entry(i, s) for i, s in enumerate(SERIES)]))
        if action == "get_vod_info":
            f = next((x for x in FILMS if x["id"] == q.get("vod_id")), None)
            return self.send(json.dumps(vod_info(f) if f else {}))
        if action == "get_series_info":
            s = next((x for x in SERIES if x["id"] == q.get("series_id")), None)
            return self.send(json.dumps(series_info(s) if s else {}))
        if action in ("get_short_epg", "get_simple_data_table"):
            return self.send(json.dumps({"epg_listings": []}))
        return self.send("[]")

if __name__ == "__main__":
    print(f"Showcase server on {BASE} (user showcase / password xorvio)", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
