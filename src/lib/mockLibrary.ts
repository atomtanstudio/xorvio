// A media folder's own library page for the browser preview (mock.ts): a
// made-up tree of clips for each folder source, GIFs, animations and videos
// as the app tells them apart (xorvio-model's FileFacts::kind), read a few
// at a time as the app reads them, and changed in memory as the page asks.

import type { Backend } from "./api";
import type { ClipKind, DeletedBatch, FileFacts, FolderEntry, FolderListing, FolderQuery, FolderVideo, Movie, SourceConfig } from "./types";

interface Clip {
  id: string;
  /** Below the source's folder, "/" between parts. */
  path: string;
  addedAt: number;
  facts: FileFacts;
  /** Not read yet: the page can't place it by type or length. */
  unread: boolean;
}

interface Tree {
  root: string;
  clips: Clip[];
  /** Folders below the root, empty ones too. */
  folders: Set<string>;
  deleted: (DeletedBatch & { clips: Clip[]; folderPaths: string[] })[];
}

type LibraryCalls = Pick<
  Backend,
  | "folderListing"
  | "folderVideoIds"
  | "deleteFolderItems"
  | "recentlyDeleted"
  | "restoreDeleted"
  | "emptyRecentlyDeleted"
  | "moveFolderItems"
  | "renameFolderVideo"
  | "createMediaFolder"
  | "renameMediaFolder"
  | "onFolderFacts"
>;

const now = () => Math.floor(Date.now() / 1000);
/** A folder the preview's login can read but not change. */
const READ_ONLY = "feedsilo";
const readOnly = (path: string) => path === READ_ONLY || path.startsWith(`${READ_ONLY}/`);
const refuse = () => {
  throw new Error(`“${READ_ONLY}” is read-only for you: you can see what's in it, but nothing in it can be added, deleted, moved or renamed. Give your login write access to “${READ_ONLY}” where it's kept (for a network share, in the server's settings for it), then try again.`);
};

/** The twin of FileFacts::kind. */
function clipKind(facts: FileFacts | null): ClipKind | null {
  if (!facts?.durationSeconds) return null;
  if (facts.audio || facts.durationSeconds > 30) return "video";
  const side = Math.max(facts.width ?? 1e6, facts.height ?? 1e6);
  const bitrate = facts.sizeBytes != null ? (facts.sizeBytes * 8) / facts.durationSeconds : Infinity;
  return side <= 540 || bitrate < 600_000 ? "gif" : "animation";
}

/** A steady pseudo-random number from 0 to 1 for a seed. */
function noise(seed: number) {
  const x = Math.sin(seed * 9301 + 49297) * 233280;
  return x - Math.floor(x);
}

const MEMES = ["Dancing cat", "Surprised Pikachu", "This is fine", "Distracted boyfriend", "Blinking guy", "Mind blown", "Deal with it", "Facepalm", "Slow clap", "Nope", "Shrug", "Mic drop"];
const SONGS = ["XTC - Dear God", "Talking Heads - Once in a Lifetime", "Björk - Army of Me", "New Order - Blue Monday", "Kate Bush - Running Up That Hill", "Devo - Whip It"];

