# xorvio.com

The Xorvio website: the landing page plus the pages the App Store needs (privacy policy, support, terms, acknowledgements). Static HTML served by the `xorvio-web` Cloudflare Worker, which also sends `www` to the bare domain and adds security headers.

```
src/            layout.html, pages/ (index, film, 404), styles.css, site.js, assets/ (the film's poster), media.json
content/        privacy, terms, support and acknowledgements, as HTML fragments
build.mjs       builds dist/: wraps pages in the layout, expands <x-shot>/<x-wall>/<x-glyph>
images.mjs      AVIF/WebP sizes of the captures and posters, favicons from the app icon
og.py           the 1200 × 630 social preview
worker/         the Worker (redirect, headers, caching, the launch film from R2)
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

## The launch film

`/film` plays the launch film. It's too big for the static assets (25 MB a file, and they don't answer byte ranges, which Safari needs before it plays a video at all), so it lives in the `xorvio-media` R2 bucket and the Worker serves it at `/media/<file>`. The file name carries a content hash and is cached for a year, so a new cut gets a new name. To replace the film:

```bash
ffmpeg -i xorvio-launch.mp4 -c:v libx264 -preset slow -crf 20 -profile:v high -level 4.1 -pix_fmt yuv420p -c:a aac -b:a 160k -ac 2 -sn -movflags +faststart film.mp4
mv film.mp4 "xorvio-launch.$(shasum -a 1 film.mp4 | cut -c1-10).mp4"
npx wrangler r2 object put "xorvio-media/xorvio-launch.<hash>.mp4" --file "xorvio-launch.<hash>.mp4" --content-type video/mp4 --content-disposition 'inline; filename="xorvio-launch.mp4"'
```

Then name the file in `src/media.json`, replace `src/assets/film-poster.jpg` (a 1920 × 1080 frame, shown before play) and `src/assets/film-og.jpg` (the 1200 × 630 link preview, with the play button drawn on), and build and deploy. For the local preview, upload with `--local` as well: `wrangler dev` reads its own copy of the bucket.

## Screenshots

Every screenshot is the real app showing a library that doesn't exist: 24 films, 10 series and 22 channels, all invented (`showcase/catalog.py`, `showcase/episodes.py`), with generated artwork. Nothing belongs to anyone else, so the captures are also safe for App Store screenshots. They're 2880 × 1800, the Mac App Store size.

1. `showcase/run-showcase.sh --reset` builds the development app, launches an isolated copy named Xorvio (bundle id `com.xorvio.Xorvio.showcase`, its own home folder and preferences, TVmaze off) and starts `showcase/server.py`, a local Xtream-style server for the fictional library.
2. `showcase/setup-library.sh` connects the source and adds viewing progress, the watchlist, live reminders and favorite channels.
3. Drive the app with `showcase/shoot.sh "<command>" …` (commands are in `DebugAutomation.swift`). `capture:<name>[@<window>]` writes `showcase/.run/debug/capture-<name>.png`, glass and video included, without Screen Recording permission.
4. Copy the captures you want to `shots/<name>.png`, then `npm run images && npm run build`.
5. `showcase/teardown.sh` cancels the copy's reminders, removes its source and Keychain entry, and stops the server.

The captures (`shots/`, about 60 MB) and the artwork (`showcase/art/`) aren't in git. The artwork is in the H3LIX library: `showcase/art/*.json` records which images were chosen, and `showcase/build_out.py` downloads them again into `showcase/art/out` (posters from GPT Image 2.5 on Dzine, backdrops and stills from Z-Image Turbo with 2× upscales, channel logos from `showcase/make_logos.py`).

Keep the site to features the App Store build actually has, and never mention anything from Xorvio Personal.
