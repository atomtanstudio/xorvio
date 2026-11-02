// Renaming, hiding and locking channels and categories, channel groups, the
// parental PIN, catch-up offsets and versions of a channel: what the
// right-click menus and Settings offer. Each change reloads what the store
// knows, so menus and lists follow.

import { useEffect } from "react";
import { confirmAction } from "../components/Confirm";
import { askCatchupOffset, askName, askPin, changePin, showVersions, type PinChange } from "../components/EditSheets";
import { playVersion } from "./actions";
import { api, errorText } from "./api";
import { useApp } from "./store";
import type { Channel, FavoriteList } from "./types";
import { FAVORITES_LIST } from "./types";
import { versionIds } from "./versions";

const store = () => useApp.getState();

/** Something that can be renamed, hidden or locked: a channel or a category. */
export interface Editable {
  id: string;
  name: string;
  /** A row of similar channels hides and locks as one. */
  versions?: Channel["versions"];
}

/** Does the same to every version a row stands for. */
async function eachVersion(item: Editable, work: (id: string) => Promise<unknown>) {
  for (const id of versionIds(item)) await work(id);
}

async function change(work: () => Promise<unknown>, done?: string) {
  try {
    await work();
    await store().loadEdits();
    if (done) store().showToast(done);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

// MARK: Names and hiding

export function renameItem(item: Editable, what: "Channel" | "Category") {
  const edit = store().edits.get(item.id);
  const original = edit?.originalName ?? item.name;
  return askName({
    title: `Rename ${what}`,
    initial: item.name,
    placeholder: original,
    note: edit?.displayName ? `Your source calls it “${original}”.` : "Only Xorvio uses this name. Your source keeps its own, and Reset Name goes back to it.",
    confirmLabel: "Save",
    submit: async (name) => {
      await api.renameItem(item.id, name === original ? null : name);
      await store().loadEdits();
    },
    reset: edit?.displayName
      ? async () => {
          await api.renameItem(item.id, null);
          await store().loadEdits();
        }
      : undefined,
  });
}

export const resetName = (item: Editable) => change(() => api.renameItem(item.id, null));

export function setHidden(item: Editable, hidden: boolean) {
  return change(() => eachVersion(item, (id) => api.setHidden(id, hidden)), hidden ? `“${item.name}” is hidden. Settings › Hidden & Locked shows it again.` : undefined);
}

export const showAllHidden = () => change(() => api.showAllHidden(), "Everything hidden is showing again.");

// MARK: Locks and the PIN

/** Locks behind the PIN, or removes a lock (asking for the PIN first if it hasn't been entered). */
export async function setLocked(item: Editable, locked: boolean) {
  const { parental } = store();
  if (!parental.hasPin) {
    const open = await confirmAction({
      title: "Set a PIN first",
      message: "Create a parental PIN in Settings, then lock channels and categories here.",
      confirmLabel: "Open Settings",
    });
    if (open) store().go("settings");
    return;
  }
  if (!locked && !parental.unlocked && !(await askPin("Enter the PIN to remove this lock."))) return;
  const done = !locked ? undefined : parental.unlocked ? `“${item.name}” is locked. It's hidden whenever Xorvio is locked.` : `“${item.name}” is locked.`;
  return change(() => eachVersion(item, (id) => api.setLocked(id, locked)), done);
}

// MARK: Replays and versions

/** Asks how far to move a row's replays, and moves every version of it alike. */
export function editCatchupOffset(channel: Channel) {
  return askCatchupOffset({
    channel,
    submit: async (minutes) => {
      await eachVersion(channel, (id) => api.setCatchupOffset(id, minutes));
      await store().loadEdits();
    },
  });
}

/** The versions of a row of similar channels, to play one or always play it. */
export function chooseVersion(row: Channel) {
  return showVersions({ row, play: (id) => playVersion(row, id), prefer: (id) => preferVersion(row, id) });
}

/** "Always play this version" for a row of similar channels; null goes back to the best one. */
export async function preferVersion(row: Channel, versionId: string | null) {
  const version = row.versions?.find((v) => v.id === versionId);
  const chosen = row.versions?.find((v) => v.preferred);
  if (versionId === null) {
    if (chosen) await change(() => api.setPreferredVersion(chosen.id, false), `“${row.name}” plays its best version again.`);
    return;
  }
  await change(() => api.setPreferredVersion(versionId, true), version ? `“${row.name}” will always play “${version.name}”.` : undefined);
}

/** How long an entered PIN lasts, as Settings › Parental Controls chose. */
function unlockedFor() {
  const minutes = store().settings.parentalLockMinutes;
  if (!minutes) return "until Xorvio quits or you lock them again";
  return `for ${minutes >= 60 ? `${minutes / 60} hour${minutes === 60 ? "" : "s"}` : `${minutes} minutes`}, or until you lock them again`;
}

/** Shows locked channels and categories until they lock again (Settings says when). */
export async function showLocked() {
  if (await askPin(`Locked channels and categories show ${unlockedFor()}.`)) await store().loadEdits();
}

/**
 * Reloads what's shown once an entered PIN locks again by itself, so locked
 * channels and categories leave every list without waiting for something
 * else to change.
 */
export function useRelock() {
  const locksAt = useApp((s) => s.parental.locksAt);
  useEffect(() => {
    if (!locksAt) return;
    // Timers can't wait longer than about 24 days; four hours is the most there is.
    const wait = Math.min(2 ** 31 - 1, Math.max(0, locksAt * 1000 - Date.now()) + 500);
    const timer = setTimeout(() => store().loadEdits().catch(() => {}), wait);
    return () => clearTimeout(timer);
  }, [locksAt]);
}

export const lockAgain = () => change(() => api.lockParental(), "Locked again.");

export async function editPin(mode: PinChange) {
  if (!(await changePin(mode))) return;
  await store().loadEdits();
  store().showToast(mode === "remove" ? "The parental PIN is removed." : mode === "create" ? "PIN set. Locked channels and categories are hidden until it's entered." : "PIN changed.");
}

// MARK: Groups

/** Makes a group, with a first channel when given. Resolves to the new group. */
export async function newGroup(channel?: Channel) {
  let created = null as FavoriteList | null;
  await askName({
    title: "New Group",
    placeholder: "Name, such as Sports or Kids",
    note: channel ? `${channel.name} goes in first. Add more by right-clicking channels.` : "Groups appear in Live TV under My Groups. Add channels by right-clicking them.",
    confirmLabel: "Create",
    submit: async (name) => {
      created = await api.createFavoriteList(name, channel?.id ?? null);
      await store().loadEdits();
    },
  });
  if (created && channel) store().showToast(`Added to ${created.name}.`);
  return created;
}

export function renameGroup(group: FavoriteList) {
  return askName({
    title: "Rename Group",
    initial: group.name,
    confirmLabel: "Save",
    submit: async (name) => {
      await api.renameFavoriteList(group.id, name);
      await store().loadEdits();
    },
  });
}

export async function deleteGroup(group: FavoriteList) {
  const ok = await confirmAction({ title: `Delete “${group.name}”?`, message: "The group goes; its channels stay in your sources.", confirmLabel: "Delete", destructive: true });
  if (ok) await change(() => api.deleteFavoriteList(group.id));
}

/** Adds a row to a group, or takes it out with every version of it that's there. */
export function toggleInGroup(group: FavoriteList, channel: Channel) {
  const present = versionIds(channel).filter((id) => group.channelIds.includes(id));
  if (present.length) return change(async () => Promise.all(present.map((id) => api.setInFavoriteList(group.id, id, false))));
  return change(() => api.setInFavoriteList(group.id, channel.id, true), `Added to ${group.name}.`);
}

/** A row is in a group when any of its versions is. */
export function inGroup(group: FavoriteList, channel: Channel) {
  return versionIds(channel).some((id) => group.channelIds.includes(id));
}

/** Takes a channel out of a list: Favorites or a group. */
export function removeFromList(listId: string, channel: Channel) {
  if (listId === FAVORITES_LIST) return store().toggleFavorite(channel.id);
  return change(() => api.setInFavoriteList(listId, channel.id, false));
}

/** Puts a list's channels in a new order. Hidden ones not shown keep their places. */
export function reorderList(listId: string, channelIds: string[]) {
  return change(() => api.reorderFavoriteList(listId, channelIds));
}

/** Moves a channel one place up or down among the list's channels shown. */
export function moveInList(listId: string, shown: Channel[], channel: Channel, by: -1 | 1) {
  const ids = shown.map((c) => c.id);
  const from = ids.indexOf(channel.id);
  const to = from + by;
  if (from < 0 || to < 0 || to >= ids.length) return;
  [ids[from], ids[to]] = [ids[to], ids[from]];
  return reorderList(listId, ids);
}

/** Moves the dragged channel to where another one is. */
export function dropInList(listId: string, shown: Channel[], draggedId: string, targetId: string) {
  const ids = shown.map((c) => c.id).filter((id) => id !== draggedId);
  const at = ids.indexOf(targetId);
  if (at < 0 || draggedId === targetId) return;
  // Dragging down lands after the target, dragging up before it.
  const movingDown = shown.findIndex((c) => c.id === draggedId) < shown.findIndex((c) => c.id === targetId);
  ids.splice(movingDown ? at + 1 : at, 0, draggedId);
  return reorderList(listId, ids);
}
