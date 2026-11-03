//! Xtream Codes providers: the `player_api.php` client, the catalog it builds,
//! and the stream and replay links made from a login at playback time.

use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use xorvio_model::{
    Category, CategoryKind, Channel, Episode, LiveStreamFormat, Movie, Series, SourceSecrets, StreamLocator, Timestamp,
    XtreamPath,
};

use super::{ids, xtream_details};
use crate::error::SourceError;
use crate::http::Http;
use crate::json::{self, Object};
use crate::names;
use crate::snapshot::{CatalogSnapshot, KeptKind};

/// A provider login. Only ever built from the keychain, at the moment it's needed.
#[derive(Clone, PartialEq, Eq)]
pub struct XtreamCredentials {
    /// Normalized: scheme, host, port and any base path, with no trailing slash.
    pub server: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for XtreamCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XtreamCredentials").field("server", &crate::redact::host_description(&self.server)).finish_non_exhaustive()
    }
}

impl XtreamCredentials {
    pub fn from_secrets(secrets: &SourceSecrets) -> Option<Self> {
        let server = Self::normalize_server(secrets.server_url.as_deref()?)?;
        let username = secrets.username.clone().filter(|name| !name.is_empty())?;
        let password = secrets.password.clone().filter(|word| !word.is_empty())?;
        Some(Self { server, username, password })
    }

    /// Accepts "host:port", "http://host:port/" or a pasted `get.php?username=…`
    /// playlist link, and returns "http://host:port".
    pub fn normalize_server(text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("http://{trimmed}") };
        let parsed = url::Url::parse(&with_scheme).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        let host = parsed.host_str().filter(|host| !host.is_empty())?;
        let mut path = parsed.path().to_string();
        for suffix in ["/get.php", "/player_api.php", "/xmltv.php"] {
            if let Some(stripped) = path.strip_suffix(suffix) {
                path = stripped.to_string();
            }
        }
        let path = path.trim_end_matches('/');
        let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
        Some(format!("{}://{host}{port}{path}", parsed.scheme()))
    }

    /// Recognizes an Xtream-style playlist link, so the richer Xtream
    /// connection can be offered instead.
    pub fn detect(playlist_url: &str) -> Option<Self> {
        let parsed = url::Url::parse(playlist_url.trim()).ok()?;
        if !parsed.path().ends_with("get.php") {
            return None;
        }
        let value = |name: &str| parsed.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
        Some(Self {
            server: Self::normalize_server(playlist_url)?,
            username: value("username").filter(|v| !v.is_empty())?,
            password: value("password").filter(|v| !v.is_empty())?,
        })
    }

    /// `server` with path segments added, each one escaped.
    fn link(&self, segments: &[&str]) -> String {
        match url::Url::parse(&self.server) {
            Ok(mut link) => {
                if let Ok(mut path) = link.path_segments_mut() {
                    path.pop_if_empty().extend(segments);
                }
                link.to_string()
            }
            Err(_) => format!("{}/{}", self.server, segments.join("/")),
        }
    }

    fn api_url(&self, action: Option<&str>, extra: &[(&str, &str)]) -> String {
        let mut link = self.link(&["player_api.php"]);
        if let Ok(mut parsed) = url::Url::parse(&link) {
            {
                let mut query = parsed.query_pairs_mut();
                query.append_pair("username", &self.username).append_pair("password", &self.password);
                if let Some(action) = action {
                    query.append_pair("action", action);
                }
                for (name, value) in extra {
                    query.append_pair(name, value);
                }
            }
            link = parsed.to_string();
        }
        link
    }

    pub fn guide_url(&self) -> String {
        let link = self.link(&["xmltv.php"]);
        match url::Url::parse(&link) {
            Ok(mut parsed) => {
                parsed.query_pairs_mut().append_pair("username", &self.username).append_pair("password", &self.password);
                parsed.to_string()
            }
            Err(_) => link,
        }
    }

    /// `{server}/{live|movie|series}/{user}/{pass}/{id}.{ext}`
    pub fn stream_url(&self, path: XtreamPath, stream_id: &str, extension: &str) -> String {
        let kind = match path {
            XtreamPath::Live => "live",
            XtreamPath::Movie => "movie",
            XtreamPath::Series => "series",
        };
        self.link(&[kind, &self.username, &self.password, &format!("{stream_id}.{extension}")])
    }

    /// The provider's replay ("timeshift") link for a past programme. Panels
    /// read the start time in the server's own time zone.
    pub fn replay_url(
        &self,
        stream_id: &str,
        start: Timestamp,
        duration_seconds: i64,
        extension: &str,
        server_time_zone: Option<&str>,
    ) -> String {
        let minutes = ((duration_seconds.max(0) + 59) / 60).max(1);
        let start_text = server_local_time(start, server_time_zone);
        self.link(&[
            "timeshift",
            &self.username,
            &self.password,
            &minutes.to_string(),
            &start_text,
            &format!("{stream_id}.{extension}"),
        ])
    }
}

