//! Telling ad breaks from programme by black and silence.
//!
//! Broadcasters put a moment of black picture and silence between spots,
//! and between the programme and a break. So:
//!
//! 1. A **boundary** is black picture (at least 0.08 s, 98% of the picture
//!    dark) overlapping silence (at least 0.08 s below −50 dB), within a
//!    quarter of a second of each other, no longer than 5 s (longer is a
//!    dark scene or a fault, not a join). Boundaries within a second of each
//!    other are one.
//! 2. A **spot** is the stretch between two boundaries, measured middle to
//!    middle, within a second of 10, 15, 20, 30 or 60 s.
//! 3. A **break** is three or more spots back to back adding up to 85 s – 6¼
//!    minutes (about 1½ to 6 minutes). It runs from the start of its first
//!    boundary's black to the end of its last one's, where the programme
//!    comes back.
//!
//! Everything else is programme. That's deliberately strict: a break missed
//! costs a press of the remote, a stretch of programme skipped costs the
//! programme. A run of spot-length stretches longer than a break is left
//! alone too. Scene changes (`scdet`) aren't used: they come at every cut,
//! so they'd add boundaries in the programme without finding any spot the
//! black-and-silence boundaries don't.

use std::sync::LazyLock;

use regex::Regex;

use super::AdBreak;

/// A spot's usual lengths, in seconds.
const SPOTS: [f64; 5] = [10.0, 15.0, 20.0, 30.0, 60.0];
/// How far a stretch can be from a spot's length.
const SPOT_TOLERANCE: f64 = 1.0;
/// Spots in a row before they're taken for a break.
const LEAST_SPOTS: usize = 3;
/// A break's length, from first boundary to last.
const BREAK_LENGTH: (f64, f64) = (85.0, 375.0);
/// Black and silence this close together are one boundary.
const TOGETHER: f64 = 0.25;
/// Boundaries closer than this are one.
const MERGE: f64 = 1.0;
/// A black, silent moment longer than this isn't a join between spots.
const LONGEST_BOUNDARY: f64 = 5.0;

/// ffmpeg's filters for what the rules read: `blackdetect` on the picture
/// and `silencedetect` on the sound.
pub(crate) const BLACK_FILTER: &str = "blackdetect=d=0.08:pic_th=0.98:pix_th=0.10";
pub(crate) const SILENCE_FILTER: &str = "silencedetect=n=-50dB:d=0.08";

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Span {
    pub start: f64,
    pub end: f64,
}

impl Span {
    fn middle(&self) -> f64 {
        (self.start + self.end) / 2.0
    }
}

/// What ffmpeg's detectors reported, as it reports them.
#[derive(Debug, Default)]
pub(crate) struct Detected {
    pub black: Vec<Span>,
    pub silence: Vec<Span>,
    /// A silence that has started and not yet ended.
    silent_since: Option<f64>,
}

static BLACK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"black_start:\s*(-?[0-9.]+)\s+black_end:\s*(-?[0-9.]+)").expect("valid pattern"));
static SILENCE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"silence_start:\s*(-?[0-9.]+(?:e-?[0-9]+)?)").expect("valid pattern"));
static SILENCE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"silence_end:\s*(-?[0-9.]+(?:e-?[0-9]+)?)").expect("valid pattern"));

impl Detected {
    /// Takes in one line of ffmpeg's log.
    pub fn read(&mut self, line: &str) {
        let number = |text: &str| text.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n.max(0.0));
        if let Some(found) = BLACK.captures(line) {
            if let (Some(start), Some(end)) = (number(&found[1]), number(&found[2]))
                && end > start
            {
                self.black.push(Span { start, end });
            }
        } else if let Some(found) = SILENCE_START.captures(line) {
            self.silent_since = number(&found[1]);
        } else if let Some(found) = SILENCE_END.captures(line)
            && let (Some(start), Some(end)) = (self.silent_since.take(), number(&found[1]))
            && end > start
        {
            self.silence.push(Span { start, end });
        }
    }

    /// Whether the detectors reported anything at all.
    pub fn read_anything(&self) -> bool {
        !self.black.is_empty() || !self.silence.is_empty() || self.silent_since.is_some()
    }

    /// The breaks the rules find.
    pub fn breaks(&self) -> Vec<AdBreak> {
        breaks(&boundaries(&self.black, &self.silence))
    }
}

