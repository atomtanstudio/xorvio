// Remote-style keys on Home, as Xorvio for Mac's HomeKeyboard: the arrows
// move a highlight the way an Apple TV remote moves focus.
//
// - ←/→ move along a row (on the hero, between featured titles), ↑/↓ between
//   rows, keeping the column; ↑ from the first row goes to the hero.
// - ← from a row's first card goes back to the sidebar, and → in the sidebar
//   comes back into Home, like the menu on Apple TV.
// - Return plays or opens what's highlighted (it's a button with focus, so
//   the ring shows and Return clicks it); Esc lets go.
//
// Typing in a text field, sheets, menus, Multiview and the full-screen player
// keep their keys.

import { useEffect, type RefObject } from "react";
import { useApp } from "../lib/store";
import { useMultiview } from "../player/multiviewStore";

/** What the hero does with ← and →. */
export interface HeroControls {
  step(by: 1 | -1): void;
}

/** Shown on whatever the keys moved to, so the ring shows however the browser judges focus. */
const MARK = "key-focus";

const prefersStill = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

function isTyping(target: Element | null) {
  return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || (target instanceof HTMLElement && target.isContentEditable);
}

/** Something over Home has the keys: a sheet, a menu, the full player, Multiview. */
function covered() {
  const { tab, stack, presentation, programSheet, guideMatch, copies } = useApp.getState();
  return (
    tab !== "home" ||
    stack.length > 0 ||
    presentation === "full" ||
    !!programSheet ||
    !!guideMatch ||
    !!copies ||
    useMultiview.getState().open ||
    !!document.querySelector(".sheet-backdrop, [role='dialog'], [role='menu']")
  );
}

/** Highlights `element` and brings it into view: "top" scrolls Home back to the hero. */
function focus(element: HTMLElement, block: ScrollLogicalPosition | "top") {
  document.querySelector(`.${MARK}`)?.classList.remove(MARK);
  element.classList.add(MARK);
  element.addEventListener("blur", () => element.classList.remove(MARK), { once: true });
  element.focus({ preventScroll: true });
  const behavior = prefersStill() ? "auto" : "smooth";
  if (block === "top") element.closest(".content")?.scrollTo({ top: 0, behavior });
  else element.scrollIntoView({ block, inline: "nearest", behavior });
}

export function useHomeKeyboard(root: RefObject<HTMLElement | null>, hero: RefObject<HeroControls | null>) {
  useEffect(() => {
    const heroButton = () => root.current?.querySelector<HTMLElement>(".hero .hero-actions .btn-primary") ?? null;
    /** Rows with cards, top to bottom, each as its cards. */
    const rows = () =>
      [...(root.current?.querySelectorAll<HTMLElement>(".home-rows .shelf") ?? [])]
        .map((shelf) => [...shelf.querySelectorAll<HTMLElement>(".shelf-track .card")])
        .filter((cards) => cards.length > 0);
    const firstStop = () => heroButton() ?? rows()[0]?.[0] ?? null;

    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || !root.current || covered()) return;
      const active = document.activeElement;
      if (isTyping(active)) return;
      const arrow = e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "ArrowUp" || e.key === "ArrowDown";
      const inHome = !!active && root.current.contains(active);

      if (e.key === "Escape" && inHome && active instanceof HTMLElement) {
        active.blur();
        return;
      }
      if (!arrow) return;

      // The sidebar: → comes back into Home.
      if (active?.closest(".sidebar")) {
        const start = e.key === "ArrowRight" ? firstStop() : null;
        if (start) {
          e.preventDefault();
          focus(start, start === heroButton() ? "top" : "center");
        }
        return;
      }
      // Nothing highlighted yet: the first arrow starts at the top.
      if (!active || !inHome) {
        if (active && active !== document.body) return;
        const start = firstStop();
        if (start) {
          e.preventDefault();
          focus(start, start === heroButton() ? "top" : "center");
        }
        return;
      }

      e.preventDefault();
      const all = rows();
      const onHero = !!active.closest(".hero");
      if (onHero) {
        if (e.key === "ArrowLeft") hero.current?.step(-1);
        else if (e.key === "ArrowRight") hero.current?.step(1);
        else if (e.key === "ArrowDown" && all[0]) focus(all[0][0], "center");
        // A new featured title brings new buttons: keep the highlight on Play.
        if (e.key === "ArrowLeft" || e.key === "ArrowRight") requestAnimationFrame(() => heroButton() && focus(heroButton()!, "nearest"));
        return;
      }
      let row = all.findIndex((cards) => cards.some((card) => card === active || card.contains(active)));
      let column = row >= 0 ? all[row].findIndex((card) => card === active || card.contains(active)) : 0;
      if (row < 0) {
        // Focus elsewhere in a row (its "…" or Show All): start at its first card.
        const shelf = active.closest(".shelf");
        row = shelf ? all.findIndex((cards) => shelf.contains(cards[0])) : -1;
        column = 0;
        // Anywhere else (a banner's button, Customize Home): back to the top.
        if (row < 0) {
          const start = firstStop();
          if (start) focus(start, start === heroButton() ? "top" : "center");
          return;
        }
        if (e.key === "ArrowRight" || e.key === "ArrowDown") return focus(all[row][0], "center");
      }
      const cards = all[row];
      switch (e.key) {
        case "ArrowLeft":
          if (column > 0) focus(cards[column - 1], "nearest");
          else document.querySelector<HTMLElement>(".sidebar .sidebar-item.selected")?.focus();
          break;
        case "ArrowRight":
          if (column < cards.length - 1) focus(cards[column + 1], "nearest");
          break;
        case "ArrowUp":
          if (row > 0) focus(all[row - 1][Math.min(column, all[row - 1].length - 1)], "center");
          else {
            const button = heroButton();
            if (button) focus(button, "top");
          }
          break;
        case "ArrowDown":
          if (row < all.length - 1) focus(all[row + 1][Math.min(column, all[row + 1].length - 1)], "center");
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [root, hero]);
}
