// Checks Multiview's rules: `node src/player/multiviewRules.test.ts` (Node
// 23.6 or later runs TypeScript as it is, so nothing needs installing). It
// stops at the first rule broken, with what was expected.

import {
  afterRemove,
  allPaused,
  buffersOnPause,
  canInset,
  fitGrid,
  focusAfterAdd,
  focusAfterRemove,
  hideChanges,
  insetChanges,
  layoutFor,
  listenTo,
  moveTile,
  resumeAt,
  secondsBehind,
  skipInBuffer,
  stepAudio,
  tileForKey,
  tileToKeep,
  toggleExpanded,
  usesConnection,
  watchingFrom,
} from "./multiviewRules.ts";
import { bufferRange } from "./playerRules.ts";

let checks = 0;
function check(name: string, actual: unknown, expected: unknown) {
  checks += 1;
  const got = JSON.stringify(actual);
  const wanted = JSON.stringify(expected);
  if (got !== wanted) throw new Error(`${name}: expected ${wanted}, got ${got}`);
}

// Layouts
const places = (ids: string[], expanded: string | null = null) => layoutFor(ids, expanded).places.map((p) => [p.id, p.column, p.row, p.span]);
check("a lone tile asks for another beside it", layoutFor(["a"], null).invite, { column: 2, row: 1 });
check("two sit side by side", places(["a", "b"]), [["a", 1, 1, 1], ["b", 2, 1, 1]]);
check("two leave no room to invite", layoutFor(["a", "b"], null).invite, null);
check("three: the first large, two stacked beside it", places(["a", "b", "c"]), [["a", 1, 1, 2], ["b", 3, 1, 1], ["c", 3, 2, 1]]);
check("three use a 3×2 grid", [layoutFor(["a", "b", "c"], null).columns, layoutFor(["a", "b", "c"], null).rows], [3, 2]);
check("four make a 2×2 grid", places(["a", "b", "c", "d"]), [["a", 1, 1, 1], ["b", 2, 1, 1], ["c", 1, 2, 1], ["d", 2, 2, 1]]);
check("an expanded tile fills the view", places(["a", "b", "c"], "b"), [["b", 1, 1, 1]]);
check("the others keep playing out of sight", layoutFor(["a", "b", "c"], "b").hidden, ["a", "c"]);
check("expanding a tile that's gone shows the grid", places(["a", "b"], "z"), [["a", 1, 1, 1], ["b", 2, 1, 1]]);

// Fitting 16:9 cells
check("a 2×2 grid fills a 16:9 space", fitGrid({ width: 1600, height: 900 }, { columns: 2, rows: 2 }, 0), { width: 1600, height: 900 });
check("side by side letterboxes top and bottom", fitGrid({ width: 1600, height: 900 }, { columns: 2, rows: 1 }, 0), { width: 1600, height: 450 });
check("three fit by width in a wide space", fitGrid({ width: 1200, height: 900 }, { columns: 3, rows: 2 }, 0), { width: 1200, height: 450 });
check("a tall space letterboxes at the sides", fitGrid({ width: 1600, height: 450 }, { columns: 2, rows: 2 }, 0), { width: 800, height: 450 });
check("gaps come out of the space", fitGrid({ width: 1610, height: 2000 }, { columns: 2, rows: 1 }, 10), { width: 1610, height: 450 });
check("no space, no grid", fitGrid({ width: 0, height: 0 }, { columns: 2, rows: 2 }, 6), { width: 0, height: 0 });

