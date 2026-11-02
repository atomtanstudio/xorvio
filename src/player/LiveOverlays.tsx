// What the full player shows over a live channel, as on TiviMate: the
// channel list (Return opens it, up and down move, Return watches, left and
// right change the group), the Now/Next card (after a channel change for a
// few seconds, and with G), and the channel number being typed. Their state
// is here rather than in the player, so the keys and the menu bar can work
// them as plain functions.

import { useVirtualizer } from "@tanstack/react-virtual";
import { Bell, BellRing, ChevronDown, ChevronLeft, ChevronRight, RotateCcw, Star, Volume2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { create } from "zustand";
import { ChannelLogo } from "../components/Artwork";
import { openMenu } from "../components/Menu";
import { playReplay, programEntry, toggleFavoriteChannel, toggleWatchlist } from "../lib/actions";
import { api } from "../lib/api";
import { clockTime, nowSeconds } from "../lib/format";
import { useApp } from "../lib/store";
import type { Channel, ChannelFilter, Program } from "../lib/types";
import { isFavoriteChannel, sameChannel } from "../lib/versions";
import { DIGIT_PAUSE_MS, moveHighlight, NOT_FOUND_MS, stepChannel, typeDigit } from "./playerRules";
import { channelNumbered, playFromList, playNumbered } from "./zap";

interface ListGroup {
  key: string;
  name: string;
  filter: ChannelFilter;
}

interface LiveOverlays {
  /** The channel list is open. */
  list: boolean;
  groups: ListGroup[];
  group: number;
  /** The group's channels; null while they load. */
  channels: Channel[] | null;
  highlight: number;
  /** The Now/Next card shows; `pinned` when G (or its button) opened it, so it stays. */
  nowNext: boolean;
  pinned: boolean;
  /** A channel number being typed, and whether there's no such channel. */
  digits: string;
  notFound: boolean;
}

export const useLiveOverlays = create<LiveOverlays>(() => ({
  list: false,
  groups: [],
  group: 0,
  channels: null,
  highlight: 0,
  nowNext: false,
  pinned: false,
  digits: "",
  notFound: false,
}));

const state = () => useLiveOverlays.getState();
const set = (patch: Partial<LiveOverlays>) => useLiveOverlays.setState(patch);
let loads = 0;
const groupChannels = new Map<string, Promise<Channel[]>>();

// MARK: The channel list

/** The groups the list moves through: every channel, Favorites, the person's own groups, then the provider's categories. */
async function listGroups(): Promise<ListGroup[]> {
  const categories = await api.categories("live").catch(() => []);
  const { favorites, groups } = useApp.getState();
  return [
    { key: "all", name: "All Channels", filter: {} },
    ...(favorites.size ? [{ key: "favorites", name: "Favorites", filter: { favoritesOnly: true } }] : []),
    ...groups.map((g) => ({ key: `list:${g.id}`, name: g.name, filter: { listId: g.id } })),
    ...categories.map((c) => ({ key: `category:${c.id}`, name: c.name, filter: { categoryId: c.id } })),
  ];
}

async function showGroup(index: number) {
  const group = state().groups[index];
  if (!group) return;
  const load = ++loads;
  set({ group: index, channels: null, highlight: 0 });
  let channels = groupChannels.get(group.key);
  if (!channels) {
    channels = api.channels(group.filter).catch(() => [] as Channel[]);
    groupChannels.set(group.key, channels);
  }
  const list = await channels;
  if (load !== loads || !state().list) return;
  const playing = useApp.getState().nowPlaying?.channel;
  const at = playing ? list.findIndex((c) => sameChannel(c, playing)) : -1;
  set({ channels: list, highlight: Math.max(0, at) });
}

/** Opens the channel list on the playing channel's category (or every channel), or closes it. */
export async function toggleChannelList(open = !state().list) {
  if (!open) return closeChannelList();
  const channel = useApp.getState().nowPlaying?.channel;
  if (!channel) return;
  // Lists are read again each time it opens: favourites and names may have changed.
  groupChannels.clear();
  set({ list: true, channels: null, groups: [] });
  const groups = await listGroups();
  if (!state().list) return;
  set({ groups });
  const start = groups.findIndex((g) => g.key === `category:${channel.categoryId}`);
  showGroup(start >= 0 ? start : 0);
}

export function closeChannelList() {
  loads += 1;
  set({ list: false });
}

/** Up and down in the list. */
export function moveListHighlight(by: number) {
  const { channels, highlight } = state();
  if (channels) set({ highlight: moveHighlight(highlight, by, channels.length) });
}

/** Left and right: the group before or after, going round. */
export function stepListGroup(by: 1 | -1) {
  const { groups, group } = state();
  if (groups.length) showGroup(stepChannel(group, by, groups.length));
}

/** Return in the list: watch the channel highlighted, and close it. */
export function watchHighlighted() {
  const { channels, highlight } = state();
  const channel = channels?.[highlight];
  if (channel) playFromList(channel, channels!);
  closeChannelList();
}

// MARK: Now and next

let nowNextTimer: ReturnType<typeof setTimeout> | undefined;
let hovering = false;
/** How long the card shows after a channel change. */
const FLASH_MS = 5000;

function hideLater(ms: number) {
  clearTimeout(nowNextTimer);
  nowNextTimer = setTimeout(() => !hovering && !state().pinned && set({ nowNext: false }), ms);
}

/** What's on, for a few seconds after changing channel. */
export function flashNowNext() {
  if (state().pinned) return;
  set({ nowNext: true });
  hideLater(FLASH_MS);
}

/** G: the Now/Next card, until G again (which also puts away one shown after a channel change). */
export function toggleNowNext() {
  clearTimeout(nowNextTimer);
  const open = !state().nowNext;
  set({ nowNext: open, pinned: open });
}

export function closeNowNext() {
  clearTimeout(nowNextTimer);
  hovering = false;
  set({ nowNext: false, pinned: false });
}

// MARK: Channel numbers

let digitTimer: ReturnType<typeof setTimeout> | undefined;

/** A digit typed over a live channel: after a short pause the channel with that number plays, as on a TV remote. */
export function typeChannelDigit(key: string) {
  const digits = typeDigit(state().notFound ? "" : state().digits, key);
  set({ digits, notFound: false });
  clearTimeout(digitTimer);
  digitTimer = setTimeout(async () => {
    const found = await channelNumbered(digits);
    if (state().digits !== digits) return;
    if (found) {
      set({ digits: "" });
      playNumbered(found);
      return;
    }
    set({ notFound: true });
    digitTimer = setTimeout(() => state().digits === digits && set({ digits: "", notFound: false }), NOT_FOUND_MS);
  }, DIGIT_PAUSE_MS);
}

/** Esc while typing a number: forget it. */
export function cancelChannelDigits() {
  clearTimeout(digitTimer);
  set({ digits: "", notFound: false });
}

// Nothing of this outlives the channel; a film never shows it.
useApp.subscribe((now, before) => {
  if (now.nowPlaying === before.nowPlaying) return;
  if (!now.nowPlaying?.channel || now.nowPlaying.kind !== "channel") {
    closeChannelList();
    closeNowNext();
    cancelChannelDigits();
  } else if (now.nowPlaying.channel.id !== before.nowPlaying?.channel?.id) flashNowNext();
});

// MARK: Views

/** The programme on now and the one after, from a channel's listings. */
function nowAndNext(programs: Program[], t: number) {
  const current = programs.find((p) => p.start <= t && t < p.end) ?? null;
  const next = programs.find((p) => p.start >= (current?.end ?? t)) ?? null;
  return { current, next };
}

const ROW = 56;

export function ChannelListOverlay() {
  const { groups, group, channels, highlight } = useLiveOverlays();
  const playing = useApp((s) => s.nowPlaying?.channel);
  const favorites = useApp((s) => s.favorites);
  const scroller = useRef<HTMLDivElement>(null);
  const [onNow, setOnNow] = useState<Map<string, Program[]>>(new Map());
  const list = channels ?? [];
  const virtualizer = useVirtualizer({ count: list.length, getScrollElement: () => scroller.current, estimateSize: () => ROW, overscan: 6 });
  const rows = virtualizer.getVirtualItems();
  const first = rows[0]?.index ?? 0;
  const last = rows[rows.length - 1]?.index ?? 0;

  useEffect(() => setOnNow(new Map()), [channels]);

  // What's on, for the rows in view.
  useEffect(() => {
    const ids = list.slice(first, last + 1).map((c) => c.id).filter((id) => !onNow.has(id));
    if (!ids.length) return;
    let cancelled = false;
    api
      .nowNext(ids)
      .then((found) => {
        if (cancelled) return;
        setOnNow((before) => {
          const after = new Map(before);
          ids.forEach((id) => after.set(id, []));
          found.forEach((row) => after.set(row.channel.id, row.programs));
          return after;
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [list, first, last, onNow]);

  // The highlight stays in view; a new group opens on it in the middle.
  const opened = useRef<Channel[] | null>(null);
  useEffect(() => {
    if (!channels?.length) return;
    const fresh = opened.current !== channels;
    opened.current = channels;
    virtualizer.scrollToIndex(highlight, { align: fresh ? "center" : "auto" });
  }, [channels, highlight, virtualizer]);

  const name = groups[group]?.name ?? "Channels";
  const t = nowSeconds();
  return (
    <div className="live-list glass" onMouseDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()} role="dialog" aria-label="Channel list">
      <div className="live-list-header">
        <button className="player-icon" onClick={() => stepListGroup(-1)} aria-label="Previous group" title="Previous group (←)" disabled={groups.length < 2}>
          <ChevronLeft size={17} />
        </button>
        <button
          className="live-list-group"
          disabled={!groups.length}
          onClick={(e) =>
            openMenu(
              e,
              [{ heading: "Groups" }, ...groups.map((g, i) => ({ label: g.name, icon: i === group ? <ChevronRight size={14} /> : <span style={{ width: 14 }} />, onSelect: () => showGroup(i) }))],
              true,
            )
          }
        >
          <span>{name}</span>
          <ChevronDown size={14} />
        </button>
        <button className="player-icon" onClick={() => stepListGroup(1)} aria-label="Next group" title="Next group (→)" disabled={groups.length < 2}>
          <ChevronRight size={17} />
        </button>
        <div style={{ flex: 1 }} />
        <button className="player-icon" onClick={closeChannelList} aria-label="Close the channel list" title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
      <div className="live-list-hint">↑ ↓ to move · ← → for groups · Return to watch</div>
      <div className="live-list-rows" ref={scroller}>
        {channels === null ? (
          <div className="spinner" style={{ margin: "24px auto" }} />
        ) : !list.length ? (
          <p className="live-list-empty">No channels in this group.</p>
        ) : (
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {rows.map((row) => {
              const channel = list[row.index];
              const { current } = nowAndNext(onNow.get(channel.id) ?? [], t);
              const isPlaying = !!playing && sameChannel(playing, channel);
              return (
                <button
                  key={channel.id}
                  className={`live-list-row ${row.index === highlight ? "highlighted" : ""} ${isPlaying ? "playing" : ""}`}
                  style={{ top: row.start, height: ROW }}
                  onClick={() => {
                    set({ highlight: row.index });
                    playFromList(channel, list);
                  }}
                >
                  <span className="num">{channel.number ?? ""}</span>
                  <ChannelLogo src={channel.logoUrl} name={channel.name} size={30} />
                  <span className="live-list-text">
                    <span className="name">
                      {channel.name}
                      {isPlaying && <Volume2 size={11} className="playing-mark" aria-label="Playing" />}
                      {isFavoriteChannel(favorites, channel) && <Star size={10} fill="#f5c84c" color="#f5c84c" aria-label="Favorite" />}
                    </span>
                    {current && (
                      <>
                        <span className="now">{current.title}</span>
                        <span className="overlay-meter">
                          <span style={{ width: `${Math.min(100, ((t - current.start) / (current.end - current.start)) * 100)}%` }} />
                        </span>
                      </>
                    )}
                  </span>
                </button>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

/** What's on now and next on the playing channel, with From Start, Remind Me and Favorite. */
export function NowNextCard({ channel }: { channel: Channel }) {
  const favorites = useApp((s) => s.favorites);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const [programs, setPrograms] = useState<Program[] | null>(null);
  const [, tick] = useState(0);

  // Gone from under the pointer (Esc, the list opening): it isn't hovered any more.
  useEffect(
    () => () => {
      hovering = false;
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    setPrograms(null);
    const load = () =>
      api
        .nowNext([channel.id])
        .then(([row]) => !cancelled && setPrograms(row?.programs ?? []))
        .catch(() => !cancelled && setPrograms([]));
    load();
    const timer = setInterval(() => {
      tick((n) => n + 1);
      load();
    }, 60_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [channel.id]);

  const t = nowSeconds();
  const { current, next } = nowAndNext(programs ?? [], t);
  const favorite = isFavoriteChannel(favorites, channel);
  const reminded = !!next && watchlistIds.has(next.id);
  const left = current ? Math.max(1, Math.round((current.end - t) / 60)) : 0;
  return (
    <div
      className="now-next glass"
      onMouseEnter={() => {
        hovering = true;
        clearTimeout(nowNextTimer);
      }}
      onMouseLeave={() => {
        hovering = false;
        if (!state().pinned) hideLater(2500);
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      aria-label={`Now and next on ${channel.name}`}
    >
      <div className="now-next-channel">
        <ChannelLogo src={channel.logoUrl} name={channel.name} size={40} />
        {channel.number != null && <span className="num">{channel.number}</span>}
      </div>
      <div className="now-next-now">
        <div className="now-next-label">{channel.name}</div>
        {current ? (
          <>
            <div className="now-next-title">
              <span>{current.title}</span>
              {current.flags.isNew && <span className="badge">New</span>}
            </div>
            <div className="now-next-meta">
              <span>
                {clockTime(current.start)} – {clockTime(current.end)}
              </span>
              <span className="overlay-meter">
                <span style={{ width: `${Math.min(100, ((t - current.start) / (current.end - current.start)) * 100)}%` }} />
              </span>
              <span>{left} min left</span>
            </div>
            {(current.summary ?? current.subtitle) && <p className="now-next-summary">{current.summary ?? current.subtitle}</p>}
          </>
        ) : (
          <div className="now-next-title muted">{programs === null ? " " : "No guide information"}</div>
        )}
      </div>
      {next && (
        <div className="now-next-next">
          <div className="now-next-label">Next · {clockTime(next.start)}</div>
          <div className="now-next-next-title">{next.title}</div>
        </div>
      )}
      <div className="now-next-actions">
        {current && channel.catchupDays > 0 && (
          <button className="btn btn-glass btn-small" onClick={() => playReplay(channel, current)}>
            <RotateCcw size={13} /> From Start
          </button>
        )}
        {next && (
          <button className={`btn btn-glass btn-small ${reminded ? "on" : ""}`} onClick={() => toggleWatchlist(programEntry(next, channel))} title={`Remind me when ${next.title} starts`}>
            {reminded ? <BellRing size={13} /> : <Bell size={13} />} {reminded ? "Reminder Set" : "Remind Me"}
          </button>
        )}
        <button className={`btn btn-glass btn-small ${favorite ? "on" : ""}`} onClick={() => toggleFavoriteChannel(channel)}>
          <Star size={13} fill={favorite ? "currentColor" : "none"} /> {favorite ? "Favorite" : "Add Favorite"}
        </button>
      </div>
    </div>
  );
}

/** The channel number being typed, large at the top right. */
export function ChannelNumberEntry() {
  const { digits, notFound } = useLiveOverlays();
  if (!digits) return null;
  return (
    <div className="number-entry glass" aria-live="polite" aria-label={notFound ? `No channel ${digits}` : `Channel ${digits}`}>
      <div className="number-entry-digits">{digits}</div>
      {notFound && <div className="number-entry-missing">No channel {digits}</div>}
    </div>
  );
}
