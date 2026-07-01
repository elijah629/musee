use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Deserializer};
use tokio::time::sleep;

use crate::{
    audio::TrackMetadata,
    text::{canonical_primary_artist, normalize_text},
};

const MUSICBRAINZ_RELEASE_GROUP_URL: &str = "https://musicbrainz.org/ws/2/release-group";
const USER_AGENT: &str = "musee/0.3.0 ( https://github.com/elijah629/musee )";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(1100);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AlbumLookupKey {
    artist: String,
    album: String,
}

pub struct GenreLookup {
    client: Client,
    cache: HashMap<AlbumLookupKey, Option<String>>,
    last_request: Option<Instant>,
}

impl GenreLookup {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            cache: HashMap::new(),
            last_request: None,
        })
    }

    pub async fn genre_for_album(&mut self, track: &TrackMetadata) -> Result<Option<String>> {
        let key = AlbumLookupKey {
            artist: normalize_text(&canonical_primary_artist(&track.albumartist), false),
            album: normalize_text(&track.album, false),
        };

        if key.album == "Unknown" || key.album.is_empty() {
            return Ok(None);
        }

        if let Some(cached) = self.cache.get(&key) {
            return Ok(cached.clone());
        }

        self.wait_for_rate_limit().await;
        let query = build_query(&key);
        let response = self
            .client
            .get(MUSICBRAINZ_RELEASE_GROUP_URL)
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .with_context(|| format!("genre lookup request failed for album {}", key.album))?
            .error_for_status()
            .with_context(|| format!("genre lookup failed for album {}", key.album))?
            .json::<ReleaseGroupSearchResponse>()
            .await
            .with_context(|| format!("failed to decode genre lookup for album {}", key.album))?;
        self.last_request = Some(Instant::now());

        let genre = choose_genre(response)
            .map(|genre| normalize_genre_name(&genre))
            .filter(|genre| !genre.is_empty());
        self.cache.insert(key, genre.clone());
        Ok(genre)
    }

    async fn wait_for_rate_limit(&self) {
        if let Some(last_request) = self.last_request {
            let elapsed = last_request.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                sleep(MIN_REQUEST_INTERVAL - elapsed).await;
            }
        }
    }
}

fn build_query(key: &AlbumLookupKey) -> String {
    [
        format!("releasegroup:\"{}\"", escape_query_value(&key.album)),
        format!("artist:\"{}\"", escape_query_value(&key.artist)),
    ]
    .join(" AND ")
}

fn escape_query_value(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', "\\\"")
}

fn choose_genre(response: ReleaseGroupSearchResponse) -> Option<String> {
    let mut best_score = i64::MIN;
    let mut best_genre = None;

    for release_group in response.release_groups {
        let result_score = i64::from(release_group.score.unwrap_or_default());
        let Some(genre) = best_genre_in_release_group(&release_group) else {
            continue;
        };
        let total_score = result_score * 1000 + i64::from(normalized_count(genre.count));
        if total_score > best_score {
            best_score = total_score;
            best_genre = Some(genre.name);
        }
    }

    best_genre
}

fn best_genre_in_release_group(release_group: &ReleaseGroup) -> Option<GenreTag> {
    let mut candidates = release_group.genres.clone();
    if candidates.is_empty() {
        candidates.extend(
            release_group
                .tags
                .iter()
                .filter(|tag| looks_like_genre(&tag.name))
                .cloned(),
        );
    }

    candidates
        .into_iter()
        .max_by_key(|tag| (normalized_count(tag.count), tag.name.len()))
}

fn looks_like_genre(value: &str) -> bool {
    let genre = value.trim().to_ascii_lowercase();
    if genre.is_empty() {
        return false;
    }
    !matches!(
        genre.as_str(),
        "favorites"
            | "favourite"
            | "favorite"
            | "seen live"
            | "live"
            | "american"
            | "british"
            | "male vocalists"
            | "female vocalists"
            | "albums i own"
            | "awesome"
            | "my favorites"
    )
}

