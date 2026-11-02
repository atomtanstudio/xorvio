import { useState, type CSSProperties, type ReactNode } from "react";
import { initials } from "../lib/format";
import { imageSrc } from "../lib/images";

/** Deterministic dark gradient for missing artwork, so placeholders don't all look alike. */
export function placeholderGradient(seed: string) {
  let h = 0;
  for (const c of seed) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hue = h % 360;
  return `linear-gradient(135deg, hsl(${hue} 32% 26%), hsl(${(hue + 40) % 360} 38% 14%))`;
}

/**
 * The demo library's artwork is drawn here rather than downloaded:
 * `xorvio-art://poster/slug?v=2&t=Title` names a kind, a variation and the
 * title to letter on it.
 */
export function generatedArt(src: string | null | undefined) {
  if (!src?.startsWith("xorvio-art://")) return null;
  try {
    const url = new URL(src.replace("xorvio-art://", "https://art/"));
    const [kind = "poster", slug = ""] = url.pathname.split("/").filter(Boolean);
    return { kind, slug, variation: Number(url.searchParams.get("v") ?? 0), title: url.searchParams.get("t") ?? "" };
  } catch {
    return null;
  }
}

function generatedBackground(seed: string, variation: number, kind: string) {
  let h = variation * 47;
  for (const c of seed) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hue = h % 360;
  const angle = kind === "poster" ? 160 : 115;
  return `radial-gradient(120% 90% at 78% 18%, hsl(${(hue + 30) % 360} 55% 46% / 0.75), transparent 60%), linear-gradient(${angle}deg, hsl(${hue} 45% 24%), hsl(${(hue + 200) % 360} 40% 9%))`;
}

interface ArtProps {
  src?: string | null;
  alt?: string;
  /** Text drawn on the placeholder when there's no image (usually the title). */
  fallback?: string;
  className?: string;
  style?: CSSProperties;
  children?: ReactNode;
  eager?: boolean;
}

/** An image that fades in once decoded, over a placeholder that names the title. */
export function Art({ src, alt = "", fallback, className = "", style, children, eager }: ArtProps) {
  const [loaded, setLoaded] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const generated = generatedArt(src);
  if (generated) {
    // Backdrops and stills carry no lettering; the card's own caption names them.
    const lettered = generated.kind === "poster" || generated.kind === "logo";
    return (
      <div className={`art generated ${className}`} style={{ background: generatedBackground(generated.slug, generated.variation, generated.kind), ...style }}>
        {lettered && <div className="placeholder generated-title">{generated.title}</div>}
        {children}
      </div>
    );
  }
  const showImage = src && failed !== src;
  return (
    <div className={`art ${className}`} style={{ background: showImage && loaded === src ? undefined : placeholderGradient(fallback ?? alt ?? ""), ...style }}>
      {(!showImage || loaded !== src) && fallback && <div className="placeholder">{fallback}</div>}
      {showImage && (
        <img
          key={src}
          src={imageSrc(src)}
          alt={alt}
          loading={eager ? "eager" : "lazy"}
          decoding="async"
          className={loaded === src ? "loaded" : ""}
          onLoad={() => setLoaded(src)}
          onError={() => setFailed(src)}
        />
      )}
      {children}
    </div>
  );
}

/** A provider's channel logo on a plate, or the tidied name when there isn't one. */
export function ChannelLogo({ src, name, size = 38 }: { src?: string | null; name: string; size?: number }) {
  const [failed, setFailed] = useState(src?.startsWith("xorvio-art://") ?? false);
  return (
    <div className="channel-logo" style={{ width: size, height: size }}>
      {src && !failed ? (
        <img src={imageSrc(src)} alt="" loading="lazy" onError={() => setFailed(true)} />
      ) : (
        <span className="initials">{initials(name, 3) || name.slice(0, 3).toUpperCase()}</span>
      )}
    </div>
  );
}

