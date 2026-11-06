//! Keeping a remuxed film a little ahead of the player, rather than the
//! whole film ahead. ffmpeg copies a film many times faster than it plays,
//! and the web view's Media Source holds only a couple of hundred megabytes
//! of it: mpegts.js, told it's full, drops the connection and never asks
//! again, so the film stops a few minutes in. So the gateway reads ffmpeg's
//! output only while it's less than a minute (and 64 MB) ahead of where the
//! player says it is; ffmpeg waits meanwhile, and its connection to the
//! provider with it.

use std::collections::VecDeque;

/// How far ahead of the player a remuxed stream runs, in seconds of video…
pub(crate) const AHEAD_SECONDS: f64 = 60.0;
/// …and in bytes, which is the limit for high-bitrate films (64 MB is
/// 13 seconds of a 40 Mbps Blu-ray remux).
pub(crate) const AHEAD_BYTES: u64 = 64 * 1024 * 1024;
/// A position reported this far beyond what's been written can't be this
/// stream's (a player can't be ahead of what it was sent): it's from an
/// earlier player of the same address, whose ffmpeg started again.
const SLACK: f64 = 5.0;
/// Seconds of video between the marks kept for the bytes ahead.
const MARK_EVERY: f64 = 0.5;

const PACKET: usize = 188;
const SYNC: u8 = 0x47;
/// MPEG timestamps count 90 kHz ticks in 33 bits, then start again from 0.
const WRAP: i64 = 1 << 33;
const TICKS: f64 = 90_000.0;

/// How a remuxed stream is paced: what ffmpeg has written so far, and
/// where the player last said it was.
#[derive(Debug, Default)]
pub(crate) struct Pacer {
    clock: OutputClock,
    /// Seconds into the stream (from its start offset, as the player counts).
    position: f64,
}

impl Pacer {
    /// Takes note of output on its way to the player.
    pub fn wrote(&mut self, chunk: &[u8]) {
        self.clock.feed(chunk);
    }

    /// The player's position, as it reported it.
    pub fn heard(&mut self, reported: f64) {
        let written = self.clock.seconds().unwrap_or(0.0);
        if reported.is_finite() && reported >= 0.0 && reported <= written + SLACK {
            self.position = reported;
        }
    }

    /// Whether the output is far enough ahead of the player to wait for it.
    /// Output with no timestamps in it is never held.
    pub fn is_ahead(&mut self) -> bool {
        let Some(written) = self.clock.seconds() else {
            return false;
        };
        written - self.position >= AHEAD_SECONDS
            || self.clock.bytes - self.clock.bytes_by(self.position) >= AHEAD_BYTES
    }
}

/// How much video ffmpeg has written, from the timestamps in its MPEG-TS,
/// and how many bytes it took.
#[derive(Debug, Default)]
struct OutputClock {
    /// The start of a packet split between two reads.
    partial: Vec<u8>,
    /// The earliest and latest timestamps, and the last one read, unwrapped.
    first: Option<i64>,
    latest: Option<i64>,
    last: Option<i64>,
    /// What's added to a timestamp once it has wrapped.
    epoch: i64,
    bytes: u64,
    /// (seconds of video, bytes written by then), oldest first.
    marks: VecDeque<(f64, u64)>,
}

impl OutputClock {
    fn feed(&mut self, mut data: &[u8]) {
        self.bytes += data.len() as u64;
        if !self.partial.is_empty() {
            let take = (PACKET - self.partial.len()).min(data.len());
            self.partial.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.partial.len() < PACKET {
                return;
            }
            let packet = std::mem::take(&mut self.partial);
            self.packet(&packet);
        }
        while data.len() >= PACKET {
            if data[0] != SYNC {
                // ffmpeg's packets are aligned from the first byte; this is
                // only in case they ever aren't.
                match data.iter().position(|&b| b == SYNC) {
                    Some(at) => data = &data[at..],
                    None => return,
                }
                continue;
            }
            self.packet(&data[..PACKET]);
            data = &data[PACKET..];
        }
        self.partial.extend_from_slice(data);
        if let Some(seconds) = self.seconds()
            && self.marks.back().is_none_or(|&(at, _)| seconds >= at + MARK_EVERY)
        {
            self.marks.push_back((seconds, self.bytes));
        }
    }

