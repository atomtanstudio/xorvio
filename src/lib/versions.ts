// Channels a provider lists several times ("News 24 HD", "News 24 4K") show
// as one row standing for every version (Settings › Guide › Group similar
// channels). Favourites, groups, hiding and locks still belong to real
// channel ids, so whatever someone does to a row applies to its versions.

import type { Channel } from "./types";

/** Every channel a row stands for: its versions, or just itself. */
export function versionIds(channel: Pick<Channel, "id" | "versions">): string[] {
  return channel.versions?.length ? channel.versions.map((v) => v.id) : [channel.id];
}

/** One channel, or two versions of one: Last Channel and "playing" go by this, not by id. */
export function sameChannel(a: Channel, b: Channel) {
  return a.id === b.id || versionIds(a).includes(b.id) || versionIds(b).includes(a.id);
}

/** A row is a favourite when any of its versions is. */
export function isFavoriteChannel(favorites: Set<string>, channel: Channel) {
  return versionIds(channel).some((id) => favorites.has(id));
}
