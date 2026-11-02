// Pictures for titles a list shows without one: posters from TMDB (with a
// key), and for a media folder's videos, a frame of the video. Only titles
// on screen are asked about, once each, a few at a time and only after
// scrolling stops, so a fast scroll through a large grid never sends a
// burst of requests or makes a pile of frames. The backend keeps what it
// finds, so the next list shows those pictures straight away.

import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { api } from "./api";

/** Titles asked about this session, found or not. */
const asked = new Set<string>();
/** What was found, shared by every list. */
const useFound = create<{ posters: Map<string, string>; add(found: { id: string; posterUrl: string }[]): void }>((set) => ({
  posters: new Map(),
  add: (found) => set((s) => ({ posters: new Map([...s.posters, ...found.map((f) => [f.id, f.posterUrl] as const)]) })),
}));

/** How long scrolling must have stopped before titles on screen are asked about. */
const SETTLE_MS = 700;
const AT_ONCE = 12;

interface Titled {
  id: string;
  sourceId: string;
  posterUrl: string | null;
}

/**
 * Asks for the posters of `shown` (what's on screen) that lack one, once it
 * stops changing, and returns every poster found so far by id.
 */
export function useFilledPosters(shown: Titled[]): Map<string, string> {
  const posters = useFound((s) => s.posters);
  const wanted = useMemo(
    () => shown.filter((t) => !t.posterUrl && !asked.has(t.id) && t.sourceId !== "demo" && t.sourceId !== "addon").map((t) => t.id),
    [shown],
  );
  const key = wanted.join("\n");
  const [round, setRound] = useState(0);
  useEffect(() => {
    if (!wanted.length) return;
    let cancelled = false;
    const timer = setTimeout(async () => {
      if (cancelled) return;
      const remaining = wanted.filter((id) => !asked.has(id));
      if (!remaining.length) return;
      const batch = remaining.slice(0, AT_ONCE);
      batch.forEach((id) => asked.add(id));
      const found = await api.fillPosters(batch).catch(() => []);
      if (found.length) useFound.getState().add(found);
      // More on screen than one batch: the next one, once this is done.
      if (!cancelled && remaining.length > batch.length) setRound((r) => r + 1);
    }, SETTLE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, round]);
  return posters;
}