    /// The presentation timestamp at the start of a PES packet, if this
    /// transport packet starts one that has it.
    fn packet(&mut self, packet: &[u8]) {
        // A packet that starts a payload, and has one.
        if packet[0] != SYNC || packet[1] & 0x40 == 0 || packet[3] & 0x10 == 0 {
            return;
        }
        let start = if packet[3] & 0x20 != 0 { 5 + packet[4] as usize } else { 4 };
        let Some(pes) = packet.get(start..).filter(|pes| pes.len() >= 14) else {
            return;
        };
        // Audio, video, or private stream 1 (where AC-3 goes), with a PTS.
        let id = pes[3];
        if pes[..3] != [0, 0, 1] || !(id == 0xBD || (0xC0..=0xEF).contains(&id)) || pes[7] & 0x80 == 0 {
            return;
        }
        let b = |i: usize| i64::from(pes[i]);
        let pts = ((b(9) >> 1) & 0x07) << 30 | b(10) << 22 | (b(11) >> 1) << 15 | b(12) << 7 | b(13) >> 1;
        self.timestamp(pts);
    }

    fn timestamp(&mut self, pts: i64) {
        let mut at = pts + self.epoch;
        if let Some(last) = self.last {
            if at < last - WRAP / 2 {
                // Past 2^33: the count started again.
                self.epoch += WRAP;
                at += WRAP;
            } else if at > last + WRAP / 2 {
                // A straggler from just before it did.
                at -= WRAP;
                self.note(at);
                return;
            }
        }
        self.last = Some(at);
        self.note(at);
    }

    fn note(&mut self, at: i64) {
        self.first = Some(self.first.map_or(at, |first| first.min(at)));
        self.latest = Some(self.latest.map_or(at, |latest| latest.max(at)));
    }

    /// Seconds of video written, from its first timestamp.
    fn seconds(&self) -> Option<f64> {
        Some((self.latest? - self.first?) as f64 / TICKS)
    }

