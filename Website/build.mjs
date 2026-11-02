// Builds the static site into ./dist: pages from src/pages and content/, wrapped in
// src/layout.html, with custom tags (<x-shot>, <x-wall>, <x-glyph>…) expanded and
// the stylesheet and script fingerprinted. Run images.mjs first when captures change.
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";

const ROOT = new URL(".", import.meta.url).pathname;
const SRC = join(ROOT, "src");
const OUT = join(ROOT, "dist");
const read = (p) => readFileSync(p, "utf8");
const images = existsSync(join(SRC, "images.json")) ? JSON.parse(read(join(SRC, "images.json"))) : { shots: {}, posters: {}, icon: {} };

// Keep generated images; rebuild everything else.
mkdirSync(OUT, { recursive: true });
for (const entry of readdirSync(OUT)) {
  if (!["img", "favicon.ico", "favicon-32.png", "favicon-192.png", "apple-touch-icon.png", "og.jpg"].includes(entry)) rmSync(join(OUT, entry), { recursive: true });
}

function fingerprint(file, ext) {
  const body = read(join(SRC, file));
  const name = `/${basename(file, ext)}.${createHash("sha1").update(body).digest("hex").slice(0, 10)}${ext}`;
  writeFileSync(join(OUT, name), body);
  return name;
}
const css = fingerprint("styles.css", ".css");
const js = fingerprint("site.js", ".js");
mkdirSync(join(OUT, "fonts"), { recursive: true });
copyFileSync(join(ROOT, "node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2"), join(OUT, "fonts/inter-latin-wght-normal.woff2"));