// Who's heard
check("the first tile is heard", focusAfterAdd({ audio: null, expanded: null }, "a"), { audio: "a", expanded: null });
check("adding keeps the sound where it is", focusAfterAdd({ audio: "a", expanded: null }, "b"), { audio: "a", expanded: null });
check("adding shows the grid again", focusAfterAdd({ audio: "a", expanded: "a" }, "b"), { audio: "a", expanded: null });
check("removing the heard tile moves the sound to the first left", focusAfterRemove(["a", "b", "c"], { audio: "a", expanded: null }, "a"), { audio: "b", expanded: null });
check("removing another keeps the sound", focusAfterRemove(["a", "b", "c"], { audio: "c", expanded: null }, "a"), { audio: "c", expanded: null });
check("removing the expanded tile returns to the grid", focusAfterRemove(["a", "b", "c"], { audio: "b", expanded: "b" }, "b"), { audio: "a", expanded: null });
check("listening moves the sound", listenTo({ audio: "a", expanded: null }, "b"), { audio: "b", expanded: null });
check("listening while expanded brings that tile to the front", listenTo({ audio: "a", expanded: "a" }, "b"), { audio: "b", expanded: "b" });
check("expanding a tile makes it heard", toggleExpanded({ audio: "a", expanded: null }, "b"), { audio: "b", expanded: "b" });
check("expanding the expanded tile returns to the grid, sound and all", toggleExpanded({ audio: "b", expanded: "b" }, "b"), { audio: "b", expanded: null });

// Keys
check("1 is the first tile", tileForKey(["a", "b"], "1"), "a");
check("a key past the tiles picks nothing", tileForKey(["a", "b"], "3"), null);
check("only 1–4 pick", tileForKey(["a", "b", "c", "d"], "5"), null);
check("next goes round", stepAudio(["a", "b", "c"], "c", 1), "a");
check("previous goes round", stepAudio(["a", "b", "c"], "a", -1), "c");
check("with nothing heard, next is the first", stepAudio(["a", "b"], null, 1), "a");
check("no tiles, no next", stepAudio([], null, 1), null);

// Order
const tiles = [{ id: "a" }, { id: "b" }, { id: "c" }];
check("moving later swaps with the next", moveTile(tiles, "a", 1).map((t) => t.id), ["b", "a", "c"]);
check("moving earlier swaps with the one before", moveTile(tiles, "c", -1).map((t) => t.id), ["a", "c", "b"]);
check("the ends stay put", moveTile(tiles, "a", -1).map((t) => t.id), ["a", "b", "c"]);

// Leaving
check("two left stay in Multiview", afterRemove(2), "stay");
check("one left goes back to the player", afterRemove(1), "player");
check("none left stops", afterRemove(0), "stop");
check("closing keeps the tile being heard", tileToKeep(["a", "b"], "b"), "b");
check("closing with nothing heard keeps the first", tileToKeep(["a", "b"], null), "a");
check("nothing to keep", tileToKeep([], null), null);

// Connections
const sources = [
  { id: "a", sourceId: "x" },
  { id: "b", sourceId: "x" },
  { id: "c", sourceId: "y" },
];
check("counts the source's other tiles", watchingFrom(sources, "x"), 2);
check("a tile being swapped doesn't count", watchingFrom(sources, "x", "a"), 1);
check("other sources don't count", watchingFrom(sources, "z"), 0);

// Pausing and rewinding
const direct = { isLive: true, timeshift: null };
const buffered = { isLive: true, timeshift: { windowSeconds: 1800 } };
check("a paused tile moves onto a buffer", buffersOnPause("paused", direct), true);
check("whatever the player's setting", buffersOnPause("always", direct), true);
check("unless the live buffer is off", buffersOnPause("off", direct), false);
check("a tile on a buffer stays on it", buffersOnPause("paused", buffered), false);
check("nothing to buffer before the stream arrives", buffersOnPause("paused", null), false);
check("a recording or replay isn't live", buffersOnPause("paused", { isLive: false, timeshift: null }), false);

