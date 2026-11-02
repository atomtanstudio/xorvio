import { useVirtualizer } from "@tanstack/react-virtual";
import { Bell, ChevronDown, ChevronLeft, ChevronRight, CircleDot, CircleStop, Clock, EyeOff, Grid2x2Plus, History, LayoutGrid, ListVideo, Lock, LockOpen, Maximize2, Plus, RotateCcw, Search, SlidersHorizontal, Star, Undo2 } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChannelLogo } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { categoryMenu, channelMenu, groupMenu, programMenu, type ChannelListContext } from "../components/menus";
import { cancelOrStopRecording, lastChannel, playChannel, playLastChannel, playReplay, programEntry, recordChannelNow, recordProgram, toggleFavoriteChannel, toggleWatchlist } from "../lib/actions";
import { api } from "../lib/api";
import { dropInList, lockAgain, newGroup, showLocked } from "../lib/channelEdits";
import { clockTime, dayName, duration, nowSeconds, remaining } from "../lib/format";
import { useGuideJump } from "../lib/guideJump";
import { recordingFor, useApp } from "../lib/store";
import type { Category, Channel, ItemEdit, Program, ProgramGenre, ProgramHit, Timestamp } from "../lib/types";
import { FAVORITES_LIST } from "../lib/types";
import { isFavoriteChannel, sameChannel } from "../lib/versions";
import { addToMultiview } from "../player/multiviewStore";
import { DockTarget } from "../player/Player";

type Group = { type: "all" } | { type: "favorites" } | { type: "recent" } | { type: "category"; id: string } | { type: "list"; id: string };

const CHANNEL_COLUMN = 235;
const HOUR = 3600;

