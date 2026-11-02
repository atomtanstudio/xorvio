// Captures the Rust + Tauri app's interface for the website, from the browser
// preview (`npm run dev` in the repository root serves it on port 1420 against
// the fictional showcase library in src/lib/mock.ts). Each shot is 1440 × 900
// points at 2x in Playwright's WebKit, the engine the app uses on macOS, with
// the window's traffic lights drawn where the app puts them.
//
//   node showcase/capture-ui.mjs [shot …]     # all shots when none are named
//
// Writes ../shots/<shot>.png. The preview plays one stand-in film for every
// stream; live tiles get the showcase's broadcast stills (art/media/clip-*.mp4)
// instead, as each channel would show its own picture.
import { webkit } from "playwright";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const HERE = new URL(".", import.meta.url).pathname;
const OUT = join(HERE, "../shots");
const BASE = process.env.XORVIO_PREVIEW ?? "http://localhost:1420/";
const clip = (key) => new URL(`/Website/showcase/art/media/clip-${key}.mp4`, BASE).href;

// The demo banner shows only when the demo library is the sole source.
const CHROME_CSS = `.home-banners { display: none !important; }`;

/** macOS's close, minimize and zoom buttons, where tauri.conf.json places them. */
async function trafficLights(page) {
  await page.evaluate(() => {
    if (document.getElementById("xv-lights")) return;
    const lights = document.createElement("div");
    lights.id = "xv-lights";
    lights.style.cssText = "position:fixed;left:19px;top:20px;display:flex;gap:9px;z-index:2147483647;pointer-events:none";
    for (const [fill, line] of [["#ff5f57", "#e14640"], ["#febc2e", "#dfa023"], ["#28c840", "#1aab29"]]) {
      const dot = document.createElement("span");
      dot.style.cssText = `width:14px;height:14px;border-radius:50%;background:${fill};box-shadow:inset 0 0 0 0.5px ${line}`;
      lights.append(dot);
    }
    document.body.append(lights);
  });
}

async function open(browser, { scheme = "dark", query = "" } = {}) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: scheme });
  await page.goto(BASE + query, { waitUntil: "networkidle" });
  await page.addStyleTag({ content: CHROME_CSS });
  await page.waitForTimeout(1500);
  return page;
}

const nav = (page, label) => page.locator("nav, aside").getByText(label, { exact: true }).first().click();

/** Waits for every image on screen to finish loading. */
async function settle(page, ms = 1500) {
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.evaluate(async () => {
    const pending = [...document.images].filter((img) => !img.complete && img.getBoundingClientRect().top < innerHeight);
    await Promise.race([Promise.all(pending.map((img) => new Promise((r) => { img.onload = img.onerror = r; }))), new Promise((r) => setTimeout(r, 6000))]);
  });
  await page.waitForTimeout(ms);
}

/** Scrolls the screen's own scroller (not the window) by some points. */
async function scrollMain(page, dy) {
  await page.mouse.move(900, 500);
  await page.mouse.wheel(0, dy);
  await page.waitForTimeout(900);
}

/** Gives each live video its own stand-in picture, in order. */
async function liveClips(page, keys) {
  await page.evaluate(async (srcs) => {
    const videos = [...document.querySelectorAll("video")];
    await Promise.all(videos.map((video, i) => new Promise((resolve) => {
      if (!srcs[i]) return resolve();
      video.src = srcs[i];
      video.muted = true;
      video.onloadeddata = () => { video.currentTime = 3; resolve(); };
      video.play().catch(() => {});
      setTimeout(resolve, 4000);
    })));
  }, keys.map(clip));
  await page.waitForTimeout(1200);
}

