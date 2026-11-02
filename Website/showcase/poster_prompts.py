"""Full-typography poster prompts for GPT Image 2.5 (Dzine), built from catalog.py."""
import json, re
from catalog import FILMS, SERIES

TYPE = {
    "thin-sans": "large, elegant, thin, widely tracked sans-serif capitals",
    "condensed": "tall, bold, condensed sans-serif capitals, tightly spaced, slightly distressed",
    "slab": "heavy, weathered slab-serif capitals with a worn texture",
    "serif-italic": "a graceful, romantic italic serif",
    "didot": "high-contrast, elegant Didone serif capitals",
    "wide-sans": "wide, geometric, extended sans-serif capitals",
    "storybook": "whimsical hand-lettered storybook serif with a soft golden glow",
    "rounded": "playful, chunky rounded lettering",
    "bodoni": "luxurious high-contrast serif capitals in brushed gold",
    "caslon": "classic old-style serif capitals",
}

def art(text):
    return re.sub(r"\s*(Empty|Large empty|Dark|Huge|Open|Black|Misty|Smoky|Golden|Blue|Pale|Hazy|High dark|Red|Snow|Clouds|Warm|Dark snowy|Dark stormy|Dark canopy|Dark sea|Dark tunnel|Dark carpet|Dark vault|Dark forest|Dark glass|Dark shelves|Shelves rising|Dark dusk|Dark foreground|Open twilight|Open sky|Misty open|Black water|Huge sky|Dark warm|Dark sky)[^.]*\bat the (top|bottom)( of the frame)?\.", "", text).strip()

def billing(names, extra):
    return ("Below the title a small, condensed, realistic credits billing block in light grey using only these names: "
            + ", ".join(names) + f", and under it \"{extra}\". ")

def film(f):
    top = "   ".join(n.upper() for n in f["cast"][:3]) if not f["cast"][0].startswith("Narrated") else f["cast"][0].upper()
    where = "near the top" if f["place"] == "top" else "near the bottom"
    other = "bottom" if f["place"] == "top" else "top"
    return (f"A premium theatrical one-sheet movie poster for a fictional {f['genre'].lower()} film. Artwork: {art(f['poster'])} "
            f"Photographic, cinematic, rich color grade, subtle film grain. Typography: the cast names \"{top}\" in small, widely spaced capitals "
            f"at the {'top' if f['place'] == 'bottom' else 'bottom'} edge. The title \"{f['title'].upper()}\" {where}, set in {TYPE[f['type']]}, "
            f"with the tagline \"{f['tagline']}\" in small type close to it. "
            + billing([f["director"]] + [c for c in f["cast"] if not c.startswith("Narrated")], f"A {f['director'].upper()} FILM")
            + "The billing block sits at the very bottom edge. No studio logos, no rating box, no websites, no dates, no other names or words.")

def series(s):
    names = [c for c in s["cast"] if not c.startswith(("Narrated", "Hosted"))] or [s["cast"][0].split(" by ")[-1]]
    where = "in the lower third"
    return (f"A premium key-art poster for a fictional {s['genre'].lower()} television series. Artwork: {art(s['poster'])} "
            f"Photographic, cinematic, rich color grade, subtle film grain. Typography: the title \"{s['title'].upper()}\" {where}, set in {TYPE[s['type']]}. "
            f"Above the title, in small widely spaced capitals, \"A {s['network'].upper()} ORIGINAL SERIES\". "
            f"Below the title, \"{'NEW SERIES' if s['seasons'] == 1 else 'SEASON ' + str(s['seasons'])}\" in small capitals, and under that a small credits line using only the names "
            + ", ".join(names + s["creators"]) + ". No studio logos, no rating box, no websites, no dates, no other names or words.")

if __name__ == "__main__":
    out = [(f"film-poster-{f['id']}", film(f)) for f in FILMS] + [(f"series-poster-{s['id']}", series(s)) for s in SERIES]
    json.dump(out, open("art/poster_prompts.json", "w"), indent=1)
    for k, p in out[:2] + out[-1:]: print(k, "::", p, "\n")
    print(len(out))