function makeTree(root: string): Tree {
  const clips: Clip[] = [];
  let seed = 1;
  const add = (path: string, facts: FileFacts) => {
    seed += 1;
    clips.push({ id: `clip-${root}-${path}`, path, addedAt: now() - Math.floor(noise(seed) * 90 * 86400), facts, unread: noise(seed + 0.5) < 0.12 });
  };
  for (let i = 0; i < 260; i++) {
    const r = noise(i + 3);
    const name = r < 0.25 ? `${MEMES[i % MEMES.length]}${i >= MEMES.length ? ` ${Math.floor(i / MEMES.length) + 1}` : ""}` : crypto.randomUUID();
    if (r < 0.62) add(`imported/2026/${name}.mp4`, { durationSeconds: 1 + noise(i) * 12, width: noise(i + 1) < 0.5 ? 498 : 480, height: 280, audio: false, sizeBytes: 20_000 + Math.floor(noise(i + 2) * 100_000) });
    else if (r < 0.82) add(`imported/2026/${name}.mp4`, { durationSeconds: 4 + noise(i), width: 720, height: 1280, audio: false, sizeBytes: 1_000_000 + Math.floor(noise(i + 2) * 3_000_000) });
    else add(`imported/2026/${name}.mp4`, { durationSeconds: 5 + noise(i) * 300, width: 1280, height: 720, audio: true, sizeBytes: 5_000_000 + Math.floor(noise(i + 2) * 80_000_000) });
  }
  for (let i = 1; i <= 40; i++) add(`generated-video-batches/needs-sorted/clip_${String(i).padStart(4, "0")}.mp4`, { durationSeconds: 5, width: 1280, height: 720, audio: false, sizeBytes: 2_400_000 });
  SONGS.forEach((song) => add(`Music Videos/${song}.mp4`, { durationSeconds: 200 + noise(seed) * 120, width: 1920, height: 1080, audio: true, sizeBytes: 120_000_000 }));
  add("Beach Trip.mov", { durationSeconds: 94, width: 1920, height: 1080, audio: true, sizeBytes: 300_000_000 });
  // As a network share can keep a folder: readable, not changeable.
  for (let i = 1; i <= 6; i++) add(`${READ_ONLY}/meme-${i}.mp4`, { durationSeconds: 2 + i, width: 498, height: 280, audio: false, sizeBytes: 40_000 });
  const folders = new Set<string>(["imported", "imported/2026", "generated-video-batches", "generated-video-batches/needs-sorted", "Music Videos", "Keep", READ_ONLY]);
  return { root, clips, folders, deleted: [] };
}

