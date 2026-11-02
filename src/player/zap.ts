// Changing channel from the player: up and down (the keys, the channel bar,
// the media keys, the menu bar), a number typed, or the channel list. Up and
// down show the channel at once and open its stream once the zapping stops,
// as the Swift app's channel switch delay did, so the channels passed on
// the way never open a provider connection. They move through the list the
// channel was chosen from in the player's channel list, or else its category.

import { channelItem, playChannel } from "../lib/actions";
import { api } from "../lib/api";
import { useApp, type NowPlaying } from "../lib/store";
import type { Channel } from "../lib/types";
import { sameChannel } from "../lib/versions";
import { isPopOutWindow } from "../lib/windows";
import { channelForNumber, stepChannel, zapDelay } from "./playerRules";

/** The list a channel was chosen from, while it (or a channel zapped to from it) plays. */
let context: { channels: Channel[]; item: NowPlaying } | null = null;
/** The playing channel's category, read once so quick presses all move from the same list. */
let category: { key: string | null; channels: Channel[] } | null = null;
let everything: Promise<Channel[]> | null = null;
let lastZap: number | null = null;
let pending: ReturnType<typeof setTimeout> | undefined;

// Renamed, hidden or locked channels change what the lists hold.
useApp.subscribe((now, before) => {
  if (now.editsRevision === before.editsRevision && now.guideRevision === before.guideRevision) return;
  category = null;
  everything = null;
});

/** The channels up and down move through for `channel`, when they're at hand. */
function listAtHand(channel: Channel, playing: NowPlaying): Channel[] | null {
  if (context && context.item === playing && context.channels.some((c) => sameChannel(c, channel))) return context.channels;
  if (category && category.key === channel.categoryId && category.channels.some((c) => sameChannel(c, channel))) return category.channels;
  return null;
}

async function loadCategory(channel: Channel) {
  const channels = await api.channels({ categoryId: channel.categoryId }).catch(() => [] as Channel[]);
  category = { key: channel.categoryId, channels };
  return channels;
}

/** Every channel, for numbers typed; read once, and again after changes to them. */
export function allChannels(): Promise<Channel[]> {
  everything ??= api.channels({}).catch(() => {
    everything = null;
    return [] as Channel[];
  });
  return everything;
}

/** The presentation a channel change keeps: where the video is now, full screen when it isn't anywhere. */
function keptPresentation() {
  const shown = useApp.getState().presentation;
  return shown === "hidden" ? "full" : shown;
}

/** Shows a channel now and opens its stream once the zapping stops. */
export function zapTo(channel: Channel) {
  const app = useApp.getState();
  const presentation = keptPresentation();
  // While the video is popped out, the main window's choices play there.
  if (app.poppedOut && !isPopOutWindow) return playChannel(channel, presentation);
  const now = Date.now();
  const delay = zapDelay(lastZap === null ? null : now - lastZap);
  lastZap = now;
  clearTimeout(pending);
  const item = channelItem(channel);
  const fromContext = !!context && context.item === app.nowPlaying;
  // The stream before goes now, so the provider's connection is free by the time this one opens.
  if (app.stream) api.releaseStream(app.stream.url).catch(() => {});
  useApp.setState({ nowPlaying: item, stream: null, playError: null, presentation });
  if (fromContext && context) context = { ...context, item };
  pending = setTimeout(() => {
    const { nowPlaying, presentation: shown, play } = useApp.getState();
    if (nowPlaying === item) play(item, shown);
  }, delay);
}

/** Channel up (`by` 1, the next in the list) or down (-1); from a replay, the channel beside its own, live. */
export async function zap(by: 1 | -1) {
  const playing = useApp.getState().nowPlaying;
  const channel = playing?.channel;
  if (!playing || !channel) return;
  const channels = listAtHand(channel, playing) ?? (await loadCategory(channel));
  // Zapped again, or closed, while the list loaded: that press is spent.
  if (useApp.getState().nowPlaying !== playing) return;
  const i = channels.findIndex((c) => sameChannel(c, channel));
  const to = channels[stepChannel(i, by, channels.length)];
  if (to && !sameChannel(to, channel)) zapTo(to);
}

/** Channel up: the next channel in the list. */
export const channelUp = () => zap(1);
/** Channel down: the one before. */
export const channelDown = () => zap(-1);

/** Plays a channel picked from `channels` (the player's channel list), which up and down then move through. */
export function playFromList(channel: Channel, channels: Channel[]) {
  clearTimeout(pending);
  const before = useApp.getState().nowPlaying;
  // The channel playing already plays on, rather than tuning in again (a second request to the provider).
  const already = before?.kind === "channel" && !!before.channel && sameChannel(before.channel, channel) && !!useApp.getState().stream;
  if (!already) playChannel(channel, keptPresentation());
  const playing = useApp.getState().nowPlaying;
  context = playing ? { channels, item: playing } : null;
}

/** The channel a typed number means, or null when there's none. */
export async function channelNumbered(digits: string): Promise<Channel | null> {
  const playing = useApp.getState().nowPlaying;
  const channel = playing?.channel;
  const near = channel && playing ? (listAtHand(channel, playing) ?? []) : [];
  const found = channelForNumber(digits, near, channel?.sourceId);
  return found ?? channelForNumber(digits, await allChannels(), channel?.sourceId);
}

/** Plays a channel by its number, as typed: at once, since a number is chosen on purpose. */
export function playNumbered(channel: Channel) {
  clearTimeout(pending);
  playChannel(channel, keptPresentation());
}

// The playing channel's category is read ahead, so the first press of up or down moves at once.
useApp.subscribe((now, before) => {
  const channel = now.nowPlaying?.channel;
  if (!channel || now.nowPlaying === before.nowPlaying || now.nowPlaying?.kind !== "channel") return;
  if (!listAtHand(channel, now.nowPlaying!)) loadCategory(channel);
});