/// "2026-09-25:14-30" in the named zone, or this computer's when the provider
/// didn't say (or named one that doesn't exist).
fn server_local_time(start: Timestamp, zone: Option<&str>) -> String {
    const FORMAT: &str = "%Y-%m-%d:%H-%M";
    let Some(utc) = DateTime::from_timestamp(start, 0) else { return String::new() };
    match zone.and_then(|name| name.parse::<chrono_tz::Tz>().ok()) {
        Some(zone) => utc.with_timezone(&zone).format(FORMAT).to_string(),
        None => Local.from_utc_datetime(&utc.naive_utc()).format(FORMAT).to_string(),
    }
}

/// What the provider says about the account.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Account {
    pub expires: Option<Timestamp>,
    pub max_connections: Option<i64>,
    pub server_time_zone: Option<String>,
}

/// A `player_api.php` client. Every request carries the source's user agent.
pub(crate) struct XtreamClient<'a> {
    pub http: &'a Http,
    pub credentials: &'a XtreamCredentials,
    pub user_agent: Option<&'a str>,
}

impl XtreamClient<'_> {
    async fn get(&self, action: Option<&str>, extra: &[(&str, &str)]) -> Result<Value, SourceError> {
        self.http.json(&self.credentials.api_url(action, extra), self.user_agent).await
    }

    pub async fn authenticate(&self, now: Timestamp) -> Result<Account, SourceError> {
        let root = self.get(None, &[]).await?;
        parse_account(&root, now)
    }

    pub async fn list(&self, action: &str) -> Result<Vec<Object>, SourceError> {
        let value = self.get(Some(action), &[]).await?;
        Ok(json::objects(Some(&value)).into_iter().cloned().collect())
    }

    pub async fn series_info(&self, series_id: &str) -> Result<Value, SourceError> {
        self.get(Some("get_series_info"), &[("series_id", series_id)]).await
    }

    pub async fn vod_info(&self, vod_id: &str) -> Result<Value, SourceError> {
        self.get(Some("get_vod_info"), &[("vod_id", vod_id)]).await
    }
}

pub(crate) fn parse_account(root: &Value, now: Timestamp) -> Result<Account, SourceError> {
    let root = root.as_object().ok_or(SourceError::AuthenticationFailed)?;
    let user = json::object(root, "user_info").ok_or(SourceError::AuthenticationFailed)?;
    if json::int(user.get("auth")) != Some(1) {
        return Err(SourceError::AuthenticationFailed);
    }
    let status = json::string(user.get("status")).map(|s| s.to_lowercase()).unwrap_or_else(|| "active".into());
    let expires = json::int(user.get("exp_date")).filter(|expiry| *expiry > 0);
    if status == "expired" || expires.is_some_and(|expiry| expiry < now) {
        return Err(SourceError::AccountExpired);
    }
    let server_time_zone = json::object(root, "server_info")
        .and_then(|server| json::string(server.get("timezone")))
        .filter(|zone| zone.parse::<chrono_tz::Tz>().is_ok());
    Ok(Account { expires, max_connections: json::int(user.get("max_connections")), server_time_zone })
}

