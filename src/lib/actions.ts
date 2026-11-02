// What people do with titles, channels and programmes, in one place so every
// screen offers the same choices: play, open, save to the watchlist, record.

import { api, errorText } from "./api";
import { episodeCode, nowSeconds, whenLabel } from "./format";
import { recordingFor, useApp, type NowPlaying } from "./store";
import type { Channel, ContinueItem, Episode, LaunchScreen, Movie, Program, ScheduleRequest, Series, WatchlistEntry } from "./types";
import { sameChannel, versionIds } from "./versions";

const store = () => useApp.getState();

/** `at` plays from that moment instead (another copy switched to while one played). */
export async function playMovie(movie: Movie, fromStart = false, at?: number) {
  // Add-on titles have no address of their own: choose one of their copies.
  if (movie.stream.type === "addon") {
    store().showCopies({ mediaType: "movie", mediaId: movie.stream.mediaId, itemId: movie.id, title: movie.title, subtitle: movie.year ? String(movie.year) : null, artwork: movie.backdropUrl ?? movie.posterUrl, year: movie.year, fromStart });
    return;
  }
  const saved = fromStart || at !== undefined ? null : await api.progress(movie.id).catch(() => null);
  store().play({
    request: { kind: "movie", movieId: movie.id },
    kind: "movie",
    itemId: movie.id,
    sourceId: movie.sourceId,
    title: movie.title,
    subtitle: movie.year ? String(movie.year) : null,
    artworkUrl: movie.backdropUrl ?? movie.posterUrl,
    resumeFrom: at ?? (saved && !saved.finished ? saved.positionSeconds : 0),
    durationHint: movie.durationSeconds,
  });
}

export async function playEpisode(episode: Episode, series: Series | null, fromStart = false, at?: number) {
  if (episode.stream.type === "addon") {
    store().showCopies({
      mediaType: "series",
      mediaId: episode.stream.mediaId,
      itemId: episode.id,
      title: series?.title ?? episode.title,
      subtitle: `${episodeCode(episode.season, episode.number)} · ${episode.title}`,
      artwork: episode.stillUrl ?? series?.backdropUrl,
      year: series?.year,
      episode: { season: episode.season, number: episode.number },
      fromStart,
    });
    return;
  }
  const saved = fromStart || at !== undefined ? null : await api.progress(episode.id).catch(() => null);
  store().play({
    request: { kind: "episode", episodeId: episode.id },
    kind: "episode",
    itemId: episode.id,
    seriesId: episode.seriesId,
    sourceId: episode.sourceId,
    title: series?.title ?? episode.title,
    subtitle: `${episodeCode(episode.season, episode.number)} · ${episode.title}`,
    artworkUrl: episode.stillUrl ?? series?.backdropUrl,
    resumeFrom: at ?? (saved && !saved.finished ? saved.positionSeconds : 0),
    durationHint: episode.durationSeconds,
  });
}

/** A live channel as the player (or a Multiview tile) plays it. */
export function channelItem(channel: Channel): NowPlaying {
  return {
    request: { kind: "channel", channelId: channel.id },
    kind: "channel",
    itemId: channel.id,
    sourceId: channel.sourceId,
    title: channel.name,
    subtitle: channel.number ? String(channel.number) : null,
    artworkUrl: channel.logoUrl,
    channel,
  };
}

/** Plays a channel; in Live TV it plays in the docked panel first, as TiviMate does. */
export function playChannel(channel: Channel, presentation: "full" | "docked" | "mini" = "full") {
  store().play(channelItem(channel), presentation);
}

/**
 * Plays one version of a grouped row (from its Versions sheet). It plays as
 * itself, so recordings and history name the version watched, and carries
 * the row's versions so Last Channel and Live TV still see one channel.
 */
export async function playVersion(row: Channel, versionId: string) {
  const version = versionId === row.id ? row : await api.channel(versionId).catch(() => null);
  if (!version) {
    store().showToast("That version is no longer in your sources.", "error");
    return;
  }
  const { presentation, tab } = store();
  playChannel({ ...version, versions: row.versions }, presentation !== "hidden" ? presentation : tab === "live" ? "docked" : "full");
}

/** The channel tuned to before the one playing (or the last one, when nothing is). */
export function lastChannel(): Channel | null {
  const { channelTrail, nowPlaying } = store();
  const playing = nowPlaying?.channel;
  return channelTrail.find((c) => !playing || !sameChannel(c, playing)) ?? null;
}

