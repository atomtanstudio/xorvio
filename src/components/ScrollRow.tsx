// A horizontal row that scrolls three ways: a trackpad or touch screen
// natively, the arrows that appear at either side while there's more that
// way, and a mouse dragging it along with a little momentum, as Xorvio for
// Mac's rows do.

import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

/** Which ends of a row are reached, kept up to date as it scrolls or resizes. */
function useEdges(ref: RefObject<HTMLDivElement | null>, deps: unknown[]) {
  const [edges, setEdges] = useState({ start: true, end: true });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => setEdges({ start: el.scrollLeft < 4, end: el.scrollLeft + el.clientWidth >= el.scrollWidth - 4 });
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    for (const child of Array.from(el.children)) observer.observe(child);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return edges;
}

/**
 * Lets a mouse drag the row. A drag starts only after the pointer moves six
 * points, so clicks still open cards; the click that ends a drag is swallowed
 * so letting go doesn't open one. Touch and trackpads keep native scrolling.
 */
export function useDragScroll(ref: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let startX = 0;
    let startScroll = 0;
    let lastX = 0;
    let lastT = 0;
    let velocity = 0;
    let dragging = false;
    let frame = 0;

    const onMove = (e: PointerEvent) => {
      const dx = e.clientX - startX;
      if (!dragging && Math.abs(dx) < 6) return;
      if (!dragging) {
        dragging = true;
        el.classList.add("dragging");
      }
      el.scrollLeft = startScroll - dx;
      const t = performance.now();
      velocity = (e.clientX - lastX) / Math.max(1, t - lastT);
      lastX = e.clientX;
      lastT = t;
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      if (!dragging) return;
      dragging = false;
      el.classList.remove("dragging");
      const swallow = (event: MouseEvent) => {
        event.stopPropagation();
        event.preventDefault();
      };
      el.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => el.removeEventListener("click", swallow, { capture: true }), 0);
      // Carry on a little in the direction it was thrown.
      let v = velocity * 16;
      const glide = () => {
        if (Math.abs(v) < 0.5) return;
        el.scrollLeft -= v;
        v *= 0.92;
        frame = requestAnimationFrame(glide);
      };
      frame = requestAnimationFrame(glide);
    };
    const onDown = (e: PointerEvent) => {
      if (e.pointerType !== "mouse" || e.button !== 0 || el.scrollWidth <= el.clientWidth) return;
      cancelAnimationFrame(frame);
      startX = lastX = e.clientX;
      startScroll = el.scrollLeft;
      lastT = performance.now();
      velocity = 0;
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    };
    el.addEventListener("pointerdown", onDown);
    return () => {
      el.removeEventListener("pointerdown", onDown);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      cancelAnimationFrame(frame);
    };
  }, [ref]);
}

interface ScrollRowProps {
  /** The track's class (its padding and gap): "shelf-track", "episode-strip"… */
  className: string;
  /** Extra class for the wrapper, to place the arrows. */
  wrapClassName?: string;
  children: ReactNode;
}

export function ScrollRow({ className, wrapClassName = "", children }: ScrollRowProps) {
  const track = useRef<HTMLDivElement>(null);
  const edges = useEdges(track, [children]);
  useDragScroll(track);
  const page = (direction: 1 | -1) => {
    const el = track.current;
    if (!el) return;
    el.scrollBy({ left: direction * (el.clientWidth - 120), behavior: "smooth" });
    // Re-check the ends once the glide settles, in case no scroll event arrives.
    setTimeout(() => el.dispatchEvent(new Event("scroll")), 450);
  };
  return (
    <div className={`shelf-wrap ${wrapClassName}`}>
      <button className="shelf-arrow left glass" hidden={edges.start} onClick={() => page(-1)} aria-label="Previous">
        <ChevronLeft size={18} />
      </button>
      <div className={className} ref={track}>
        {children}
      </div>
      <button className="shelf-arrow right glass" hidden={edges.end} onClick={() => page(1)} aria-label="Next">
        <ChevronRight size={18} />
      </button>
    </div>
  );
}
