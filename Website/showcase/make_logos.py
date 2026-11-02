"""Draws the fictional channel logos in catalog.py as transparent PNGs (art/out/logo-<id>.png)."""
import math, os
from PIL import Image, ImageDraw, ImageFont, ImageFilter
from catalog import CHANNELS

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "art", "out")
S = 4  # supersampling
W, H = 1500 * S, 360 * S
AVENIR = "/System/Library/Fonts/Avenir Next.ttc"
AVENIR_C = "/System/Library/Fonts/Avenir Next Condensed.ttc"
FUTURA = "/System/Library/Fonts/Supplemental/Futura.ttc"
DIDOT = "/System/Library/Fonts/Supplemental/Didot.ttc"
HELV = "/System/Library/Fonts/HelveticaNeue.ttc"

def font(path, index, size):
    return ImageFont.truetype(path, size * S, index=index)

def text_size(d, s, f, spacing=0):
    box = d.textbbox((0, 0), s, font=f)
    return box[2] - box[0] + spacing * S * max(0, len(s) - 1), box[3] - box[1], box

def draw_text(d, xy, s, f, fill, spacing=0):
    """Text with letter spacing, top-left at xy (ink box)."""
    x, y = xy
    if spacing == 0:
        box = d.textbbox((0, 0), s, font=f)
        d.text((x - box[0], y - box[1]), s, font=f, fill=fill)
        return
    box = d.textbbox((0, 0), s, font=f)
    for ch in s:
        d.text((x - box[0] if ch == s[0] else x, y - box[1]), ch, font=f, fill=fill)
        x += d.textlength(ch, font=f) + spacing * S