const range = { start: 100, end: 400 };
check("behind live, to the second", secondsBehind(range, 309.6), 90);
check("at the edge", secondsBehind(range, 400.4), 0);
check("no buffer, not behind", secondsBehind(null, 50), 0);
check("back ten seconds", skipInBuffer(range, 300, -10), 290);
check("back a minute, no further than the buffer keeps", skipInBuffer(range, 130, -60), 100);
check("forward, short of the very end", skipInBuffer(range, 395, 10), 399);
check("carries on where it was", resumeAt(range, 250), 250);
check("paused longer than the buffer keeps: its oldest part", resumeAt(range, 40), 101);
check("no range yet", resumeAt(null, 40), 40);

const fakeVideo = (ranges: [number, number][]) => ({ seekable: { length: ranges.length, start: (i: number) => ranges[i][0], end: (i: number) => ranges[i][1] } });
check("a buffer's range", bufferRange(fakeVideo([[3, 60]])), { start: 3, end: 60 });
check("no range until the playlist loads", bufferRange(fakeVideo([])), null);
check("an endless one isn't a range", bufferRange(fakeVideo([[0, Infinity]])), null);

check("all paused", allPaused([{ paused: true, error: null }, { paused: true, error: null }]), true);
check("one playing", allPaused([{ paused: true, error: null }, { paused: false, error: null }]), false);
check("a failed tile doesn't count", allPaused([{ paused: true, error: null }, { paused: false, error: "offline" }]), true);
check("none to pause", allPaused([]), false);

// Connections
check("a provider's film takes a connection", usesConnection("xtream", false), true);
check("so does its channel", usesConnection("xtream", true), true);
check("a playlist's channel too", usesConnection("m3uUrl", true), true);
check("a media server's film doesn't take a tuner", usesConnection("jellyfin", false), false);
check("its channel does", usesConnection("plex", true), true);
check("a media folder's video takes nothing", usesConnection("folder", false), false);
check("a source not known yet counts", usesConnection(null, false), true);

// The inset
check("the inset is for two", [canInset(1), canInset(2), canInset(3)], [false, true, false]);
const inset = layoutFor(["a", "b"], null, true);
check("the first fills the view", inset.places.map((p) => [p.id, p.column, p.row, p.span]), [["a", 1, 1, 1]]);
check("the second is the inset", inset.inset, "b");
check("in one 16:9 cell", [inset.columns, inset.rows], [1, 1]);
check("nothing out of sight or invited", [inset.hidden, inset.invite], [[], null]);
check("three tiles make the grid, inset or not", layoutFor(["a", "b", "c"], null, true).inset, null);
check("the inset wins over an expanded tile", layoutFor(["a", "b"], "b", true).places.map((p) => p.id), ["a"]);
check("without it, no inset", layoutFor(["a", "b"], null).inset, null);

const tile = (id: string, live: boolean, paused = false, insetPaused = false) => ({ id, live, paused, insetPaused });
check("the film going small pauses", insetChanges(tile("game", true), tile("film", false)), [{ id: "film", paused: true, insetPaused: true }]);
check("the game going small plays on", insetChanges(tile("film", false), tile("game", true)), []);
check("the film the inset paused plays when it's big", insetChanges(tile("film", false, true, true), tile("game", true)), [{ id: "film", paused: false, insetPaused: false }]);
check("one someone paused stays paused when it's big", insetChanges(tile("film", false, true), tile("game", true)), []);
check("a film already paused going small isn't the inset's to play later", insetChanges(tile("game", true), tile("film", false, true)), []);
check("two films: the big one plays, the small one waits", insetChanges(tile("a", false, true, true), tile("b", false)), [
  { id: "a", paused: false, insetPaused: false },
  { id: "b", paused: true, insetPaused: true },
]);
check("a channel paused into its buffer stays so when it's big", insetChanges(tile("game", true, true), tile("film", false, true, true)), []);
check("hiding a film playing small pauses it", hideChanges(tile("film", false)), [{ id: "film", paused: true, insetPaused: true }]);
check("hiding a channel lets it play on", hideChanges(tile("game", true)), []);
check("hiding a paused film changes nothing", hideChanges(tile("film", false, true)), []);

console.log(`Multiview rules: ${checks} checks passed`);
