// Subtitles in the player, from four places: a remuxed stream's own tracks
// (WebVTT that ffmpeg writes beside the video as it streams, read again as it
// grows), subtitle add-ons' files (OpenSubtitles and the like), subtitle files
// beside a media folder's video (listed and read like add-ons' files), and
// tracks the web view finds itself in HLS and MP4 streams. Ours go in one text
// track of our own, so a subtitle delay can move them. A remuxed stream's
// picture subtitles (PGS, VobSub, DVB) can't be text: ffmpeg draws them into
// the video instead, restarting the stream.

import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { api } from "../lib/api";
import { languageName } from "../lib/languages";
import { useApp, type NowPlaying } from "../lib/store";
import type { AddonSubtitle, ResolvedStream, TrackChoice } from "../lib/types";
import { subtitlesFor } from "./playerRules";

export interface SubtitleOption {
  key: string;
  label: string;
  detail: string | null;
  language: string | null;
  /** Picture subtitles, which are drawn into the video (the stream restarts, encoding it). */
  picture: number | null;
}

interface Cue {
  start: number;
  end: number;
  text: string;
}

const OFF = "off";
/** Where a media folder's subtitle files are (xorvio-core's folder_file_url). */
const FOLDER_FILE = /Windows/.test(typeof navigator === "undefined" ? "" : navigator.userAgent) ? "http://xorvio-folder.localhost/" : "xorvio-folder://";
/** A growing subtitle file is read again this often. */
const POLL_MS = 1500;

/** The text track Xorvio fills itself, made once per video element. */
const ownTracks = new WeakMap<HTMLVideoElement, TextTrack>();
function ownTrack(video: HTMLVideoElement) {
  let track = ownTracks.get(video);
  if (!track) {
    track = video.addTextTrack("subtitles", "Xorvio", "");
    ownTracks.set(video, track);
  }
  return track;
}

function nativeTracks(video: HTMLVideoElement) {
  const own = ownTracks.get(video);
  return Array.from(video.textTracks).filter((t) => t !== own && (t.kind === "subtitles" || t.kind === "captions"));
}

function clear(track: TextTrack) {
  for (const cue of Array.from(track.cues ?? [])) track.removeCue(cue);
}

/** "01:02:03.500" or "02:03.500" in seconds. */
function seconds(stamp: string) {
  const parts = stamp.trim().split(":").map(Number);
  return parts.reduce((total, part) => total * 60 + part, 0);
}

/** The complete cues in a piece of WebVTT, and what's left of a cue still being written. */
export function parseCues(text: string, complete: boolean): { cues: Cue[]; rest: string } {
  const blocks = text.replace(/\r\n?/g, "\n").split(/\n{2,}/);
  const rest = complete ? "" : (blocks.pop() ?? "");
  const cues: Cue[] = [];
  for (const block of blocks) {
    const lines = block.split("\n");
    const timing = lines.findIndex((line) => line.includes("-->"));
    if (timing < 0) continue;
    const [from, to] = lines[timing].split("-->");
    const start = seconds(from);
    const end = seconds(to.trim().split(/\s+/)[0]);
    const body = lines.slice(timing + 1).join("\n").trim();
    if (Number.isFinite(start) && Number.isFinite(end) && end > start && body) cues.push({ start, end, text: body });
  }
  return { cues, rest };
}

function addCues(track: TextTrack, cues: Cue[], shift: number) {
  for (const cue of cues) {
    const start = cue.start + shift;
    const end = cue.end + shift;
    if (end > 0) track.addCue(new VTTCue(Math.max(0, start), end, cue.text));
  }
}