let svgCount = 0;
const lockupSource = read(join(ROOT, "../Design/Brand/xorvio-lockup.svg")).replace(/ width="\d+" height="\d+"/, "");
const lockup = () => {
  const n = ++svgCount;
  return lockupSource.replace(/id="(x[tl])"/g, `id="$1${n}"`).replace(/url\(#(x[tl])\)/g, `url(#$1${n})`).replace(/<title>Xorvio<\/title>/, "");
};

const attr = (tag, name) => (tag.match(new RegExp(`${name}="([^"]*)"`)) || [])[1];
const hasAttr = (tag, name) => new RegExp(`\\s${name}(\\s|>|=)`).test(tag);
const srcset = (list) => list.map(([url, w]) => `${url} ${w}w`).join(", ");

function picture(entry, { alt, sizes, loading, className, priority }) {
  const fallback = entry.webp[Math.min(1, entry.webp.length - 1)][0];
  return `<picture${className ? ` class="${className}"` : ""}>` +
    `<source type="image/avif" srcset="${srcset(entry.avif)}" sizes="${sizes}">` +
    `<source type="image/webp" srcset="${srcset(entry.webp)}" sizes="${sizes}">` +
    `<img src="${fallback}" width="${entry.w}" height="${entry.h}" alt="${alt}" loading="${loading}" decoding="async"${priority ? ' fetchpriority="high"' : ""}>` +
    `</picture>`;
}

function shot(tag) {
  const name = attr(tag, "name");
  const entry = images.shots[name];
  const alt = attr(tag, "alt") || "";
  if (!entry) return `<div class="shot shot--missing" role="img" aria-label="${alt}"></div>`;
  const eager = attr(tag, "loading") === "eager";
  const pic = picture(entry, { alt, sizes: attr(tag, "sizes") || "(min-width: 1360px) 1280px, 100vw", loading: eager ? "eager" : "lazy", priority: eager });
  const full = entry.webp[entry.webp.length - 1][0];
  const cls = ["shot", attr(tag, "class")].filter(Boolean).join(" ");
  return hasAttr(tag, "zoom")
    ? `<button type="button" class="${cls}" data-zoom="${full}" aria-label="View larger: ${alt}">${pic}</button>`
    : `<div class="${cls}">${pic}</div>`;
}

function wall() {
  const keys = Object.keys(images.posters);
  if (!keys.length) return "";
  // A fixed shuffle, so the wall looks the same on every build.
  const order = keys.map((k, i) => [k, (i * 7919) % 101]).sort((a, b) => a[1] - b[1]).map(([k]) => k);
  const cols = 11, rows = 4;
  let html = '<div class="wall">';
  for (let c = 0; c < cols; c++) {
    html += '<div class="wall__col">';
    for (let r = 0; r < rows; r++) {
      const entry = images.posters[order[(c * rows + r) % order.length]];
      html += picture(entry, { alt: "", sizes: "200px", loading: "eager" });
    }
    html += "</div>";
  }
  return html + "</div>";
}

const GLYPHS = {
  profiles: '<path d="M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Zm7.5 0a3 3 0 1 0 0-6M3 20c0-3.3 2.7-6 6-6s6 2.7 6 6m2.5-6c2.5.4 4.5 2.7 4.5 5.5"/>',
  lock: '<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5M12 14.5v2.5"/>',
  play: '<rect x="3" y="5" width="18" height="14" rx="3"/><path d="m10 9.2 5 2.8-5 2.8z"/>',
  keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2.5"/><path d="M6.5 10h1m3 0h1m3 0h1m3 0h.5M6.5 14h11"/>',
  sparkle: '<path d="M12 3.5 13.6 9l5.4 1.6-5.4 1.6L12 17.6l-1.6-5.4L5 10.6 10.4 9zM18.5 15.5l.7 2.1 2.1.7-2.1.7-.7 2.1-.7-2.1-2.1-.7 2.1-.7z"/>',
  access: '<circle cx="12" cy="4.8" r="1.8"/><path d="M5 8.5c2.3.8 4.6 1.2 7 1.2s4.7-.4 7-1.2M12 9.7v4.8m0 0-3 6.3m3-6.3 3 6.3"/>',
  sync: '<path d="M19.5 12a7.5 7.5 0 0 1-13 5.1M4.5 12a7.5 7.5 0 0 1 13-5.1"/><path d="M17.5 3.5v3.4h-3.4M6.5 20.5v-3.4h3.4"/>',
  // Platforms as plain shapes: Apple's and Microsoft's logos aren't ours to use.
  apple: '<rect x="4" y="5" width="16" height="11" rx="1.8"/><path d="M2.5 19h19"/>',
  windows: '<rect x="3" y="4" width="18" height="13" rx="1.8"/><path d="M9 20.5h6M12 17v3.5"/>',
  linux: '<rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="m7 10 3 2.5L7 15M12.5 15h4.5"/>',
};
// GitHub's mark, which GitHub allows for linking to a repository.
const GITHUB_MARK = '<svg class="gh-mark" viewBox="0 0 16 16" aria-hidden="true"><path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg>';
// The project's repository, used wherever a page says {{github}}.
const GITHUB = "https://github.com/atomtanstudio/xorvio";
// The newest release's files ({{download}}/Xorvio_aarch64.dmg), under the
// version-free names release.yml gives them.
const DOWNLOAD = `${GITHUB}/releases/latest/download`;
const glyph = (tag) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${GLYPHS[attr(tag, "name")] || ""}</svg>`;
const appIcon = (size) => images.icon?.webp ? picture(images.icon, { alt: "Xorvio app icon", sizes: `${size}px`, loading: "lazy" }) : "";

function expand(html) {
  return html
    .replace(/<x-shot\b[^>]*><\/x-shot>/g, shot)
    .replace(/<x-wall><\/x-wall>/g, wall)
    .replace(/<x-glyph\b[^>]*><\/x-glyph>/g, glyph)
    .replace(/<x-appicon><\/x-appicon>/g, () => appIcon(172))
    .replace(/<x-icon><\/x-icon>/g, () => appIcon(40))
    .replace(/<x-github><\/x-github>/g, GITHUB_MARK);
}

function frontMatter(text) {
  const m = text.match(/^---\n([\s\S]*?)\n---\n/);
  if (!m) return [{}, text];
  const meta = Object.fromEntries(m[1].split("\n").map((l) => [l.slice(0, l.indexOf(":")).trim(), l.slice(l.indexOf(":") + 1).trim()]));
  return [meta, text.slice(m[0].length)];
}

// Legal and support pages: a prose column with a contents list built from the h2s.
function prose(fragment, withToc) {
  let body = fragment.trim();
  if (withToc) {
    const items = [...body.matchAll(/<h2 id="([^"]+)">([\s\S]*?)<\/h2>/g)].map(([, id, text]) => `<li><a href="#${id}">${text.replace(/<[^>]+>/g, "")}</a></li>`);
    if (items.length > 3) body = body.replace(/(<p class="lede">[\s\S]*?<\/p>)/, `$1\n<nav class="toc" aria-label="On this page"><strong>On this page</strong><ol>${items.join("")}</ol></nav>`);
  }
  return `<article class="prose">\n${body}\n</article>`;
}

const layout = read(join(SRC, "layout.html"));
function render(file, meta, body) {
  svgCount = 0;
  const preload = meta.preloadShot && images.shots[meta.preloadShot]
    ? `<link rel="preload" as="image" type="image/avif" imagesrcset="${srcset(images.shots[meta.preloadShot].avif)}" imagesizes="(min-width: 1360px) 1280px, 100vw" fetchpriority="high">`
    : "";
  const html = layout
    .replace(/{{title}}/g, meta.title)
    .replace(/{{ogTitle}}/g, meta.ogTitle || meta.title)
    .replace(/{{description}}/g, meta.description)
    .replace(/{{path}}/g, meta.path)
    .replace(/{{bodyClass}}/g, meta.bodyClass || "page")
    .replace(/{{preload}}/g, preload)
    .replace(/{{css}}/g, css)
    .replace(/{{js}}/g, js)
    .replace(/{{lockup}}/g, lockup)
    .replace(/{{body}}/g, () => expand(body))
    .replace(/{{download}}/g, DOWNLOAD)
    .replace(/{{github}}/g, GITHUB)
    .replace(/<x-github><\/x-github>/g, GITHUB_MARK);
  writeFileSync(join(OUT, file), html);
  console.log("page", file);
}

for (const file of readdirSync(join(SRC, "pages")).filter((f) => f.endsWith(".html"))) {
  const [meta, body] = frontMatter(read(join(SRC, "pages", file)));
  render(file, meta, body);
}

const LEGAL = {
  privacy: { title: "Privacy Policy — Xorvio", description: "Xorvio collects no data. What stays on your computer, what the app connects to, and the choices you have.", toc: true },
  terms: { title: "Terms of Use — Xorvio", description: "The terms for using Xorvio, free software under the GPL, and this website.", toc: true },
  support: { title: "Support — Xorvio", description: "Get help with Xorvio on Mac, Windows and Linux: contact support, report an issue on GitHub, and answers to common questions.", toc: false },
  acknowledgements: { title: "Acknowledgements — Xorvio", description: "Open-source software, data sources and trademarks used by Xorvio.", toc: false },
};
for (const [name, meta] of Object.entries(LEGAL)) {
  const path = join(ROOT, "content", `${name}.html`);
  if (!existsSync(path)) continue;
  render(`${name}.html`, { ...meta, path: `/${name}`, bodyClass: "page" }, prose(read(path), meta.toc));
}
console.log("done");
