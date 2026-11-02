# Xorvio brand marks

Vector sources for the logo. The app draws the same geometry in code
(`BrandMark` in `src/components/Artwork.tsx`), so it
stays sharp at any size; keep the two in step if either changes.

| File | Use |
| --- | --- |
| `xorvio-mark.svg` / `-light.svg` | The mark alone: two bands crossing as an X, the top-left band passing over the other. |
| `xorvio-lockup.svg` / `-light.svg` | Mark and wordmark side by side, as at the top of the sidebar. |
| `xorvio-wordmark.svg` / `-light.svg` | "xorvio" alone. |
| `xorvio-app-icon.svg` | The app icon: the mark on a navy tile with a soft glow, on Apple's 1024-point grid (824-point shape, continuous corners). |

The plain files are for dark backgrounds (the app's default look); the `-light`
files use deeper blues and a near-black wordmark for light backgrounds.

`make_icon.py` rebuilds the app icon from the same geometry and writes the PNG
sizes into `App/Assets.xcassets/AppIcon.appiconset` and `App/Resources/AppIcon.icns`
(`python3 Design/Brand/make_icon.py`; needs `brew install librsvg`). A faint grain
in the tile keeps its dark gradient from showing steps.

**Geometry** (120-unit square): bands 24 wide with round ends, from (28,28) to
(92,92) and (28,92) to (92,28); the lower band stops 5 units short of the top
band on each side. Wordmark: monoline, x-height 44, stroke 10, round ends.

**Colors**

| | Top band | Lower band | Wordmark |
| --- | --- | --- | --- |
| On dark | `#DCEAFF` → `#5C93FF` | `#3F4FD8` → `#86ABFF` | `#F2F5FA` |
| On light | `#7FAEFF` → `#1F5BD8` | `#2433A8` → `#5A86F2` | `#11151C` |

**Use:** keep clear space of at least half the mark's height around it. Below
16 px, use the mark without the wordmark. Don't recolor the bands separately,
outline them, or add effects; the app icon builds on the mark with its own
depth and lighting.