/// Reads a provider's whole catalog. Live channels are required; films and
/// series are optional, but a failed download isn't the same as "none": the
/// previous lists are kept and the problem reported, rather than silently
/// emptying the library.
pub(crate) async fn fetch_catalog(
    client: &XtreamClient<'_>,
    source_id: &str,
    live_format: LiveStreamFormat,
    now: Timestamp,
    reading: impl Fn(),
) -> Result<CatalogSnapshot, SourceError> {
    let account = client.authenticate(now).await?;
    reading();
    let (live_categories, vod_categories, series_categories, live_streams, vod_streams, series) = tokio::join!(
        client.list("get_live_categories"),
        client.list("get_vod_categories"),
        client.list("get_series_categories"),
        client.list("get_live_streams"),
        client.list("get_vod_streams"),
        client.list("get_series"),
    );
    let lists = Lists { live_categories: live_categories?, live_streams: live_streams?, ..Default::default() };
    let mut kept = Vec::new();
    let mut warnings = Vec::new();
    let (lists, movies_ok) = match (vod_categories, vod_streams) {
        (Ok(categories), Ok(streams)) => (Lists { vod_categories: categories, vod_streams: streams, ..lists }, true),
        (Err(error), _) | (_, Err(error)) => {
            warnings.push(format!("Films couldn't be downloaded ({}), so the previous list is still shown.", plain(&error)));
            (lists, false)
        }
    };
    if !movies_ok {
        kept.push(KeptKind::Movies);
    }
    let lists = match (series_categories, series) {
        (Ok(categories), Ok(series)) => Lists { series_categories: categories, series, ..lists },
        (Err(error), _) | (_, Err(error)) => {
            kept.push(KeptKind::Series);
            warnings.push(format!("Series couldn't be downloaded ({}), so the previous list is still shown.", plain(&error)));
            lists
        }
    };

    let mut snapshot = build(source_id, live_format, &lists);
    snapshot.discovered_epg_url = Some(client.credentials.guide_url());
    snapshot.max_connections = account.max_connections;
    snapshot.account_expires = account.expires;
    snapshot.server_time_zone = account.server_time_zone;
    snapshot.capabilities.guide = true;
    for kind in kept {
        snapshot.keep(kind);
    }
    snapshot.warnings = warnings;
    Ok(snapshot)
}

/// An error as a clause inside a sentence.
fn plain(error: &SourceError) -> String {
    error.to_string().trim_end_matches('.').to_string()
}

/// The six lists a catalog is built from.
#[derive(Debug, Default)]
pub(crate) struct Lists {
    pub live_categories: Vec<Object>,
    pub vod_categories: Vec<Object>,
    pub series_categories: Vec<Object>,
    pub live_streams: Vec<Object>,
    pub vod_streams: Vec<Object>,
    pub series: Vec<Object>,
}

/// A plausible release year, not a provider's placeholder.
fn plausible_year(year: Option<i64>) -> Option<i64> {
    year.filter(|year| (1870..=2200).contains(year))
}