// The logo, drawn from the same geometry as BrandMark.swift and the SVGs in
// Design/Brand: the mark is two bands crossing as an X, the top-left one
// passing over the other; the wordmark is "xorvio" in a round-ended monoline.

const markPalette = {
  dark: { top: ["#DCEAFF", "#5C93FF"], low: ["#3F4FD8", "#86ABFF"] },
  light: { top: ["#7FAEFF", "#1F5BD8"], low: ["#2433A8", "#5A86F2"] },
};

/** The mark's shapes in its 120-unit square (the shape spans 16…104). */
function MarkShapes({ id, theme }: { id: string; theme: "dark" | "light" }) {
  const palette = markPalette[theme];
  return (
    <>
      <defs>
        <linearGradient id={`${id}-top`} gradientUnits="userSpaceOnUse" x1="16" y1="16" x2="104" y2="104">
          <stop offset="0" stopColor={palette.top[0]} />
          <stop offset="1" stopColor={palette.top[1]} />
        </linearGradient>
        <linearGradient id={`${id}-low`} gradientUnits="userSpaceOnUse" x1="16" y1="104" x2="104" y2="16">
          <stop offset="0" stopColor={palette.low[0]} />
          <stop offset="1" stopColor={palette.low[1]} />
        </linearGradient>
      </defs>
      <path d="M36.49,100.49 L56.46,80.51 L39.49,63.54 L19.51,83.51 A12,12 0 0 0 36.49,100.49 Z" fill={`url(#${id}-low)`} />
      <path d="M80.51,56.46 L100.49,36.49 A12,12 0 0 0 83.51,19.51 L63.54,39.49 L80.51,56.46 Z" fill={`url(#${id}-low)`} />
      <path d="M19.51,36.49 L83.51,100.49 A12,12 0 0 0 100.49,83.51 L36.49,19.51 A12,12 0 0 0 19.51,36.49 Z" fill={`url(#${id}-top)`} />
    </>
  );
}

function useTheme(): "dark" | "light" {
  return typeof document !== "undefined" && document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

/** The mark on its own. */
export function BrandMark({ size = 22 }: { size?: number }) {
  const theme = useTheme();
  return (
    <svg width={size} height={size} viewBox="16 16 88 88" aria-hidden="true">
      <MarkShapes id="xv-mark" theme={theme} />
    </svg>
  );
}

/**
 * Mark and wordmark side by side, sized by height, in the proportions Xorvio
 * for Mac shows them in its sidebar: the mark fills the height, and after a
 * gap of half a mark comes the wordmark, 3.7 marks wide, its x-height band
 * centred on the mark.
 */
export function BrandLockup({ height = 20 }: { height?: number }) {
  const theme = useTheme();
  // In units of the lockup's height (1 = the mark's side); the wordmark is
  // drawn in its own units (268 wide, x-height band 0…44).
  const wordX = 1.525;
  const scale = 3.7 / 268;
  const top = 0.5 - 22 * scale;
  const width = wordX + 268 * scale;
  const ink = theme === "light" ? "#111418" : "#F2F5FA";
  return (
    <svg height={height} width={height * width} viewBox={`0 0 ${width} 1`} role="img" aria-label="Xorvio" style={{ display: "block", overflow: "visible" }}>
      <g transform="scale(0.011364) translate(-16,-16)">
        <MarkShapes id="xv-lockup" theme={theme} />
      </g>
      <g transform={`translate(${wordX},${top}) scale(${scale})`} fill="none" stroke={ink} strokeWidth="10" strokeLinecap="round" strokeLinejoin="round">
        <path d="M5,5 L41,39 M5,39 L41,5 M116,39 V22 A17,17 0 0 1 133,5 H138 M153,5 L171,39 L189,5 M208,5 V39" />
        <circle cx="79" cy="22" r="18" />
        <circle cx="245" cy="22" r="18" />
        <circle cx="208" cy="-12" r="6.2" fill={ink} stroke="none" />
      </g>
    </svg>
  );
}
