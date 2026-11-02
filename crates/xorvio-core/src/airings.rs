//! A programme's airings, by title, for series recordings: every time the
//! guide lists it on one channel, or on any channel. Providers list one
//! channel several times ("News 24 HD", "News 24 4K"), so each airing says
//! which channel it's on as a family the versions share, and the channel to
//! record it from is the version that plays.

use std::collections::HashMap;

use xorvio_model::{Channel, Program, Timestamp};

use crate::edits::Edits;
use crate::names;
use crate::watchlist::normalized;
use crate::{Core, Result, now};

/// How many listings a title search looks through for an any-channel series.
const SEARCH_LIMIT: usize = 2000;

/// One listing of a programme.
#[derive(Debug, Clone, PartialEq)]
pub struct Airing {
    pub program: Program,
    /// The channel to record it from: with similar channels grouped, the
    /// version of the channel that plays.
    pub channel: Channel,
    /// Shared by the versions of one channel, so the same airing on several
    /// of them can be recorded once.
    pub family: String,
}

impl Core {
    /// Programmes titled `title` that start between `from` and `to`, soonest
    /// first: on `channel_id`'s guide, or with `any_channel` on every
    /// channel's (hidden and locked ones left out, whatever the profile or
    /// the PIN's state just now). Titles match as the watchlist matches
    /// them: case, punctuation and a "Live:" marker don't count. At the same
    /// time, better versions of a channel come first.
    pub fn airings(&self, title: &str, channel_id: &str, any_channel: bool, from: Timestamp, to: Timestamp) -> Result<Vec<Airing>> {
        let key = normalized(title);
        if key.is_empty() || to <= from {
            return Ok(Vec::new());
        }
        let edits = self.current_edits();
        let listed: Vec<(Program, Channel)> = if any_channel {
            let excluded = self.excluded_from_recording()?;
            self.catalog
                .search_programs(search_word(&key), &self.active_source_ids(), from, now(), SEARCH_LIMIT, &excluded.ids())?
                .into_iter()
                .filter(|(_, channel)| !excluded.channel(channel))
                .collect()
        } else {
            let Some(channel) = self.catalog.channel(channel_id)? else { return Ok(Vec::new()) };
            self.catalog.channel_programs(&channel.id, from, to)?.into_iter().map(|program| (program, channel.clone())).collect()
        };

        let mut playing = PlayingVersions::default();
        let mut found: Vec<(Airing, (names::Quality, bool))> = Vec::new();
        for (program, channel) in listed {
            if program.start < from || program.start >= to || normalized(&program.title) != key {
                continue;
            }
            let version = names::version_name(&channel.name);
            let rank = (version.quality, !version.backup);
            // A series made on one channel records from that channel, whichever version plays.
            let channel = if any_channel && self.groups_similar_channels() {
                playing.of(self, &edits, channel)?
            } else {
                self.present_channel_with(&edits, channel)
            };
            found.push((Airing { program, channel, family: version.key }, rank));
        }
        found.sort_by(|(a, a_rank), (b, b_rank)| a.program.start.cmp(&b.program.start).then(b_rank.cmp(a_rank)));
        Ok(found.into_iter().map(|(airing, _)| airing).collect())
    }
}

/// The word of a title to look the guide up by: the longest, which matches
/// the fewest other programmes, and one in plain letters when there is one,
/// since the database folds case only for those.
fn search_word(key: &str) -> &str {
    fn longest<'a>(words: impl Iterator<Item = &'a str>) -> Option<&'a str> {
        words.fold(None, |best: Option<&str>, word| match best {
            Some(best) if best.chars().count() >= word.chars().count() => Some(best),
            _ => Some(word),
        })
    }
    let words = || key.split(' ').filter(|word| *word != "at");
    longest(words().filter(|word| word.is_ascii())).or_else(|| longest(words())).unwrap_or(key)
}

/// The version that plays for each channel, worked out once per category
/// from the category's rows of similar channels.
#[derive(Default)]
struct PlayingVersions(HashMap<(String, Option<String>), Vec<Channel>>);

