//! The installed add-ons working together: rows to browse, search, a title's
//! details, its copies and its subtitles. Every call asks the add-ons that can
//! answer it at the same time, each with its own time limit, and one add-on
//! failing never fails the call while another answers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::stream::{self, StreamExt};
use xorvio_model::{language_code, 
    AddonCatalogRow, AddonInfo, AddonMeta, AddonStream, AddonSubtitle, Movie, Series,
};

use crate::client::{AddonClient, AddonError};
use crate::subtitle_text;
use crate::labels::{self, Reading};
use crate::mapping;
use crate::protocol::{CatalogDef, Manifest, Meta};
use crate::registry::{self, StoredAddon};
use crate::secrets::SecretStore;

/// Kinds of catalog Xorvio can show as rows.
const ROW_TYPES: [&str; 3] = ["movie", "series", "other"];
/// Catalog requests in flight at once.
const PARALLEL_REQUESTS: usize = 6;
/// Rows on Discover; more would be a wall of requests for little gain.
const MAX_ROWS: usize = 40;
const META_CACHE_TTL: Duration = Duration::from_secs(3600);
const META_CACHE_LIMIT: usize = 500;
/// Subtitle files are tens of kilobytes; anything this big isn't one.
const MAX_SUBTITLE_BYTES: usize = 8 * 1024 * 1024;

/// The installed add-ons. Shared as `Arc<Addons>`.
pub struct Addons {
    path: PathBuf,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
    entries: RwLock<Vec<Entry>>,
    meta_cache: Mutex<HashMap<String, CachedMeta>>,
}

struct Entry {
    stored: StoredAddon,
    /// `None` when its link is missing from the secret store; it's listed but
    /// can't be asked anything until it's installed again.
    client: Option<AddonClient>,
}

/// An add-on that's on and can be asked.
#[derive(Clone)]
struct Live {
    id: String,
    name: String,
    manifest: Manifest,
    client: AddonClient,
}

struct CachedMeta {
    fetched: Instant,
    addon: Live,
    meta: Meta,
}

impl Addons {
    /// Reads `addons.json` in `data_dir` and each add-on's link from `secrets`.
    pub fn open(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> anyhow::Result<Arc<Addons>> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Xorvio/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let path = registry::path(data_dir);
        let entries = registry::load(&path)?
            .into_iter()
            .map(|stored| {
                let client = match secrets.get(&registry::secret_key(&stored.id)) {
                    Ok(Some(link)) => AddonClient::new(&link, http.clone()).ok(),
                    Ok(None) => {
                        tracing::warn!(host = %stored.host, "an add-on's link is missing from the secret store");
                        None
                    }
                    Err(error) => {
                        tracing::warn!(host = %stored.host, "an add-on's link couldn't be read: {error:#}");
                        None
                    }
                };
                Entry { stored, client }
            })
            .collect();
        Ok(Arc::new(Addons {
            path,
            secrets,
            http,
            entries: RwLock::new(entries),
            meta_cache: Mutex::new(HashMap::new()),
        }))
    }

    // MARK: Managing add-ons

    /// The installed add-ons in the user's order.
    pub fn list(&self) -> Vec<AddonInfo> {
        self.read()
            .iter()
            .map(|e| registry::info(&e.stored))
            .collect()
    }

    /// Checks a manifest link and installs the add-on, or updates it when it's
    /// already installed (set up again with other settings, say).
    pub async fn install(&self, manifest_url: &str) -> anyhow::Result<AddonInfo> {
        let client = AddonClient::new(manifest_url, self.http.clone())?;
        let manifest = client.manifest().await?;
        if !is_useful(&manifest) {
            return Err(AddonError::NothingToUse(manifest.name).into());
        }
        self.secrets
            .set(&registry::secret_key(&manifest.id), client.base())?;
        let stored = StoredAddon {
            id: manifest.id.clone(),
            enabled: true,
            host: client.host().to_string(),
            manifest,
            installed_at: unix_now(),
        };
        let info = registry::info(&stored);
        let mut entries = self.write();
        // The same link installed under an id it no longer uses goes.
        let mut replaced = Vec::new();
        entries.retain(|e| {
            let same_link = e.client.as_ref().is_some_and(|c| c.base() == client.base());
            if same_link && e.stored.id != stored.id {
                replaced.push(e.stored.id.clone());
                return false;
            }
            true
        });
        let entry = Entry {
            stored,
            client: Some(client),
        };
        match entries.iter().position(|e| e.stored.id == entry.stored.id) {
            Some(index) => entries[index] = entry,
            None => entries.push(entry),
        }
        self.persist(&entries)?;
        drop(entries);
        for id in replaced {
            self.forget_link(&id);
        }
        self.clear_cache();
        Ok(info)
    }

