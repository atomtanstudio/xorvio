// Makes the site's images: responsive AVIF and WebP versions of the app captures
// (shots/*.png) and posters, the app icon sizes and favicons. Output file names
// carry a content hash, so they can be cached forever. Writes src/images.json.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";

const ROOT = new URL(".", import.meta.url).pathname;
const OUT = join(ROOT, "dist");
const APP = join(ROOT, "..");
const SHOTS = join(ROOT, "shots");
const ART = join(ROOT, "showcase/art/out");
const SHOT_WIDTHS = [960, 1440, 2160, 2880];
const POSTER_WIDTHS = [240, 480];

const hash = (file, extra) => createHash("sha1").update(readFileSync(file)).update(extra).digest("hex").slice(0, 10);
const size = (file) => execFileSync("magick", ["identify", "-format", "%w %h", file]).toString().trim().split(" ").map(Number);

function variants(src, folder, name, widths, quality) {
  const [w, h] = size(src);
  const dir = join(OUT, "img", folder);
  mkdirSync(dir, { recursive: true });
  const entry = { w, h, avif: [], webp: [] };
  for (const width of widths.filter((x) => x <= w)) {
    for (const format of ["avif", "webp"]) {
      const file = `${name}-${width}.${hash(src, `${width}${format}${quality[format]}`)}.${format}`;
      const path = join(dir, file);
      if (!existsSync(path)) {
        execFileSync("magick", [src, "-resize", `${width}x`, "-strip", "-quality", String(quality[format]),
          ...(format === "avif" ? ["-define", "heic:speed=6"] : ["-define", "webp:method=6", "-define", "webp:alpha-quality=100"]), path]);
      }
      entry[format].push([`/img/${folder}/${file}`, width]);
    }
  }
  return entry;
}

const manifest = { shots: {}, posters: {}, icon: {} };

if (existsSync(SHOTS)) {
  for (const file of readdirSync(SHOTS).filter((f) => f.endsWith(".png")).sort()) {
    const name = basename(file, ".png");
    manifest.shots[name] = variants(join(SHOTS, file), "shots", name, SHOT_WIDTHS, { avif: 62, webp: 84 });
    console.log("shot", name);
  }
}
if (existsSync(ART)) {
  for (const file of readdirSync(ART).filter((f) => /^(film|series)-poster-\d+\.jpg$/.test(f)).sort()) {
    const name = basename(file, ".jpg");
    manifest.posters[name] = variants(join(ART, file), "posters", name, POSTER_WIDTHS, { avif: 55, webp: 78 });
  }
  console.log("posters", Object.keys(manifest.posters).length);
}

// App icon, favicons and touch icon, all from the app's own icon. The 1024 icon sits
// on Apple's grid (an 824-point tile at 100,100), so the tab icons crop to the tile.
const iconSet = join(APP, "App/Assets.xcassets/AppIcon.appiconset/icon_1024.png");
manifest.icon = variants(iconSet, "icon", "xorvio-icon", [128, 256, 512], { avif: 70, webp: 88 });
const tile = ["-crop", "824x824+100+100", "+repage"];
execFileSync("magick", [iconSet, ...tile, "-resize", "32x32", join(OUT, "favicon-32.png")]);
execFileSync("magick", [iconSet, ...tile, "-resize", "192x192", join(OUT, "favicon-192.png")]);
execFileSync("magick", [iconSet, ...tile, "-define", "icon:auto-resize=48,32,16", join(OUT, "favicon.ico")]);
// iOS rounds the corners itself, so the touch icon is a full square in the tile's navy.
execFileSync("magick", [iconSet, ...tile, "-background", "#181e3b", "-flatten", "-resize", "180x180", join(OUT, "apple-touch-icon.png")]);
rmSync(join(OUT, "favicon.svg"), { force: true });

writeFileSync(join(ROOT, "src/images.json"), JSON.stringify(manifest, null, 1));
console.log("wrote src/images.json");
