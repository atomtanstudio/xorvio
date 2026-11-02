# xorvio.com

The Xorvio website: the landing page plus the privacy policy, support, terms and acknowledgements pages. Static HTML served by the `xorvio-web` Cloudflare Worker, which also sends `www` to the bare domain and adds security headers.

```
src/            layout.html, pages/ (index, 404), styles.css, site.js
content/        privacy, terms, support and acknowledgements, as HTML fragments
build.mjs       builds dist/: wraps pages in the layout, expands <x-shot>/<x-wall>/<x-glyph>
images.mjs      AVIF/WebP sizes of the captures and posters, favicons from the app icon
og.py           the 1200 × 630 social preview
worker/         the Worker (redirect, headers, caching)
showcase/       tools that make the screenshots (below)
```

## Build and deploy

```bash
npm install
npm run images     # after captures or posters change; needs ImageMagick and Pillow
npm run build
npm run deploy     # builds, then wrangler deploy to xorvio.com/* and www.xorvio.com/*
```

The `website` entry in `.claude/launch.json` previews the site with `wrangler dev` on port 8787.

## Screenshots

Every screenshot is the real app showing a library that doesn't exist: 24 films, 10 series and 22 channels, all invented (`showcase/catalog.py`, `showcase/episodes.py`), with generated artwork. Nothing belongs to anyone else.

1. In the repository root, `npm run dev` serves the interface on port 1420 against that library (`src/lib/mock.ts`).
2. `node showcase/capture-ui.mjs [shot …]` captures it in Playwright's WebKit at 1440 × 900 points, 2x, into `shots/<shot>.png`.
3. Then `npm run images && npm run build`.

The captures (`shots/`, about 60 MB) and the artwork (`showcase/art/`) aren't in git. The artwork is in the H3LIX library: `showcase/art/*.json` records which images were chosen, and `showcase/build_out.py` downloads them again into `showcase/art/out` (posters from GPT Image 2.5 on Dzine, backdrops and stills from Z-Image Turbo with 2× upscales, channel logos from `showcase/make_logos.py`).

Keep the site to features the app actually has. Add-ons may be described generically; never name or recommend one.
