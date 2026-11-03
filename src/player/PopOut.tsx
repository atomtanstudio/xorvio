// The pop-out player's window: the player alone, playing what the main
// window handed over (src-tauri/src/popout.rs). Docking hands it back;
// closing the window stops it.

import { useEffect } from "react";
import { Toast, useTheme } from "../App";
import { ConfirmHost } from "../components/Confirm";
import { MenuHost } from "../components/Menu";
import { lastChannel } from "../lib/actions";
import { api } from "../lib/api";
import { followMenu, followMenuState, usePlaybackReport } from "../lib/commands";
import { useApp, type PopOutHandoff } from "../lib/store";
import { CopiesSheetHost } from "../screens/Copies";
import { PlayerHost, SAVE_PROGRESS_EVENT } from "./Player";

function play({ item, position }: PopOutHandoff) {
  useApp.getState().play({ ...item, resumeFrom: position > 0 ? position : undefined }, "full");
}

async function closeWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().destroy();
}

export function PopOutApp() {
  const settings = useApp((s) => s.settings);
  useTheme(settings.appearance, settings.density);

  useEffect(() => {
    const { loadSettings, loadLibrary } = useApp.getState();
    loadSettings().catch(() => {});
    loadLibrary().catch(() => {});
    api.takePopOut().then((handoff) => handoff && play(handoff));
    const offPlay = api.onPopOutPlay(play);
    // While it's open, the menu bar's Playback items act here, where the video is.
    const offMenu = followMenu();
    const offMenuState = followMenuState(
      () => ({ playback: { ...usePlaybackReport.getState(), lastChannel: !!lastChannel(), multiview: false, allPaused: false, poppedOut: true } }),
      (changed) => {
        const offs = [usePlaybackReport.subscribe(changed), useApp.subscribe(changed)];
        return () => offs.forEach((off) => off());
      },
    );
    // When playback ends here (the player's close button), the window goes too.
    const offStopped = useApp.subscribe((now, before) => {
      if (before.nowPlaying && !now.nowPlaying) closeWindow().catch(() => {});
    });
    // Closing the window saves where it got to and lets the stream go, so the
    // provider's connection is free again.
    let offClose: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
      getCurrentWindow()
        .onCloseRequested(() => {
          window.dispatchEvent(new Event(SAVE_PROGRESS_EVENT));
          const { stream } = useApp.getState();
          if (stream) api.releaseStream(stream.url).catch(() => {});
        })
        .then((off) => (cancelled ? off() : (offClose = off))),
    );
    return () => {
      cancelled = true;
      offPlay();
      offMenu();
      offMenuState();
      offStopped();
      offClose?.();
    };
  }, []);

  return (
    <div className="app popout-app">
      <PlayerHost />
      {/* Switching to another copy of an add-on title happens here too. */}
      <CopiesSheetHost />
      <MenuHost />
      <ConfirmHost />
      <Toast />
    </div>
  );
}
