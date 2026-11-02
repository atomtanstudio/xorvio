// Handing a stream to another player on this Mac (IINA, VLC, Infuse), for
// what they play better. The gateway gives them an address of their own,
// the original stream with the provider's headers, which lasts until Xorvio
// quits; the provider's own address never leaves the backend.

import { ExternalLink } from "lucide-react";
import type { MenuEntry } from "../components/Menu";
import { api, errorText } from "../lib/api";
import { useApp, type NowPlaying } from "../lib/store";
import type { ExternalPlayer, PlayRequest } from "../lib/types";

let players: ExternalPlayer[] = [];
let checkedAt = 0;

/** Looks for the players again, at most once a minute (one can be installed while Xorvio runs). */
export function refreshExternalPlayers() {
  if (Date.now() - checkedAt < 60_000) return;
  checkedAt = Date.now();
  api
    .externalPlayers()
    .then((found) => (players = found))
    .catch(() => {});
}

/** The players found when last looked, since menus are built at once. */
export function externalPlayers() {
  refreshExternalPlayers();
  return players;
}

/**
 * What the other player shows: "Severance - S01E03 - Good News About Hell",
 * "The Matrix (1999)". Infuse looks titles up by names like these.
 */
export function externalTitle(item: Pick<NowPlaying, "kind" | "title" | "subtitle">) {
  const { kind, title, subtitle } = item;
  if (kind === "episode" && subtitle) return `${title} - ${subtitle.replace(/^S(\d+) E(\d+)/, "S$1E$2").replace(" · ", " - ")}`;
  if (kind === "movie" && subtitle && /^\d{4}$/.test(subtitle)) return `${title} (${subtitle})`;
  return title;
}

/** Opens a stream in another player; says why in a toast when it can't. True once it's opened. */
export async function openInPlayer(player: ExternalPlayer, source: { streamUrl?: string; request?: PlayRequest }, title: string, positionSeconds: number | null = null) {
  try {
    await api.openInPlayer(player.id, source.streamUrl ?? null, source.request ?? null, title, positionSeconds);
    return true;
  } catch (error) {
    useApp.getState().showToast(errorText(error), "error");
    return false;
  }
}

/** Where a film or episode was left, to start there in the other player. */
async function savedPosition(itemId: string) {
  const saved = await api.progress(itemId).catch(() => null);
  return saved && !saved.finished ? saved.positionSeconds : null;
}

/** "Open in IINA" and the others installed, for a title's or a channel's menu. */
export function openInEntries(request: PlayRequest, title: string, resumeItemId?: string): MenuEntry[] {
  return externalPlayers().map((player) => ({
    label: `Open in ${player.name}`,
    icon: <ExternalLink size={15} strokeWidth={1.8} />,
    onSelect: async () => openInPlayer(player, { request }, title, resumeItemId ? await savedPosition(resumeItemId) : null),
  }));
}