    pub fn remove(&self, id: &str) -> anyhow::Result<()> {
        let mut entries = self.write();
        let index = position(&entries, id)?;
        entries.remove(index);
        self.persist(&entries)?;
        drop(entries);
        self.forget_link(id);
        self.clear_cache();
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut entries = self.write();
        let index = position(&entries, id)?;
        entries[index].stored.enabled = enabled;
        self.persist(&entries)?;
        drop(entries);
        self.clear_cache();
        Ok(())
    }

    /// Moves an add-on to `to_index` (clamped to the end). Order matters:
    /// earlier add-ons' rows come first and their details win.
    pub fn move_addon(&self, id: &str, to_index: usize) -> anyhow::Result<()> {
        let mut entries = self.write();
        let index = position(&entries, id)?;
        let entry = entries.remove(index);
        let to_index = to_index.min(entries.len());
        entries.insert(to_index, entry);
        self.persist(&entries)?;
        Ok(())
    }

    /// Reads every add-on's manifest again (new catalogs, new genres), keeping
    /// the saved one for any that doesn't answer. Worth calling once at launch.
    pub async fn refresh_manifests(&self) -> anyhow::Result<()> {
        let clients: Vec<(String, AddonClient)> = self
            .read()
            .iter()
            .filter_map(|e| e.client.clone().map(|c| (e.stored.id.clone(), c)))
            .collect();
        let fresh = futures_util::future::join_all(
            clients
                .into_iter()
                .map(|(id, client)| async move { (id, client.manifest().await) }),
        )
        .await;
        let mut entries = self.write();
        let mut changed = false;
        for (id, result) in fresh {
            match result {
                Ok(manifest) if manifest.id == id => {
                    if let Some(entry) = entries.iter_mut().find(|e| e.stored.id == id)
                        && entry.stored.manifest != manifest
                    {
                        entry.stored.manifest = manifest;
                        changed = true;
                    }
                }
                Ok(_) => tracing::warn!(
                    "an add-on now reports a different id; keeping the saved manifest"
                ),
                Err(error) => tracing::info!("an add-on's manifest couldn't be refreshed: {error}"),
            }
        }
        if changed {
            self.persist(&entries)?;
        }
        Ok(())
    }

    // MARK: Browsing

    /// The first page of every browsable catalog of every add-on that's on,
    /// in add-on order. Rows that fail or come back empty are left out.
    pub async fn discover_rows(&self) -> anyhow::Result<Vec<AddonCatalogRow>> {
        let live = self.live();
        let jobs: Vec<(Live, CatalogDef)> = live
            .iter()
            .flat_map(|addon| {
                addon
                    .manifest
                    .catalogs
                    .iter()
                    .filter(|c| ROW_TYPES.contains(&c.media_type.as_str()) && c.is_browsable())
                    .map(move |c| (addon.clone(), c.clone()))
            })
            .take(MAX_ROWS)
            .collect();
        let total = jobs.len();
        let results: Vec<_> = stream::iter(jobs)
            .map(|(addon, catalog)| async move {
                let result = addon
                    .client
                    .catalog(&catalog.media_type, &catalog.id, &[])
                    .await;
                (addon, catalog, result)
            })
            .buffered(PARALLEL_REQUESTS)
            .collect()
            .await;
        let mut rows = Vec::new();
        let mut first_error = None;
        let mut failures = 0;
        for (addon, catalog, result) in results {
            match result {
                Ok(metas) => {
                    let row = row(&live, &addon, &catalog, &metas, None);
                    if !row.movies.is_empty() || !row.series.is_empty() {
                        rows.push(row);
                    }
                }
                Err(error) => {
                    tracing::info!("a catalog row couldn't be loaded: {error}");
                    failures += 1;
                    first_error.get_or_insert(error);
                }
            }
        }
        // Only when every single row failed is that worth telling.
        match first_error {
            Some(error) if failures == total => Err(error.into()),
            _ => Ok(rows),
        }
    }