pub(crate) fn build(source_id: &str, live_format: LiveStreamFormat, lists: &Lists) -> CatalogSnapshot {
    let mut snapshot = CatalogSnapshot::default();
    for (list, kind) in [
        (&lists.live_categories, CategoryKind::Live),
        (&lists.vod_categories, CategoryKind::Movie),
        (&lists.series_categories, CategoryKind::Series),
    ] {
        for raw in list {
            let (Some(key), Some(name)) = (json::string(raw.get("category_id")), json::string(raw.get("category_name")))
            else {
                continue;
            };
            let sort_index = snapshot.categories.len() as i64;
            snapshot.categories.push(Category {
                id: ids::category(source_id, kind, &key),
                source_id: source_id.into(),
                kind,
                name,
                sort_index,
            });
        }
    }
    let category_id = |raw: &Object, kind: CategoryKind| {
        json::string(raw.get("category_id")).filter(|key| !key.is_empty()).map(|key| ids::category(source_id, kind, &key))
    };

    for raw in &lists.live_streams {
        let (Some(stream_id), Some(name)) = (json::string(raw.get("stream_id")), json::string(raw.get("name"))) else {
            continue;
        };
        let archive = if json::int(raw.get("tv_archive")) == Some(1) {
            json::int(raw.get("tv_archive_duration")).unwrap_or(0).clamp(0, 365)
        } else {
            0
        };
        let sort_index = snapshot.channels.len() as i64;
        snapshot.channels.push(Channel {
            id: ids::channel(source_id, &format!("x:{stream_id}")),
            source_id: source_id.into(),
            name,
            number: json::int(raw.get("num")),
            logo_url: json::url(raw.get("stream_icon")),
            category_id: category_id(raw, CategoryKind::Live),
            epg_id: json::string(raw.get("epg_channel_id")).filter(|id| !id.trim().is_empty()),
            stream: StreamLocator::Xtream {
                path: XtreamPath::Live,
                stream_id,
                extension: live_format.extension().into(),
            },
            catchup_days: archive,
            catchup_source: None,
            sort_index,
            catchup_offset_minutes: 0,
            versions: Vec::new(),
        });
    }

    for raw in &lists.vod_streams {
        let (Some(stream_id), Some(name)) = (json::string(raw.get("stream_id")), json::string(raw.get("name"))) else {
            continue;
        };
        let (title, title_year) = names::split_year(&name);
        let id = ids::movie(source_id, &format!("x:{stream_id}"));
        let details = xtream_details::from_list_entry(raw);
        let backdrop_url = details.backdrop_urls.first().cloned();
        if !details.is_empty() {
            snapshot.details.insert(id.clone(), details);
        }
        let extension = json::string(raw.get("container_extension")).filter(|ext| !ext.trim().is_empty());
        snapshot.movies.push(Movie {
            id,
            source_id: source_id.into(),
            title,
            year: plausible_year(json::int(raw.get("year"))).or(title_year),
            poster_url: json::url(raw.get("stream_icon")),
            backdrop_url,
            category_id: category_id(raw, CategoryKind::Movie),
            rating: ten_point(json::double(raw.get("rating"))),
            added_at: json::int(raw.get("added")).filter(|added| *added > 0),
            plot: xtream_details::text(raw.get("plot")),
            genre: xtream_details::text(raw.get("genre")),
            tmdb_id: xtream_details::text(raw.get("tmdb")).or_else(|| xtream_details::text(raw.get("tmdb_id"))),
            imdb_id: None,
            duration_seconds: None,
            stream: StreamLocator::Xtream {
                path: XtreamPath::Movie,
                stream_id,
                extension: extension.unwrap_or_else(|| "mp4".into()),
            },
        });
    }

    for raw in &lists.series {
        let (Some(series_id), Some(name)) = (json::string(raw.get("series_id")), json::string(raw.get("name"))) else {
            continue;
        };
        let (title, title_year) = names::split_year(&name);
        let release = json::string(raw.get("releaseDate")).or_else(|| json::string(raw.get("release_date")));
        let id = ids::series(source_id, &format!("x:{series_id}"));
        let details = xtream_details::from_list_entry(raw);
        let backdrop_url = details.backdrop_urls.first().cloned();
        if !details.is_empty() {
            snapshot.details.insert(id.clone(), details);
        }
        snapshot.series.push(Series {
            id,
            source_id: source_id.into(),
            title,
            year: title_year.or_else(|| plausible_year(release.and_then(|date| date.get(..4)?.parse().ok()))),
            poster_url: json::url(raw.get("cover")),
            backdrop_url,
            category_id: category_id(raw, CategoryKind::Series),
            rating: ten_point(json::double(raw.get("rating"))),
            added_at: json::int(raw.get("last_modified")).filter(|added| *added > 0),
            plot: xtream_details::text(raw.get("plot")),
            genre: xtream_details::text(raw.get("genre")),
            tmdb_id: xtream_details::text(raw.get("tmdb")).or_else(|| xtream_details::text(raw.get("tmdb_id"))),
            imdb_id: None,
            remote_id: Some(series_id),
        });
    }

    snapshot.derive_capabilities();
    snapshot
}

