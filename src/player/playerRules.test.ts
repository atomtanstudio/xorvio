// Checks the player's rules: `node src/player/playerRules.test.ts` (Node
// 23.6 or later runs TypeScript as it is, so nothing needs installing). It
// stops at the first rule broken, with what was expected.

import {
  audioFor,
  channelForNumber,
  classifyFailure,
  isRecoverable,
  moveHighlight,
  nextAttempt,
  normalizedLanguage,
  parseCorner,
  reconnectDelays,
  reconnectStatus,
  resumePoint,
  snapCorner,
  STEADY_MS,
  stepChannel,
  subtitlesFor,
  typeDigit,
  withAudio,
  withSubtitles,
  zapDelay,
  RAPID_SETTLE_MS,
  ZAP_SETTLE_MS,
  type FailureContext,
} from "./playerRules.ts";

let checks = 0;
function check(name: string, actual: unknown, expected: unknown) {
  checks += 1;
  const got = JSON.stringify(actual);
  const wanted = JSON.stringify(expected);
  if (got !== wanted) throw new Error(`${name}: expected ${wanted}, got ${got}`);
}

// Failures, by kind
const live: FailureContext = { live: true, online: true, hasPlayed: true, mode: "mpegts" };
const film: FailureContext = { live: false, online: true, hasPlayed: false, mode: "native" };
const kind = (problem: Parameters<typeof classifyFailure>[0], context = film) => classifyFailure(problem, context).kind;
check("no connection at all is offline, whatever else went wrong", kind({ status: 403 }, { ...film, online: false }), "offline");
check("403 is the provider refusing the account", kind({ status: 403 }), "signIn");
check("403 says why it might have", classifyFailure({ status: 403 }, film).message.includes("connection limit"), true);
check("401 asks for the sign-in", kind({ status: 401 }), "signIn");
check("the backend's words about a login", kind({ message: "The provider rejected the username or password." }), "signIn");
check("missing details are a sign-in problem", kind({ message: "This source is missing a server address, username and password. Edit the source to add it." }), "signIn");
check("404 is not found", kind({ status: 404 }), "notFound");
check("410 is not found too", kind({ status: 410 }), "notFound");
check("a channel that's gone from the sources", kind({ message: "That channel is no longer in your sources." }), "notFound");
check("not found suggests a refresh", classifyFailure({ status: 404 }, film).suggestion, "Refresh the source in Sources to update its catalogue.");
check("a known status beats the words", kind({ status: 502, message: "The provider rejected the username or password." }), "network");
check("502 is the stream out of reach", classifyFailure({ status: 502 }, film).message, "The stream couldn't be reached.");
check("504 is the provider being slow", classifyFailure({ status: 504 }, film).message, "The provider took too long to answer.");
check("a media network error", kind({ mediaError: 2 }), "network");
check("mpegts.js losing the connection", kind({ message: "NetworkError: EarlyEof" }), "network");
check("an engine's code isn't shown", classifyFailure({ message: "NetworkError: Exception" }, film).message, "The connection to the stream was lost.");
check("a format the web view can't play", kind({ mediaError: 4 }), "unsupported");
check("a codec mpegts.js can't play", kind({ message: "MediaError: CodecUnsupported" }), "unsupported");
check("no ffmpeg is unsupported", kind({ message: "This video needs ffmpeg to play, and Xorvio couldn't find it." }), "unsupported");
check("an MPEG-TS channel that won't play suggests HLS", classifyFailure({ mediaError: 4 }, { ...live, hasPlayed: false }).suggestion.includes("live format to HLS"), true);
check("a film that won't play suggests another copy", classifyFailure({ mediaError: 4 }, film).suggestion.includes("another copy"), true);
check("a decode error once it's played is a stream that broke off", kind({ mediaError: 3 }, live), "ended");
check("a live channel ending is the stream ending", kind({ ended: true }, live), "ended");
check("never starting is a stall", kind({ stalled: true }, { ...live, hasPlayed: false }), "stalled");
check("a stall before it played says it didn't start", classifyFailure({ stalled: true }, { ...live, hasPlayed: false }).title, "The stream didn't start");
check("a stall once it played says it stopped", classifyFailure({ stalled: true }, live).title, "The stream stopped");
check("anything else", kind({ message: "Something odd" }), "unknown");
check("anything else says what it was", classifyFailure({ message: "Something odd" }, film).message, "Something odd");