    /// One page of one catalog, optionally of one genre, `skip` titles in.
    pub async fn catalog(
        &self,
        addon_id: &str,
        media_type: &str,
        catalog_id: &str,
        genre: Option<&str>,
        skip: usize,
    ) -> anyhow::Result<AddonCatalogRow> {
        let addon = self.live_addon(addon_id)?;
        let mut extra: Vec<(&str, String)> = Vec::new();
        if let Some(genre) = genre.filter(|g| !g.is_empty()) {
            extra.push(("genre", genre.to_string()));
        }
        if skip > 0 {
            extra.push(("skip", skip.to_string()));
        }
        let metas = addon.client.catalog(media_type, catalog_id, &extra).await?;
        let catalog = addon
            .manifest
            .catalogs
            .iter()
            .find(|c| c.media_type == media_type && c.id == catalog_id)
            .cloned()
            .unwrap_or_else(|| CatalogDef {
                media_type: media_type.to_string(),
                id: catalog_id.to_string(),
                name: None,
                extra_names: Vec::new(),
                required_extras: Vec::new(),
                genres: Vec::new(),
            });
        Ok(row(&self.live(), &addon, &catalog, &metas, genre))
    }

    /// Films and series matching `text` from every catalog that can search.
    /// Titles that share no word with the search are dropped (some add-ons
    /// answer with loosely related titles), and exact title matches come first.
    pub async fn search(&self, text: &str) -> anyhow::Result<(Vec<Movie>, Vec<Series>)> {
        let query = text.trim();
        if query.chars().count() < 2 {
            return Ok((Vec::new(), Vec::new()));
        }
        let live = self.live();
        let jobs: Vec<(Live, CatalogDef)> = live
            .iter()
            .flat_map(|addon| {
                addon
                    .manifest
                    .catalogs
                    .iter()
                    .filter(|c| {
                        matches!(c.media_type.as_str(), "movie" | "series") && c.supports_search()
                    })
                    .map(move |c| (addon.clone(), c.clone()))
            })
            .collect();
        let total = jobs.len();
        let results: Vec<_> = stream::iter(jobs)
            .map(|(addon, catalog)| async move {
                let extra = [("search", query.to_string())];
                let result = addon
                    .client
                    .catalog(&catalog.media_type, &catalog.id, &extra)
                    .await;
                (catalog, result)
            })
            .buffered(PARALLEL_REQUESTS)
            .collect()
            .await;

        let words: HashSet<String> = mapping::words(query).into_iter().collect();
        let mut seen = HashSet::new();
        let (mut movies, mut series) = (Vec::new(), Vec::new());
        let mut failures = Vec::new();
        for (catalog, result) in results {
            let metas = match result {
                Ok(metas) => metas,
                Err(error) => {
                    failures.push(error);
                    continue;
                }
            };
            for meta in metas {
                let media_type = type_of(&meta, &catalog);
                if !words.is_empty()
                    && !mapping::words(&meta.name).iter().any(|w| words.contains(w))
                {
                    continue;
                }
                if !reachable(&live, media_type, &meta.id)
                    || !seen.insert(mapping::item_id(media_type, &meta.id))
                {
                    continue;
                }
                match media_type {
                    "series" => series.push(mapping::series(&meta)),
                    "movie" => movies.push(mapping::movie(&meta, "movie")),
                    _ => {}
                }
            }
        }
        if total > 0 && failures.len() == total {
            return Err(failures.swap_remove(0).into());
        }
        let key = mapping::match_key(query);
        movies.sort_by_key(|m| mapping::match_key(&m.title) != key);
        series.sort_by_key(|s| mapping::match_key(&s.title) != key);
        Ok((movies, series))
    }

    // MARK: Titles

    /// A title's full record from the first add-on (in order) that has it.
    /// `media_id` is the add-on's id ("tt0141842") or Xorvio's ("addon|series|tt0141842").
    pub async fn meta(&self, media_type: &str, media_id: &str) -> anyhow::Result<AddonMeta> {
        let (media_type, media_id) = resolve_ids(media_type, media_id);
        let (_, mut meta) = self.meta_record(&media_type, &media_id).await?;
        if meta.media_type.is_empty() {
            meta.media_type = media_type.clone();
        }
        let details = mapping::details(&meta);
        Ok(if media_type == "series" {
            let series = mapping::series(&meta);
            let episodes = mapping::episodes(&meta, &series.id, unix_now());
            AddonMeta {
                movie: None,
                series: Some(series),
                episodes,
                details,
            }
        } else {
            AddonMeta {
                movie: Some(mapping::movie(&meta, &media_type)),
                series: None,
                episodes: Vec::new(),
                details,
            }
        })
    }