/** The add-on ids subtitle add-ons know a title by: an IMDb id, with ":season:episode" for an episode. */
async function addonRef(item: NowPlaying): Promise<{ type: string; id: string } | null> {
  const [source, type, ...rest] = item.itemId.split("|");
  if (source === "addon") {
    // "addon|movie|tt123" or "addon|series|tt123|ep|tt123:1:2"
    if (type === "movie") return { type: "movie", id: rest.join("|") };
    const episode = item.itemId.split("|ep|")[1];
    return episode ? { type: "series", id: episode } : null;
  }
  if (item.kind === "movie") {
    const movie = await api.movie(item.itemId).catch(() => null);
    return movie?.imdbId ? { type: "movie", id: movie.imdbId } : null;
  }
  if (item.kind === "episode" && item.seriesId) {
    const [series, episodes] = await Promise.all([api.series(item.seriesId).catch(() => null), api.episodes(item.seriesId).catch(() => [])]);
    const episode = episodes.find((e) => e.id === item.itemId);
    return series?.imdbId && episode ? { type: "series", id: `${series.imdbId}:${episode.season}:${episode.number}` } : null;
  }
  return null;
}

/**
 * `memory` is what was chosen for the series an episode belongs to, which
 * wins over the preferred language in Settings: "off" stays off, and a track
 * is waited for until one in its language turns up.
 */