/** Adds a row to Favorites, or takes it out with every version of it that's there. */
export async function toggleFavoriteChannel(channel: Channel) {
  const { favorites, toggleFavorite } = store();
  const marked = versionIds(channel).filter((id) => favorites.has(id));
  if (!marked.length) return toggleFavorite(channel.id);
  for (const id of marked) await toggleFavorite(id);
}

/** Flips back to the last channel, keeping the video where it is: full screen, docked or in the corner. */
export function playLastChannel() {
  const channel = lastChannel();
  if (!channel) {
    store().showToast("There's no other channel to go back to yet.");
    return;
  }
  const { presentation, tab } = store();
  playChannel(channel, presentation !== "hidden" ? presentation : tab === "live" ? "docked" : "full");
}

/**
 * Opens the screen chosen in Settings for launch. Called once, when settings
 * first load; anything already opened or playing by then (a reminder clicked,
 * the menu bar's Show Recordings) wins.
 */
export async function openLaunchScreen(choice: LaunchScreen) {
  const { tab, stack, nowPlaying, go } = store();
  if (choice === "home" || tab !== "home" || stack.length || nowPlaying) return;
  go(choice === "watchlist" ? "watchlist" : "live");
  if (choice !== "lastChannel") return;
  // Hidden and locked channels aren't among the recent ones, so they never start by themselves.
  const [channel] = await api.recentChannels(1).catch(() => []);
  if (channel && store().tab === "live" && !store().nowPlaying) playChannel(channel, "docked");
}

export function playReplay(channel: Channel, program: Program) {
  store().play({
    request: { kind: "replay", channelId: channel.id, start: program.start, end: program.end },
    kind: "program",
    itemId: program.id,
    sourceId: channel.sourceId,
    title: program.title,
    subtitle: `${channel.name} · Replay`,
    artworkUrl: program.imageUrl,
    channel,
    durationHint: program.end - program.start,
  });
}

/** Plays a Continue Watching card: resumes it, or starts the episode that's up next. */
export async function resumeContinue(item: ContinueItem, fromStart = false) {
  if (item.kind === "movie") {
    const movie = await api.movie(item.itemId);
    if (movie) playMovie(movie, fromStart);
  } else if (item.kind === "episode") {
    const seriesId = item.itemId.split("|ep|")[0];
    const [series, episodes] = await Promise.all([api.series(seriesId), api.episodes(seriesId)]);
    const episode = episodes.find((e) => e.id === item.itemId);
    if (episode) playEpisode(episode, series, fromStart);
  }
}

