// Copies of an add-on title: every stream the installed add-ons offer, best
// first, then the same title from your provider (as Xorvio Personal lists it
// under "Your Provider"). Copies that need a torrent engine are listed but
// can't be played.

import { Play, Server } from "lucide-react";
import { useEffect, useState } from "react";
import { Sheet } from "../components/Sheet";
import { playEpisode, playMovie } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { fileSize, playerClock } from "../lib/format";
import { useApp, type CopiesRequest, type NowPlaying } from "../lib/store";
import type { AddonStream, Episode, Movie, Series } from "../lib/types";
import { resumePlayback } from "../player/controls";

const qualityRank = (q: string | null) => (q === "4K" || q === "2160p" ? 4 : q === "1080p" ? 3 : q === "720p" ? 2 : q ? 1 : 0);

/** Titles compare without case, punctuation or a leading "The". */
function titleKey(title: string) {
  return title
    .toLowerCase()
    .replace(/\(\d{4}\)|\[.*?\]/g, "")
    .replace(/^(en|us|uk|4k|hd)\s*[-|:]\s*/i, "")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .replace(/^the /, "")
    .trim();
}

interface ProviderCopy {
  label: string;
  meta: string;
  play(): void;
}

/** The same film, or episode, in your provider's library. */
async function providerCopies(request: CopiesRequest): Promise<ProviderCopy[]> {
  const results = await api.search(request.title).catch(() => null);
  if (!results) return [];
  const key = titleKey(request.title);
  const sameYear = (year: number | null) => !request.year || !year || Math.abs(year - request.year) <= 1;
  const at = request.switching?.at;
  if (!request.episode) {
    return results.movies
      .filter((m: Movie) => m.sourceId !== "addon" && titleKey(m.title) === key && sameYear(m.year))
      .map((m) => ({ label: m.title, meta: [m.year, m.durationSeconds ? `${Math.round(m.durationSeconds / 60)} min` : null].filter(Boolean).join(" · "), play: () => playMovie(m, request.fromStart, at) }));
  }
  const { season, number } = request.episode;
  const shows = results.series.filter((s: Series) => s.sourceId !== "addon" && titleKey(s.title) === key);
  const copies: ProviderCopy[] = [];
  for (const show of shows.slice(0, 3)) {
    const episodes = await api.episodes(show.id).catch(() => [] as Episode[]);
    const episode = episodes.find((e) => e.season === season && e.number === number);
    if (episode) copies.push({ label: show.title, meta: `S${String(season).padStart(2, "0")} E${String(number).padStart(2, "0")} · ${episode.title}`, play: () => playEpisode(episode, show, request.fromStart, at) });
  }
  return copies;
}

function useCopies(request: CopiesRequest) {
  const [copies, setCopies] = useState<AddonStream[] | null>(null);
  const [provider, setProvider] = useState<ProviderCopy[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setCopies(null);
    api
      .addonStreams(request.mediaType, request.mediaId)
      .then((list) => !cancelled && setCopies([...list].sort((a, b) => Number(!!b.target) - Number(!!a.target) || qualityRank(b.quality) - qualityRank(a.quality) || (b.sizeBytes ?? 0) - (a.sizeBytes ?? 0))))
      .catch((e) => !cancelled && setError(errorText(e)));
    providerCopies(request).then((list) => !cancelled && setProvider(list));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request.mediaType, request.mediaId]);
  return { copies, provider, error };
}

/**
 * The copies of the add-on film or episode playing, to switch to another
 * from `at` seconds in; null for anything else.
 */
export function switchRequest(item: NowPlaying, at: number, resume: boolean): CopiesRequest | null {
  if (item.sourceId !== "addon" || (item.kind !== "movie" && item.kind !== "episode")) return null;
  // "addon|movie|tt123", or "addon|series|tt123|ep|tt123:1:2" for an episode.
  const [, type, ...rest] = item.itemId.split("|");
  const episodeId = item.itemId.split("|ep|")[1];
  if (type === "movie" && rest.length) {
    const year = Number(item.subtitle);
    return { mediaType: "movie", mediaId: rest.join("|"), itemId: item.itemId, title: item.title, subtitle: item.subtitle, artwork: item.artworkUrl, year: Number.isInteger(year) && year > 1800 ? year : null, switching: { at, playingUrl: playingUrl(item), resume } };
  }
  if (type !== "series" || !episodeId) return null;
  // Its season and number, for your provider's copy of the same episode: from "tt123:1:2", or "S01 E02 · …".
  const [, season, number] = episodeId.match(/:(\d+):(\d+)$/) ?? item.subtitle?.match(/S(\d+)\s*E(\d+)/) ?? [];
  return {
    mediaType: "series",
    mediaId: episodeId,
    itemId: item.itemId,
    title: item.title,
    subtitle: item.subtitle,
    artwork: item.artworkUrl,
    episode: season && number ? { season: Number(season), number: Number(number) } : null,
    switching: { at, playingUrl: playingUrl(item), resume },
  };
}

const playingUrl = (item: NowPlaying) => (item.request.kind === "target" ? item.request.target.url : null);

/** Plays a copy, resuming where this title was left (or the moment a switch was asked for from) unless asked to start over. */
export async function playCopy(copy: AddonStream, request: CopiesRequest) {
  if (!copy.target) return;
  if (request.switching) {
    playCopyAt(copy, request, request.switching.at);
    return;
  }
  const saved = await api.progress(request.itemId).catch(() => null);
  const resume = !request.fromStart && saved && !saved.finished;
  playCopyAt(copy, request, resume ? saved.positionSeconds : 0, saved?.durationSeconds ?? null);
}

