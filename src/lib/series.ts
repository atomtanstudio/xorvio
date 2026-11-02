// Series recordings and saving from catch-up, as the guide's sheets and
// menus and the Recordings screen offer them. The recorder schedules a
// series' airings itself; this keeps the list of series fresh and opens the
// sheets (SeriesSheet.tsx) for making, changing and deleting one.

import { create } from "zustand";
import { api, errorText } from "./api";
import { nowSeconds } from "./format";
import { useApp } from "./store";
import type { Channel, Program, ScheduleRequest, SeriesRule, SeriesRuleOptions } from "./types";

export type SeriesSheetRequest = { type: "new"; program: Program; channel: Channel } | { type: "edit"; rule: SeriesRule } | { type: "delete"; rule: SeriesRule };

interface SeriesState {
  rules: SeriesRule[];
  loaded: boolean;
  /** Reads the series again, with their next airings and counts. */
  refresh(): Promise<void>;
  sheet: SeriesSheetRequest | null;
  show(sheet: SeriesSheetRequest | null): void;
}

export const useSeries = create<SeriesState>((set) => ({
  rules: [],
  loaded: false,
  refresh: async () => {
    const rules = await api.seriesRules();
    set({ rules, loaded: true });
  },
  sheet: null,
  show: (sheet) => set({ sheet }),
}));

const app = () => useApp.getState();

/** Titles as the guide matches them: case and punctuation set aside, "@", "v" and "vs" read as "at", a leading "Live:" ignored (xorvio-core's watchlist::normalized). */
export function titleKey(title: string) {
  const words = title
    .toLowerCase()
    .replace(/@/g, " at ")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .map((word) => (word === "v" || word === "vs" || word === "versus" ? "at" : word));
  if (words.length > 1 && words[0] === "live") words.shift();
  return words.join(" ");
}

/** The series a guide programme is part of, when one records it. */
export function seriesFor(rules: SeriesRule[], program: Program, channel: Channel) {
  const key = titleKey(program.title);
  return rules.find((rule) => titleKey(rule.title) === key && (rule.anyChannel || rule.channelId === channel.id || channel.versions?.some((v) => v.id === rule.channelId)));
}

/** "Record Series" on a programme: the sheet for a new series, or for the one that already records it. */
export function recordSeries(program: Program, channel: Channel) {
  const existing = seriesFor(useSeries.getState().rules, program, channel);
  useSeries.getState().show(existing ? { type: "edit", rule: existing } : { type: "new", program, channel });
}

export function editSeries(rule: SeriesRule) {
  useSeries.getState().show({ type: "edit", rule });
}

/** Asks whether to keep the series' recordings, then deletes it. */
export function deleteSeries(rule: SeriesRule) {
  useSeries.getState().show({ type: "delete", rule });
}

/** What a new series starts with: new episodes only, from this channel, keeping everything, with the usual padding. */
export const defaultSeriesOptions: SeriesRuleOptions = { anyChannel: false, newOnly: true, keep: 0, paddingBeforeSeconds: null, paddingAfterSeconds: null };

export async function createSeries(program: Program, channel: Channel, options: SeriesRuleOptions) {
  const rule = await api.createSeriesRule(program.id, channel.id, options);
  await Promise.all([useSeries.getState().refresh(), app().refreshRecordings()]).catch(() => {});
  app().showToast(rule.scheduledCount ? `Recording “${rule.title}”: ${rule.scheduledCount} airing${rule.scheduledCount === 1 ? "" : "s"} scheduled` : `“${rule.title}” will record when the guide lists it again`);
  return rule;
}

export async function updateSeries(rule: SeriesRule, options: SeriesRuleOptions) {
  const saved = await api.updateSeriesRule(rule.id, options);
  await Promise.all([useSeries.getState().refresh(), app().refreshRecordings()]).catch(() => {});
  return saved;
}

export async function removeSeries(rule: SeriesRule, deleteRecordings: boolean) {
  await api.deleteSeriesRule(rule.id, deleteRecordings);
  await Promise.all([useSeries.getState().refresh(), app().refreshRecordings()]).catch(() => {});
  app().showToast(deleteRecordings ? `Deleted the series “${rule.title}” and moved its recordings to the Trash` : `Deleted the series “${rule.title}”. Its recordings are still in Recordings.`);
}

/** A programme that aired on a channel whose catch-up still has it: where the guide offers Play Replay. */
export function canSaveFromCatchup(program: Program, channel: Channel) {
  const t = nowSeconds();
  return program.end <= t && channel.catchupDays > 0 && t - program.start < channel.catchupDays * 86400;
}

/** Saves a programme that aired from its channel's catch-up, with the usual padding where the archive allows. */
export async function saveFromCatchup(program: Program, channel: Channel) {
  const { settings } = app();
  const request: ScheduleRequest = {
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
  try {
    const recording = await api.recordFromCatchup(request);
    app().upsertRecording(recording);
    app().showToast(`Saving “${program.title}” from catch-up. It's in Recordings.`);
  } catch (error) {
    app().showToast(errorText(error), "error");
  }
}

/** A recording being saved from catch-up for this programme, or one already saved. */
export function catchupSaveFor(program: Program, channel: Channel) {
  for (const r of app().recordings.values()) {
    if (r.fromCatchup && r.channelId === channel.id && r.start === program.start && r.status !== "failed" && r.status !== "cancelled") return r;
  }
  return undefined;
}

/** "Any channel" or the channel's name; "New episodes" or "Every airing"; what it keeps. */
export function seriesSummary(rule: SeriesRuleOptions & { channelName: string }) {
  const parts = [rule.anyChannel ? "Any channel" : rule.channelName, rule.newOnly ? "New episodes" : "Every airing"];
  parts.push(rule.keep > 0 ? `Keeps the latest ${rule.keep}` : "Keeps all");
  return parts.join(" · ");
}
