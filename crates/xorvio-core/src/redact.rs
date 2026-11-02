//! Keeps logins out of messages and logs.
//!
//! Providers put usernames, passwords and tokens in their links
//! (`http://host/live/USER/PASS/123.ts`, `get.php?username=…`), so anything
//! that might hold a link goes through [`redact`] before it's stored as an
//! error or written to a log.

use regex::Regex;
use std::sync::LazyLock;
use xorvio_model::SourceSecrets;

const MASK: &str = "•••";

static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // URL query strings: keep keys, drop values.
        (r#"([?&][A-Za-z0-9_\-]+)=[^&\s"']*"#, "$1=•••"),
        // Xtream-style path credentials: /live/user/pass/123.ts, /movie/…, /series/…, /timeshift/user/pass/…
        (r"/(live|movie|series|timeshift)/[^/\s]+/[^/\s]+/", "/$1/•••/•••/"),
        // Xtream playlist links: scheme://host/USER/PASS/12345 (with or without an extension).
        (r#"(://[^/\s]+)/[^/\s]+/[^/\s]+/(\d+(?:\.[A-Za-z0-9]+)?)([\s"']|$)"#, "$1/•••/•••/$2$3"),
        // Userinfo: scheme://user:pass@host or scheme://user@host.
        (r"://[^/@\s]+:[^/@\s]+@", "://•••:•••@"),
        (r"://[^/@:\s]+@", "://•••@"),
        // Fragments, where some services put tokens: #token=…
        (r#"(://[^\s"'#]+)#[^\s"']*"#, "$1#•••"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (Regex::new(pattern).expect("built-in pattern"), replacement))
    .collect()
});

/// The parts of a source's settings that could turn up inside a message: the
/// login, and the token-like parts of its links.
pub(crate) fn secret_values(secrets: &SourceSecrets) -> Vec<String> {
    let mut values: Vec<String> = [&secrets.username, &secrets.password, &secrets.token].into_iter().flatten().cloned().collect();
    for link in [&secrets.playlist_url, &secrets.epg_url, &secrets.server_url].into_iter().flatten() {
        let Ok(parsed) = url::Url::parse(link) else { continue };
        values.extend(parsed.query_pairs().map(|(_, value)| value.into_owned()));
        values.push(parsed.username().to_string());
        values.extend(parsed.password().map(str::to_string));
        // Tokens in a path carry digits or are long; words like "playlist" aren't secrets.
        for segment in parsed.path_segments().into_iter().flatten() {
            if segment.len() >= 4
                && !segment.contains('.')
                && (segment.len() >= 12 || segment.chars().any(|c| c.is_ascii_digit()))
            {
                values.push(segment.to_string());
            }
        }
    }
    values.retain(|value| value.chars().count() >= 4);
    values
}

/// `text` with every known secret and every link-shaped login masked.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    let mut sorted: Vec<&String> = secrets.iter().filter(|secret| secret.chars().count() >= 4).collect();
    // Longer first, so a password that contains the username is masked whole.
    sorted.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in sorted {
        result = mask_whole(&result, secret);
    }
    for (rule, replacement) in RULES.iter() {
        result = rule.replace_all(&result, *replacement).into_owned();
    }
    result
}

/// `text` with `secret` masked wherever it stands on its own: between
/// slashes, after an equals sign, in a sentence. A login like "user" inside
/// "username" is left alone, so messages stay readable.
fn mask_whole(text: &str, secret: &str) -> String {
    let is_word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for (start, _) in text.match_indices(secret) {
        if start < last {
            continue;
        }
        let end = start + secret.len();
        if is_word(text[..start].chars().next_back()) || is_word(text[end..].chars().next()) {
            continue;
        }
        result.push_str(&text[last..start]);
        result.push_str(MASK);
        last = end;
    }
    result.push_str(&text[last..]);
    result
}

/// A link as its server alone ("panel.example:8080"), for showing which
/// provider a source points at.
pub fn host_description(link: &str) -> Option<String> {
    let parsed = url::Url::parse(link).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xtream_links_lose_their_login() {
        let line = redact("Opening http://panel.example:8080/jane/hunter22/48211.ts failed", &[]);
        assert!(!line.contains("jane") && !line.contains("hunter22"), "{line}");
        assert!(line.contains("panel.example:8080") && line.contains("48211.ts"));
        let line = redact("GET http://host:8080/live/alice/s3cret/123.ts and http://host/get.php?username=alice&password=s3cret", &[]);
        assert!(!line.contains("s3cret") && !line.contains("alice"), "{line}");
    }

    #[test]
    fn registered_logins_are_removed_anywhere() {
        let secrets = SourceSecrets {
            playlist_url: Some("https://p.example/get/Tk9abc123xyz/list.m3u?x=1".into()),
            username: Some("walter77".into()),
            password: Some("pa55word".into()),
            ..Default::default()
        };
        let values = secret_values(&secrets);
        let line = redact("server said: user walter77 with pa55word and token Tk9abc123xyz isn't allowed", &values);
        assert!(!line.contains("walter77") && !line.contains("pa55word") && !line.contains("Tk9abc123xyz"), "{line}");
        assert!(!values.iter().any(|value| value == "list.m3u" || value == "get"));
    }

    #[test]
    fn short_logins_dont_garble_words() {
        let values = vec!["user".to_string(), "pass".to_string()];
        let line = redact("The provider rejected the username or password for user (pass).", &values);
        assert_eq!(line, "The provider rejected the username or password for ••• (•••).");
        assert_eq!(redact("http://h/live/user/pass/1.ts", &values), "http://h/live/•••/•••/1.ts");
    }

    #[test]
    fn user_only_logins_and_fragments_are_redacted() {
        let line = redact("rtmp://jane@media.example/live and https://cdn.example/play#token=s3cr3t and https://bob:pw@host/x", &[]);
        assert!(!line.contains("jane@") && !line.contains("s3cr3t") && !line.contains("bob:pw"), "{line}");
    }

    #[test]
    fn hosts_only() {
        assert_eq!(host_description("http://u:p@panel.example:8080/get.php?x=1").as_deref(), Some("panel.example:8080"));
        assert_eq!(host_description("https://cdn.example/list.m3u").as_deref(), Some("cdn.example"));
        assert_eq!(host_description("not a link"), None);
    }
}