function playCopyAt(copy: AddonStream, request: CopiesRequest, at: number, durationHint: number | null = null) {
  if (!copy.target) return;
  const isEpisode = request.itemId.includes("|ep|");
  useApp.getState().play({
    request: { kind: "target", target: copy.target },
    kind: isEpisode ? "episode" : "movie",
    itemId: request.itemId,
    seriesId: isEpisode ? request.itemId.split("|ep|")[0] : null,
    sourceId: "addon",
    title: request.title,
    subtitle: request.subtitle,
    artworkUrl: request.artwork,
    resumeFrom: at,
    durationHint,
    copy: { addonName: copy.addonName, quality: copy.quality },
  });
}

/** The playable copy most like the one before: same add-on and quality if there is one, else the best. */
export function similarCopy(copies: AddonStream[], like: NowPlaying["copy"]) {
  const playable = copies.filter((c) => c.target).sort((a, b) => qualityRank(b.quality) - qualityRank(a.quality) || (b.sizeBytes ?? 0) - (a.sizeBytes ?? 0));
  return (
    playable.find((c) => like && c.addonName === like.addonName && c.quality === like.quality) ??
    playable.find((c) => like && c.quality === like.quality) ??
    playable[0] ??
    null
  );
}

interface CopyListsProps {
  copies: AddonStream[] | null;
  provider: ProviderCopy[];
  error: string | null;
  onPlay: (c: AddonStream) => void;
  onProvider: (c: ProviderCopy) => void;
  /** The copy playing now, marked so it isn't chosen again by mistake. */
  playingUrl?: string | null;
}

function CopyLists({ copies, provider, error, onPlay, onProvider, playingUrl }: CopyListsProps) {
  return (
    <>
      {error ? (
        <p className="muted">{error}</p>
      ) : copies === null ? (
        <div className="spinner" style={{ margin: "12px 0" }} />
      ) : copies.length === 0 ? (
        <p className="muted">None of your add-ons has a copy of this.</p>
      ) : (
        <div className="copies">
          {copies.map((c, i) => (
            <button key={i} className="copy-row" disabled={!c.target} onClick={() => onPlay(c)} title={c.target ? undefined : "This copy needs a torrent engine, which Xorvio doesn't include."}>
              <Play size={14} fill="currentColor" strokeWidth={0} />
              <span className="copy-quality">{c.quality ?? "—"}</span>
              <span className="copy-text">
                <span className="copy-name">{c.name}</span>
                <span className="copy-meta">{[playingUrl && c.target?.url === playingUrl ? "Playing now" : null, c.addonName, c.sizeBytes ? fileSize(c.sizeBytes) : null, ...c.tags].filter(Boolean).join(" · ")}</span>
              </span>
            </button>
          ))}
        </div>
      )}
      {provider.length > 0 && (
        <>
          <h3 className="copies-heading">Your Provider</h3>
          <div className="copies">
            {provider.map((c, i) => (
              <button key={i} className="copy-row" onClick={() => onProvider(c)}>
                <Server size={14} />
                <span className="copy-quality">IPTV</span>
                <span className="copy-text">
                  <span className="copy-name">{c.label}</span>
                  <span className="copy-meta">{c.meta}</span>
                </span>
              </button>
            ))}
          </div>
        </>
      )}
    </>
  );
}

/** A title page's copies. With saved progress, each picks up where it was left. */
export function CopiesSection({ resumeSeconds, ...request }: CopiesRequest & { resumeSeconds?: number | null }) {
  const { copies, provider, error } = useCopies(request);
  return (
    <section className="title-section">
      <h2 className="section-title">Copies</h2>
      <p className="section-subtitle">
        From your add-ons{provider.length ? " and your provider" : ""}
        {resumeSeconds ? ` · plays from ${playerClock(resumeSeconds)}` : ""}
      </p>
      <CopyLists copies={copies} provider={provider} error={error} onPlay={(c) => playCopy(c, request)} onProvider={(c) => c.play()} />
    </section>
  );
}

/** `onDismiss` is closing it without choosing (Esc, the close button), when that's different from `onClose`. */
export function CopiesSheet({ onClose, onDismiss, ...request }: CopiesRequest & { onClose: () => void; onDismiss?: () => void }) {
  const { copies, provider, error } = useCopies(request);
  const { switching } = request;
  const name = request.subtitle ? `${request.title} · ${request.subtitle}` : request.title;
  return (
    <Sheet
      title={switching ? "Switch to Another Copy" : request.fromStart ? "Play from the Beginning" : "Choose a Copy"}
      subtitle={switching && switching.at > 5 ? `${name} · plays on from ${playerClock(switching.at)}` : name}
      onClose={onDismiss ?? onClose}
      width={620}
    >
      <CopyLists
        copies={copies}
        provider={provider}
        error={error}
        playingUrl={switching?.playingUrl}
        onPlay={(c) => {
          onClose();
          playCopy(c, request);
        }}
        onProvider={(c) => {
          onClose();
          c.play();
        }}
      />
    </Sheet>
  );
}

/** The copies sheet any screen can open (Continue Watching, the hero). */
export function CopiesSheetHost() {
  const request = useApp((s) => s.copies);
  const showCopies = useApp((s) => s.showCopies);
  if (!request) return null;
  return (
    <CopiesSheet
      {...request}
      onClose={() => showCopies(null)}
      onDismiss={() => {
        showCopies(null);
        // Nothing chosen instead of the copy playing: it plays on.
        if (request.switching?.resume) resumePlayback();
      }}
    />
  );
}
