//! The diagnostic log in Settings › Diagnostics: what Xorvio's parts write
//! to the log, kept in memory (the latest 2,000 events) so someone can see
//! why a source, a guide or a channel failed, and export it for a bug report.
//! Warnings and errors are always kept; the rest only while detailed events
//! are on. Every line is redacted as it's kept, and again with the sources'
//! own logins when it's exported. Nothing leaves the computer unless someone
//! exports it.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use xorvio_model::{DiagnosticEvent, DiagnosticLevel};

/// Events kept; older ones go first.
const CAPACITY: usize = 2000;

/// The longest message kept, so one runaway reply can't fill the log.
const LONGEST: usize = 4000;

#[derive(Default)]
pub struct DiagnosticsLog {
    events: Mutex<VecDeque<DiagnosticEvent>>,
    detailed: AtomicBool,
    next_id: AtomicU64,
}

impl DiagnosticsLog {
    pub fn set_detailed(&self, on: bool) {
        self.detailed.store(on, Ordering::Relaxed);
    }

    /// Everything kept, newest first.
    pub fn events(&self) -> Vec<DiagnosticEvent> {
        self.events.lock().unwrap().iter().rev().cloned().collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    /// The log as text, oldest first, as it's exported, in local time.
    pub fn export_text(&self) -> String {
        let local = |at: i64| chrono::DateTime::from_timestamp_millis(at).unwrap_or_default().with_timezone(&chrono::Local);
        let now = local(unix_millis()).format("%Y-%m-%d %H:%M %:z");
        let mut text = format!("Xorvio {} diagnostic log, exported {now}\n", env!("CARGO_PKG_VERSION"));
        text.push_str("Logins, tokens and link parameters are removed.\n\n");
        for event in self.events.lock().unwrap().iter() {
            let level = match event.level {
                DiagnosticLevel::Info => "INFO",
                DiagnosticLevel::Warning => "WARNING",
                DiagnosticLevel::Error => "ERROR",
            };
            let _ = writeln!(text, "{} [{level}] {}: {}", local(event.at).format("%Y-%m-%d %H:%M:%S%.3f"), event.area, event.message);
        }
        text
    }

    fn keep(&self, level: DiagnosticLevel, area: &str, message: String) {
        let event = DiagnosticEvent {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            at: unix_millis(),
            level,
            area: area.to_string(),
            message: xorvio_core::redact(&message, &[]),
        };
        let mut events = self.events.lock().unwrap();
        if events.len() == CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }
}

/// Feeds the log from `tracing`, alongside the usual output.
pub struct Capture(pub Arc<DiagnosticsLog>);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let target = metadata.target();
        let level = match *metadata.level() {
            Level::ERROR => DiagnosticLevel::Error,
            Level::WARN => DiagnosticLevel::Warning,
            _ => DiagnosticLevel::Info,
        };
        if level == DiagnosticLevel::Info {
            // The rest is kept only on request, and only Xorvio's own (the
            // HTTP and web view libraries are far too chatty to be useful).
            let ours = target.starts_with("xorvio") || target == "interface";
            if !self.0.detailed.load(Ordering::Relaxed) || !ours {
                return;
            }
        }
        let mut message = Message::default();
        event.record(&mut message);
        self.0.keep(level, area(target), message.text);
    }
}

/// Which part of Xorvio an event came from, by the module that logged it.
fn area(target: &str) -> &'static str {
    let within = |prefix: &str| target.starts_with(prefix);
    // The interface logs what its player runs into.
    if target == "interface" {
        "Playback"
    } else if within("xorvio_media::recorder") || target.contains("recording") {
        "Recording"
    } else if within("xorvio_media") {
        "Playback"
    } else if target.contains("xmltv") || target.contains("guide") {
        "Guide"
    } else if ["trakt", "tmdb", "tvmaze", "segments"].iter().any(|service| target.contains(service)) {
        "Services"
    } else if within("xorvio_addons") {
        "Add-ons"
    } else if target.contains("::store") {
        "Storage"
    } else if within("xorvio_core::http") || within("reqwest") || within("hyper") {
        "Network"
    } else if within("xorvio_core") {
        "Sources"
    } else {
        "App"
    }
}

/// An event's message, followed by its other fields as `name=value`.
#[derive(Default)]
struct Message {
    text: String,
}

impl Message {
    fn add(&mut self, field: &Field, value: &str) {
        if self.text.len() >= LONGEST {
            return;
        }
        if field.name() == "message" {
            self.text.insert_str(0, value);
        } else {
            let _ = write!(self.text, " {}={value}", field.name());
        }
        if self.text.len() > LONGEST {
            let mut end = LONGEST;
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.truncate(end);
            self.text.push('…');
        }
    }
}

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.add(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.add(field, &format!("{value:?}"));
    }
}

