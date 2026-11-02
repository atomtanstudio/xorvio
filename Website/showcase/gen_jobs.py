"""Builds the artwork job list from catalog.py: (key, aspect, prompt)."""
from catalog import *

def jobs():
    out = []
    for f in FILMS:
        out.append((f"film-poster-{f['id']}", "2:3", POSTER + f["poster"] + " " + STYLE))
        out.append((f"film-backdrop-{f['id']}", "16:9", BACKDROP + f["backdrop"] + " " + STYLE))
    for s in SERIES:
        out.append((f"series-poster-{s['id']}", "2:3", POSTER.replace("movie", "television series") + s["poster"] + " " + STYLE))
        out.append((f"series-backdrop-{s['id']}", "16:9", BACKDROP + s["backdrop"] + " " + STYLE))
    for sid, stills in EPISODE_STILLS.items():
        for i, p in enumerate(stills):
            out.append((f"still-{sid}-{i+1}", "16:9", "Cinematic television drama still with no text. " + p + " " + STYLE))
    for key, p in PROGRAMME_ART.items():
        out.append((f"prog-{key}", "16:9", "Wide broadcast television image with no text. " + p + " Photographic, high detail, vivid, sharp."))
    return out
