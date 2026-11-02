//! Shared pieces for running ffmpeg and ffprobe against a [`StreamTarget`].
//! Stream addresses can hold credentials, so anything ffmpeg prints is
//! stripped of addresses before it's logged or turned into an error.

use std::sync::LazyLock;

use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;
use xorvio_model::StreamTarget;

/// Windows' flag for starting a console program without a console window.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command running ffmpeg or ffprobe. They're console programs, and on
/// Windows each would open a console window of its own beside the app,
/// which has none to share; there they start without one. On Linux they're
/// told where the distribution keeps its certificates.
pub(crate) fn tool_command(program: &std::path::Path) -> tokio::process::Command {
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(unused_mut))]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    #[cfg(target_os = "linux")]
    if std::env::var_os("SSL_CERT_FILE").is_none()
        && let Some(bundle) = *CERTIFICATES
    {
        command.env("SSL_CERT_FILE", bundle);
    }
    command
}

/// The system's certificate bundle. Xorvio's own Linux ffmpeg carries its
/// own OpenSSL, which looks in /etc/ssl as Debian, Ubuntu, Arch and current
/// Fedora arrange it; older Fedora, RHEL and others keep theirs elsewhere.
/// Only a file that's there is named: a missing one would fail every check.
#[cfg(target_os = "linux")]
static CERTIFICATES: LazyLock<Option<&'static str>> = LazyLock::new(|| {
    [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
        "/etc/ssl/cert.pem",
    ]
    .into_iter()
    .find(|path| std::path::Path::new(path).is_file())
});

/// Where an input lives: a web address or a file on this computer.
pub(crate) enum Input {
    Web(url::Url),
    File(std::path::PathBuf),
}

impl Input {
    pub fn of(target: &StreamTarget) -> Option<Input> {
        let text = target.url.trim();
        if let Ok(url) = url::Url::parse(text) {
            return match url.scheme() {
                "http" | "https" => Some(Input::Web(url)),
                "file" => url.to_file_path().ok().map(Input::File),
                // A Windows path such as C:\… parses as a URL with scheme "c".
                scheme if scheme.len() == 1 => Some(Input::File(text.into())),
                _ => None,
            };
        }
        let path = std::path::PathBuf::from(text);
        path.is_absolute().then_some(Input::File(path))
    }

    pub fn is_web(&self) -> bool {
        matches!(self, Input::Web(_))
    }

    /// What ffmpeg's `-i` takes.
    pub fn ffmpeg_input(&self) -> String {
        match self {
            Input::Web(url) => url.to_string(),
            Input::File(path) => path.to_string_lossy().into_owned(),
        }
    }
}

/// What ffmpeg may use to read a web input: the web, and what HLS needs
/// (encrypted segments, `data:` keys). Never files, so a playlist from a
/// provider or an add-on can't make it read one from this computer.
const WEB_PROTOCOLS: &str = "http,https,tls,tcp,crypto,data,httpproxy";

/// The options for a web input: the protocols ffmpeg may use for it, and the
/// target's user agent and headers.
pub(crate) fn web_input_args(target: &StreamTarget) -> Vec<String> {
    let mut args = vec!["-protocol_whitelist".to_string(), WEB_PROTOCOLS.to_string()];
    if let Some(agent) = target.user_agent.as_deref().filter(|a| !a.is_empty() && !a.contains(['\r', '\n'])) {
        args.extend(["-user_agent".to_string(), agent.to_string()]);
    }
    let headers: String = target
        .headers
        .iter()
        .filter(|(name, value)| is_safe_header(name, value))
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    if !headers.is_empty() {
        args.extend(["-headers".to_string(), headers]);
    }
    args
}

/// Header names and values that can't break the request they're put in.
pub(crate) fn is_safe_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
        && !value.contains(['\r', '\n'])
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        )
}

/// A command for work nobody is waiting on (thumbnails, finding ad breaks),
/// run at a low priority so playback and recording come first: through
/// `nice` where there is one, below normal priority on Windows.
pub(crate) fn background(program: &std::path::Path) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let nice = std::path::Path::new("/usr/bin/nice");
        if nice.exists() {
            let mut command = tokio::process::Command::new(nice);
            command.args(["-n", "15"]).arg(program);
            return command;
        }
    }
    #[allow(unused_mut)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    }
    command
}

static ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s'\x22]*[^\s'\x22:,.;)]").expect("valid pattern")
});

/// Text with every address in it replaced, for logs and errors.
pub(crate) fn redact(text: &str) -> String {
    ADDRESS.replace_all(text, "<address>").into_owned()
}

/// Reads ffmpeg's error output so its pipe never fills, logging it without
/// addresses and keeping the last few lines to explain a failure.
pub(crate) async fn drain_stderr(stderr: ChildStderr, label: &'static str) -> Vec<String> {
    let mut lines = BufReader::new(stderr).lines();
    let mut tail = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = redact(line.trim());
        if line.is_empty() {
            continue;
        }
        tracing::debug!(target: "xorvio_media::ffmpeg", "{label}: {line}");
        if tail.len() == 12 {
            tail.remove(0);
        }
        tail.push(line);
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_addresses() {
        assert_eq!(
            redact("http://user:pass@host:8080/live/u/p/1.ts: Server returned 403 Forbidden"),
            "<address>: Server returned 403 Forbidden"
        );
        assert_eq!(
            redact("Opening 'https://a.example/x?token=1' for reading"),
            "Opening '<address>' for reading"
        );
    }

    #[test]
    fn reads_inputs() {
        let target = |url: &str| StreamTarget {
            url: url.into(),
            user_agent: None,
            headers: vec![],
            is_live: false,
        };
        assert!(
            Input::of(&target("https://a.example/v.mp4"))
                .unwrap()
                .is_web()
        );
        // An absolute path and a file address, as this platform writes them.
        let (path, file) = if cfg!(windows) {
            (r"C:\tmp\a.ts", "file:///C:/tmp/a%20b.ts")
        } else {
            ("/tmp/a.ts", "file:///tmp/a%20b.ts")
        };
        assert!(matches!(Input::of(&target(path)), Some(Input::File(_))));
        assert!(matches!(Input::of(&target(file)), Some(Input::File(p)) if p.ends_with("a b.ts")));
        assert!(Input::of(&target("rtmp://a.example/live")).is_none());
        assert!(Input::of(&target("relative/path.ts")).is_none());
    }

    #[test]
    fn web_input_args_skip_unsafe_headers() {
        let target = StreamTarget {
            url: "http://a".into(),
            user_agent: Some("Player/1".into()),
            headers: vec![
                ("Referer".into(), "http://r".into()),
                ("Bad\nName".into(), "x".into()),
                ("X-A".into(), "a\r\nb".into()),
            ],
            is_live: true,
        };
        assert_eq!(
            web_input_args(&target),
            [
                "-protocol_whitelist",
                WEB_PROTOCOLS,
                "-user_agent",
                "Player/1",
                "-headers",
                "Referer: http://r\r\n"
            ]
        );
        // A user agent that would add a header of its own is left out.
        let sneaky = StreamTarget { user_agent: Some("Player/1\r\nX-Evil: 1".into()), headers: vec![], ..target };
        assert_eq!(web_input_args(&sneaky), ["-protocol_whitelist", WEB_PROTOCOLS]);
    }
}