/// Standard error, where the console shows Xorvio's log, with logins masked
/// as the diagnostic log masks them: providers put them in their links, and
/// a desktop may keep this output (journald on Linux).
#[derive(Clone, Copy, Default)]
pub struct RedactedStderr;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RedactedStderr {
    type Writer = RedactedLine<std::io::Stderr>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactedLine { text: Vec::new(), out: std::io::stderr() }
    }
}

/// One event's text, written out redacted once it's complete.
pub struct RedactedLine<W: std::io::Write> {
    text: Vec<u8>,
    out: W,
}

impl<W: std::io::Write> std::io::Write for RedactedLine<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.text.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<W: std::io::Write> Drop for RedactedLine<W> {
    fn drop(&mut self) {
        let text = xorvio_core::redact(&String::from_utf8_lossy(&self.text), &[]);
        let _ = self.out.write_all(text.as_bytes());
    }
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn with_log(work: impl FnOnce()) -> Arc<DiagnosticsLog> {
        let log = Arc::new(DiagnosticsLog::default());
        let subscriber = tracing_subscriber::registry().with(Capture(log.clone()));
        tracing::subscriber::with_default(subscriber, work);
        log
    }

    #[test]
    fn the_console_never_shows_logins() {
        use std::io::Write as _;
        let mut shown = Vec::new();
        {
            let mut line = RedactedLine { text: Vec::new(), out: &mut shown };
            write!(line, "WARN xorvio_media: http://provider.example:8080/live/").unwrap();
            writeln!(line, "jane/s3cret/1201.ts?token=abc123 stalled").unwrap();
        }
        let shown = String::from_utf8(shown).unwrap();
        assert!(!shown.contains("jane") && !shown.contains("s3cret") && !shown.contains("abc123"), "{shown}");
        assert!(shown.contains("stalled") && shown.ends_with('\n'), "{shown}");
    }

    #[test]
    fn warnings_are_kept_and_detail_only_on_request() {
        let log = with_log(|| {
            tracing::warn!(target: "xorvio_core::refresh", "the guide couldn't be read");
            tracing::info!(target: "xorvio_core::refresh", "refreshed 3 sources");
            tracing::debug!(target: "hyper::client", "connection pooled");
        });
        let events = log.events();
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].level, events[0].area.as_str()), (DiagnosticLevel::Warning, "Sources"));

        let detailed = Arc::new(DiagnosticsLog::default());
        detailed.set_detailed(true);
        let subscriber = tracing_subscriber::registry().with(Capture(detailed.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "xorvio_media::gateway", "remuxing");
            tracing::info!(target: "hyper::client", "connection pooled");
            tracing::error!(target: "interface", "the video couldn't be decoded");
        });
        let areas: Vec<_> = detailed.events().iter().map(|event| event.area.clone()).collect();
        assert_eq!(areas, ["Playback", "Playback"]);
    }

    #[test]
    fn logins_never_reach_the_log() {
        let log = with_log(|| {
            tracing::warn!(target: "xorvio_media::gateway", url = "http://panel.example:8080/live/jane/hunter22/1.ts", "the stream stopped");
            tracing::error!(target: "xorvio_core::refresh", "GET http://host/get.php?username=jane&password=hunter22 failed");
        });
        for event in log.events() {
            assert!(!event.message.contains("jane") && !event.message.contains("hunter22"), "{}", event.message);
        }
        assert!(log.events()[1].message.starts_with("the stream stopped url="));
    }

    #[test]
    fn the_log_keeps_the_latest_events() {
        let log = with_log(|| {
            for n in 0..CAPACITY + 5 {
                tracing::warn!(target: "xorvio_lib", "event {n}");
            }
        });
        let events = log.events();
        assert_eq!(events.len(), CAPACITY);
        assert_eq!(events[0].message, format!("event {}", CAPACITY + 4));
        assert!(log.export_text().lines().nth(3).unwrap().ends_with("[WARNING] App: event 5"));
    }

    #[test]
    fn areas_follow_the_module() {
        assert_eq!(area("xorvio_media::recorder::job"), "Recording");
        assert_eq!(area("xorvio_core::sources::xmltv"), "Guide");
        assert_eq!(area("xorvio_core::trakt::sync"), "Services");
        assert_eq!(area("xorvio_addons::client"), "Add-ons");
        assert_eq!(area("xorvio_core::store::catalog"), "Storage");
        assert_eq!(area("xorvio_core::servers"), "Sources");
        assert_eq!(area("xorvio_lib::commands"), "App");
    }
}
