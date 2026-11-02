//! Rich details for films and series as they're kept in the catalog.
//!
//! The shared [`MediaDetails`] type is what the interface draws. The stored
//! record keeps a little more: the provider's cast as plain names, apart from
//! people with photos and characters from a supplementary source, so TVmaze
//! can fill in faces without ever replacing the provider's own list.

use serde::{Deserialize, Serialize};
use xorvio_model::{CastMember, MediaDetails, SeasonInfo, TitleRating, VideoFormat, VideoLink};

/// Every field is optional when reading, so details saved by an earlier
/// version (without newer fields) still load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct DetailsRecord {
    pub backdrop_urls: Vec<String>,
    pub logo_url: Option<String>,
    /// Names as the provider lists them.
    pub cast: Vec<String>,
    /// Cast with characters and photos, from TVmaze.
    pub people: Vec<CastMember>,
    pub director: Option<String>,
    pub creators: Vec<String>,
    pub release_date: Option<String>,
    pub country: Option<String>,
    pub age_rating: Option<String>,
    pub trailer_youtube_id: Option<String>,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i64>,
    pub imdb_id: Option<String>,
    pub original_title: Option<String>,
    pub tagline: Option<String>,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub video: Option<VideoFormat>,
    pub audio_channels: Option<i64>,
    pub seasons: Vec<SeasonInfo>,
    pub attribution: Option<String>,
    /// Scores, one per source of them.
    pub ratings: Vec<TitleRating>,
    pub videos: Vec<VideoLink>,
}

impl DetailsRecord {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The year from the release date ("2023-04-12" → 2023).
    pub fn release_year(&self) -> Option<i64> {
        self.release_date.as_deref()?.get(..4)?.parse().ok()
    }

    /// Combines two sets of details. Values present in `newer` win; missing
    /// values never erase what's already known.
    pub fn merged(&self, newer: &DetailsRecord) -> DetailsRecord {
        fn list<T: Clone>(old: &[T], new: &[T]) -> Vec<T> {
            if new.is_empty() { old.to_vec() } else { new.to_vec() }
        }
        fn value<T: Clone>(old: &Option<T>, new: &Option<T>) -> Option<T> {
            new.clone().or_else(|| old.clone())
        }
        DetailsRecord {
            backdrop_urls: list(&self.backdrop_urls, &newer.backdrop_urls),
            logo_url: value(&self.logo_url, &newer.logo_url),
            cast: list(&self.cast, &newer.cast),
            people: list(&self.people, &newer.people),
            director: value(&self.director, &newer.director),
            creators: list(&self.creators, &newer.creators),
            release_date: value(&self.release_date, &newer.release_date),
            country: value(&self.country, &newer.country),
            age_rating: value(&self.age_rating, &newer.age_rating),
            trailer_youtube_id: value(&self.trailer_youtube_id, &newer.trailer_youtube_id),
            genres: list(&self.genres, &newer.genres),
            runtime_minutes: value(&self.runtime_minutes, &newer.runtime_minutes),
            imdb_id: value(&self.imdb_id, &newer.imdb_id),
            original_title: value(&self.original_title, &newer.original_title),
            tagline: value(&self.tagline, &newer.tagline),
            overview: value(&self.overview, &newer.overview),
            network: value(&self.network, &newer.network),
            video: value(&self.video, &newer.video),
            audio_channels: value(&self.audio_channels, &newer.audio_channels),
            seasons: list(&self.seasons, &newer.seasons),
            attribution: value(&self.attribution, &newer.attribution),
            ratings: ratings(&self.ratings, &newer.ratings),
            videos: list(&self.videos, &newer.videos),
        }
    }

    /// Fills only what's missing, for supplementary sources that must never
    /// override the provider's own data.
    pub fn filling_from(&self, other: &DetailsRecord) -> DetailsRecord {
        other.merged(self)
    }

    /// The stored form of details that arrived ready-made (an add-on's).
    pub fn from_model(details: MediaDetails) -> Self {
        Self {
            backdrop_urls: details.backdrop_urls,
            logo_url: details.logo_url,
            cast: Vec::new(),
            people: details.people,
            director: details.director,
            creators: details.creators,
            release_date: details.release_date,
            country: details.country,
            age_rating: details.age_rating,
            trailer_youtube_id: details.trailer_youtube_id,
            genres: details.genres,
            runtime_minutes: details.runtime_minutes,
            imdb_id: details.imdb_id,
            original_title: details.original_title,
            tagline: details.tagline,
            overview: details.overview,
            network: details.network,
            video: details.video,
            audio_channels: details.audio_channels,
            seasons: details.seasons,
            attribution: details.attribution,
            ratings: details.ratings,
            videos: details.videos,
        }
    }

    /// The shared form: people with photos when a source has them, otherwise
    /// the provider's names.
    pub fn into_model(self) -> MediaDetails {
        let people = if self.people.is_empty() {
            self.cast.into_iter().map(|name| CastMember { name, role: None, photo_url: None, tmdb_id: None }).collect()
        } else {
            self.people
        };
        MediaDetails {
            backdrop_urls: self.backdrop_urls,
            logo_url: self.logo_url,
            people,
            director: self.director,
            creators: self.creators,
            release_date: self.release_date,
            country: self.country,
            age_rating: self.age_rating,
            trailer_youtube_id: self.trailer_youtube_id,
            genres: self.genres,
            runtime_minutes: self.runtime_minutes,
            imdb_id: self.imdb_id,
            tagline: self.tagline,
            overview: self.overview,
            network: self.network,
            video: self.video,
            audio_channels: self.audio_channels,
            seasons: self.seasons,
            attribution: self.attribution,
            ratings: self.ratings,
            videos: self.videos,
            original_title: self.original_title,
        }
    }
}

/// Each source's score once: the newer one where both have it, in the
/// older's order with new sources after.
fn ratings(old: &[TitleRating], new: &[TitleRating]) -> Vec<TitleRating> {
    let mut combined: Vec<TitleRating> =
        old.iter().map(|rating| new.iter().find(|newer| newer.source == rating.source).unwrap_or(rating).clone()).collect();
    combined.extend(new.iter().filter(|rating| !old.iter().any(|older| older.source == rating.source)).cloned());
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplements_never_override_the_provider() {
        let provider = DetailsRecord {
            cast: vec!["Mara Ellison".into()],
            director: Some("Joon Park".into()),
            ..Default::default()
        };
        let tvmaze = DetailsRecord {
            people: vec![CastMember { name: "Mara Ellison".into(), role: Some("Ada".into()), photo_url: None, tmdb_id: None }],
            director: Some("Someone Else".into()),
            backdrop_urls: vec!["https://img.example/b.jpg".into()],
            attribution: Some("TVmaze".into()),
            ..Default::default()
        };
        let combined = provider.filling_from(&tvmaze);
        assert_eq!(combined.director.as_deref(), Some("Joon Park"));
        assert_eq!(combined.backdrop_urls.len(), 1);
        assert_eq!(combined.cast, ["Mara Ellison"]);
        let model = combined.into_model();
        assert_eq!(model.people[0].role.as_deref(), Some("Ada"), "faces from TVmaze show when there are some");
    }

    #[test]
    fn old_records_still_load() {
        let record: DetailsRecord = serde_json::from_str(r#"{"director":"A","unknownField":1}"#).unwrap();
        assert_eq!(record.director.as_deref(), Some("A"));
        assert!(record.backdrop_urls.is_empty());
    }
}