impl PlayingVersions {
    fn of(&mut self, core: &Core, edits: &Edits, channel: Channel) -> Result<Channel> {
        let scope = (channel.source_id.clone(), channel.category_id.clone());
        if !self.0.contains_key(&scope) {
            let listed = core.catalog.channels(&scope.0, scope.1.as_deref(), None, &[])?;
            self.0.insert(scope.clone(), core.group_similar(listed, edits));
        }
        let row = self.0[&scope].iter().find(|row| row.id == channel.id || row.versions.iter().any(|version| version.id == channel.id));
        Ok(match row {
            Some(row) => row.clone(),
            None => core.present_channel_with(edits, channel),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use xorvio_model::{LiveStreamFormat, NewSource, SourceKind, SourceSecrets};

    use super::*;
    use crate::{ChannelFilter, MemorySecrets};

    /// Serves a playlist at /list.m3u and its guide at /guide.xml on 127.0.0.1.
    async fn serve(playlist: String, guide: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let playlist = playlist.replace("{base}", &base);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (playlist, guide) = (playlist.clone(), guide.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => head.extend_from_slice(&buffer[..count]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head);
                    let (status, body) = match text.split_whitespace().nth(1) {
                        Some("/list.m3u") => (200, playlist),
                        Some("/guide.xml") => (200, guide),
                        _ => (404, String::new()),
                    };
                    let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        base
    }

    fn xmltv_time(moment: Timestamp) -> String {
        chrono::DateTime::from_timestamp(moment, 0).unwrap().format("%Y%m%d%H%M%S +0000").to_string()
    }

    /// Hour-long programmes: (guide channel, title, minutes from `base`).
    fn guide(base: Timestamp, programmes: &[(&str, &str, i64)]) -> String {
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
        for (channel, title, minutes) in programmes {
            let start = base + minutes * 60;
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>\n",
                xmltv_time(start),
                xmltv_time(start + 3600)
            ));
        }
        xml.push_str("</tv>\n");
        xml
    }

    #[tokio::test]
    async fn finds_a_titles_airings_on_one_channel_or_any() {
        let playlist = "#EXTM3U url-tvg=\"{base}/guide.xml\"\n\
            #EXTINF:-1 tvg-id=\"harbor\" group-title=\"News\",Harbor News HD\n{base}/live/h-hd.m3u8\n\
            #EXTINF:-1 tvg-id=\"harbor\" group-title=\"News\",US: Harbor News 4K\n{base}/live/h-4k.m3u8\n\
            #EXTINF:-1 tvg-id=\"harbor1\" group-title=\"News\",Harbor News +1\n{base}/live/h-plus.m3u8\n\
            #EXTINF:-1 tvg-id=\"summit\" group-title=\"Sports\",Summit Sports\n{base}/live/s.m3u8\n"
            .to_string();
        let base = now() - now() % 60 + 60;
        let listings = guide(
            base,
            &[
                ("harbor", "Tidewater", 60),
                ("harbor", "Other Show", 120),
                ("harbor", "Live: Tidewater", 180),
                ("harbor1", "Tidewater", 120),
                ("summit", "TIDEWATER!", 60),
                ("summit", "Tidewater Tonight", 240),
            ],
        );
        let server = serve(playlist, listings).await;
        let folder = tempfile::tempdir().unwrap();
        let core = Core::open(folder.path(), Arc::new(MemorySecrets::new())).unwrap();
        let source = core
            .add_source(NewSource {
                name: "Playlist".into(),
                kind: SourceKind::M3uUrl,
                secrets: SourceSecrets { playlist_url: Some(format!("{server}/list.m3u")), ..Default::default() },
                file_path: None,
                live_format: LiveStreamFormat::Hls,
                user_agent: None,
            })
            .await
            .unwrap();
        let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
        assert_eq!(status.last_error, None);
        let each = core.channels(&ChannelFilter { group_similar: Some(false), ..Default::default() }).unwrap();
        // In the playlist's order (their names are shown tidied).
        let id = |position: usize| each[position].id.clone();
        let (hd, four_k, plus, summit) = (id(0), id(1), id(2), id(3));
        let (from, to) = (now(), now() + 86_400);

        // On one channel: its guide only, titles matched without case,
        // punctuation or a "Live:" marker.
        let on_hd = core.airings("tidewater", &hd, false, from, to).unwrap();
        assert_eq!(on_hd.iter().map(|a| a.program.start).collect::<Vec<_>>(), [base + 3600, base + 3 * 3600]);
        assert!(on_hd.iter().all(|a| a.channel.id == hd), "a series made on a channel records from it");
        assert_eq!(on_hd[0].family, "harbornews");

        // On any channel: the version of a channel that plays, and its family.
        let anywhere = core.airings("Tidewater", &hd, true, from, to).unwrap();
        let found: Vec<(&str, Timestamp, &str)> =
            anywhere.iter().map(|a| (a.channel.id.as_str(), a.program.start - base, a.family.as_str())).collect();
        assert_eq!(
            found,
            [
                (four_k.as_str(), 3600, "harbornews"),
                (summit.as_str(), 3600, "summitsports"),
                (plus.as_str(), 2 * 3600, "harbornews+1"),
                (four_k.as_str(), 3 * 3600, "harbornews"),
            ]
        );

        // Hidden channels are left out; what's already started isn't asked for.
        core.set_hidden(&summit, true).unwrap();
        let anywhere = core.airings("Tidewater", &hd, true, base + 2 * 3600, to).unwrap();
        assert_eq!(anywhere.iter().map(|a| a.channel.id.as_str()).collect::<Vec<_>>(), [plus.as_str(), four_k.as_str()]);
        assert!(core.airings("", &hd, true, from, to).unwrap().is_empty());
    }

    #[test]
    fn searches_by_the_longest_plain_word() {
        assert_eq!(search_word("the evening show"), "evening");
        assert_eq!(search_word("city at harbor"), "harbor");
        assert_eq!(search_word("ça va très bien"), "bien");
        assert_eq!(search_word("été"), "été");
    }
}