def lockup(mark, word, f, color, gap=34, spacing=0, sub=None, subf=None, subcolor=None):
    """Mark drawn by `mark(d, x, y, size)` left of a wordmark, centered on the canvas."""
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    msize = 220 * S if mark else 0
    tw, th, _ = text_size(d, word, f, spacing)
    sw = text_size(d, sub, subf, 0)[0] if sub else 0
    block_w = msize + (gap * S if mark else 0) + max(tw, sw)
    x0 = (W - block_w) // 2
    if mark:
        mark(d, x0, (H - msize) // 2, msize)
    tx = x0 + msize + (gap * S if mark else 0)
    total_h = th + ((text_size(d, sub, subf)[1] + 14 * S) if sub else 0)
    ty = (H - total_h) // 2
    draw_text(d, (tx, ty), word, f, color, spacing)
    if sub:
        draw_text(d, (tx + 4 * S, ty + th + 14 * S), sub, subf, subcolor or color, 6)
    return img

def save(img, cid):
    img = img.resize((W // S, H // S), Image.LANCZOS)
    bbox = img.getbbox()
    if bbox:
        pad = 18
        img = img.crop((max(0, bbox[0] - pad), max(0, bbox[1] - pad), min(img.width, bbox[2] + pad), min(img.height, bbox[3] + pad)))
    img.save(os.path.join(OUT, f"logo-{cid}.png"))

def hexrgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4)) + (255,)

def make(c):
    style, a, b = c["logo"]
    A, B = hexrgb(a), hexrgb(b)
    white = (255, 255, 255, 255)
    if style == "disc":  # Meridian 24
        def mark(d, x, y, s):
            d.ellipse((x, y, x + s, y + s), fill=A)
            f = font(AVENIR, 8, 118)
            w, h, box = text_size(d, "24", f)
            d.text((x + (s - w) / 2 - box[0], y + (s - h) / 2 - box[1]), "24", font=f, fill=B)
        return lockup(mark, "MERIDIAN", font(AVENIR, 0, 112), white, spacing=4)
    if style == "arrow":  # Northwind World
        def mark(d, x, y, s):
            d.polygon([(x + s * .15, y + s * .85), (x + s * .5, y + s * .1), (x + s * .85, y + s * .85), (x + s * .5, y + s * .62)], fill=A)
        return lockup(mark, "NORTHWIND", font(FUTURA, 2, 104), white, spacing=2, sub="WORLD NEWS", subf=font(FUTURA, 0, 40), subcolor=A)
    if style == "block":  # Civic Desk, one colour with knocked-out letters
        img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        f = font(HELV, 9, 150)
        w, h, box = text_size(d, "CIVIC", f, 6)
        pw, ph = w + 120 * S, h + 90 * S
        x, y = (W - pw) // 2, (H - ph) // 2
        d.rounded_rectangle((x, y, x + pw, y + ph), radius=18 * S, fill=A)
        mask = Image.new("L", (W, H), 0)
        md = ImageDraw.Draw(mask)
        draw_text(md, (x + 60 * S, y + 45 * S), "CIVIC", f, 255, 6)
        img.putalpha(Image.composite(Image.new("L", (W, H), 0), img.getchannel("A"), mask))
        return img
    if style == "shield":  # Fieldhouse Sports
        label = "FH" if c["short"] == "FH" else "FH2"
        def mark(d, x, y, s):
            pts = [(x + s * .1, y + s * .05), (x + s * .9, y + s * .05), (x + s * .9, y + s * .55), (x + s * .5, y + s * .97), (x + s * .1, y + s * .55)]
            d.polygon(pts, fill=A)
            f = font(AVENIR_C, 8, 96 if label == "FH" else 80)
            w, h, box = text_size(d, label, f)
            d.text((x + (s - w) / 2 - box[0], y + s * .42 - h / 2 - box[1]), label, font=f, fill=B)
        return lockup(mark, "FIELDHOUSE", font(AVENIR_C, 8, 120), white, spacing=2, sub="SPORTS", subf=font(AVENIR_C, 2, 44), subcolor=A)
    if style == "pill":  # Pitchside
        img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        f = font(AVENIR, 9, 108)
        w, h, box = text_size(d, "PITCHSIDE", f)
        pw, ph = w + 110 * S, h + 80 * S
        x, y = (W - pw) // 2, (H - ph) // 2
        d.rounded_rectangle((x, y, x + pw, y + ph), radius=ph // 2, fill=A)
        d.text((x + 55 * S - box[0], y + 40 * S - box[1]), "PITCHSIDE", font=f, fill=B)
        return img
    if style == "slash":  # Raceline
        def mark(d, x, y, s):
            for i in range(3):
                ox = x + i * s * .24
                d.polygon([(ox + s * .2, y + s * .15), (ox + s * .36, y + s * .15), (ox + s * .16, y + s * .85), (ox, y + s * .85)], fill=A)
        return lockup(mark, "RACELINE", font(AVENIR, 9, 124), white, gap=10)
    if style == "peak":  # Summit Sports
        def mark(d, x, y, s):
            d.polygon([(x, y + s * .9), (x + s * .42, y + s * .12), (x + s * .62, y + s * .48), (x + s * .74, y + s * .32), (x + s, y + s * .9)], fill=A)
            d.polygon([(x + s * .42, y + s * .12), (x + s * .52, y + s * .3), (x + s * .42, y + s * .26), (x + s * .33, y + s * .3)], fill=B)
        return lockup(mark, "SUMMIT", font(FUTURA, 2, 120), white, spacing=8, sub="SPORTS", subf=font(FUTURA, 0, 42), subcolor=A)
    if style == "frame":  # Reserve Cinema
        img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        f = font(DIDOT, 2, 118)
        w, h, box = text_size(d, "RESERVE", f, 10)
        pw, ph = w + 120 * S, h + 150 * S
        x, y = (W - pw) // 2, (H - ph) // 2
        d.rectangle((x, y, x + pw, y + ph), outline=A, width=7 * S)
        d.rectangle((x + 16 * S, y + 16 * S, x + pw - 16 * S, y + ph - 16 * S), outline=A, width=2 * S)
        draw_text(d, (x + 60 * S, y + 52 * S), "RESERVE", f, A, 10)
        sf = font(DIDOT, 0, 34)
        sw = text_size(d, "C I N E M A", sf)[0]
        draw_text(d, (x + (pw - sw) // 2, y + 52 * S + h + 26 * S), "C I N E M A", sf, A)
        return img
    if style == "circle-serif":  # Noir Classics, one colour
        def mark(d, x, y, s):
            d.ellipse((x, y, x + s, y + s), outline=A, width=10 * S)
            f = font(DIDOT, 1, 150)
            w, h, box = text_size(d, "N", f)
            d.text((x + (s - w) / 2 - box[0], y + (s - h) / 2 - box[1]), "N", font=f, fill=A)
        return lockup(mark, "NOIR", font(DIDOT, 0, 140), A, spacing=18, sub="CLASSICS", subf=font(DIDOT, 0, 40))
    if style == "star":  # Premiere
        def mark(d, x, y, s):
            cx, cy, r1, r2 = x + s / 2, y + s / 2, s / 2, s / 4.6
            pts = [(cx + (r1 if i % 2 == 0 else r2) * math.sin(i * math.pi / 5), cy - (r1 if i % 2 == 0 else r2) * math.cos(i * math.pi / 5)) for i in range(10)]
            d.polygon(pts, fill=A)
        return lockup(mark, "PREMIERE", font(AVENIR, 2, 110), white, spacing=10)
    if style == "glow":  # Lumen
        def mark(d, x, y, s):
            for i in range(30, 0, -1):
                t = i / 30
                col = (int(247 * (1 - t) + 255 * t), int(107 * (1 - t) + 200 * t), int(21 * (1 - t) + 90 * t), 255)
                r = s / 2 * (0.4 + 0.6 * (1 - t)) if False else s / 2 * t
                d.ellipse((x + s / 2 - r, y + s / 2 - r, x + s / 2 + r, y + s / 2 + r), fill=col)
        return lockup(mark, "lumen", font(AVENIR, 2, 150), white, gap=26)
    if style == "sun":  # Brightside
        def mark(d, x, y, s):
            cx, cy = x + s / 2, y + s / 2
            for i in range(12):
                a = i * math.pi / 6
                d.line((cx + s * .3 * math.cos(a), cy + s * .3 * math.sin(a), cx + s * .48 * math.cos(a), cy + s * .48 * math.sin(a)), fill=A, width=int(s * .07))
            d.ellipse((cx - s * .22, cy - s * .22, cx + s * .22, cy + s * .22), fill=A)
        return lockup(mark, "brightside", font(AVENIR, 0, 124), white, gap=22)
    if style == "ring":  # Halo Comedy
        def mark(d, x, y, s):
            d.ellipse((x, y + s * .25, x + s, y + s * .75), outline=A, width=int(s * .11))
        return lockup(mark, "HALO", font(FUTURA, 2, 150), white, spacing=14, sub="COMEDY", subf=font(FUTURA, 0, 44), subcolor=A)
    if style == "kite":  # Kite Kids
        def mark(d, x, y, s):
            cx = x + s / 2
            d.polygon([(cx, y), (x + s * .88, y + s * .38), (cx, y + s), (x + s * .12, y + s * .38)], fill=A)
            d.polygon([(cx, y), (x + s * .88, y + s * .38), (cx, y + s * .38)], fill=hexrgb("#FFD60A"))
            d.polygon([(cx, y + s * .38), (x + s * .12, y + s * .38), (cx, y + s)], fill=hexrgb("#0091FF"))
        return lockup(mark, "kite", font(AVENIR, 8, 160), white, sub="KIDS", subf=font(AVENIR, 0, 50), subcolor=hexrgb("#FFD60A"))
    if style == "planet":  # Little Planet
        def mark(d, x, y, s):
            d.ellipse((x + s * .2, y + s * .2, x + s * .8, y + s * .8), fill=A)
            d.arc((x - s * .02, y + s * .36, x + s * 1.02, y + s * .64), start=200, end=340, fill=hexrgb("#FFD60A"), width=int(s * .06))
            d.arc((x - s * .02, y + s * .36, x + s * 1.02, y + s * .64), start=0, end=170, fill=hexrgb("#FFD60A"), width=int(s * .06))
        return lockup(mark, "little planet", font(AVENIR, 0, 104), white, gap=20)
    if style == "leaf":  # Wild Atlas
        def mark(d, x, y, s):
            d.chord((x, y, x + s * 1.4, y + s * 1.4), start=180, end=270, fill=A)
            d.chord((x - s * .4, y - s * .4, x + s, y + s), start=0, end=90, fill=A)
            d.line((x + s * .12, y + s * .88, x + s * .88, y + s * .12), fill=(0, 0, 0, 0), width=int(s * .05))
        return lockup(mark, "WILD ATLAS", font(AVENIR, 2, 100), white, spacing=8)
    if style == "orbit":  # Orbit Science
        def mark(d, x, y, s):
            d.ellipse((x, y + s * .3, x + s, y + s * .7), outline=A, width=int(s * .06))
            d.ellipse((x + s * .32, y + s * .32, x + s * .68, y + s * .68), fill=A)
            d.ellipse((x + s * .82, y + s * .3, x + s * .98, y + s * .46), fill=hexrgb("#B4B4FF"))
        return lockup(mark, "ORBIT", font(FUTURA, 2, 140), white, spacing=10, sub="SCIENCE", subf=font(FUTURA, 0, 44), subcolor=hexrgb("#9E9EFF"))
    if style == "wave":  # Pulse Music
        def mark(d, x, y, s):
            heights = [.3, .6, .95, .55, .8, .4, .25]
            bw = s / (len(heights) * 1.6)
            for i, h in enumerate(heights):
                bx = x + i * bw * 1.6
                d.rounded_rectangle((bx, y + s * (1 - h) / 2, bx + bw, y + s * (1 + h) / 2), radius=bw / 2, fill=A)
        return lockup(mark, "PULSE", font(AVENIR, 9, 140), white, gap=24, spacing=4)
    if style == "moon":  # Low Light Jazz, one colour
        def mark(d, x, y, s):
            d.ellipse((x + s * .1, y + s * .1, x + s * .9, y + s * .9), fill=B)
            d.ellipse((x + s * .32, y + s * .02, x + s * 1.02, y + s * .72), fill=(0, 0, 0, 0))
        return lockup(mark, "LOW LIGHT", font(DIDOT, 0, 108), B, spacing=10, sub="JAZZ", subf=font(DIDOT, 1, 46))
    if style == "flame":  # Hearth
        def mark(d, x, y, s):
            d.ellipse((x + s * .2, y + s * .35, x + s * .8, y + s * .95), fill=A)
            d.polygon([(x + s * .5, y), (x + s * .78, y + s * .55), (x + s * .22, y + s * .55)], fill=A)
            d.ellipse((x + s * .36, y + s * .58, x + s * .64, y + s * .88), fill=hexrgb("#FFB224"))
        return lockup(mark, "hearth", font(AVENIR, 2, 150), white, gap=20)
    if style == "compass":  # Wanderline
        def mark(d, x, y, s):
            cx, cy = x + s / 2, y + s / 2
            d.ellipse((x + s * .05, y + s * .05, x + s * .95, y + s * .95), outline=A, width=int(s * .05))
            d.polygon([(cx, y + s * .12), (cx + s * .1, cy), (cx, y + s * .88), (cx - s * .1, cy)], fill=A)
            d.polygon([(x + s * .12, cy), (cx, cy - s * .1), (x + s * .88, cy), (cx, cy + s * .1)], fill=(255, 255, 255, 255))
        return lockup(mark, "WANDERLINE", font(AVENIR, 2, 96), white, spacing=8)
    raise ValueError(style)

if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for c in CHANNELS:
        save(make(c), c["id"])
    print("logos:", len(CHANNELS))
