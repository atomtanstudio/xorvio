// Checks Multiview's rules: `node src/player/multiviewRules.test.ts` (Node
// 23.6 or later runs TypeScript as it is, so nothing needs installing). It
// stops at the first rule broken, with what was expected.

import { afterRemove, fitGrid, focusAfterAdd, focusAfterRemove, layoutFor, listenTo, moveTile, stepAudio, tileForKey, tileToKeep, toggleExpanded, watchingFrom } from "./multiviewRules.ts";

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

console.log(`Multiview rules: ${checks} checks passed`);
