#!/usr/bin/env python3
"""Builds Xorvio's app icon from the logo geometry.

Writes xorvio-app-icon.svg here, the PNG sizes into App/Assets.xcassets/AppIcon.appiconset
and App/Resources/AppIcon.icns. Needs rsvg-convert (brew install librsvg) and iconutil.
Run from anywhere: python3 Design/Brand/make_icon.py
"""
import math, os, shutil, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..'))

# ---- The mark (120-unit square, the shape spans 16…104), as in BrandMark.swift.
R, GAP = 12.0, 5.0
S2 = math.sqrt(0.5)

def band(a, b, round_a=True, round_b=True):
    ux, uy = b[0]-a[0], b[1]-a[1]; L = math.hypot(ux, uy); ux, uy = ux/L, uy/L
    nx, ny = -uy, ux
    pt = lambda p, s: f'{p[0]+s*R*nx:.3f},{p[1]+s*R*ny:.3f}'
    d = f'M{pt(a,1)} L{pt(b,1)} '
    d += f'A{R:g},{R:g} 0 0 0 {pt(b,-1)} ' if round_b else f'L{pt(b,-1)} '
    d += f'L{pt(a,-1)} '
    d += f'A{R:g},{R:g} 0 0 0 {pt(a,1)} ' if round_a else f'L{pt(a,1)} '
    return d + 'Z'

cut = 64/math.sqrt(2) - (R + GAP)
q1 = (28 + cut*S2, 92 - cut*S2); q2 = (120-q1[0], 120-q1[1])
TOP = band((28,28), (92,92))
LOW = band((28,92), q1, round_b=False) + ' ' + band(q2, (92,28), round_a=False)

# ---- The tile: Apple's icon grid (824-point shape on a 1024 canvas). A superellipse
# with exponent 5 matches the continuous-corner shape closely.
def squircle(cx, cy, half, n=5.0, steps=720):
    pts = []
    for i in range(steps):
        t = 2*math.pi*i/steps
        c, s = math.cos(t), math.sin(t)
        x = half * math.copysign(abs(c)**(2/n), c)
        y = half * math.copysign(abs(s)**(2/n), s)
        pts.append(f'{cx+x:.2f},{cy+y:.2f}')
    return 'M' + ' L'.join(pts) + ' Z'

TILE = squircle(512, 512, 412)

def icon_svg():
    k = 470/88                      # the mark's shape is 470 px across
    ox = 512 - 60*k; oy = 512 - 60*k + 4
    mark_t = f'translate({ox:.2f},{oy:.2f}) scale({k:.4f})'
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
<defs>
  <linearGradient id="bg" x1="0" y1="100" x2="0" y2="924" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#25355A"/><stop offset="0.55" stop-color="#141D33"/><stop offset="1" stop-color="#0A0F1C"/>
  </linearGradient>
  <radialGradient id="glow" cx="512" cy="560" r="380" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#4C7DFF" stop-opacity="0.42"/><stop offset="0.6" stop-color="#3B5BD9" stop-opacity="0.12"/><stop offset="1" stop-color="#3B5BD9" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="rim" x1="0" y1="100" x2="0" y2="924" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.32"/><stop offset="0.18" stop-color="#FFFFFF" stop-opacity="0.06"/>
    <stop offset="0.85" stop-color="#FFFFFF" stop-opacity="0.02"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0.10"/>
  </linearGradient>
  <linearGradient id="top" x1="16" y1="16" x2="104" y2="104" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#E4EFFF"/><stop offset="1" stop-color="#5C93FF"/></linearGradient>
  <linearGradient id="low" x1="16" y1="104" x2="104" y2="16" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#4353E0"/><stop offset="1" stop-color="#8DB0FF"/></linearGradient>
  <linearGradient id="sheen" x1="68.49" y1="51.51" x2="60" y2="60" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.38"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
  </linearGradient>
  <clipPath id="tileclip"><path d="{TILE}"/></clipPath>
  <!-- A faint grain dithers the dark gradient, which would otherwise show steps. -->
  <filter id="grain" x="0" y="0" width="100%" height="100%" filterUnits="userSpaceOnUse">
    <feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" seed="7" result="n"/>
    <feColorMatrix in="n" type="matrix" values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  1.6 0 0 0 -0.3"/>
  </filter>
  <linearGradient id="edge" x1="28" y1="28" x2="92" y2="92" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.55"/><stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0.12"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0.25"/>
  </linearGradient>
  <filter id="tileshadow" x="-10%" y="-10%" width="120%" height="125%">
    <feGaussianBlur in="SourceAlpha" stdDeviation="12"/><feOffset dy="12" result="b"/>
    <feFlood flood-color="#000" flood-opacity="0.35"/><feComposite in2="b" operator="in"/>
    <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
  <filter id="cast" x="-30%" y="-30%" width="160%" height="160%">
    <feGaussianBlur in="SourceAlpha" stdDeviation="2.2"/><feOffset dx="0" dy="1.6" result="b"/>
    <feFlood flood-color="#030614" flood-opacity="0.55"/><feComposite in2="b" operator="in"/>
    <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
</defs>
<g filter="url(#tileshadow)"><path d="{TILE}" fill="url(#bg)"/></g>
<g clip-path="url(#tileclip)">
  <rect x="100" y="100" width="824" height="824" fill="url(#glow)"/>
  <rect x="100" y="100" width="824" height="824" filter="url(#grain)" opacity="0.05"/>
  <g transform="{mark_t}">
    <g filter="url(#cast)"><path d="{LOW}" fill="url(#low)"/></g>
    <path d="{LOW}" fill="url(#sheen)" opacity="0.6"/>
    <g filter="url(#cast)"><path d="{TOP}" fill="url(#top)"/></g>
    <path d="{TOP}" fill="url(#sheen)"/>
    <path d="{TOP}" fill="none" stroke="url(#edge)" stroke-width="0.6"/>
  </g>
</g>
<path d="{TILE}" fill="none" stroke="url(#rim)" stroke-width="3"/>
</svg>
'''

def main():
    svg = icon_svg()
    svg_path = os.path.join(HERE, 'xorvio-app-icon.svg')
    open(svg_path, 'w').write(svg)
    appiconset = os.path.join(ROOT, 'App', 'Assets.xcassets', 'AppIcon.appiconset')
    for size in [16, 32, 64, 128, 256, 512, 1024]:
        subprocess.run(['rsvg-convert', '-w', str(size), '-h', str(size), svg_path, '-o', os.path.join(appiconset, f'icon_{size}.png')], check=True)
    # The .icns the development bundle uses.
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, 'AppIcon.iconset')
        os.makedirs(iconset)
        for base in [16, 32, 128, 256, 512]:
            shutil.copy(os.path.join(appiconset, f'icon_{base}.png'), os.path.join(iconset, f'icon_{base}x{base}.png'))
            shutil.copy(os.path.join(appiconset, f'icon_{base*2}.png'), os.path.join(iconset, f'icon_{base}x{base}@2x.png'))
        subprocess.run(['iconutil', '-c', 'icns', iconset, '-o', os.path.join(ROOT, 'App', 'Resources', 'AppIcon.icns')], check=True)
    print('Wrote', svg_path, 'the icon set and AppIcon.icns')

if __name__ == '__main__':
    import sys
    if '--svg-only' in sys.argv:
        open(os.path.join(HERE, 'xorvio-app-icon.svg'), 'w').write(icon_svg())
    else:
        main()
