// A title's trailer, faded in behind its film or series page as Netflix
// does: YouTube's player on a page the media gateway serves (YouTube's
// player won't play in the app's own pages), muted until sound is turned
// on, and enlarged past YouTube's title and logo. It starts loading as the
// page opens and shows the moment YouTube plays, never plays while anything
// else does, and fades back to the still backdrop just before the end, so
// YouTube's suggestions never show. Settings › Profiles and Home turns it
// off, and so does the Mac's Reduce Motion.

import { RotateCcw, Volume2, VolumeX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import { useMultiview } from "../player/multiviewStore";

/** What a YouTube video id looks like (providers and add-ons pass on whatever they're given). */
const YOUTUBE_ID = /^[\w-]{6,20}$/;
/** After YouTube says it's playing, so its first frame is drawn before the fade. */
const SHOW_AFTER = 150;
/** Seconds before the end it fades away, before YouTube's suggestions. */
const END_BEFORE = 2.5;
/** A trailer that hasn't started by then isn't going to (blocked, or YouTube unreachable). */
const GIVE_UP_AFTER = 15000;
/** As long as the fade in styles/screens.css. */
const FADE = 700;

/** Sound turned on stays on for the session, as on Netflix. */
let soundOn = false;

type Phase = "waiting" | "playing" | "ended" | "failed";

const prefersStill = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

export function TitleTrailer({ youtubeId }: { youtubeId?: string | null }) {
  const enabled = useApp((s) => s.settings.titleTrailers);
  const busy = useApp((s) => !!s.nowPlaying || s.poppedOut);
  const multiview = useMultiview((s) => s.open);
  const valid = !!youtubeId && YOUTUBE_ID.test(youtubeId);
  const idle = enabled && valid && !busy && !multiview && !prefersStill();

  const frame = useRef<HTMLIFrameElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("waiting");
  const [visible, setVisible] = useState(false);
  const [sound, setSound] = useState(soundOn);

  const send = (command: "mute" | "unmute" | "stop") => frame.current?.contentWindow?.postMessage({ xorvioTrailer: true, command }, "*");
  const replay = () =>
    api
      .trailerPage(youtubeId!)
      .then((url) => {
        setSrc(url);
        setPhase("playing");
      })
      .catch(() => setPhase("failed"));

  // Another title: start over.
  useEffect(() => {
    setSrc(null);
    setVisible(false);
    setPhase("waiting");
  }, [youtubeId]);

  // Starts as soon as the page knows its trailer; stops the moment anything else plays (and starts again after).
  useEffect(() => {
    if (!idle) {
      setSrc(null);
      setVisible(false);
      setPhase((p) => (p === "playing" ? "waiting" : p));
      return;
    }
    if (phase !== "waiting" || document.visibilityState !== "visible") return;
    let cancelled = false;
    api
      .trailerPage(youtubeId!)
      .then((url) => {
        if (cancelled) return;
        setSrc(url);
        setPhase("playing");
      })
      .catch(() => !cancelled && setPhase("failed"));
    return () => {
      cancelled = true;
    };
  }, [idle, phase, youtubeId]);

  // What the player says: shown once it plays, gone near the end or on an error.
  useEffect(() => {
    if (!src) return;
    const timers: ReturnType<typeof setTimeout>[] = [];
    let shown = false;
    let over = false;
    const end = (next: Phase) => {
      if (over) return;
      over = true;
      setVisible(false);
      setPhase(next);
      timers.push(setTimeout(() => setSrc(null), FADE));
    };
    const giveUp = setTimeout(() => end("failed"), GIVE_UP_AFTER);
    const heard = (event: MessageEvent) => {
      const message = event.data;
      if (event.source !== frame.current?.contentWindow || !message?.xorvioTrailer) return;
      if (message.type === "state" && message.value === 1 && !shown) {
        shown = true;
        clearTimeout(giveUp);
        if (soundOn) send("unmute");
        timers.push(setTimeout(() => !over && setVisible(true), SHOW_AFTER));
      } else if (message.type === "state" && message.value === 0) {
        end("ended");
      } else if (message.type === "time" && message.value?.d > 0 && message.value.d - message.value.t < END_BEFORE) {
        send("stop");
        end("ended");
      } else if (message.type === "error") {
        end("failed");
      }
    };
    window.addEventListener("message", heard);
    return () => {
      window.removeEventListener("message", heard);
      clearTimeout(giveUp);
      timers.forEach(clearTimeout);
    };
  }, [src]);

  const toggleSound = () => {
    soundOn = !soundOn;
    setSound(soundOn);
    send(soundOn ? "unmute" : "mute");
  };

  return (
    <>
      {src && (
        <div className={`title-trailer ${visible ? "visible" : ""}`} aria-hidden="true">
          <iframe ref={frame} src={src} title="Trailer" allow="autoplay; encrypted-media" tabIndex={-1} />
        </div>
      )}
      {visible && (
        <button className="title-trailer-button" onClick={toggleSound} aria-label={sound ? "Mute the trailer" : "Play the trailer's sound"} title={sound ? "Mute" : "Sound on"}>
          {sound ? <Volume2 size={18} /> : <VolumeX size={18} />}
        </button>
      )}
      {phase === "ended" && !src && idle && (
        <button className="title-trailer-button" onClick={replay} aria-label="Play the trailer again" title="Play the trailer again">
          <RotateCcw size={17} />
        </button>
      )}
    </>
  );
}