fn normalize_genre_name(value: &str) -> String {
    let value = normalize_text(value, false);
    if value == "Unknown" {
        return String::new();
    }

    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "hip hop" => "Hip Hop".to_string(),
        "r&b" => "R&B".to_string(),
        "drum and bass" => "Drum and Bass".to_string(),
        "idm" => "IDM".to_string(),
        _ => title_case(&value),
    }
}

fn title_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut start = true;
    for ch in value.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '/' {
            start = true;
            out.push(ch);
        } else if start {
            for upper in ch.to_uppercase() {
                out.push(upper);
            }
            start = false;
        } else {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct ReleaseGroupSearchResponse {
    #[serde(default, rename = "release-groups")]
    release_groups: Vec<ReleaseGroup>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReleaseGroup {
    #[serde(default, deserialize_with = "deserialize_optional_score")]
    score: Option<u32>,
    #[serde(default)]
    genres: Vec<GenreTag>,
    #[serde(default)]
    tags: Vec<GenreTag>,
}

#[derive(Clone, Debug, Deserialize)]
struct GenreTag {
    #[serde(default, deserialize_with = "deserialize_optional_count")]
    count: Option<i32>,
    name: String,
}

fn deserialize_optional_score<'de, D>(deserializer: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ScoreValue {
        Number(u32),
        Text(String),
    }

    let value = Option::<ScoreValue>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(ScoreValue::Number(value)) => Ok(Some(value)),
        Some(ScoreValue::Text(value)) => value
            .parse::<u32>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_optional_count<'de, D>(deserializer: D) -> std::result::Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CountValue {
        Signed(i32),
        Unsigned(u32),
        Text(String),
    }

    let value = Option::<CountValue>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(CountValue::Signed(value)) => Ok(Some(value)),
        Some(CountValue::Unsigned(value)) => Ok(Some(value.min(i32::MAX as u32) as i32)),
        Some(CountValue::Text(value)) => value
            .parse::<i32>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn normalized_count(count: Option<i32>) -> i32 {
    count.unwrap_or_default().max(0)
}

#[cfg(test)]
mod tests {
    use super::{
        AlbumLookupKey, ReleaseGroupSearchResponse, build_query, choose_genre, normalize_genre_name,
    };

    #[test]
    fn builds_album_query() {
        let key = AlbumLookupKey {
            artist: "Artist".to_string(),
            album: "Album".to_string(),
        };
        assert_eq!(
            build_query(&key),
            "releasegroup:\"Album\" AND artist:\"Artist\""
        );
    }

    #[test]
    fn normalizes_common_genres() {
        assert_eq!(normalize_genre_name("hip hop"), "Hip Hop");
        assert_eq!(normalize_genre_name("R&B"), "R&B");
    }

    #[test]
    fn parses_score_from_string() {
        let response: ReleaseGroupSearchResponse = serde_json::from_str(
            r#"{"release-groups":[{"score":"100","genres":[{"name":"hip hop","count":5}]}]}"#,
        )
        .expect("valid response");
        assert_eq!(choose_genre(response), Some("hip hop".to_string()));
    }

    #[test]
    fn skips_hits_without_genre_and_uses_later_match() {
        let response: ReleaseGroupSearchResponse = serde_json::from_str(
            r#"{
                "release-groups":[
                    {"score":"100","genres":[],"tags":[]},
                    {"score":"90","genres":[{"name":"jazz","count":7}],"tags":[]}
                ]
            }"#,
        )
        .expect("valid response");
        assert_eq!(choose_genre(response), Some("jazz".to_string()));
    }

    #[test]
    fn tolerates_negative_count_values() {
        let response: ReleaseGroupSearchResponse = serde_json::from_str(
            r#"{
                "release-groups":[
                    {"score":"100","genres":[{"name":"rock","count":-1}]}
                ]
            }"#,
        )
        .expect("valid response");
        assert_eq!(choose_genre(response), Some("rock".to_string()));
    }
}