/** A frame of the clip, as the app makes one: a colour of its own and what it is. */
function frame(clip: Clip) {
  const hue = Math.floor(noise(clip.id.length * 31 + clip.path.charCodeAt(clip.path.length - 6)) * 360);
  const label = clipKind(clip.facts)?.toUpperCase() ?? "";
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue},55%,38%)"/><stop offset="1" stop-color="hsl(${(hue + 50) % 360},60%,18%)"/></linearGradient></defs><rect width="320" height="180" fill="url(#g)"/><text x="160" y="100" font-family="sans-serif" font-size="22" font-weight="700" fill="rgba(255,255,255,.55)" text-anchor="middle">${label}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

const baseName = (path: string) => path.split("/").pop() ?? path;
const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");
const stem = (name: string) => (name.includes(".") ? name.slice(0, name.lastIndexOf(".")) : name);

export function libraryBackend(sourceOf: (id: string) => SourceConfig | undefined): LibraryCalls {
  const trees = new Map<string, Tree>();
  const factListeners = new Set<(count: number) => void>();
  const tree = (sourceId: string) => {
    const config = sourceOf(sourceId);
    if (!config?.folders?.length) throw new Error("This source isn't a media folder.");
    let found = trees.get(sourceId);
    if (!found) {
      found = makeTree(config.folders[0]);
      trees.set(sourceId, found);
    }
    return found;
  };
  // A few files read every second or two, as the app reads them.
  setInterval(() => {
    let read = 0;
    for (const t of trees.values())
      for (const clip of t.clips)
        if (clip.unread && read < 8) {
          clip.unread = false;
          read += 1;
        }
    if (read) factListeners.forEach((l) => l(read));
  }, 1500);

  const relative = (t: Tree, path: string | null) => {
    if (!path || path === t.root) return "";
    if (!path.startsWith(`${t.root}/`)) throw new Error("That isn't in this source's folders.");
    return path.slice(t.root.length + 1);
  };
  const absolute = (t: Tree, path: string) => (path ? `${t.root}/${path}` : t.root);
  const movie = (sourceId: string, t: Tree, clip: Clip): Movie => ({
    id: clip.id,
    sourceId,
    title: stem(baseName(clip.path)),
    year: null,
    posterUrl: frame(clip),
    backdropUrl: null,
    categoryId: null,
    rating: null,
    addedAt: clip.addedAt,
    plot: null,
    genre: null,
    tmdbId: null,
    imdbId: null,
    durationSeconds: clip.unread ? null : Math.round(clip.facts.durationSeconds ?? 0),
    stream: { type: "file", path: absolute(t, clip.path) },
  });
  const matching = (query: FolderQuery) => {
    const t = tree(query.sourceId);
    const here = relative(t, query.folder);
    const inside = (path: string) => (here ? path.startsWith(`${here}/`) : true) && (query.below || parentOf(path) === here);
    const placed = t.clips.filter((clip) => inside(clip.path));
    const facts = (clip: Clip) => (clip.unread ? null : clip.facts);
    const chosen = placed.filter((clip) => {
      const f = facts(clip);
      if (query.kind && clipKind(f) !== query.kind) return false;
      if (query.length === "short" && !(f?.durationSeconds && f.durationSeconds < 60)) return false;
      if (query.length === "long" && !(f?.durationSeconds && f.durationSeconds >= 60)) return false;
      return true;
    });
    const by = {
      added: (a: Clip, b: Clip) => b.addedAt - a.addedAt,
      name: (a: Clip, b: Clip) => baseName(a.path).localeCompare(baseName(b.path)),
      length: (a: Clip, b: Clip) => (facts(b)?.durationSeconds ?? -1) - (facts(a)?.durationSeconds ?? -1),
      size: (a: Clip, b: Clip) => (facts(b)?.sizeBytes ?? -1) - (facts(a)?.sizeBytes ?? -1),
    }[query.sort];
    return { t, here, placed, chosen: chosen.sort(by), facts };
  };
  const videosBelow = (t: Tree, folder: string) => t.clips.filter((clip) => clip.path.startsWith(`${folder}/`)).length;
  const check = (name: string) => {
    const trimmed = name.trim();
    if (!trimmed) throw new Error("Give it a name.");
    if (trimmed.startsWith(".") || /[/\\:]/.test(trimmed)) throw new Error("A name can't start with a full stop or contain / \\ or :.");
    return trimmed;
  };
  const taken = (t: Tree, path: string) => t.folders.has(path) || t.clips.some((clip) => clip.path.toLowerCase() === path.toLowerCase());
  const unused = (t: Tree, path: string) => {
    if (!taken(t, path)) return path;
    const name = baseName(path);
    const dot = name.lastIndexOf(".");
    const [base, extension] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
    for (let n = 2; ; n++) {
      const candidate = `${parentOf(path) ? `${parentOf(path)}/` : ""}${base} (${n})${extension}`;
      if (!taken(t, candidate)) return candidate;
    }
  };
  /** Moves a folder (and everything below it) to `to`. */
  const moveFolder = (t: Tree, from: string, to: string) => {
    for (const folder of [...t.folders]) {
      if (folder === from || folder.startsWith(`${from}/`)) {
        t.folders.delete(folder);
        t.folders.add(to + folder.slice(from.length));
      }
    }
    for (const clip of t.clips) if (clip.path.startsWith(`${from}/`)) Object.assign(clip, { path: to + clip.path.slice(from.length), id: `clip-${t.root}-${to + clip.path.slice(from.length)}` });
  };

  return {
    folderListing: async (query) => {
      const { t, here, chosen, placed, facts } = matching(query);
      if (here && !t.folders.has(here)) throw new Error("That folder isn't there any more.");
      const trail: FolderEntry[] = [{ name: baseName(t.root), path: t.root, videos: 0 }];
      here.split("/").filter(Boolean).reduce((path, part) => {
        const next = path ? `${path}/${part}` : part;
        trail.push({ name: part, path: absolute(t, next), videos: 0 });
        return next;
      }, "");
      const folders: FolderEntry[] =
        query.offset > 0
          ? []
          : [...t.folders]
              .filter((folder) => parentOf(folder) === here)
              .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))
              .map((folder) => ({ name: baseName(folder), path: absolute(t, folder), videos: videosBelow(t, folder) }));
      const videos: FolderVideo[] = chosen.slice(query.offset, query.offset + query.limit).map((clip) => ({ movie: movie(query.sourceId, t, clip), path: clip.path, facts: facts(clip), kind: clipKind(facts(clip)) }));
      const listing: FolderListing = { folder: absolute(t, here), trail, folders, videos, total: chosen.length, unread: placed.filter((clip) => clip.unread).length };
      return listing;
    },
    folderVideoIds: async (query) => matching(query).chosen.map((clip) => clip.id),
    deleteFolderItems: async (sourceId, videos, folders) => {
      const t = tree(sourceId);
      const folderPaths = folders.map((folder) => relative(t, folder)).filter(Boolean);
      const gone = t.clips.filter((clip) => videos.includes(clip.id) || folderPaths.some((folder) => clip.path.startsWith(`${folder}/`)));
      if (gone.some((clip) => readOnly(clip.path)) || folderPaths.some(readOnly)) refuse();
      t.clips = t.clips.filter((clip) => !gone.includes(clip));
      for (const folder of [...t.folders]) if (folderPaths.some((f) => folder === f || folder.startsWith(`${f}/`))) t.folders.delete(folder);
      const loose = gone.filter((clip) => !folderPaths.some((folder) => clip.path.startsWith(`${folder}/`)));
      t.deleted.unshift({
        id: `batch-${Date.now()}`,
        root: t.root,
        deletedAt: now(),
        entries: [...folderPaths.map((path) => ({ path, folder: true })), ...loose.map((clip) => ({ path: clip.path, folder: false }))],
        clips: gone,
        folderPaths,
      });
      return gone.length;
    },
    recentlyDeleted: async (sourceId) => tree(sourceId).deleted.map(({ id, root, deletedAt, entries }) => ({ id, root, deletedAt, entries })),
    restoreDeleted: async (sourceId, batchId) => {
      const t = tree(sourceId);
      const batch = t.deleted.find((b) => b.id === batchId);
      if (!batch) throw new Error("That's no longer in Recently Deleted.");
      batch.folderPaths.forEach((folder) => t.folders.add(folder));
      for (const clip of batch.clips) {
        clip.path = unused(t, clip.path);
        t.clips.push(clip);
      }
      t.deleted = t.deleted.filter((b) => b !== batch);
    },
    emptyRecentlyDeleted: async (sourceId, batchIds) => {
      const t = tree(sourceId);
      t.deleted = batchIds ? t.deleted.filter((b) => !batchIds.includes(b.id)) : [];
    },
    moveFolderItems: async (sourceId, videos, folders, to) => {
      const t = tree(sourceId);
      const target = relative(t, to);
      if (readOnly(target) || folders.some((f) => readOnly(relative(t, f))) || t.clips.some((c) => videos.includes(c.id) && readOnly(c.path))) refuse();
      for (const folder of folders.map((f) => relative(t, f))) {
        if (target === folder || target.startsWith(`${folder}/`)) throw new Error("A folder can't go inside itself.");
        if (parentOf(folder) === target) continue;
        moveFolder(t, folder, unused(t, target ? `${target}/${baseName(folder)}` : baseName(folder)));
      }
      for (const clip of t.clips.filter((c) => videos.includes(c.id))) {
        if (parentOf(clip.path) === target) continue;
        clip.path = unused(t, target ? `${target}/${baseName(clip.path)}` : baseName(clip.path));
        clip.id = `clip-${t.root}-${clip.path}`;
      }
    },
    renameFolderVideo: async (sourceId, itemId, name) => {
      const t = tree(sourceId);
      const clip = t.clips.find((c) => c.id === itemId);
      if (!clip) throw new Error("That video isn't there any more.");
      const extension = baseName(clip.path).includes(".") ? baseName(clip.path).slice(baseName(clip.path).lastIndexOf(".")) : "";
      const path = `${parentOf(clip.path) ? `${parentOf(clip.path)}/` : ""}${check(name)}${extension}`;
      if (path !== clip.path && taken(t, path)) throw new Error(`There's already something called “${baseName(path)}” there.`);
      Object.assign(clip, { path, id: `clip-${t.root}-${path}` });
    },
    createMediaFolder: async (sourceId, parent, name) => {
      const t = tree(sourceId);
      const at = relative(t, parent);
      const path = at ? `${at}/${check(name)}` : check(name);
      if (taken(t, path)) throw new Error(`There's already something called “${baseName(path)}” there.`);
      t.folders.add(path);
      return absolute(t, path);
    },
    renameMediaFolder: async (sourceId, path, name) => {
      const t = tree(sourceId);
      const from = relative(t, path);
      if (!from) throw new Error("That isn't in this source's folders.");
      const to = `${parentOf(from) ? `${parentOf(from)}/` : ""}${check(name)}`;
      if (to !== from && taken(t, to)) throw new Error(`There's already something called “${name.trim()}” there.`);
      moveFolder(t, from, to);
    },
    onFolderFacts: (handler) => {
      factListeners.add(handler);
      return () => factListeners.delete(handler);
    },
  };
}
