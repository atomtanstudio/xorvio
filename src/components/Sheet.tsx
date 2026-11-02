import { useEffect, type ReactNode } from "react";

/** A modal sheet over the window. Esc or a click outside closes it. */
export function Sheet({ title, subtitle, children, footer, onClose, width }: { title?: ReactNode; subtitle?: ReactNode; children: ReactNode; footer?: ReactNode; onClose: () => void; width?: number }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet" role="dialog" aria-modal="true" style={width ? { width: `min(${width}px, calc(100vw - 48px))` } : undefined}>
        {(title || subtitle) && (
          <div className="sheet-header">
            {title && <h2>{title}</h2>}
            {subtitle && <p>{subtitle}</p>}
          </div>
        )}
        <div className="sheet-body">{children}</div>
        {footer && <div className="sheet-footer">{footer}</div>}
      </div>
    </div>
  );
}