/// Episodes from `get_series_info`, fetched when a series is opened. Panels
/// send seasons as an object keyed by number, a list of lists, or one flat list.
pub(crate) fn episodes(info: &Value, series: &Series) -> Vec<Episode> {
    let groups: Vec<(Option<i64>, Vec<&Object>)> = match info.get("episodes") {
        Some(Value::Object(seasons)) => {
            seasons.iter().map(|(key, list)| (key.trim().parse().ok(), json::objects(Some(list)))).collect()
        }
        Some(Value::Array(items)) if items.iter().any(Value::is_array) => items
            .iter()
            .enumerate()
            .map(|(index, list)| (Some(index as i64 + 1), json::objects(Some(list))))
            .collect(),
        Some(list @ Value::Array(_)) => vec![(None, json::objects(Some(list)))],
        _ => return Vec::new(),
    };
    let mut result: Vec<Episode> = Vec::new();
    for (season_key, list) in groups {
        for raw in list {
            let Some(episode_id) = json::string(raw.get("id")).filter(|id| !id.is_empty()) else { continue };
            let season = json::int(raw.get("season")).or(season_key).unwrap_or(1);
            let number = json::int(raw.get("episode_num")).unwrap_or(result.len() as i64 + 1);
            let info = raw.get("info").and_then(Value::as_object);
            let field = |key: &str| info.and_then(|info| info.get(key));
            let extension = json::string(raw.get("container_extension")).filter(|ext| !ext.trim().is_empty());
            result.push(Episode {
                id: ids::episode(&series.id, &format!("x:{episode_id}")),
                series_id: series.id.clone(),
                source_id: series.source_id.clone(),
                season,
                number,
                title: json::string(raw.get("title"))
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| format!("Episode {number}")),
                plot: xtream_details::text(field("plot")),
                duration_seconds: json::int(field("duration_secs")).filter(|seconds| *seconds > 0),
                still_url: json::url(field("movie_image")),
                stream: StreamLocator::Xtream {
                    path: XtreamPath::Series,
                    stream_id: episode_id,
                    extension: extension.unwrap_or_else(|| "mp4".into()),
                },
                air_date: json::string(field("air_date").or_else(|| field("releasedate"))).and_then(|day| day_timestamp(&day)),
            });
        }
    }
    result.sort_by_key(|episode| (episode.season, episode.number));
    result
}

/// A day as providers write it ("2025-09-24"), at midday UTC so it reads as
/// the same day in every time zone.
pub(crate) fn day_timestamp(text: &str) -> Option<Timestamp> {
    let day = chrono::NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(12, 0, 0)?.and_utc().timestamp())
}

