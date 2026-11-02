import { ChevronRight, Ellipsis } from "lucide-react";
import type { ReactNode } from "react";
import { ScrollRow } from "./ScrollRow";

interface ShelfProps {
  title: string;
  subtitle?: string;
  onShowAll?: () => void;
  onMore?: (e: React.MouseEvent) => void;
  children: ReactNode;
}

/** A titled, horizontally scrolling row: arrows on hover, and a mouse can drag it. */
export function Shelf({ title, subtitle, onShowAll, onMore, children }: ShelfProps) {
  return (
    <section className="shelf">
      <div className="shelf-header">
        <h2 className="shelf-title">{title}</h2>
        {onShowAll && (
          <button className="link-button" onClick={onShowAll}>
            Show All <ChevronRight size={14} strokeWidth={2.5} />
          </button>
        )}
        {onMore && (
          <button className="icon-button more" onClick={onMore} aria-label={`${title} options`}>
            <Ellipsis size={18} />
          </button>
        )}
      </div>
      {subtitle && <p className="shelf-subtitle">{subtitle}</p>}
      <ScrollRow className="shelf-track">{children}</ScrollRow>
    </section>
  );
}