    /// The bytes written by the time the video reached `seconds`. Marks
    /// from before then are let go, keeping the one at or before it.
    fn bytes_by(&mut self, seconds: f64) -> u64 {
        while self.marks.len() > 1 && self.marks[1].0 <= seconds {
            self.marks.pop_front();
        }
        self.marks.front().filter(|&&(at, _)| at <= seconds).map_or(0, |&(_, bytes)| bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport packet starting a PES packet with this PTS (stream 0xE0,
    /// video), padded with an adaptation field when `padded`.
    fn pes_packet(pts: i64, id: u8, padded: bool) -> Vec<u8> {
        let mut packet = vec![SYNC, 0x40 | 0x01, 0x00];
        if padded {
            packet.push(0x30); // adaptation field and payload
            packet.extend([7, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        } else {
            packet.push(0x10); // payload only
        }
        let pts = pts & (WRAP - 1);
        packet.extend([0, 0, 1, id, 0, 0, 0x80, 0x80, 5]);
        packet.extend([
            0x21 | (((pts >> 30) & 0x07) << 1) as u8,
            (pts >> 22) as u8,
            0x01 | (((pts >> 15) & 0x7F) << 1) as u8,
            (pts >> 7) as u8,
            0x01 | ((pts & 0x7F) << 1) as u8,
        ]);
        packet.resize(PACKET, 0xFF);
        packet
    }

    /// A packet carrying the rest of a PES packet: no timestamp.
    fn continuation() -> Vec<u8> {
        let mut packet = vec![SYNC, 0x01, 0x00, 0x11];
        packet.resize(PACKET, 0xAB);
        packet
    }

    /// `seconds` of output, a video and an audio PES packet every 40 ms
    /// starting at `from`, each followed by `filler` packets.
    fn output(from: f64, seconds: f64, filler: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for frame in 0..(seconds / 0.04).round() as i64 {
            let pts = (from * TICKS) as i64 + frame * 3600;
            out.extend(pes_packet(pts, 0xE0, false));
            out.extend(pes_packet(pts, 0xC0, true));
            for _ in 0..filler {
                out.extend(continuation());
            }
        }
        out
    }

    fn written(pacer: &Pacer) -> f64 {
        pacer.clock.seconds().unwrap_or(-1.0)
    }

    #[test]
    fn reads_timestamps_across_reads_of_any_size() {
        let out = output(10.0, 20.0, 3);
        for size in [1, 100, 188, 1000, 65_536] {
            let mut pacer = Pacer::default();
            for chunk in out.chunks(size) {
                pacer.wrote(chunk);
            }
            assert!((written(&pacer) - 19.96).abs() < 0.01, "{size}: {}", written(&pacer));
            assert_eq!(pacer.clock.bytes, out.len() as u64);
        }
    }

    #[test]
    fn waits_a_minute_ahead_of_the_player() {
        let mut pacer = Pacer::default();
        pacer.wrote(&output(0.0, 59.0, 0));
        assert!(!pacer.is_ahead());
        pacer.wrote(&output(59.0, 2.0, 0));
        assert!(pacer.is_ahead(), "{}", written(&pacer));
        pacer.heard(10.0);
        assert!(!pacer.is_ahead(), "the player moved on");
        pacer.wrote(&output(61.0, 10.0, 0));
        assert!(pacer.is_ahead());
    }

    #[test]
    fn waits_at_64_mb_ahead_for_high_bitrates() {
        let mut pacer = Pacer::default();
        // 40 packets of filler per 40 ms is about 1.5 Mbps: 20 s is under
        // 64 MB at that rate; a lot more filler isn't.
        pacer.wrote(&output(0.0, 20.0, 40));
        assert!(!pacer.is_ahead());
        let heavy = output(20.0, 10.0, 2_000);
        assert!(heavy.len() as u64 > AHEAD_BYTES);
        // In reads the size the gateway makes.
        for chunk in heavy.chunks(64 * 1024) {
            pacer.wrote(chunk);
        }
        assert!(written(&pacer) < AHEAD_SECONDS);
        assert!(pacer.is_ahead(), "64 MB ahead, though only 30 s");
        // Played past the heavy part's start: what's ahead is counted from there.
        pacer.heard(25.0);
        assert!(!pacer.is_ahead());
    }

    #[test]
    fn positions_from_an_earlier_player_are_ignored() {
        let mut pacer = Pacer::default();
        pacer.wrote(&output(0.0, 30.0, 0));
        pacer.heard(20.0);
        // The same address opened again from its start: a fresh pacer, and
        // the old player's last word arrives late.
        let mut fresh = Pacer::default();
        fresh.wrote(&output(0.0, 61.0, 0));
        fresh.heard(1200.0);
        assert!(fresh.is_ahead(), "a position past what's written isn't this player's");
        fresh.heard(f64::NAN);
        fresh.heard(-3.0);
        assert!(fresh.is_ahead());
        fresh.heard(64.0);
        assert!(!fresh.is_ahead(), "within a few seconds of what's written counts");
    }

    #[test]
    fn output_without_timestamps_is_never_held() {
        let mut pacer = Pacer::default();
        for _ in 0..1000 {
            pacer.wrote(&continuation().repeat(400));
        }
        assert!(pacer.clock.bytes > AHEAD_BYTES);
        assert!(!pacer.is_ahead());
    }

    #[test]
    fn timestamps_that_wrap_keep_counting() {
        let mut pacer = Pacer::default();
        let start = (WRAP as f64 / TICKS) - 30.0;
        pacer.wrote(&output(start, 40.0, 0));
        assert!((written(&pacer) - 39.96).abs() < 0.01, "{}", written(&pacer));
        // A straggler from before the wrap changes nothing.
        pacer.wrote(&pes_packet(WRAP - 10, 0xC0, false));
        assert!((written(&pacer) - 39.96).abs() < 0.01, "{}", written(&pacer));
    }

    #[test]
    fn lost_sync_is_found_again() {
        let mut pacer = Pacer::default();
        let mut out = vec![0x00, 0x12, 0x34];
        out.extend(output(5.0, 3.0, 1));
        pacer.wrote(&out);
        assert!((written(&pacer) - 2.96).abs() < 0.01, "{}", written(&pacer));
    }
}