    async fn meta_record(
        &self,
        media_type: &str,
        media_id: &str,
    ) -> Result<(Live, Meta), AddonError> {
        let key = format!("{media_type}\n{media_id}");
        if let Some(cached) = self.cache().get(&key)
            && cached.fetched.elapsed() < META_CACHE_TTL
        {
            return Ok((cached.addon.clone(), cached.meta.clone()));
        }
        let candidates: Vec<Live> = self
            .live()
            .into_iter()
            .filter(|a| a.manifest.supports("meta", media_type, media_id))
            .collect();
        if candidates.is_empty() {
            return Err(AddonError::NoAddonFor("details"));
        }
        let results = futures_util::future::join_all(
            candidates
                .iter()
                .map(|addon| addon.client.meta(media_type, media_id)),
        )
        .await;
        let mut first_error = None;
        for (addon, result) in candidates.into_iter().zip(results) {
            match result {
                Ok(Some(meta)) => {
                    let mut cache = self.cache();
                    if cache.len() >= META_CACHE_LIMIT {
                        cache.clear();
                    }
                    cache.insert(
                        key,
                        CachedMeta {
                            fetched: Instant::now(),
                            addon: addon.clone(),
                            meta: meta.clone(),
                        },
                    );
                    return Ok((addon, meta));
                }
                Ok(None) | Err(AddonError::NotFound(_)) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or(AddonError::NoAddonFor("details")))
    }

    /// Every stream add-on's copies of a title or episode, in add-on order,
    /// playable ones first. Copies that need a torrent engine are listed with
    /// no target. When there are none and an add-on said why (an expired
    /// account, say), that's the error.
    pub async fn streams(
        &self,
        media_type: &str,
        media_id: &str,
    ) -> anyhow::Result<Vec<AddonStream>> {
        let (media_type, media_id) = resolve_ids(media_type, media_id);
        let candidates: Vec<Live> = self
            .live()
            .into_iter()
            .filter(|a| a.manifest.supports("stream", &media_type, &media_id))
            .collect();
        let results = futures_util::future::join_all(
            candidates
                .iter()
                .map(|addon| addon.client.streams(&media_type, &media_id)),
        )
        .await;
        let mut copies = Vec::new();
        let mut notices = Vec::new();
        let mut failures = Vec::new();
        for (addon, result) in candidates.iter().zip(results) {
            match result {
                Ok(streams) => {
                    for stream in &streams {
                        match labels::read(stream, &addon.id, &addon.name) {
                            Reading::Copy(copy) => copies.push(*copy),
                            Reading::Notice(text) => notices.push(text),
                            Reading::Skip => {}
                        }
                    }
                }
                Err(error) => failures.push(error),
            }
        }
        if copies.is_empty() {
            // Some add-ons list a title's files with the title itself.
            copies = self.embedded_streams(&media_type, &media_id).await;
        }
        if copies.is_empty() {
            if let Some(notice) = notices.into_iter().next() {
                return Err(AddonError::Notice(notice).into());
            }
            if let Some(failure) = failures.into_iter().next() {
                return Err(failure.into());
            }
            if candidates.is_empty() {
                return Err(AddonError::NoAddonFor("streams").into());
            }
        }
        let mut seen = HashSet::new();
        copies.retain(|copy| {
            let key = match &copy.target {
                Some(target) => target.url.clone(),
                None => format!("{}\n{}\n{:?}", copy.addon_id, copy.name, copy.description),
            };
            seen.insert(key)
        });
        copies.sort_by_key(|copy| copy.target.is_none());
        Ok(copies)
    }

    async fn embedded_streams(&self, media_type: &str, media_id: &str) -> Vec<AddonStream> {
        let Ok((addon, meta)) = self.meta_record(media_type, media_id).await else {
            return Vec::new();
        };
        meta.videos
            .iter()
            .filter(|video| meta.id == media_id || video.id == media_id)
            .flat_map(|video| {
                video.streams.iter().map(move |stream| {
                    let mut stream = stream.clone();
                    if stream.name.is_none() {
                        stream.name = video.title.clone().or_else(|| stream.filename.clone());
                    }
                    stream
                })
            })
            .filter_map(
                |stream| match labels::read(&stream, &addon.id, &addon.name) {
                    Reading::Copy(copy) => Some(*copy),
                    _ => None,
                },
            )
            .collect()
    }

    /// Subtitle files from every subtitle add-on. Add-ons that fail or are
    /// slow are skipped: subtitles never hold up playback with an error.
    pub async fn subtitles(
        &self,
        media_type: &str,
        media_id: &str,
    ) -> anyhow::Result<Vec<AddonSubtitle>> {
        let (media_type, media_id) = resolve_ids(media_type, media_id);
        let candidates: Vec<Live> = self
            .live()
            .into_iter()
            .filter(|a| a.manifest.supports("subtitles", &media_type, &media_id))
            .collect();
        let results = futures_util::future::join_all(
            candidates
                .iter()
                .map(|addon| addon.client.subtitles(&media_type, &media_id)),
        )
        .await;
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        for (addon, result) in candidates.iter().zip(results) {
            match result {
                Ok(list) => found.extend(
                    list.into_iter()
                        .filter(|s| seen.insert(s.url.clone()))
                        .map(|s| AddonSubtitle {
                            addon_name: addon.name.clone(),
                            language: language_code(&s.language).unwrap_or(s.language),
                            url: s.url,
                        }),
                ),
                Err(error) => tracing::info!("subtitles couldn't be loaded: {error}"),
            }
        }
        Ok(found)
    }

    /// A subtitle file an add-on listed, as WebVTT.
    pub async fn subtitle_text(&self, url: &str) -> anyhow::Result<String> {
        let url = url::Url::parse(url)?;
        anyhow::ensure!(matches!(url.scheme(), "http" | "https"), "That isn't a web address.");
        // reqwest names the address in its errors; subtitle links stay out of them too.
        let bytes = async {
            let response = self.http.get(url).timeout(Duration::from_secs(20)).send().await?.error_for_status()?;
            crate::client::read_capped(response, MAX_SUBTITLE_BYTES).await
        }
        .await
        .map_err(|error| anyhow::anyhow!("The subtitles couldn't be downloaded: {}", error.without_url()))?;
        let Some(bytes) = bytes else { anyhow::bail!("That subtitle file is too big.") };
        Ok(subtitle_text::to_webvtt(&bytes))
    }

    // MARK: Internals

    fn read(&self) -> RwLockReadGuard<'_, Vec<Entry>> {
        self.entries.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Vec<Entry>> {
        self.entries.write().unwrap_or_else(|e| e.into_inner())
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, CachedMeta>> {
        self.meta_cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear_cache(&self) {
        self.cache().clear();
    }

    fn persist(&self, entries: &[Entry]) -> anyhow::Result<()> {
        let stored: Vec<StoredAddon> = entries.iter().map(|e| e.stored.clone()).collect();
        registry::save(&self.path, &stored)
    }

    fn forget_link(&self, id: &str) {
        if let Err(error) = self.secrets.delete(&registry::secret_key(id)) {
            tracing::warn!("an add-on's link couldn't be removed from the secret store: {error:#}");
        }
    }

    /// The add-ons that are on and can be asked, in order.
    fn live(&self) -> Vec<Live> {
        self.read()
            .iter()
            .filter(|e| e.stored.enabled)
            .filter_map(|e| {
                Some(Live {
                    id: e.stored.id.clone(),
                    name: e.stored.manifest.name.clone(),
                    manifest: e.stored.manifest.clone(),
                    client: e.client.clone()?,
                })
            })
            .collect()
    }

    fn live_addon(&self, id: &str) -> Result<Live, AddonError> {
        let entries = self.read();
        let entry = entries
            .iter()
            .find(|e| e.stored.id == id)
            .ok_or(AddonError::NotInstalled)?;
        if !entry.stored.enabled {
            return Err(AddonError::Disabled);
        }
        let client = entry.client.clone().ok_or(AddonError::NotInstalled)?;
        Ok(Live {
            id: entry.stored.id.clone(),
            name: entry.stored.manifest.name.clone(),
            manifest: entry.stored.manifest.clone(),
            client,
        })
    }
}

fn position(entries: &[Entry], id: &str) -> Result<usize, AddonError> {
    entries
        .iter()
        .position(|e| e.stored.id == id)
        .ok_or(AddonError::NotInstalled)
}

/// Worth installing: it lists titles, describes them, or offers streams or subtitles.
fn is_useful(manifest: &Manifest) -> bool {
    let has_rows = manifest
        .catalogs
        .iter()
        .any(|c| ROW_TYPES.contains(&c.media_type.as_str()));
    let serves = manifest
        .resources
        .iter()
        .any(|r| matches!(r.name.as_str(), "meta" | "stream" | "subtitles"));
    has_rows || serves
}

/// Some add-on can describe or play this title; otherwise it would be a dead tile.
fn reachable(live: &[Live], media_type: &str, id: &str) -> bool {
    live.iter().any(|a| {
        a.manifest.supports("meta", media_type, id) || a.manifest.supports("stream", media_type, id)
    })
}

/// A meta's own type when it's one Xorvio shows, else its catalog's.
fn type_of<'a>(meta: &'a Meta, catalog: &'a CatalogDef) -> &'a str {
    if ROW_TYPES.contains(&meta.media_type.as_str()) {
        &meta.media_type
    } else {
        &catalog.media_type
    }
}

fn row(
    live: &[Live],
    addon: &Live,
    catalog: &CatalogDef,
    metas: &[Meta],
    genre: Option<&str>,
) -> AddonCatalogRow {
    let (mut movies, mut series) = (Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    for meta in metas {
        let media_type = type_of(meta, catalog);
        if !reachable(live, media_type, &meta.id)
            || !seen.insert(mapping::item_id(media_type, &meta.id))
        {
            continue;
        }
        match media_type {
            "series" => series.push(mapping::series(meta)),
            "movie" | "other" => movies.push(mapping::movie(meta, media_type)),
            _ => {}
        }
    }
    let mut title = row_title(catalog, &addon.name);
    if let Some(genre) = genre.filter(|g| !g.is_empty()) {
        title = format!("{title} · {genre}");
    }
    AddonCatalogRow {
        addon_id: addon.id.clone(),
        catalog_id: catalog.id.clone(),
        media_type: catalog.media_type.clone(),
        title,
        movies,
        series,
    }
}

/// A catalog's name as a row title, saying whether it's films or series
/// unless the name already does ("Popular" becomes "Popular Films").
fn row_title(catalog: &CatalogDef, addon_name: &str) -> String {
    let kind = match catalog.media_type.as_str() {
        "movie" => "Films",
        "series" => "Series",
        _ => {
            return catalog
                .name
                .clone()
                .unwrap_or_else(|| addon_name.to_string());
        }
    };
    let Some(name) = catalog.name.as_deref().filter(|n| !n.trim().is_empty()) else {
        return kind.to_string();
    };
    const KIND_WORDS: [&str; 8] = [
        "movie", "movies", "film", "films", "series", "show", "shows", "tv",
    ];
    let lower = name.to_lowercase();
    let says_kind = lower
        .split(|c: char| !c.is_alphabetic())
        .any(|w| KIND_WORDS.contains(&w));
    if says_kind {
        name.to_string()
    } else {
        format!("{name} {kind}")
    }
}

/// Accepts the add-on's own id or Xorvio's `addon|<type>|<id>`.
fn resolve_ids(media_type: &str, media_id: &str) -> (String, String) {
    match mapping::parse_item_id(media_id) {
        Some((parsed_type, id)) => (
            if media_type.is_empty() {
                parsed_type
            } else {
                media_type.to_string()
            },
            id,
        ),
        None => (media_type.to_string(), media_id.to_string()),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(media_type: &str, name: Option<&str>) -> CatalogDef {
        CatalogDef {
            media_type: media_type.into(),
            id: "x".into(),
            name: name.map(String::from),
            extra_names: vec![],
            required_extras: vec![],
            genres: vec![],
        }
    }

    #[test]
    fn row_titles() {
        assert_eq!(
            row_title(&catalog("movie", Some("Popular")), "A"),
            "Popular Films"
        );
        assert_eq!(row_title(&catalog("series", Some("New TV")), "A"), "New TV");
        assert_eq!(row_title(&catalog("movie", None), "A"), "Films");
        assert_eq!(row_title(&catalog("other", None), "Files"), "Files");
    }

    #[test]
    fn resolves_xorvio_ids() {
        assert_eq!(
            resolve_ids("movie", "addon|movie|tt1"),
            ("movie".into(), "tt1".into())
        );
        assert_eq!(
            resolve_ids("", "addon|series|tt1"),
            ("series".into(), "tt1".into())
        );
        assert_eq!(
            resolve_ids("series", "tt1:1:2"),
            ("series".into(), "tt1:1:2".into())
        );
    }
}
