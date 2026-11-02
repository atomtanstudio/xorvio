"""Builds dist/og.jpg, the 1200 x 630 social preview, from a real capture."""
import os, subprocess
from PIL import Image, ImageDraw, ImageFilter, ImageFont
HERE = os.path.dirname(os.path.abspath(__file__))
W, H = 1200, 630
img = Image.new("RGB", (W, H), (5, 7, 10))
glow = Image.new("RGB", (W, H), (5, 7, 10))
d = ImageDraw.Draw(glow)
d.ellipse((520, 140, 1340, 820), fill=(48, 70, 170))
d.ellipse((380, 380, 900, 820), fill=(52, 60, 190))
img = Image.blend(img, glow.filter(ImageFilter.GaussianBlur(140)), 0.75)
shot = Image.open(os.path.join(HERE, "shots/home.png")).convert("RGBA")
shot = shot.resize((900, 562), Image.LANCZOS)
shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
ImageDraw.Draw(shadow).rounded_rectangle((470, 205, 470 + 900, 205 + 562), radius=22, fill=(0, 0, 0, 200))
img = Image.alpha_composite(img.convert("RGBA"), shadow.filter(ImageFilter.GaussianBlur(30)))
img.alpha_composite(shot, (450, 175))
# Brand lockup from the vector source, then the headline.
subprocess.run(["rsvg-convert", "-h", "56", os.path.join(HERE, "../Design/Brand/xorvio-lockup.svg"), "-o", "/tmp/xorvio-og-lockup.png"], check=True)
lock = Image.open("/tmp/xorvio-og-lockup.png").convert("RGBA")
img.alpha_composite(lock, (64, 64))
font = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 64)
try:
    font.set_variation_by_name("Bold")
except Exception:
    pass
small = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 24)
draw = ImageDraw.Draw(img)
y = 190
for line in ["Everything", "you watch,", "in its best", "light."]:
    draw.text((64, y), line, font=font, fill=(243, 245, 247))
    y += 70
draw.text((66, y + 22), "Free and open source · Mac, Windows, Linux", font=small, fill=(121, 174, 255))
img.convert("RGB").save(os.path.join(HERE, "dist/og.jpg"), quality=88, optimize=True, progressive=True)
print("wrote dist/og.jpg")
