// Choosing the guide channel a channel shows, for channels whose own guide id
// finds nothing or the wrong listings. The choice wins over the channel's own
// id and name, and stays through every refresh, until Reset.

import { Check, Search } from "lucide-react";
import { useEffect, useState } from "react";
import { api, errorText } from "../lib/api";
import { useApp } from "../lib/store";
import type { Channel, GuideChannel, GuideMatch } from "../lib/types";
import { ChannelLogo } from "./Artwork";
import { Sheet } from "./Sheet";

export function GuideMatchSheet({ channel, onClose }: { channel: Channel; onClose: () => void }) {
  const [match, setMatch] = useState<GuideMatch | null>(null);
  const [text, setText] = useState(channel.name);
  const [found, setFound] = useState<GuideChannel[] | null>(null);
  const [busy, setBusy] = useState(false);
  const { showToast, guideChanged } = useApp.getState();

  useEffect(() => {
    api.guideMatch(channel.id).then(setMatch).catch(() => {});
  }, [channel.id]);

  // Searches as the name is typed, once typing pauses.
  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .guideChannels(text)
        .then((list) => !cancelled && setFound(list))
        .catch(() => !cancelled && setFound([]));
    }, 180);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [text]);

  const choose = async (choice: GuideChannel) => {
    setBusy(true);
    try {
      await api.matchGuide(channel.id, choice.guideId, choice.epgId);
      guideChanged();
      showToast(`${channel.name} now shows the guide for ${choice.name ?? choice.epgId}`);
      onClose();
    } catch (error) {
      showToast(errorText(error), "error");
      setBusy(false);
    }
  };

  const reset = async () => {
    setBusy(true);
    try {
      await api.resetGuideMatch(channel.id);
      guideChanged();
      showToast(`${channel.name} finds its guide by itself again`);
      onClose();
    } catch (error) {
      showToast(errorText(error), "error");
      setBusy(false);
    }
  };

  const isCurrent = (g: GuideChannel) => g.guideId === match?.current?.guideId && g.epgId === match.current.epgId;

  return (
    <Sheet
      title="Match Guide"
      subtitle={`Choose the guide channel ${channel.name} shows. Your choice stays when guides refresh.`}
      onClose={onClose}
      width={600}
      footer={
        <>
          {match?.chosen && (
            <button className="btn-plain" disabled={busy} onClick={reset} style={{ marginRight: "auto" }}>
              Reset
            </button>
          )}
          <button className="btn-plain" onClick={onClose}>
            Cancel
          </button>
        </>
      }
    >
      <div className="guide-match-current">
        <ChannelLogo src={channel.logoUrl} name={channel.name} size={34} />
        <div className="guide-match-text">
          <span className="guide-match-name">{channel.name}</span>
          <span className="muted small">
            {match == null
              ? "\u00a0"
              : match.current
                ? `${match.chosen ? "Chosen by you" : "Matched"}: ${guideChannelLabel(match.current)}`
                : "No guide has this channel yet."}
          </span>
        </div>
      </div>
      <label className="guide-match-search">
        <Search size={14} />
        <input className="text-input" autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder="Guide channel name or id" spellCheck={false} autoComplete="off" />
      </label>
      <div className="guide-match-list" role="listbox" aria-label="Guide channels">
        {found == null ? (
          <div className="spinner" style={{ margin: "14px auto" }} />
        ) : found.length === 0 ? (
          <p className="muted guide-match-empty">{text.trim() ? "No guide channel has that name or id. Try part of it, or the id the guide uses." : "The guides have no channels yet. Refresh your sources or add a guide in Sources."}</p>
        ) : (
          found.map((g) => (
            <button key={`${g.guideId}|${g.epgId}`} className={`copy-row guide-match-row ${isCurrent(g) ? "current" : ""}`} disabled={busy} onClick={() => choose(g)} role="option" aria-selected={isCurrent(g)}>
              <span className="copy-text">
                <span className="copy-name">{g.name ?? g.epgId}</span>
                <span className="copy-meta">{[g.epgId, g.guideName, g.nowTitle ? `Now: ${g.nowTitle}` : null].filter(Boolean).join(" · ")}</span>
              </span>
              {isCurrent(g) && <Check size={15} className="guide-match-check" />}
            </button>
          ))
        )}
      </div>
    </Sheet>
  );
}

/** "Harbor News (harbor.uk) from My Provider". */
function guideChannelLabel(g: GuideChannel) {
  return `${g.name ?? g.epgId}${g.name ? ` (${g.epgId})` : ""} from ${g.guideName}`;
}