/** Marks a Continue Watching card watched, so its series moves on to the next episode. */
export async function markContinueWatched(item: ContinueItem) {
  const seriesId = item.kind === "episode" ? item.itemId.split("|ep|")[0] : null;
  try {
    await api.setWatched([{ itemId: item.itemId, kind: item.kind, sourceId: item.sourceId, seriesId, positionSeconds: 0, durationSeconds: 0, finished: true, updatedAt: nowSeconds() }], true);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
  useApp.setState((s) => ({ libraryRevision: s.libraryRevision + 1 }));
}

/** Takes a card off Continue Watching (a wrong copy, or something not worth finishing). */
export async function removeFromContinueWatching(item: ContinueItem) {
  try {
    await api.removeFromContinueWatching(item.itemId);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
  useApp.setState((s) => ({ libraryRevision: s.libraryRevision + 1 }));
}

export function openMovie(movie: Movie) {
  store().push({ type: "movie", id: movie.id });
}

export function openSeries(series: Series) {
  store().push({ type: "series", id: series.id });
}

export function movieEntry(movie: Movie): WatchlistEntry {
  return { id: movie.id, kind: "movie", sourceId: movie.sourceId, title: movie.title, subtitle: movie.year ? String(movie.year) : null, artworkUrl: movie.posterUrl ?? movie.backdropUrl, addedAt: nowSeconds(), channelId: null, start: null, end: null, remind: false };
}

export function seriesEntry(series: Series): WatchlistEntry {
  return { id: series.id, kind: "series", sourceId: series.sourceId, title: series.title, subtitle: series.year ? String(series.year) : null, artworkUrl: series.posterUrl ?? series.backdropUrl, addedAt: nowSeconds(), channelId: null, start: null, end: null, remind: false };
}

export function programEntry(program: Program, channel: Channel): WatchlistEntry {
  return {
    id: program.id,
    kind: "program",
    sourceId: program.sourceId,
    title: program.title,
    subtitle: channel.name,
    artworkUrl: program.imageUrl,
    addedAt: nowSeconds(),
    channelId: channel.id,
    start: program.start,
    end: program.end,
    remind: true,
  };
}

export async function toggleWatchlist(entry: WatchlistEntry) {
  const saved = !store().watchlistIds.has(entry.id);
  try {
    await store().setWatchlisted(entry, saved);
    store().showToast(saved ? (entry.kind === "program" ? `“${entry.title}” is on your watchlist, with a reminder ${whenLabel(entry.start!)}` : `Added “${entry.title}” to your watchlist`) : `Removed “${entry.title}” from your watchlist`);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

/** Turns a saved programme's reminder on or off; it stays on the watchlist either way. */
export async function setReminder(entry: WatchlistEntry, remind: boolean) {
  try {
    await store().setWatchlisted({ ...entry, remind }, true);
    store().showToast(remind && entry.start ? `Xorvio will remind you about “${entry.title}” ${whenLabel(entry.start)}` : `Reminder off for “${entry.title}”. It stays on your watchlist.`);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

/** Show in Guide: Live TV's guide on a saved programme's day and time, with it selected. */
export async function showEntryInGuide(entry: WatchlistEntry) {
  const hit = await api.program(entry.id).catch(() => null);
  if (hit) return store().showInGuide(hit);
  // The guide has moved on since it was saved: its channel at the saved time.
  const channel = entry.channelId ? await api.channel(entry.channelId).catch(() => null) : null;
  if (!channel || !entry.start || !entry.end) {
    store().showToast("The guide doesn't list this programme any more.", "error");
    return;
  }
  store().showInGuide({
    channel,
    program: {
      id: entry.id,
      sourceId: entry.sourceId,
      epgId: channel.epgId ?? channel.id,
      start: entry.start,
      end: entry.end,
      title: entry.title,
      subtitle: entry.subtitle,
      summary: null,
      category: null,
      imageUrl: entry.artworkUrl,
      episode: null,
      flags: { isNew: false, isLive: false, isPremiere: false, isRepeat: false },
      ageRating: null,
      starRating: null,
      year: null,
      genre: null,
    },
  });
}

function scheduleRequest(program: Program, channel: Channel): ScheduleRequest {
  const { settings } = store();
  return {
    sourceId: channel.sourceId,
    channelId: channel.id,
    channelName: channel.name,
    channelLogoUrl: channel.logoUrl,
    title: program.title,
    subtitle: program.episode ?? program.subtitle,
    summary: program.summary,
    artworkUrl: program.imageUrl,
    programId: program.id,
    start: program.start,
    end: program.end,
    paddingBeforeSeconds: settings.recordPaddingBefore * 60,
    paddingAfterSeconds: settings.recordPaddingAfter * 60,
  };
}

/** Schedules a programme, or starts recording at once if it's already on. */
export async function recordProgram(program: Program, channel: Channel) {
  try {
    const airing = program.start <= nowSeconds() && nowSeconds() < program.end;
    const request = scheduleRequest(program, channel);
    const recording = airing ? await api.recordNow(request) : await api.scheduleRecording(request);
    store().upsertRecording(recording);
    store().showToast(airing ? `Recording “${program.title}” until ${whenLabel(program.end).replace(/^(Today|Tonight) /, "")}` : `“${program.title}” will record ${whenLabel(program.start)}`);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

/** Record Now on a channel: the programme on air, or the next hour when the guide has nothing. */
export async function recordChannelNow(channel: Channel, current?: Program | null) {
  const t = nowSeconds();
  const program: Program = current ?? {
    id: `${channel.id}|manual|${t}`,
    sourceId: channel.sourceId,
    epgId: channel.epgId ?? channel.id,
    start: t,
    end: t + 3600,
    title: channel.name,
    subtitle: null,
    summary: null,
    category: null,
    imageUrl: null,
    episode: null,
    flags: { isNew: false, isLive: false, isPremiere: false, isRepeat: false },
    ageRating: null,
    starRating: null,
    year: null,
    genre: null,
  };
  await recordProgram(program, channel);
}

export async function cancelOrStopRecording(program: Program, channel: Channel) {
  const recording = recordingFor(store().recordings, channel.id, program.start, program.id);
  if (!recording) return;
  try {
    if (recording.status === "recording") {
      await api.stopRecording(recording.id);
      store().showToast(`Stopped recording “${recording.title}”. What was recorded is in Recordings.`);
    } else {
      await api.cancelRecording(recording.id);
      store().showToast(`Cancelled the recording of “${recording.title}”`);
    }
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
  await store().refreshRecordings().catch(() => {});
}