// Worth trying again
check("network trouble always is", isRecoverable("network", false, false), true);
check("a format problem never is", isRecoverable("unsupported", true, true), false);
check("a refused live channel that played is", isRecoverable("signIn", true, true), true);
check("a refused live channel that never played isn't", isRecoverable("signIn", true, false), false);
check("a refused film isn't", isRecoverable("signIn", false, true), false);
check("a film that's gone isn't", isRecoverable("notFound", false, true), false);
check("a stream that never started isn't", isRecoverable("stalled", true, false), false);
check("a stream that stopped arriving is", isRecoverable("stalled", true, true), true);

// Reconnecting
const sum = (list: number[]) => list.reduce((a, b) => a + b, 0);
check("a live channel that played keeps trying for about two minutes", sum(reconnectDelays(true, true)) >= 100 && sum(reconnectDelays(true, true)) <= 130, true);
check("each wait is at least as long as the one before", reconnectDelays(true, true).every((d, i, all) => i === 0 || d >= all[i - 1]), true);
check("a live channel that never played gives up sooner", reconnectDelays(true, false), [2, 4]);
check("a film tries three times", reconnectDelays(false, true), [1, 2, 4]);
check("the first attempt", nextAttempt({ made: 0, playingSince: null }, 0, true, true), { number: 1, of: 8, delay: 2 });
check("the third attempt waits longer", nextAttempt({ made: 2, playingSince: null }, 0, true, true), { number: 3, of: 8, delay: 5 });
check("attempts run out", nextAttempt({ made: 8, playingSince: null }, 0, true, true), null);
check("playing steadily starts the count again", nextAttempt({ made: 8, playingSince: 1000 }, 1000 + STEADY_MS + 1, true, true), { number: 1, of: 8, delay: 2 });
check("playing for a moment doesn't", nextAttempt({ made: 3, playingSince: 1000 }, 5000, true, true)?.number, 4);
check("a film carries on from where it dropped", resumePoint(false, null, 754.2), 754.2);
check("…through attempts that failed", resumePoint(false, 754.2, 3), 754.2);
check("a live channel carries on from live", resumePoint(true, null, 754.2), null);
check("the card counts down", reconnectStatus(2, 8, 2.2), "The stream dropped. Trying again in 3 s. Attempt 2 of 8.");
check("…and says when it's trying", reconnectStatus(2, 8, null), "The stream dropped. Opening it again… Attempt 2 of 8.");

// Channel numbers
check("digits gather", typeDigit(typeDigit("", "1"), "2"), "12");
check("a sixth digit starts again", typeDigit("12345", "6"), "6");
check("other keys leave the number", typeDigit("12", "a"), "12");
const channels = [
  { id: "a", number: 1, sourceId: "x" },
  { id: "b", number: 12, sourceId: "x" },
  { id: "c", number: 12, sourceId: "y" },
  { id: "d", number: null, sourceId: "y" },
];
check("a number finds its channel", channelForNumber("1", channels)?.id, "a");
check("leading zeros don't matter", channelForNumber("001", channels)?.id, "a");
check("the playing source's channel wins a tie", channelForNumber("12", channels, "y")?.id, "c");
check("otherwise the first in the list", channelForNumber("12", channels, "z")?.id, "b");
check("no such channel", channelForNumber("512", channels), null);
check("nothing typed", channelForNumber("", channels), null);

// Zapping
check("a first press waits a moment", zapDelay(null), ZAP_SETTLE_MS);
check("quick presses wait longer, so passed channels never open", zapDelay(250), RAPID_SETTLE_MS);
check("a press after a pause is a first press again", zapDelay(5000), ZAP_SETTLE_MS);
check("passed channels are skipped: the delay outlasts the gap between quick presses", zapDelay(250) > 250 && zapDelay(null) > 250, true);
check("up from the last goes round to the first", stepChannel(2, 1, 3), 0);
check("down from the first goes round to the last", stepChannel(0, -1, 3), 2);
check("a channel not in the list starts at its top", stepChannel(-1, 1, 3), 0);
check("…or its bottom going down", stepChannel(-1, -1, 3), 2);
check("an empty list has nowhere to go", stepChannel(0, 1, 0), -1);
check("the list's highlight stops at the ends", [moveHighlight(0, -1, 5), moveHighlight(4, 1, 5), moveHighlight(1, 10, 5)], [0, 4, 4]);

