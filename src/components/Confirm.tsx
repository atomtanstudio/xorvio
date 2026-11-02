// An in-app confirmation sheet. The web view's own confirm() doesn't show in
// Tauri's window on macOS (it quietly answers "no"), so every "Are you sure?"
// goes through here.

import { createPortal } from "react-dom";
import { create } from "zustand";
import { Sheet } from "./Sheet";

interface Request {
  title: string;
  message: string;
  confirmLabel: string;
  destructive: boolean;
  resolve(ok: boolean): void;
}

const useConfirm = create<{ request: Request | null; set(request: Request | null): void }>((set) => ({ request: null, set: (request) => set({ request }) }));

export function confirmAction(options: { title: string; message: string; confirmLabel?: string; destructive?: boolean }): Promise<boolean> {
  return new Promise((resolve) =>
    useConfirm.getState().set({ confirmLabel: "OK", destructive: false, ...options, resolve }),
  );
}

export function ConfirmHost() {
  const request = useConfirm((s) => s.request);
  if (!request) return null;
  const answer = (ok: boolean) => {
    useConfirm.getState().set(null);
    request.resolve(ok);
  };
  // Above any sheet that asked, including ones drawn over the whole window.
  return createPortal(
    <div className="confirm-layer">
      <Sheet
        title={request.title}
        onClose={() => answer(false)}
        width={440}
        footer={
          <>
            <button className="btn-plain" onClick={() => answer(false)}>
              Cancel
            </button>
            <button className={request.destructive ? "btn-accent danger" : "btn-accent"} onClick={() => answer(true)} autoFocus>
              {request.confirmLabel}
            </button>
          </>
        }
      >
        <p className="muted" style={{ margin: 0, lineHeight: 1.5 }}>
          {request.message}
        </p>
      </Sheet>
    </div>,
    document.body,
  );
}