/// Moments that are black and silent at once, in order.
fn boundaries(black: &[Span], silence: &[Span]) -> Vec<Span> {
    let mut found: Vec<Span> = black
        .iter()
        .filter(|b| b.end - b.start <= LONGEST_BOUNDARY)
        .filter(|b| silence.iter().any(|s| s.start < b.end + TOGETHER && b.start < s.end + TOGETHER))
        .copied()
        .collect();
    found.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Span> = Vec::with_capacity(found.len());
    for span in found {
        match merged.last_mut() {
            Some(last) if span.start - last.end < MERGE => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
}

fn is_spot(seconds: f64) -> bool {
    SPOTS.iter().any(|spot| (seconds - spot).abs() <= SPOT_TOLERANCE)
}

/// Runs of spots between boundaries that make a break.
fn breaks(boundaries: &[Span]) -> Vec<AdBreak> {
    let mut found = Vec::new();
    let mut first = 0;
    while first + 1 < boundaries.len() {
        let mut last = first;
        while last + 1 < boundaries.len() && is_spot(boundaries[last + 1].middle() - boundaries[last].middle()) {
            last += 1;
        }
        let spots = last - first;
        let length = boundaries[last].middle() - boundaries[first].middle();
        if spots >= LEAST_SPOTS && (BREAK_LENGTH.0..=BREAK_LENGTH.1).contains(&length) {
            found.push(AdBreak {
                start_seconds: boundaries[first].start,
                end_seconds: boundaries[last].end,
            });
        }
        first = last.max(first + 1);
    }
    found
}

/// The breaks in an edit decision list such as comskip writes: a line per
/// stretch, "start end action" in seconds, where action 0 (cut) or 3
/// (commercial break) is a break and the others (mute, scene marker) aren't.
pub(crate) fn read_edl(text: &str) -> Vec<AdBreak> {
    let mut found: Vec<AdBreak> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(|f| f.parse::<f64>().ok());
            let start = fields.next()??;
            let end = fields.next()??;
            let action = fields.next().flatten().unwrap_or(0.0);
            (action == 0.0 || action == 3.0).then_some(())?;
            (start.is_finite() && end.is_finite() && end - start >= 1.0).then_some(AdBreak {
                start_seconds: start.max(0.0),
                end_seconds: end,
            })
        })
        .collect();
    found.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boundaries half a second long whose middles fall at `at`.
    fn joins(at: &[f64]) -> Vec<Span> {
        at.iter().map(|t| Span { start: t - 0.25, end: t + 0.25 }).collect()
    }

    /// Boundary times for programme, then spots of these lengths, then programme.
    fn layout(before: f64, spots: &[f64]) -> Vec<f64> {
        let mut at = vec![before];
        for spot in spots {
            at.push(at.last().unwrap() + spot);
        }
        at
    }

    #[test]
    fn reads_ffmpegs_detectors() {
        let mut detected = Detected::default();
        for line in [
            "[Parsed_silencedetect_0 @ 0xc68859380] silence_start: 20.021437",
            "[Parsed_silencedetect_0 @ 0xc68859380] silence_end: 20.521354 | silence_duration: 0.499917",
            "[Parsed_blackdetect_0 @ 0xc68859440] black_start:20.021333 black_end:20.521333 black_duration:0.5",
            "[silencedetect @ 0x1] silence_start: -0.00133333",
            "[silencedetect @ 0x1] silence_end: 0.4 | silence_duration: 0.40133",
            "frame=  100 fps=0.0 q=-0.0 size=N/A time=00:00:10.00 bitrate=N/A",
            "[blackdetect @ 0x2] black_start:30 black_end:29",
        ] {
            detected.read(line);
        }
        assert_eq!(detected.black, [Span { start: 20.021333, end: 20.521333 }]);
        assert_eq!(detected.silence.len(), 2);
        assert_eq!(detected.silence[1], Span { start: 0.0, end: 0.4 }, "a start just before zero is zero");
    }

    #[test]
    fn boundaries_need_black_and_silence_together() {
        let black = [
            Span { start: 10.0, end: 10.5 },
            Span { start: 40.0, end: 40.4 },   // silent a moment after: together
            Span { start: 60.0, end: 60.5 },   // no silence: a dark shot
            Span { start: 90.0, end: 100.0 },  // too long to be a join
            Span { start: 120.0, end: 120.3 }, // and 120.6: one boundary
            Span { start: 120.6, end: 120.9 },
        ];
        let silence = [
            Span { start: 10.1, end: 10.4 },
            Span { start: 40.5, end: 41.0 },
            Span { start: 75.0, end: 76.0 },
            Span { start: 90.0, end: 100.0 },
            Span { start: 120.0, end: 121.0 },
        ];
        let found = boundaries(&black, &silence);
        assert_eq!(found, [Span { start: 10.0, end: 10.5 }, Span { start: 40.0, end: 40.4 }, Span { start: 120.0, end: 120.9 }]);
    }

    #[test]
    fn spots_back_to_back_are_a_break() {
        let found = breaks(&joins(&layout(300.0, &[30.0, 20.0, 30.0, 15.0])));
        assert_eq!(found, [AdBreak { start_seconds: 299.75, end_seconds: 395.25 }]);
        // Slightly off lengths, as measured from real joins.
        let found = breaks(&joins(&layout(50.0, &[30.4, 29.6, 10.8, 59.2, 14.5])));
        assert_eq!(found.len(), 1);
        // Two breaks with programme between.
        let mut both = layout(100.0, &[30.0, 30.0, 30.0]);
        both.extend(layout(800.0, &[60.0, 20.0, 10.0, 30.0]));
        let found = breaks(&joins(&both));
        assert_eq!(found.len(), 2);
        assert_eq!((found[1].start_seconds, found[1].end_seconds), (799.75, 920.25));
    }

    #[test]
    fn programme_isnt_taken_for_a_break() {
        // Fades to black and silence at no particular spacing.
        assert!(breaks(&joins(&[37.0, 118.0, 170.0, 283.0, 301.5, 420.0])).is_empty());
        // Two spot-length stretches aren't enough.
        assert!(breaks(&joins(&layout(100.0, &[30.0, 60.0]))).is_empty());
        // Three short ones are too short for a break.
        assert!(breaks(&joins(&layout(100.0, &[10.0, 15.0, 20.0, 10.0, 15.0]))).is_empty());
        // Spot-length stretches going on for longer than a break.
        assert!(breaks(&joins(&layout(100.0, &[60.0; 8]))).is_empty());
        // A stretch a few seconds off a spot's length breaks a run.
        assert!(breaks(&joins(&layout(100.0, &[30.0, 30.0, 33.0, 30.0, 30.0]))).is_empty());
        assert!(breaks(&[]).is_empty() && breaks(&joins(&[5.0])).is_empty());
    }

    #[test]
    fn reads_edit_decision_lists() {
        let edl = "0.00\t12.00\t0\n# a note\n610.25 790.50 3\n300 302 1\n900\t1080\n\n1200 1200.5 0\nnonsense\n";
        assert_eq!(
            read_edl(edl),
            [
                AdBreak { start_seconds: 0.0, end_seconds: 12.0 },
                AdBreak { start_seconds: 610.25, end_seconds: 790.5 },
                AdBreak { start_seconds: 900.0, end_seconds: 1080.0 },
            ]
        );
    }
}
