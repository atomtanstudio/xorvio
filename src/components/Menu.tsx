import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { create } from "zustand";

export type MenuEntry =
  | { label: string; icon?: ReactNode; onSelect: () => void; disabled?: boolean; destructive?: boolean; shortcut?: string }
  | { separator: true }
  | { heading: string };

interface MenuState {
  open: { x: number; y: number; items: MenuEntry[] } | null;
  show(x: number, y: number, items: MenuEntry[]): void;
  close(): void;
}

const useMenu = create<MenuState>((set) => ({
  open: null,
  show: (x, y, items) => set({ open: { x, y, items } }),
  close: () => set({ open: null }),
}));

/** Opens a context menu at the pointer (right-click) or under an element (a "…" button). */
export function openMenu(event: { clientX: number; clientY: number; preventDefault(): void; stopPropagation(): void; currentTarget?: EventTarget | null }, items: MenuEntry[], below = false) {
  event.preventDefault();
  event.stopPropagation();
  let { clientX: x, clientY: y } = event;
  if (below && event.currentTarget instanceof HTMLElement) {
    const rect = event.currentTarget.getBoundingClientRect();
    x = rect.left;
    y = rect.bottom + 4;
  }
  useMenu.getState().show(x, y, items);
}

export function MenuHost() {
  const open = useMenu((s) => s.open);
  const close = useMenu((s) => s.close);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });

  useLayoutEffect(() => {
    if (!open || !ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    setPos({
      x: Math.min(open.x, window.innerWidth - rect.width - 8),
      y: open.y + rect.height > window.innerHeight - 8 ? Math.max(8, open.y - rect.height) : open.y,
    });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const openedAt = performance.now();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && close();
    // The window can lose focus for an instant around the click that opened
    // the menu (activating the app); only a later switch away closes it.
    const onBlur = () => performance.now() - openedAt > 500 && close();
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", onBlur);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("resize", close);
    };
  }, [open, close]);

  if (!open) return null;
  return (
    <div ref={ref} className="menu" role="menu" style={{ left: pos.x, top: pos.y }} onContextMenu={(e) => e.preventDefault()}>
      {open.items.map((item, i) =>
        "separator" in item ? (
          <div key={i} className="menu-separator" />
        ) : "heading" in item ? (
          <div key={i} className="menu-heading">
            {item.heading}
          </div>
        ) : (
          <button
            key={i}
            role="menuitem"
            className={`menu-item ${item.destructive ? "destructive" : ""}`}
            disabled={item.disabled}
            onClick={() => {
              close();
              item.onSelect();
            }}
          >
            {item.icon}
            <span>{item.label}</span>
            {item.shortcut && <span className="shortcut">{item.shortcut}</span>}
          </button>
        ),
      )}
    </div>
  );
}