const SHOTS = {
  async home(page) {
    await settle(page, 2500);
  },
  async "home-rows"(page) {
    await settle(page);
    // Down to Continue Watching, then on a row at a time so each row's art loads.
    for (const dy of [480, 400, 400, 120]) await scrollMain(page, dy);
    await settle(page, 5000);
  },
  async movies(page) {
    await nav(page, "Movies");
    await settle(page, 2500);
  },
  async film(page) {
    await nav(page, "Movies");
    await settle(page);
    await page.getByText("Halcyon", { exact: true }).first().click();
    await settle(page, 2500);
  },
  async "series-hero"(page) {
    await nav(page, "Series");
    await settle(page);
    await page.getByText("Harbor Lights", { exact: true }).first().click();
    await settle(page, 2500);
  },
  async series(page) {
    await SHOTS["series-hero"](page);
    await scrollMain(page, 420);
    await settle(page);
  },
  async watchlist(page) {
    await nav(page, "Watchlist");
    await settle(page, 2500);
  },
  async search(page) {
    await page.getByPlaceholder("Channels, movies, series, programs").fill("harbor");
    await settle(page, 2500);
  },
  async live(page) {
    await nav(page, "Live TV");
    await settle(page);
    // The guide's rows sit under its scroller, so these are clicked in the page.
    await page.evaluate(() => [...document.querySelectorAll("button.guide-channel")].find((b) => b.textContent.trim().startsWith("201"))?.click());
    await page.waitForTimeout(800);
    await page.evaluate(() => document.querySelector("button.live-video-placeholder")?.click());
    await page.waitForTimeout(2500);
    await liveClips(page, ["prog-football"]);
    await settle(page);
  },
  async multiview(page) {
    await SHOTS.live(page);
    const press = (selector, text) => page.evaluate(([s, t]) => {
      const el = [...document.querySelectorAll(s)].find((e) => !t || e.textContent.includes(t));
      el?.click();
      return !!el;
    }, [selector, text]);
    if (!(await press('button[aria-label="Add to Multiview"]'))) throw new Error("no Add to Multiview button");
    await page.waitForTimeout(1500);
    for (const name of ["Raceline", "Meridian 24", "Wild Atlas"]) {
      if (!(await press("button.btn-glass", "Add Channel"))) throw new Error("no Add Channel button");
      await page.waitForTimeout(900);
      if (!(await press(".mv-picker-row", name))) throw new Error(`no ${name} in the picker`);
      await page.waitForTimeout(900);
      // The preview's demo provider allows two streams at once, so it warns; a real one may allow more.
      await press("button", "Add Anyway");
      await page.waitForTimeout(1500);
    }
    await liveClips(page, ["prog-football", "prog-race", "prog-news", "series-backdrop-2006"]);
    await page.mouse.move(400, 300);
    await page.waitForTimeout(600);
  },
  async player(page) {
    await nav(page, "Movies");
    await settle(page);
    await page.getByText("Neon Requiem", { exact: true }).first().click();
    await settle(page);
    await page.getByRole("button", { name: /^(Play|Resume)/ }).first().click();
    await page.waitForTimeout(4000);
    await page.evaluate(() => { const v = document.querySelector("video"); v.currentTime = 2875; v.pause(); });
    await page.waitForTimeout(1200);
    await page.mouse.move(720, 600);
    await page.mouse.move(740, 620);
    await page.waitForTimeout(700);
  },
  async mini(page) {
    await SHOTS.player(page);
    await page.evaluate(() => document.querySelector("video").play());
    await page.evaluate(() => document.querySelector('button[aria-label="Mini player"]')?.click());
    await page.waitForTimeout(1200);
    await nav(page, "Series");
    await settle(page, 2000);
    await page.mouse.move(700, 300);
  },
  async recordings(page) {
    await nav(page, "Recordings");
    // The preview's demo provider allows two streams, so it seeds conflicts and an
    // unplugged recordings drive on purpose; a normal schedule shows neither.
    await page.addStyleTag({ content: `.badge.warn, .recording-note[style*="warning"], .recording-note.error { display: none !important; }` });
    await settle(page, 2500);
    await page.evaluate(() => document.querySelectorAll(".recording-note").forEach((n) => { if (/wasn't available/.test(n.textContent)) n.remove(); }));
  },
  async sources(page) {
    await nav(page, "Sources");
    await settle(page);
    await page.getByRole("button", { name: "Add Source" }).first().click();
    await page.waitForTimeout(1200);
  },
  async light(page) {
    await nav(page, "Movies");
    await settle(page, 2500);
  },
};
SHOTS.light.options = { scheme: "light" };

const browser = await webkit.launch();
mkdirSync(OUT, { recursive: true });
const wanted = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(SHOTS);
for (const name of wanted) {
  const shot = SHOTS[name];
  if (!shot) { console.error(`no shot "${name}"`); continue; }
  const page = await open(browser, shot.options ?? {});
  try {
    await shot(page);
    await trafficLights(page);
    await page.screenshot({ path: join(OUT, `${name}.png`) });
    console.log("shot", name);
  } catch (error) {
    console.error(`${name}: ${error.message.split("\n")[0]}`);
  } finally {
    await page.close();
  }
}
await browser.close();