export function LiveTVScreen() {
  const [categories, setCategories] = useState<Category[]>([]);
  const [group, setGroup] = useState<Group>({ type: "all" });
  const [channels, setChannels] = useState<Channel[]>([]);
  const [selected, setSelected] = useState<{ channel: Channel; program: Program | null } | null>(null);
  const [mode, setMode] = useState<"guide" | "channels">("guide");
  const [filter, setFilter] = useState("");
  const [day, setDay] = useState(0);
  // A programme found by search, for the guide to scroll to once it's showing its day.
  const [target, setTarget] = useState<ProgramHit | null>(null);
  const [span, setSpan] = useState<{ from: Timestamp; to: Timestamp } | null>(null);
  const guideNav = useRef<GuideNav>({ jump: () => {}, toNow: () => {}, toTime: () => {} });
  const favorites = useApp((s) => s.favorites);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const presentation = useApp((s) => s.presentation);
  const guideRevision = useApp((s) => s.guideRevision);
  const groups = useApp((s) => s.groups);
  const edits = useApp((s) => s.edits);
  const parental = useApp((s) => s.parental);
  const editsRevision = useApp((s) => s.editsRevision);
  const groupSimilar = useApp((s) => s.settings.groupSimilarChannels);
  // Hidden channels and categories show (dimmed) so they can be shown again.
  const [showHidden, setShowHidden] = useState(false);
  const anyHidden = useMemo(() => [...edits.values()].some((e) => e.hidden && (e.kind === "channel" || e.kind === "liveCategory")), [edits]);

  useEffect(() => {
    api.categories("live", showHidden).then(setCategories).catch(() => {});
  }, [showHidden, editsRevision, guideRevision]);

  useEffect(() => {
    api.guideSpan().then(setSpan).catch(() => {});
  }, [guideRevision]);

  // A category or group that's gone (hidden, locked, deleted) gives way to All Channels.
  useEffect(() => {
    if (group.type === "list" && !groups.some((g) => g.id === group.id)) setGroup({ type: "all" });
  }, [groups, group]);
  useEffect(() => {
    if (group.type === "category" && categories.length && !categories.some((c) => c.id === group.id)) setGroup({ type: "all" });
  }, [categories, group]);
  useEffect(() => {
    if (!anyHidden) setShowHidden(false);
  }, [anyHidden]);

  useEffect(() => {
    let cancelled = false;
    const load =
      group.type === "recent"
        ? api.recentChannels(100)
        : api.channels({
            categoryId: group.type === "category" ? group.id : null,
            favoritesOnly: group.type === "favorites",
            listId: group.type === "list" ? group.id : null,
            includeHidden: showHidden,
            // Said here, not left to the backend's copy of the setting, which a
            // list loaded straight after changing it could beat.
            groupSimilar,
          });
    load
      .then((list) => {
        if (cancelled) return;
        setChannels(list);
        // A new group shows its own first channel, unless something in it is playing.
        setSelected((current) => {
          const row = current && list.find((c) => sameChannel(c, current.channel));
          return current && row ? { ...current, channel: row } : null;
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [group, showHidden, groupSimilar, editsRevision, group.type === "favorites" ? favorites : null, guideRevision]);

  // A guide matched or refreshed: the header keeps its programme if the new
  // listings still have it, and shows what's on now otherwise.
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  useEffect(() => {
    const current = selectedRef.current;
    if (!guideRevision || !current) return;
    const { channel, program } = current;
    const kept = program ? api.guide([channel.id], program.start, program.end).then(([row]) => row?.programs.find((p) => p.id === program.id) ?? null) : Promise.resolve(null);
    kept
      .then((found) => found ?? api.nowNext([channel.id]).then(([row]) => row?.programs[0] ?? null))
      .then((next) => setSelected((now) => (now?.channel.id === channel.id && now.program?.id === program?.id ? { channel: now.channel, program: next } : now)))
      .catch(() => {});
  }, [guideRevision]);

  // Coming back to Live TV docks the corner mini player in the panel again.
  useEffect(() => {
    if (useApp.getState().presentation === "mini") useApp.getState().setPresentation("docked");
  }, []);

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return q ? channels.filter((c) => c.name.toLowerCase().includes(q) || String(c.number ?? "").startsWith(q)) : channels;
  }, [channels, filter]);

  // Favorites and groups keep their own order, which their channels' menus and dragging change.
  const listContext = useMemo((): ChannelListContext | undefined => {
    if (group.type === "favorites") return { listId: FAVORITES_LIST, channels: visible };
    if (group.type === "list") return { listId: group.id, channels: visible };
    return undefined;
  }, [group, visible]);

  // The header follows what's playing until something else is chosen.
  const playingChannel = nowPlaying?.channel;
  useEffect(() => {
    if (!selected && visible[0]) {
      const first = playingChannel ?? visible[0];
      api.nowNext([first.id]).then(([row]) => setSelected({ channel: first, program: row?.programs[0] ?? null })).catch(() => setSelected({ channel: first, program: null }));
    }
  }, [visible, selected, playingChannel]);

  // Search found a programme: show its day in the guide, on a list that has its channel.
  const jumpTo = (hit: ProgramHit) => {
    setMode("guide");
    setFilter("");
    const row = channels.find((c) => sameChannel(c, hit.channel));
    if (!row) setGroup({ type: "all" });
    setDay(dayOffset(hit.program.start));
    setSelected({ channel: row ?? hit.channel, program: hit.program });
    setTarget(hit);
  };

  // Show in Guide, from a saved programme: the same, once the channels are here.
  const guideTarget = useApp((s) => s.guideTarget);
  useEffect(() => {
    if (!guideTarget || !channels.length) return;
    useApp.getState().showInGuide(null);
    jumpTo(guideTarget);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [guideTarget, channels]);
  // "Show in Guide" from search lands here the same way.
  const jump = useGuideJump((s) => s.hit);
  useEffect(() => {
    if (!jump) return;
    useGuideJump.getState().set(null);
    jumpTo(jump);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jump]);

  const choose = useCallback(
    (channel: Channel, program: Program | null) => {
      const t = nowSeconds();
      const isCurrent = !program || (program.start <= t && t < program.end);
      const playing = useApp.getState().nowPlaying?.channel;
      const playingThis = !!playing && sameChannel(playing, channel);
      setSelected({ channel, program });
      if (isCurrent) {
        // TiviMate: the first click plays in the panel, the second goes full screen.
        if (playingThis) useApp.getState().setPresentation("full");
        else playChannel(channel, "docked");
      }
    },
    [],
  );

  return (
    <div className="content live-tv fixed">
      <aside className="live-groups">
        <GroupButton icon={<LayoutGrid size={15} />} label="All Channels" selected={group.type === "all"} onClick={() => setGroup({ type: "all" })} />
        <GroupButton icon={<Star size={15} />} label="Favorites" selected={group.type === "favorites"} onClick={() => setGroup({ type: "favorites" })} />
        <GroupButton icon={<History size={15} />} label="Recently Watched" selected={group.type === "recent"} onClick={() => setGroup({ type: "recent" })} />
        {(anyHidden || showHidden) && <GroupButton icon={<EyeOff size={15} />} label="Show Hidden" selected={showHidden} onClick={() => setShowHidden(!showHidden)} />}
        {parental.hasPin &&
          (parental.unlocked ? (
            <GroupButton icon={<LockOpen size={15} />} label="Lock Again" onClick={lockAgain} />
          ) : (
            parental.lockedCount > 0 && <GroupButton icon={<Lock size={15} />} label="Show Locked…" onClick={showLocked} />
          ))}
        <div className="live-groups-heading">My Groups</div>
        {groups.map((g) => (
          <GroupButton
            key={g.id}
            icon={<ListVideo size={15} />}
            label={g.name}
            selected={group.type === "list" && group.id === g.id}
            onClick={() => setGroup({ type: "list", id: g.id })}
            onContextMenu={(e) => openMenu(e, groupMenu(g))}
          />
        ))}
        <GroupButton
          icon={<Plus size={15} />}
          label="New Group…"
          onClick={async () => {
            const created = await newGroup();
            if (created) setGroup({ type: "list", id: created.id });
          }}
        />
        {groupSections(categories).map(([source, list]) => (
          <div key={source}>
            <div className="live-groups-heading">{source}</div>
            {list.map((c) => (
              <GroupButton
                key={c.id}
                label={c.name}
                selected={group.type === "category" && group.id === c.id}
                onClick={() => setGroup({ type: "category", id: c.id })}
                onContextMenu={(e) => openMenu(e, categoryMenu(c))}
                dimmed={edits.get(c.id)?.hidden}
                trailing={editMarks(edits.get(c.id)?.hidden, edits.get(c.id)?.locked)}
                plain
              />
            ))}
          </div>
        ))}
      </aside>
      <section className="live-main">
        <LiveHeader selection={selected} docked={presentation === "docked" || (presentation !== "full" && !!nowPlaying?.channel)} />
        <div className="guide-toolbar">
          <div className="segmented">
            <button className={mode === "guide" ? "selected" : ""} onClick={() => setMode("guide")}>
              Guide
            </button>
            <button className={mode === "channels" ? "selected" : ""} onClick={() => setMode("channels")}>
              Channels
            </button>
          </div>
          {mode === "guide" && <GuideControls day={day} setDay={setDay} nav={guideNav} span={span} />}
          <div style={{ flex: 1 }} />
          <GuideSearch onPick={jumpTo} />
          <input className="text-input guide-filter" placeholder="Filter channels" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <button className="btn-plain" onClick={(e) => openMenu(e, guideOptionsMenu(), true)}>
            <SlidersHorizontal size={14} /> Guide Options
          </button>
          <span className="guide-count">{visible.length.toLocaleString()} channels</span>
        </div>
        {mode === "guide" ? (
          <GuideGrid
            channels={visible}
            selected={selected}
            onChoose={choose}
            onSelect={(channel, program) => setSelected({ channel, program })}
            day={day}
            setDay={setDay}
            nav={guideNav}
            target={target}
            onTargetShown={() => setTarget(null)}
            list={listContext}
          />
        ) : (
          <ChannelList channels={visible} selected={selected?.channel ?? null} onChoose={(c, p) => choose(c, p)} list={listContext} />
        )}
      </section>
    </div>
  );
}

/** Categories under their source's name ("Home Media"), as TiviMate lists playlists. */
function groupSections(categories: Category[]): [string, Category[]][] {
  const bySource = new Map<string, Category[]>();
  for (const c of categories) bySource.set(c.sourceId, [...(bySource.get(c.sourceId) ?? []), c]);
  return [...bySource.entries()].map(([source, list]) => [source === "demo" ? "Demo Library" : "Categories", list.sort((a, b) => a.sortIndex - b.sortIndex)]);
}

interface GroupButtonProps {
  icon?: React.ReactNode;
  label: string;
  selected?: boolean;
  onClick: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  plain?: boolean;
  /** Hidden, and showing only because Show Hidden is on. */
  dimmed?: boolean;
  trailing?: React.ReactNode;
}

function GroupButton({ icon, label, selected, onClick, onContextMenu, plain, dimmed, trailing }: GroupButtonProps) {
  return (
    <button className={`live-group ${selected ? "selected" : ""} ${plain ? "plain" : ""} ${dimmed ? "dimmed" : ""}`} onClick={onClick} onContextMenu={onContextMenu}>
      {icon}
      <span>{label}</span>
      {trailing}
    </button>
  );
}

/** Small marks for something hidden or locked. */
function editMarks(hidden: boolean | undefined, locked: boolean | undefined) {
  if (!hidden && !locked) return null;
  return (
    <span className="edit-marks">
      {hidden && <EyeOff size={11} aria-label="Hidden" />}
      {locked && <Lock size={11} aria-label="Locked" />}
    </span>
  );
}

/** "3 versions", on a row standing for a channel the provider lists several times. */
function VersionsBadge({ channel }: { channel: Channel }) {
  const count = channel.versions?.length ?? 0;
  if (count < 2) return null;
  const names = channel.versions!.map((v) => v.name).join("\n");
  return (
    <span className="versions-badge" title={`${names}\n\nRight-click for Versions`}>
      {count} versions
    </span>
  );
}

/** A channel, or the category it's in, is hidden (it shows only with Show Hidden). */
function isHidden(edits: Map<string, ItemEdit>, channel: Channel) {
  return !!edits.get(channel.id)?.hidden || (!!channel.categoryId && !!edits.get(channel.categoryId)?.hidden);
}

/** Dragging a channel onto another moves it there, in a list's own order. */
function useListDrag(list: ChannelListContext | undefined) {
  const [dragging, setDragging] = useState<string | null>(null);
  const [over, setOver] = useState<string | null>(null);
  return (channel: Channel) => {
    if (!list) return {};
    const end = () => {
      setDragging(null);
      setOver(null);
    };
    return {
      draggable: true,
      "data-drop": over === channel.id && dragging && dragging !== channel.id ? "true" : undefined,
      onDragStart: (e: React.DragEvent) => {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", channel.name);
        setDragging(channel.id);
      },
      onDragOver: (e: React.DragEvent) => {
        if (!dragging) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        if (over !== channel.id) setOver(channel.id);
      },
      onDrop: (e: React.DragEvent) => {
        e.preventDefault();
        if (dragging) dropInList(list.listId, list.channels, dragging, channel.id);
        end();
      },
      onDragEnd: end,
    };
  };
}

function guideOptionsMenu(): MenuEntry[] {
  const { settings, updateSettings } = useApp.getState();
  const colors = settings.guideColors === "category";
  return [
    { heading: "Hours Shown" },
    ...[2, 3, 4].map((h) => ({ label: `${settings.guideHours === h ? "✓ " : "   "}${h} hours`, onSelect: () => updateSettings({ guideHours: h }) })),
    { separator: true },
    { heading: "Row Height" },
    ...[
      ["Compact", 48],
      ["Standard", 60],
      ["Tall", 72],
    ].map(([label, h]) => ({ label: `${settings.guideRowHeight === h ? "✓ " : "   "}${label}`, onSelect: () => updateSettings({ guideRowHeight: h as number }) })),
    { separator: true },
    { label: `${settings.showChannelNumbers ? "✓ " : "   "}Channel Numbers`, onSelect: () => updateSettings({ showChannelNumbers: !settings.showChannelNumbers }) },
    { label: `${colors ? "✓ " : "   "}Colours by Category`, onSelect: () => updateSettings({ guideColors: colors ? "plain" : "category" }) },
    { label: `${settings.groupSimilarChannels ? "✓ " : "   "}Group Similar Channels`, onSelect: () => updateSettings({ groupSimilarChannels: !settings.groupSimilarChannels }) },
  ];
}

/** A guide cell's tint for its kind of programme, when the guide is coloured. */
function genreClass(genre: ProgramGenre | null, colored: boolean) {
  return colored && genre ? `tinted genre-${genre}` : "";
}

// MARK: Header

function LiveHeader({ selection, docked }: { selection: { channel: Channel; program: Program | null } | null; docked: boolean }) {
  const favorites = useApp((s) => s.favorites);
  const recordings = useApp((s) => s.recordings);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const channelTrail = useApp((s) => s.channelTrail);
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 30000);
    return () => clearInterval(t);
  }, []);

  if (!selection) return <div className="live-header" />;
  const { channel, program } = selection;
  const t = nowSeconds();
  const airing = program ? program.start <= t && t < program.end : true;
  const past = program ? program.end <= t : false;
  const canReplay = !!program && past && channel.catchupDays > 0;
  const recording = program ? recordingFor(recordings, channel.id, program.start, program.id) : undefined;
  const isRecording = recording && (recording.status === "recording" || recording.status === "scheduled");
  const playingThis = !!nowPlaying?.channel && sameChannel(nowPlaying.channel, channel);
  const favorite = isFavoriteChannel(favorites, channel);
  const last = channelTrail.length ? lastChannel() : null;

  return (
    <div className="live-header">
      <div className="live-header-info">
        <div className="live-channel-line">
          <ChannelLogo src={channel.logoUrl} name={channel.name} size={26} />
          {channel.number != null && <span className="num">{channel.number}</span>}
          <span className="name">{channel.name}</span>
          {airing && <span className="live-pill">LIVE</span>}
          {recording?.status === "recording" && (
            <span className="rec-badge active">
              <span className="dot" />
              REC
            </span>
          )}
          {favorite && <Star size={14} fill="#f5c84c" color="#f5c84c" />}
        </div>
        <h2 className="live-title">{program?.title ?? channel.name}</h2>
        {program && (
          <>
            <div className="live-facts">
              <span>
                {dayName(program.start) === "Today" ? new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(program.start * 1000) : dayName(program.start)} {clockTime(program.start)} – {clockTime(program.end)}
              </span>
              <span>· {duration(program.end - program.start)}</span>
              {program.category && <span>· {program.category}</span>}
              {airing && <span>· {remaining(program.end - t)}</span>}
              {program.episode && <span>· {program.episode}</span>}
            </div>
            {airing && (
              <div className="live-progress">
                <div style={{ width: `${((t - program.start) / (program.end - program.start)) * 100}%` }} />
              </div>
            )}
            {program.summary && <p className="live-summary">{program.summary}</p>}
          </>
        )}
        <div className="live-actions">
          {airing && (
            <button className="btn-plain" onClick={() => (playingThis ? useApp.getState().setPresentation("full") : playChannel(channel, "full"))}>
              <Maximize2 size={14} /> Full Screen
            </button>
          )}
          {program && airing && channel.catchupDays > 0 && (
            <button className="btn-plain" onClick={() => playReplay(channel, program)}>
              <RotateCcw size={14} /> From Start
            </button>
          )}
          {canReplay && program && (
            <button className="btn-plain" onClick={() => playReplay(channel, program)}>
              <RotateCcw size={14} /> Play Replay
            </button>
          )}
          {!past &&
            (isRecording && program ? (
              <button className="btn-plain rec-active" onClick={() => cancelOrStopRecording(program, channel)}>
                <CircleStop size={14} /> {recording!.status === "recording" ? "Stop Recording" : "Cancel Recording"}
              </button>
            ) : (
              <button className="btn-plain" onClick={() => (program ? recordProgram(program, channel) : recordChannelNow(channel))}>
                <CircleDot size={14} color="var(--recording)" /> {airing ? "Record" : "Record"}
              </button>
            ))}
          {program && !airing && !past && (
            <button className={`btn-plain ${watchlistIds.has(program.id) ? "active" : ""}`} onClick={() => toggleWatchlist(programEntry(program, channel))}>
              <Bell size={14} /> {watchlistIds.has(program.id) ? "Reminder Set" : "Remind Me"}
            </button>
          )}
          <button className={`btn-plain icon-only ${favorite ? "active" : ""}`} onClick={() => toggleFavoriteChannel(channel)} aria-label="Favorite">
            <Star size={14} fill={favorite ? "currentColor" : "none"} />
          </button>
          {airing && (
            <button className="btn-plain icon-only" onClick={() => addToMultiview(channel)} aria-label="Add to Multiview" title="Add to Multiview">
              <Grid2x2Plus size={14} />
            </button>
          )}
          {last && (
            <button className="btn-plain" onClick={playLastChannel} title={`Back to ${last.name} (L)`}>
              <Undo2 size={14} /> Last Channel
            </button>
          )}
        </div>
      </div>
      <DockTarget className={`live-video ${docked ? "" : "idle"}`}>
        {!docked && (
          <button className="live-video-placeholder" onClick={() => playChannel(channel, "docked")}>
            <ChannelLogo src={channel.logoUrl} name={channel.name} size={54} />
            <span>Watch {channel.name}</span>
          </button>
        )}
      </DockTarget>
    </div>
  );
}

// MARK: Guide grid

interface GuideNav {
  jump(direction: 1 | -1): void;
  toNow(): void;
  /** Scrolls the day shown to a moment on it. */
  toTime(t: Timestamp): void;
}

/** Times of day the guide can jump to, in hours. */
const JUMP_HOURS = [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22];

/** The day and time picker, arrows and Now button sit in the toolbar but drive the grid. */
function GuideControls({ day, setDay, nav, span }: { day: number; setDay: (d: number) => void; nav: React.RefObject<GuideNav>; span: { from: Timestamp; to: Timestamp } | null }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  // Yesterday to a week ahead, as far as the listings go; today always.
  const first = span ? Math.max(-1, Math.min(0, dayOffset(span.from))) : 0;
  const last = span ? Math.min(7, Math.max(0, dayOffset(span.to - 1))) : 0;
  const days: number[] = [];
  for (let d = Math.min(first, day); d <= Math.max(last, day); d++) days.push(d);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <>
      <div className="guide-jump" ref={box}>
        <button className="popup-button guide-day" onClick={() => setOpen((o) => !o)} aria-haspopup="true" aria-expanded={open}>
          <span>{dayName(startOfDay(day) + 12 * HOUR)}</span>
          <ChevronDown size={13} />
        </button>
        {open && (
          <div className="guide-jump-panel menu">
            <div className="guide-jump-column">
              <div className="menu-heading">Day</div>
              {days.map((d) => (
                <button
                  key={d}
                  className="menu-item"
                  onClick={() => {
                    setDay(d);
                    setOpen(false);
                  }}
                >
                  <span className="check">{d === day ? "✓" : ""}</span>
                  <span>{dayName(startOfDay(d) + 12 * HOUR)}</span>
                </button>
              ))}
            </div>
            <div className="guide-jump-column">
              <div className="menu-heading">Time</div>
              {JUMP_HOURS.map((h) => (
                <button
                  key={h}
                  className="menu-item"
                  onClick={() => {
                    nav.current.toTime(startOfDay(day) + h * HOUR);
                    setOpen(false);
                  }}
                >
                  <span>{clockTime(startOfDay(day) + h * HOUR)}</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
      <div className="arrow-pair">
        <button className="icon-button" onClick={() => nav.current.jump(-1)} aria-label="Earlier">
          <ChevronLeft size={16} />
        </button>
        <button className="icon-button" onClick={() => nav.current.jump(1)} aria-label="Later">
          <ChevronRight size={16} />
        </button>
      </div>
      <button className="btn-plain" onClick={() => nav.current.toNow()}>
        <Clock size={14} /> Now
      </button>
    </>
  );
}

function startOfDay(offsetDays: number) {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + offsetDays);
  return Math.floor(d.getTime() / 1000);
}

/** Which day a moment falls on, counted from today (-1 is yesterday). */
function dayOffset(t: Timestamp) {
  const then = new Date(t * 1000);
  then.setHours(0, 0, 0, 0);
  return Math.round((then.getTime() / 1000 - startOfDay(0)) / 86400);
}

/** Finds programmes by title, from yesterday on, and jumps the guide to the one chosen. */
function GuideSearch({ onPick }: { onPick: (hit: ProgramHit) => void }) {
  const [text, setText] = useState("");
  const [hits, setHits] = useState<ProgramHit[] | null>(null);
  const [active, setActive] = useState(0);
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (text.trim().length < 2) {
      setHits(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .searchGuide(text, startOfDay(-1))
        .then((list) => {
          if (cancelled) return;
          setHits(list);
          setActive(0);
        })
        .catch(() => !cancelled && setHits([]));
    }, 200);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [text]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", onDown, true);
    return () => window.removeEventListener("mousedown", onDown, true);
  }, [open]);

  const pick = (hit: ProgramHit) => {
    onPick(hit);
    setOpen(false);
    setText("");
    setHits(null);
    (document.activeElement as HTMLElement | null)?.blur();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (text) setText("");
      else (e.target as HTMLInputElement).blur();
      setOpen(false);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setOpen(true);
      if (hits?.length) setActive((i) => (i + (e.key === "ArrowDown" ? 1 : -1) + hits.length) % hits.length);
    } else if (e.key === "Enter" && hits?.[active]) {
      e.preventDefault();
      pick(hits[active]);
    }
  };

  const t = nowSeconds();
  return (
    <div className="guide-search" ref={box}>
      <Search size={13} />
      <input
        className="text-input"
        placeholder="Find a programme"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={onKeyDown}
        spellCheck={false}
        autoComplete="off"
        aria-label="Find a programme in the guide"
      />
      {open && hits && (
        <div className="guide-search-results menu" role="listbox">
          {hits.length === 0 ? (
            <div className="guide-search-empty">No programmes found from yesterday on.</div>
          ) : (
            hits.map((hit, i) => {
              const { program, channel } = hit;
              const airing = program.start <= t && t < program.end;
              return (
                <button
                  key={program.id}
                  className={`guide-search-row ${i === active ? "active" : ""} ${program.end <= t ? "past" : ""}`}
                  role="option"
                  aria-selected={i === active}
                  onMouseEnter={() => setActive(i)}
                  onClick={() => pick(hit)}
                >
                  <ChannelLogo src={channel.logoUrl} name={channel.name} size={24} />
                  <span className="guide-search-text">
                    <span className="title">
                      {airing && <span className="live-dot" />}
                      {program.title}
                    </span>
                    <span className="meta">
                      {channel.name} · {dayName(program.start)} {clockTime(program.start)}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

interface GuideGridProps {
  channels: Channel[];
  selected: { channel: Channel; program: Program | null } | null;
  onChoose: (c: Channel, p: Program | null) => void;
  /** Highlights a programme without playing anything (keyboard moves). */
  onSelect: (c: Channel, p: Program | null) => void;
  day: number;
  setDay: (d: number) => void;
  nav: React.RefObject<GuideNav>;
  /** A programme to scroll to and select once its channel is listed. */
  target: ProgramHit | null;
  onTargetShown: () => void;
  /** The list being shown, when it's Favorites or a group. */
  list?: ChannelListContext;
}

function GuideGrid({ channels, selected, onChoose, onSelect, day, setDay, nav, target, onTargetShown, list: listContext }: GuideGridProps) {
  const settings = useApp((s) => s.settings);
  const recordings = useApp((s) => s.recordings);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const favorites = useApp((s) => s.favorites);
  const edits = useApp((s) => s.edits);
  const playing = useApp((s) => s.nowPlaying?.channel);
  const colored = settings.guideColors === "category";
  const dragProps = useListDrag(listContext);
  const scroller = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  const [now, setNow] = useState(nowSeconds());
  const [programs, setPrograms] = useState<Map<string, Program[]>>(new Map());
  const requested = useRef(new Set<string>());
  // Cells that start behind the channel column shift their titles into view
  // (as TiviMate does), so the grid tracks its horizontal scroll.
  const [scrollLeft, setScrollLeft] = useState(0);
  const scrollFrame = useRef(0);
  const onScroll = () => {
    cancelAnimationFrame(scrollFrame.current);
    scrollFrame.current = requestAnimationFrame(() => setScrollLeft(scroller.current?.scrollLeft ?? 0));
  };

  // The visible window spans the chosen day plus a little either side.
  const from = startOfDay(day) - 2 * HOUR;
  const to = startOfDay(day + 1) + 2 * HOUR;
  const pxPerHour = Math.max(180, (width - CHANNEL_COLUMN) / settings.guideHours);
  const px = (t: number) => ((t - from) / HOUR) * pxPerHour;
  const rowHeight = settings.guideRowHeight;

  useEffect(() => {
    const t = setInterval(() => setNow(nowSeconds()), 30000);
    return () => clearInterval(t);
  }, []);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setWidth(el.clientWidth));
    observer.observe(el);
    setWidth(el.clientWidth);
    return () => observer.disconnect();
  }, []);

  const virtualizer = useVirtualizer({
    count: channels.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => rowHeight,
    overscan: 8,
  });

  // Drop cached listings when the day or channel list changes.
  useEffect(() => {
    requested.current = new Set();
    setPrograms(new Map());
  }, [day, channels]);

  const items = virtualizer.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = items[items.length - 1]?.index ?? 0;

  // Load listings for the rows in view, in one batch. Rows count as requested
  // only once the request goes out: a scroll that cancels the wait mustn't
  // leave them looking loaded.
  useEffect(() => {
    const timer = setTimeout(() => {
      const ids = channels.slice(first, last + 1).map((c) => c.id).filter((id) => !requested.current.has(id));
      if (!ids.length) return;
      ids.forEach((id) => requested.current.add(id));
      api
        .guide(ids, from, to)
        .then((rows) =>
          setPrograms((prev) => {
            const next = new Map(prev);
            rows.forEach((r) => next.set(r.channel.id, r.programs));
            return next;
          }),
        )
        .catch((error) => {
          console.error("guide", error);
          ids.forEach((id) => requested.current.delete(id));
        });
    }, 60);
    return () => clearTimeout(timer);
  }, [first, last, channels, from, to]);

  const scrollToTime = useCallback(
    (t: number, smooth = false) => {
      scroller.current?.scrollTo({ left: Math.max(0, px(t) - pxPerHour / 2), behavior: smooth ? "smooth" : "auto" });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [from, pxPerHour],
  );

  // Open on "now", or the morning of another day.
  useLayoutEffect(() => {
    scrollToTime(day === 0 ? nowSeconds() : startOfDay(day) + 8 * HOUR);
  }, [day, scrollToTime]);

  // A programme from search: its row and time, selected, once the day and the
  // list with its channel are showing (after the opening scroll above).
  useEffect(() => {
    if (!target || dayOffset(target.program.start) !== day) return;
    const index = channels.findIndex((c) => sameChannel(c, target.channel));
    if (index < 0) return;
    virtualizer.scrollToIndex(index, { align: "center" });
    scrollToTime(target.program.start);
    onSelect(channels[index], target.program);
    scroller.current?.focus({ preventScroll: true });
    onTargetShown();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target, channels, day, scrollToTime]);

  nav.current = {
    jump: (direction) => scroller.current?.scrollBy({ left: direction * pxPerHour * Math.max(1, settings.guideHours - 1), behavior: "smooth" }),
    toNow: () => (day !== 0 ? setDay(0) : scrollToTime(nowSeconds(), true)),
    toTime: (t) => scrollToTime(t, true),
  };

  // Clicks focus the grid for the arrow keys without scrolling to the clicked button.
  const focusGrid = (e: React.MouseEvent) => {
    e.preventDefault();
    scroller.current?.focus({ preventScroll: true });
  };

  // Remote-style keys: arrows move through the grid keeping the time when
  // changing channel, Return watches, replays or records, R records.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!channels.length) return;
    const ci = Math.max(0, selected ? channels.findIndex((c) => sameChannel(c, selected.channel)) : 0);
    const programAt = (channel: Channel, t: number) => programs.get(channel.id)?.find((p) => p.start <= t && t < p.end) ?? null;
    const anchor = selected?.program ? Math.max(selected.program.start, Math.min(now, selected.program.end - 1)) : now;
    const choose = (channel: Channel, program: Program | null) => {
      onSelect(channel, program);
      const el = scroller.current;
      if (program && el) {
        const x = px(program.start);
        if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth - CHANNEL_COLUMN - 120) el.scrollTo({ left: Math.max(0, x - pxPerHour / 2), behavior: "smooth" });
      }
    };
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = Math.min(channels.length - 1, Math.max(0, ci + (e.key === "ArrowDown" ? 1 : -1)));
      virtualizer.scrollToIndex(next, { align: "auto" });
      choose(channels[next], programAt(channels[next], anchor));
    } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      const channel = channels[ci];
      const list = programs.get(channel.id) ?? [];
      const i = list.findIndex((p) => p.id === selected?.program?.id);
      const next = list[i < 0 ? 0 : Math.min(list.length - 1, Math.max(0, i + (e.key === "ArrowRight" ? 1 : -1)))];
      if (next) choose(channel, next);
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      const p = selected.program;
      if (!p || (p.start <= now && now < p.end)) onChoose(selected.channel, p);
      else if (p.start > now) recordProgram(p, selected.channel);
      else if (selected.channel.catchupDays > 0) playReplay(selected.channel, p);
    } else if ((e.key === "r" || e.key === "R") && selected?.program && selected.program.end > now) {
      e.preventDefault();
      recordProgram(selected.program, selected.channel);
    }
  };

  const totalWidth = CHANNEL_COLUMN + px(to);
  const ruler: number[] = [];
  for (let t = from; t < to; t += HOUR / 2) ruler.push(t);

  return (
    <div className="guide" ref={scroller} tabIndex={0} onKeyDown={onKeyDown} onScroll={onScroll} aria-label="Programme guide">
      <div style={{ width: totalWidth, height: virtualizer.getTotalSize() + 30, position: "relative" }}>
        <div className="guide-ruler" style={{ width: totalWidth }}>
          <div className="guide-corner">{dayName(startOfDay(day) + 12 * HOUR) === "Today" ? new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(new Date()) : dayName(startOfDay(day) + 12 * HOUR)}</div>
          {ruler.map((t) => (
            <div key={t} className={`guide-tick ${t % HOUR === 0 ? "hour" : ""}`} style={{ left: CHANNEL_COLUMN + px(t) }}>
              {clockTime(t)}
            </div>
          ))}
        </div>
        {now > from && now < to && <div className="guide-now" style={{ left: CHANNEL_COLUMN + px(now), height: virtualizer.getTotalSize() }} />}
        {items.map((row) => {
          const channel = channels[row.index];
          const list = programs.get(channel.id);
          const isPlaying = !!playing && sameChannel(playing, channel);
          return (
            <div key={channel.id} className={`guide-row ${isPlaying ? "playing" : ""} ${isHidden(edits, channel) ? "dimmed" : ""}`} style={{ top: 30 + row.start, height: rowHeight, width: totalWidth }}>
              <button
                className="guide-channel"
                style={{ width: CHANNEL_COLUMN }}
                // A row that can be dragged mustn't cancel the press that starts the drag.
                onMouseDown={listContext ? () => scroller.current?.focus({ preventScroll: true }) : focusGrid}
                onClick={() => onChoose(channel, list?.find((p) => p.start <= now && now < p.end) ?? null)}
                onContextMenu={(e) => openMenu(e, channelMenu(channel, listContext))}
                {...dragProps(channel)}
              >
                {settings.showChannelNumbers && <span className="num">{channel.number ?? ""}</span>}
                <ChannelLogo src={channel.logoUrl} name={channel.name} size={rowHeight > 52 ? 38 : 30} />
                <span className="text">
                  <span className="name">{channel.name}</span>
                  <span className="marks">
                    {isPlaying && <span className="playing-mark">▶</span>}
                    {isFavoriteChannel(favorites, channel) && <Star size={11} fill="#f5c84c" color="#f5c84c" />}
                    {channel.catchupDays > 0 && <RotateCcw size={11} />}
                    {isHidden(edits, channel) && <EyeOff size={11} aria-label="Hidden" />}
                    {edits.get(channel.id)?.locked && <Lock size={11} aria-label="Locked" />}
                    <VersionsBadge channel={channel} />
                  </span>
                </span>
              </button>
              <div className="guide-programs" style={{ left: CHANNEL_COLUMN }}>
                {list === undefined ? (
                  <div className="guide-cell loading" style={{ left: px(Math.max(from, now - HOUR)), width: pxPerHour * 3 }} />
                ) : list.length === 0 ? (
                  <div className="guide-cell empty" style={{ left: 0, width: px(to) }}>
                    <span className="title">{channel.name}</span>
                  </div>
                ) : (
                  list.map((p) => {
                    const left = Math.max(0, px(p.start));
                    const right = Math.min(px(to), px(p.end));
                    if (right - left < 2) return null;
                    const timing = p.end <= now ? "past" : p.start > now ? "future" : "live";
                    const rec = recordingFor(recordings, channel.id, p.start, p.id);
                    const isSelected = selected?.program?.id === p.id;
                    const replayable = timing === "past" && channel.catchupDays > 0 && now - p.start < channel.catchupDays * 86400;
                    return (
                      <button
                        key={p.id}
                        className={`guide-cell ${timing} ${isSelected ? "selected" : ""} ${genreClass(p.genre, colored)}`}
                        style={{ left, width: right - left - 3, paddingLeft: 10 + Math.max(0, Math.min(scrollLeft - left, right - left - 90)) }}
                        onMouseDown={focusGrid}
                        onClick={() => onChoose(channel, p)}
                        onDoubleClick={() => (timing === "live" ? playChannel(channel, "full") : timing === "future" ? recordProgram(p, channel) : replayable && playReplay(channel, p))}
                        onContextMenu={(e) => openMenu(e, programMenu(p, channel))}
                        title={`${p.title}\n${clockTime(p.start)} – ${clockTime(p.end)}${p.category ? ` · ${p.category}` : ""}`}
                      >
                        <span className="cell-text">
                        <span className="title">
                          {timing === "live" && <span className="live-dot" />}
                          {rec && (rec.status === "recording" || rec.status === "scheduled") && <span className={`rec-mark ${rec.status}`} aria-label="Recording" />}
                          {p.title}
                          {p.flags.isNew && <span className="new-tag">NEW</span>}
                          {watchlistIds.has(p.id) && <Bell size={11} className="bell" fill="currentColor" />}
                          {replayable && <RotateCcw size={11} className="replay" />}
                        </span>
                        {rowHeight > 52 && <span className="time">{clockTime(p.start)} – {clockTime(p.end)}</span>}
                        </span>
                        {timing === "live" && <span className="cell-progress" style={{ width: `${((now - p.start) / (p.end - p.start)) * 100}%` }} />}
                      </button>
                    );
                  })
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// MARK: Channel list

function ChannelList({ channels, selected, onChoose, list }: { channels: Channel[]; selected: Channel | null; onChoose: (c: Channel, p: Program | null) => void; list?: ChannelListContext }) {
  const scroller = useRef<HTMLDivElement>(null);
  const edits = useApp((s) => s.edits);
  const dragProps = useListDrag(list);
  const [nowNext, setNowNext] = useState<Map<string, Program[]>>(new Map());
  const virtualizer = useVirtualizer({ count: channels.length, getScrollElement: () => scroller.current, estimateSize: () => 64, overscan: 8 });
  const items = virtualizer.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = items[items.length - 1]?.index ?? 0;

  // A reloaded list may follow a guide change, so its listings are read again.
  useEffect(() => setNowNext(new Map()), [channels]);

  useEffect(() => {
    const ids = channels.slice(first, last + 1).map((c) => c.id).filter((id) => !nowNext.has(id));
    if (!ids.length) return;
    api.nowNext(ids).then((rows) =>
      setNowNext((prev) => {
        const next = new Map(prev);
        rows.forEach((r) => next.set(r.channel.id, r.programs));
        return next;
      }),
    );
  }, [first, last, channels, nowNext]);

  const t = nowSeconds();
  return (
    <div className="channel-list" ref={scroller}>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {items.map((row) => {
          const channel = channels[row.index];
          const [current, next] = nowNext.get(channel.id) ?? [];
          return (
            <button
              key={channel.id}
              className={`channel-list-row ${selected && sameChannel(selected, channel) ? "selected" : ""} ${isHidden(edits, channel) ? "dimmed" : ""}`}
              style={{ top: row.start }}
              onClick={() => onChoose(channel, current ?? null)}
              onContextMenu={(e) => openMenu(e, channelMenu(channel, list))}
              {...dragProps(channel)}
            >
              <span className="num">{channel.number ?? ""}</span>
              <ChannelLogo src={channel.logoUrl} name={channel.name} size={40} />
              <span className="name">
                {channel.name}
                <VersionsBadge channel={channel} />
                {editMarks(isHidden(edits, channel), edits.get(channel.id)?.locked)}
              </span>
              <span className="now">
                {current ? (
                  <>
                    <span className="title">{current.title}</span>
                    <span className="progress-track accent">
                      <div style={{ width: `${((t - current.start) / (current.end - current.start)) * 100}%` }} />
                    </span>
                  </>
                ) : (
                  <span className="muted">No listings</span>
                )}
              </span>
              <span className="next">{next ? `${clockTime(next.start)}  ${next.title}` : ""}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
