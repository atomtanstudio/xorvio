// Formatting in the viewer's locale and time zone. Times are Unix seconds.

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });
const dateFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

export const nowSeconds = () => Math.floor(Date.now() / 1000);

export function clockTime(t: number) {
  return timeFormat.format(new Date(t * 1000));
}

export function timeRange(start: number, end: number) {
  return `${clockTime(start)} – ${clockTime(end)}`;
}

function startOfDay(date: Date) {
  const d = new Date(date);
  d.setHours(0, 0, 0, 0);
  return d;
}

/** "Today", "Tomorrow", "Yesterday" or "Sat, Oct 24". */
export function dayName(t: number) {
  const days = Math.round((startOfDay(new Date(t * 1000)).getTime() - startOfDay(new Date()).getTime()) / 86400000);
  if (days === 0) return "Today";
  if (days === 1) return "Tomorrow";
  if (days === -1) return "Yesterday";
  return dayFormat.format(new Date(t * 1000));
}

/** "Tonight 7:35 PM", "Tomorrow 8:00 PM", "Sat 5:35 PM". */
export function whenLabel(t: number) {
  const date = new Date(t * 1000);
  const days = Math.round((startOfDay(date).getTime() - startOfDay(new Date()).getTime()) / 86400000);
  if (days === 0) return `${date.getHours() >= 17 ? "Tonight" : "Today"} ${clockTime(t)}`;
  if (days === 1) return `Tomorrow ${clockTime(t)}`;
  if (days > 1 && days < 7) return `${weekdayFormat.format(date)} ${clockTime(t)}`;
  return `${dayFormat.format(date)} ${clockTime(t)}`;
}

/** "just now", "5 min ago", "3 hr ago", then the date. */
export function ago(t: number) {
  const minutes = Math.floor((Date.now() / 1000 - t) / 60);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} hr ago`;
  return shortDate(t);
}

export function shortDate(t: number) {
  return dateFormat.format(new Date(t * 1000));
}

/** "2 hr 1 min", "45 min". */
export function duration(seconds: number) {
  const minutes = Math.round(seconds / 60);
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h && m) return `${h} hr ${m} min`;
  if (h) return `${h} hr`;
  return `${m} min`;
}

/** "30 min left", "1 hr 15 min left". */
export function remaining(seconds: number) {
  return `${duration(Math.max(60, seconds))} left`;
}

/** Player clock: "47:55", "1:02:02". */
export function playerClock(seconds: number) {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

export function episodeCode(season: number, episode: number) {
  return `S${String(season).padStart(2, "0")} E${String(episode).padStart(2, "0")}`;
}

export function fileSize(bytes: number) {
  if (bytes < 1e6) return `${Math.round(bytes / 1e3)} KB`;
  if (bytes < 1e9) return `${(bytes / 1e6).toFixed(0)} MB`;
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

export function resolutionBadge(height?: number | null) {
  if (!height) return null;
  if (height >= 2000) return "4K";
  if (height >= 1000) return "HD";
  return null;
}

/** First genre of a provider's comma list. */
export function firstGenre(genre: string | null | undefined) {
  return genre?.split(/[,/|]/)[0]?.trim() || null;
}

/** Initials for a person or channel placeholder. */
export function initials(name: string, max = 2) {
  return name
    .split(/\s+/)
    .filter((w) => /[\p{L}\p{N}]/u.test(w[0] ?? ""))
    .slice(0, max)
    .map((w) => w[0]!.toUpperCase())
    .join("");
}
