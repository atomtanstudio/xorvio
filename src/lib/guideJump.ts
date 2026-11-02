// "Show in Guide" from anywhere: Live TV opens on the programme's day, with
// its channel listed and the programme in view.

import { create } from "zustand";
import { useApp } from "./store";
import type { ProgramHit } from "./types";

export const useGuideJump = create<{ hit: ProgramHit | null; set(hit: ProgramHit | null): void }>((set) => ({
  hit: null,
  set: (hit) => set({ hit }),
}));

/** Leaves search for Live TV's guide at this programme. */
export function showInGuide(hit: ProgramHit) {
  useGuideJump.getState().set(hit);
  useApp.getState().setSearchText("");
  useApp.getState().go("live");
}