export function useSubtitles(video: RefObject<HTMLVideoElement | null>, stream: ResolvedStream | null, item: NowPlaying | null, memory?: { choice: TrackChoice | null; ready: boolean }) {
  const preferred = useApp((s) => s.settings.subtitleLanguage);
  const [choice, setChoice] = useState(OFF);
  const [delay, setDelay] = useState(0);
  const [addonSubs, setAddonSubs] = useState<AddonSubtitle[]>([]);
  const [nativeCount, setNativeCount] = useState(0);
  // Chosen by hand (or automatically) for this item already.
  const decided = useRef(false);
  const addonTexts = useRef(new Map<string, Cue[]>());

  // A new film or episode starts with nothing chosen and no delay.
  const itemKey = item ? `${item.itemId}|${item.kind}` : "";
  useEffect(() => {
    setChoice(OFF);
    setDelay(0);
    setAddonSubs([]);
    decided.current = false;
    if (!item || item.kind === "channel") return;
    let cancelled = false;
    const fromAddons = addonRef(item)
      .then((ref) => (ref ? api.addonSubtitles(ref.type, ref.id) : []))
      .catch(() => [] as AddonSubtitle[]);
    // Files beside a media folder's video come first: they were made for that copy.
    const beside = api.folderSubtitles(item.itemId).catch(() => [] as AddonSubtitle[]);
    Promise.all([beside, fromAddons]).then(([local, addon]) => !cancelled && setAddonSubs([...local, ...addon]));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [itemKey]);

  // Tracks the web view finds itself appear after the stream loads.
  useEffect(() => {
    const el = video.current;
    if (!el) return;
    const update = () => setNativeCount(nativeTracks(el).length);
    update();
    el.textTracks.addEventListener("addtrack", update);
    el.textTracks.addEventListener("removetrack", update);
    return () => {
      el.textTracks.removeEventListener("addtrack", update);
      el.textTracks.removeEventListener("removetrack", update);
    };
  }, [video, stream]);

  const options = useMemo<SubtitleOption[]>(() => {
    const list: SubtitleOption[] = [];
    for (const t of stream?.subtitleTracks ?? []) {
      const name = languageName(t.language) ?? "Unknown language";
      list.push({
        key: `stream:${t.index}`,
        label: t.forced ? `${name} (Forced)` : name,
        detail: t.url ? t.title : [t.title, "Drawn into the picture"].filter(Boolean).join(" · "),
        language: t.language,
        picture: t.url ? null : t.index,
      });
    }
    const el = video.current;
    if (el)
      nativeTracks(el).forEach((t, i) =>
        list.push({ key: `native:${i}`, label: t.label || languageName(t.language) || `Track ${i + 1}`, detail: null, language: t.language || null, picture: null }),
      );
    const perLanguage = new Map<string, number>();
    for (const s of addonSubs) {
      const n = (perLanguage.get(s.language) ?? 0) + 1;
      perLanguage.set(s.language, n);
      list.push({ key: `addon:${s.url}`, label: `${languageName(s.language) ?? s.language}${n > 1 ? ` ${n}` : ""}`, detail: s.addonName, language: s.language, picture: null });
    }
    return list;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream, addonSubs, nativeCount]);

  // Turn on the preferred language once something in it turns up (in text:
  // drawing picture subtitles in costs encoding the video, so it's asked for).
  const remembered = memory?.choice ?? null;
  const memoryReady = memory?.ready ?? true;
  useEffect(() => {
    if (decided.current || !memoryReady) return;
    const pick = subtitlesFor(remembered, options);
    if (pick) {
      decided.current = true;
      if (pick !== OFF) setChoice(pick);
      return;
    }
    if (remembered?.subtitles?.kind === "track" || !preferred) return;
    const match = options.find((o) => o.picture === null && o.language === preferred && !o.label.endsWith("(Forced)"));
    if (match) {
      decided.current = true;
      setChoice(match.key);
    }
  }, [options, preferred, remembered, memoryReady]);

  // Show the choice.
  useEffect(() => {
    const el = video.current;
    if (!el || !stream) return;
    const own = ownTrack(el);
    clear(own);
    own.mode = "hidden";
    const natives = nativeTracks(el);
    natives.forEach((t) => (t.mode = "disabled"));
    if (choice === OFF) return;

    if (choice.startsWith("native:")) {
      const track = natives[Number(choice.slice(7))];
      if (track) track.mode = "showing";
      return;
    }

    let cancelled = false;
    own.mode = "showing";
    if (choice.startsWith("addon:")) {
      const url = choice.slice(6);
      // Add-on files are timed from the film's start; a remuxed stream's clock starts where it was started.
      const shift = delay - stream.startOffsetSeconds;
      const known = addonTexts.current.get(url);
      if (known) addCues(own, known, shift);
      else
        (url.startsWith(FOLDER_FILE) ? api.folderSubtitleText(url) : api.addonSubtitleText(url))
          .then((text) => {
            const { cues } = parseCues(text, true);
            addonTexts.current.set(url, cues);
            if (!cancelled) addCues(own, cues, shift);
          })
          .catch((e) => api.log("warn", `subtitles: ${e}`).catch(() => {}));
      return () => {
        cancelled = true;
      };
    }

    // A remuxed stream's track: timed like its video, and growing.
    const url = stream.subtitleTracks.find((t) => `stream:${t.index}` === choice)?.url;
    if (!url) return;
    let read = 0;
    let rest = "";
    const decoder = new TextDecoder();
    let timer: ReturnType<typeof setTimeout>;
    let logged = false;
    const poll = async () => {
      let bytes: Uint8Array;
      try {
        const response = await fetch(url, { headers: read ? { Range: `bytes=${read}-` } : {} });
        bytes = new Uint8Array(await response.arrayBuffer());
      } catch {
        // The stream was released or restarted; the next one brings a new address.
        if (!cancelled) timer = setTimeout(poll, POLL_MS);
        return;
      }
      if (cancelled) return;
      if (bytes.length) {
        read += bytes.length;
        const parsed = parseCues(rest + decoder.decode(bytes, { stream: true }), false);
        rest = parsed.rest;
        try {
          addCues(own, parsed.cues, delay);
          if (!logged && parsed.cues.length) {
            logged = true;
            api.log("info", `subtitles: ${parsed.cues.length} cues from ${parsed.cues[0].start.toFixed(1)} s, track ${own.mode}, video at ${el.currentTime.toFixed(1)} s`).catch(() => {});
          }
        } catch (e) {
          api.log("warn", `subtitles couldn't be shown: ${e}`).catch(() => {});
        }
      }
      timer = setTimeout(poll, POLL_MS);
    };
    poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [video, stream, choice, delay]);

  const choose = (key: string) => {
    decided.current = true;
    setChoice(key);
  };
  /** What a choice asks of the stream: a picture track to draw into it, or -1 to stop; undefined is nothing. */
  const burnFor = (key: string): number | undefined => {
    const picture = options.find((o) => o.key === key)?.picture ?? null;
    const burned = stream?.burnedSubtitle ?? null;
    if (picture !== null) return picture === burned ? undefined : picture;
    return burned === null ? undefined : -1;
  };
  return { options, choice, choose, burnFor, delay, setDelay, off: OFF };
}