// The mini player's corner
const area = { width: 1200, height: 800 };
check("a remembered corner", parseCorner("topLeft"), "topLeft");
check("anything else is the bottom right", parseCorner("middle"), "bottomRight");
check("dropped in the top left quarter", snapCorner({ x: 200, y: 150 }, { x: 0, y: 0 }, area), "topLeft");
check("dropped in the bottom right quarter", snapCorner({ x: 1000, y: 700 }, { x: 0, y: 0 }, area), "bottomRight");
check("a flick carries it on", snapCorner({ x: 560, y: 700 }, { x: 1, y: 0 }, area), "bottomRight");
check("a flick upwards", snapCorner({ x: 1000, y: 450 }, { x: 0, y: -1 }, area), "topRight");

// Audio and subtitles for a series
check("codes become ISO 639-2", [normalizedLanguage("en"), normalizedLanguage("en-GB"), normalizedLanguage("FRA"), normalizedLanguage("spa"), normalizedLanguage("und"), normalizedLanguage(null)], ["eng", "eng", "fre", "spa", null, null]);
check("an audio pick remembers its language", withAudio(null, "es"), { audioLanguage: "spa" });
check("an audio track without a language changes nothing", withAudio({ audioLanguage: "spa" }, null), { audioLanguage: "spa" });
check("subtitles off are remembered", withSubtitles({ audioLanguage: "spa" }, null), { audioLanguage: "spa", subtitles: { kind: "off" } });
check("a subtitle track is remembered by language and name", withSubtitles(null, { language: "en", label: "English (SDH)" }), { subtitles: { kind: "track", language: "eng", label: "English (SDH)" } });
const audio = [
  { id: 1, language: "eng" },
  { id: 2, language: "spa" },
];
check("the remembered language is played", audioFor({ audioLanguage: "spa" }, audio, 1), 2);
check("already in it: nothing changes", audioFor({ audioLanguage: "spa" }, audio, 2), null);
check("an episode without it plays its own", audioFor({ audioLanguage: "ger" }, audio, 1), null);
check("nothing remembered: nothing changes", audioFor(null, audio, 1), null);
check("a web view's two-letter codes match", audioFor({ audioLanguage: "spa" }, [{ id: 0, language: "en" }, { id: 1, language: "es" }], 0), 1);
const subs = [
  { key: "stream:2", label: "English (Forced)", language: "eng", picture: null },
  { key: "stream:3", label: "English", language: "eng", picture: null },
  { key: "stream:4", label: "English (SDH)", language: "eng", picture: null },
  { key: "stream:5", label: "Spanish", language: "spa", picture: 5 },
  { key: "addon:x", label: "French", language: "fre", picture: null },
];
check("off stays off", subtitlesFor({ subtitles: { kind: "off" } }, subs), "off");
check("the same name in the same language", subtitlesFor({ subtitles: { kind: "track", language: "eng", label: "English (SDH)" } }, subs), "stream:4");
check("the same language, not forced, when the name differs", subtitlesFor({ subtitles: { kind: "track", language: "eng", label: "English [CC]" } }, subs), "stream:3");
check("an add-on's file in the language", subtitlesFor({ subtitles: { kind: "track", language: "fre", label: "French 2" } }, subs), "addon:x");
check("picture subtitles aren't drawn in by themselves", subtitlesFor({ subtitles: { kind: "track", language: "spa", label: "Spanish" } }, subs), null);
check("nothing in the language yet", subtitlesFor({ subtitles: { kind: "track", language: "ger", label: "German" } }, subs), null);
check("nothing remembered leaves it to the settings", subtitlesFor({ audioLanguage: "spa" }, subs), null);

console.log(`Player rules: ${checks} checks passed`);
