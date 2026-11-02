// The window in full screen for the video, shared by the player and
// Multiview so either can undo what the other did.

async function setWindowFullscreen(on: boolean) {
  if ("__TAURI_INTERNALS__" in window) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setFullscreen(on);
  } else if (on) await document.documentElement.requestFullscreen?.().catch(() => {});
  else if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
}

export async function isWindowFullscreen() {
  if ("__TAURI_INTERNALS__" in window) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow().isFullscreen();
  }
  return !!document.fullscreenElement;
}

// Whether the player put the window in full screen. Leaving the player undoes
// only that, so a window someone made full screen themselves stays that way
// (and Xorvio opens full screen next time, as it was left).
let playerMadeFullscreen = false;

/** Whether the window is in full screen because the player (or Multiview) put it there. */
export function madeFullscreen() {
  return playerMadeFullscreen;
}

export async function togglePlayerFullscreen() {
  const next = !(await isWindowFullscreen());
  await setWindowFullscreen(next);
  playerMadeFullscreen = next;
  return next;
}

export function leavePlayerFullscreen() {
  if (!playerMadeFullscreen) return;
  playerMadeFullscreen = false;
  setWindowFullscreen(false);
}