/// Ratings on a 10-point scale. Some panels send a title's rating out of 100
/// ("55" for 5.5); anything beyond that, or zero, isn't a rating.
fn ten_point(rating: Option<f64>) -> Option<f64> {
    match rating? {
        r if r > 0.0 && r <= 10.0 => Some(r),
        r if r > 10.0 && r <= 100.0 => Some(r / 10.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn objects(value: Value) -> Vec<Object> {
        json::objects(Some(&value)).into_iter().cloned().collect()
    }

    #[test]
    fn server_normalization_and_detection() {
        assert_eq!(XtreamCredentials::normalize_server("provider.example:8080/").as_deref(), Some("http://provider.example:8080"));
        assert_eq!(
            XtreamCredentials::normalize_server("https://provider.example/panel/player_api.php?x=1").as_deref(),
            Some("https://provider.example/panel")
        );
        assert_eq!(XtreamCredentials::normalize_server("  "), None);
        assert_eq!(XtreamCredentials::normalize_server("ftp://provider.example"), None);
        let detected =
            XtreamCredentials::detect("http://provider.example:8080/get.php?username=u&password=p&type=m3u_plus").unwrap();
        assert_eq!(detected.username, "u");
        assert_eq!(detected.server, "http://provider.example:8080");
        assert_eq!(detected.stream_url(XtreamPath::Live, "9", "m3u8"), "http://provider.example:8080/live/u/p/9.m3u8");
        assert!(XtreamCredentials::detect("http://provider.example/list.m3u").is_none());
        assert!(!format!("{detected:?}").contains("u/p"), "debug output never shows the login");
    }

    #[test]
    fn links_escape_odd_logins() {
        let credentials = XtreamCredentials {
            server: "http://panel.example".into(),
            username: "a/b".into(),
            password: "p#ss word".into(),
        };
        let link = credentials.stream_url(XtreamPath::Movie, "20", "mkv");
        assert_eq!(link, "http://panel.example/movie/a%2Fb/p%23ss%20word/20.mkv");
        let api = credentials.api_url(Some("get_vod_info"), &[("vod_id", "20")]);
        assert_eq!(api, "http://panel.example/player_api.php?username=a%2Fb&password=p%23ss+word&action=get_vod_info&vod_id=20");
    }

    #[test]
    fn replay_links_use_the_server_time_zone() {
        let credentials =
            XtreamCredentials { server: "http://panel.example:8080".into(), username: "u".into(), password: "p".into() };
        // 2026-09-25 14:30 UTC is 16:30 in Berlin (summer time) and 10:30 in New York.
        let start = 1_790_346_600;
        let berlin = credentials.replay_url("42", start, 3600, "ts", Some("Europe/Berlin"));
        assert_eq!(berlin, "http://panel.example:8080/timeshift/u/p/60/2026-09-25:16-30/42.ts");
        let new_york = credentials.replay_url("42", start, 61 * 60 + 1, "ts", Some("America/New_York"));
        assert_eq!(new_york, "http://panel.example:8080/timeshift/u/p/62/2026-09-25:10-30/42.ts");
        let short = credentials.replay_url("42", start, 0, "m3u8", Some("UTC"));
        assert_eq!(short, "http://panel.example:8080/timeshift/u/p/1/2026-09-25:14-30/42.m3u8");
    }

    #[test]
    fn accounts() {
        let now = 1_800_000_000;
        let good = json!({"user_info": {"auth": 1, "status": "Active", "exp_date": "1900000000", "max_connections": "2"},
                          "server_info": {"timezone": "Europe/London"}});
        let account = parse_account(&good, now).unwrap();
        assert_eq!(account.max_connections, Some(2));
        assert_eq!(account.expires, Some(1_900_000_000));
        assert_eq!(account.server_time_zone.as_deref(), Some("Europe/London"));
        let odd = json!({"user_info": {"auth": true, "exp_date": null}, "server_info": {"timezone": "Mars/Olympus"}});
        let account = parse_account(&odd, now).unwrap();
        assert_eq!((account.expires, account.server_time_zone), (None, None));
        assert_eq!(parse_account(&json!({"user_info": {"auth": 0}}), now), Err(SourceError::AuthenticationFailed));
        assert_eq!(parse_account(&json!([]), now), Err(SourceError::AuthenticationFailed));
        assert_eq!(parse_account(&json!({"user_info": []}), now), Err(SourceError::AuthenticationFailed));
        let expired = json!({"user_info": {"auth": "1", "exp_date": 1_700_000_000}});
        assert_eq!(parse_account(&expired, now), Err(SourceError::AccountExpired));
        let marked = json!({"user_info": {"auth": 1, "status": "Expired"}});
        assert_eq!(parse_account(&marked, now), Err(SourceError::AccountExpired));
    }

    #[test]
    fn catalog_tolerates_loose_types() {
        let lists = Lists {
            live_categories: objects(json!([{"category_id": "1", "category_name": "News"}, {"category_name": "No id"}])),
            live_streams: objects(json!([
                {"stream_id": 10, "name": "Ten", "num": "3", "tv_archive": "1", "tv_archive_duration": 5, "category_id": 1,
                 "stream_icon": "", "epg_channel_id": null},
                {"stream_id": null, "name": "No id"},
                {"stream_id": 11, "name": "Eleven", "tv_archive": 0, "tv_archive_duration": "nan", "epg_channel_id": "eleven.uk"}
            ])),
            vod_streams: objects(json!({
                "a": {"stream_id": "20", "name": "Film (2020)", "rating": "7.5", "container_extension": "mkv", "added": "1700000000",
                      "backdrop_path": ["https://img.example/f.jpg"], "year": "0"},
                "b": {"stream_id": 21, "name": "Other", "rating": "nan", "container_extension": "", "plot": "null"}
            })),
            series: objects(json!([{"series_id": 30, "name": "Show", "releaseDate": "2019-01-01", "cover": "https://img.example/s.jpg",
                                    "cast": "A, B"}])),
            ..Default::default()
        };
        let snapshot = build("x", LiveStreamFormat::Mpegts, &lists);
        assert_eq!(snapshot.categories.len(), 1);
        assert_eq!(snapshot.channels.len(), 2);
        let ten = &snapshot.channels[0];
        assert_eq!((ten.catchup_days, ten.number, ten.logo_url.as_deref()), (5, Some(3), None));
        assert_eq!(ten.category_id.as_deref(), Some("x|livecat|1"));
        assert_eq!(ten.id, "x|ch|x:10");
        assert!(matches!(&ten.stream, StreamLocator::Xtream { extension, .. } if extension == "ts"));
        assert_eq!(snapshot.channels[1].catchup_days, 0);
        assert_eq!(snapshot.channels[1].epg_id.as_deref(), Some("eleven.uk"));

        let film = &snapshot.movies[0];
        assert_eq!((film.title.as_str(), film.year, film.rating, film.added_at), ("Film", Some(2020), Some(7.5), Some(1_700_000_000)));
        assert_eq!(film.backdrop_url.as_deref(), Some("https://img.example/f.jpg"));
        assert_eq!(
            film.stream,
            StreamLocator::Xtream { path: XtreamPath::Movie, stream_id: "20".into(), extension: "mkv".into() }
        );
        let other = &snapshot.movies[1];
        assert_eq!((other.rating, other.plot.as_deref()), (None, None));
        assert_eq!([ten_point(Some(55.0)), ten_point(Some(7.5)), ten_point(Some(0.0)), ten_point(Some(550.0))], [Some(5.5), Some(7.5), None, None]);
        assert!(matches!(&other.stream, StreamLocator::Xtream { extension, .. } if extension == "mp4"));
        assert_eq!(snapshot.series[0].year, Some(2019));
        assert_eq!(snapshot.series[0].remote_id.as_deref(), Some("30"));
        assert_eq!(snapshot.details[&snapshot.series[0].id].cast, ["A", "B"]);
        assert!(snapshot.capabilities.live && snapshot.capabilities.movies && snapshot.capabilities.replay);
    }

    #[test]
    fn episodes_from_every_shape() {
        let series = Series {
            id: "x|se|x:30".into(),
            source_id: "x".into(),
            title: "Show".into(),
            year: None,
            poster_url: None,
            backdrop_url: None,
            category_id: None,
            rating: None,
            added_at: None,
            plot: None,
            genre: None,
            tmdb_id: None,
            imdb_id: None,
            remote_id: Some("30".into()),
        };
        let keyed = json!({"episodes": {
            "2": [{"id": "202", "episode_num": "1", "title": "Two One", "container_extension": "mkv",
                   "info": {"plot": "P", "duration_secs": "2640", "movie_image": "https://img.example/e.jpg", "air_date": "2025-09-24"}}],
            "1": [{"id": 101, "episode_num": 2, "info": []}, {"id": 100, "episode_num": 1, "title": ""}, {"title": "no id"}]
        }});
        let list = episodes(&keyed, &series);
        assert_eq!(list.iter().map(|e| (e.season, e.number)).collect::<Vec<_>>(), [(1, 1), (1, 2), (2, 1)]);
        assert_eq!(list[0].title, "Episode 1");
        assert_eq!(list[2].air_date, Some(1_758_715_200));
        assert_eq!(list[2].duration_seconds, Some(2640));
        assert_eq!(list[2].id, "x|se|x:30|ep|x:202");

        let nested = json!({"episodes": [[{"id": 1, "episode_num": 1}], [{"id": 2, "episode_num": 1}]]});
        assert_eq!(episodes(&nested, &series).iter().map(|e| e.season).collect::<Vec<_>>(), [1, 2]);
        let flat = json!({"episodes": [{"id": 1, "season": 3, "episode_num": 4}]});
        assert_eq!(episodes(&flat, &series)[0].season, 3);
        assert!(episodes(&json!({"episodes": null}), &series).is_empty());
        assert!(episodes(&json!([]), &series).is_empty());
    }
}
