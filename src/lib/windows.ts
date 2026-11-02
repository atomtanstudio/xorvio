// Which of Xorvio's windows this web view is. The pop-out player runs the
// same interface in a window of its own (src-tauri/src/popout.rs).

interface TauriInternals {
  metadata?: { currentWindow?: { label?: string } };
}

const internals = typeof window !== "undefined" ? (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__ : undefined;

/** This web view is the pop-out player's window. */
export const isPopOutWindow = internals?.metadata?.currentWindow?.label === "popout";

/** This window's label, for events sent to it alone ("main" or "popout"). */
export const windowLabel = internals?.metadata?.currentWindow?.label ?? "main";
