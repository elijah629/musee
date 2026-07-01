use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::text::{canonical_primary_artist, normalize_text, safe_name, track_prefix};
use anyhow::{Context, Result};
use metaflac::Tag;
use tokio::task;

#[derive(Debug, Clone)]
pub struct TrackMetadata {
    pub albumartist: String,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub tracknumber: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EmbeddedLyrics {
    pub tag_keys: Vec<String>,
    pub content: String,
}

impl EmbeddedLyrics {
    pub fn has_tags(&self) -> bool {
        !self.tag_keys.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TagUpdates {
    pub albumartist: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CanonicalTrack {
    pub albumartist: String,
    pub album: String,
    pub title: String,
    pub track_prefix: Option<String>,
    pub tag_updates: TagUpdates,
}

impl TagUpdates {
    pub fn is_empty(&self) -> bool {
        self.albumartist.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.genre.is_none()
            && self.title.is_none()
    }
}

pub async fn read_track(path: &Path) -> Result<TrackMetadata> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || read_track_sync(&path))
        .await
        .context("track reader task failed")?
}

fn read_track_sync(path: &Path) -> Result<TrackMetadata> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;

    Ok(track_from_fields(
        path,
        first_text(&tag, &["ALBUMARTIST"]),
        first_text(&tag, &["ARTIST"]),
        first_text(&tag, &["ALBUM"]),
        first_text(&tag, &["TITLE"]),
        Some(first_text(&tag, &["TRACKNUMBER"])),
    ))
}

pub fn track_from_fields(
    path: &Path,
    albumartist: String,
    artist: String,
    album: String,
    mut title: String,
    tracknumber: Option<String>,
) -> TrackMetadata {
    if title.trim().is_empty() {
        title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "Unknown".to_string());
    }

    let artist = if artist.trim().is_empty() {
        if albumartist.trim().is_empty() {
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Unknown".to_string())
        } else {
            albumartist.clone()
        }
    } else {
        artist
    };

    let albumartist = if albumartist.trim().is_empty() {
        artist.clone()
    } else {
        albumartist
    };

    TrackMetadata {
        albumartist,
        artist,
        album,
        title,
        tracknumber: tracknumber.filter(|value| !value.trim().is_empty()),
    }
}

pub fn canonicalize(track: &TrackMetadata) -> CanonicalTrack {
    let albumartist = canonical_primary_artist(&track.albumartist);
    let artist = normalize_text(&track.artist, false);
    let album = normalize_text(&track.album, false);
    let title = normalize_text(&track.title, false);
    let tag_updates = TagUpdates {
        albumartist: (normalize_text(&track.albumartist, false) != albumartist)
            .then(|| albumartist.clone()),
        artist: (normalize_text(&track.artist, false) != artist).then(|| artist.clone()),
        album: (!track.album.is_empty() && normalize_text(&track.album, false) != album)
            .then(|| album.clone()),
        genre: None,
        title: (!track.title.is_empty() && normalize_text(&track.title, false) != title)
            .then(|| title.clone()),
    };

    CanonicalTrack {
        albumartist,
        album,
        title,
        track_prefix: track_prefix(track.tracknumber.as_deref()),
        tag_updates,
    }
}

pub fn canonical_destination(root: &Path, track: &CanonicalTrack, ext: &str) -> PathBuf {
    if track.album != "Unknown" {
        let dir = root
            .join(safe_name(&track.albumartist))
            .join(safe_name(&track.album));
        let filename = match &track.track_prefix {
            Some(prefix) => format!("{prefix} - {}{ext}", safe_name(&track.title)),
            None => format!("{}{ext}", safe_name(&track.title)),
        };
        dir.join(filename)
    } else {
        root.join("Non-Album")
            .join(safe_name(&track.albumartist))
            .join(format!("{}{ext}", safe_name(&track.title)))
    }
}

pub async fn apply_updates(path: &Path, updates: &TagUpdates) -> Result<bool> {
    let path = path.to_path_buf();
    let updates = updates.clone();
    task::spawn_blocking(move || apply_updates_sync(&path, &updates))
        .await
        .context("tag writer task failed")?
}

pub async fn apply_updates_and_remove_lyrics(
    path: &Path,
    updates: &TagUpdates,
    lyric_tag_keys: &[String],
) -> Result<bool> {
    let path = path.to_path_buf();
    let updates = updates.clone();
    let lyric_tag_keys = lyric_tag_keys.to_vec();
    task::spawn_blocking(move || {
        apply_updates_and_remove_lyrics_sync(&path, &updates, &lyric_tag_keys)
    })
    .await
    .context("tag writer task failed")?
}

fn apply_updates_sync(path: &Path, updates: &TagUpdates) -> Result<bool> {
    if updates.is_empty() {
        return Ok(false);
    }

    let mut tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;
    let comments = tag.vorbis_comments_mut();

    let mut changed = false;
    changed |= set_field(comments, "ALBUMARTIST", updates.albumartist.as_deref());
    changed |= set_field(comments, "ARTIST", updates.artist.as_deref());
    changed |= set_field(comments, "ALBUM", updates.album.as_deref());
    changed |= set_field(comments, "GENRE", updates.genre.as_deref());
    changed |= set_field(comments, "TITLE", updates.title.as_deref());

    if changed {
        tag.save()
            .with_context(|| format!("failed to save FLAC tags {}", path.display()))?;
    }

    Ok(changed)
}

fn apply_updates_and_remove_lyrics_sync(
    path: &Path,
    updates: &TagUpdates,
    lyric_tag_keys: &[String],
) -> Result<bool> {
    if updates.is_empty() && lyric_tag_keys.is_empty() {
        return Ok(false);
    }

    let mut tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;
    let comments = tag.vorbis_comments_mut();

    let mut changed = false;
    changed |= set_field(comments, "ALBUMARTIST", updates.albumartist.as_deref());
    changed |= set_field(comments, "ARTIST", updates.artist.as_deref());
    changed |= set_field(comments, "ALBUM", updates.album.as_deref());
    changed |= set_field(comments, "GENRE", updates.genre.as_deref());
    changed |= set_field(comments, "TITLE", updates.title.as_deref());
    for key in lyric_tag_keys {
        if comments.comments.contains_key(key) {
            comments.remove(key);
            changed = true;
        }
    }

    if changed {
        tag.save()
            .with_context(|| format!("failed to save FLAC tags {}", path.display()))?;
    }

    Ok(changed)
}

pub async fn read_embedded_lyrics(path: &Path) -> Result<EmbeddedLyrics> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || read_embedded_lyrics_sync(&path))
        .await
        .context("lyrics reader task failed")?
}

fn read_embedded_lyrics_sync(path: &Path) -> Result<EmbeddedLyrics> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;
    Ok(extract_embedded_lyrics(tag.vorbis_comments()))
}

fn extract_embedded_lyrics(comments: Option<&metaflac::block::VorbisComment>) -> EmbeddedLyrics {
    let Some(comments) = comments else {
        return EmbeddedLyrics::default();
    };

    let mut lyric_entries = comments
        .comments
        .iter()
        .filter(|(key, _)| is_lyric_tag(key))
        .collect::<Vec<_>>();
    lyric_entries.sort_by_key(|(key, _)| *key);

    let tag_keys = lyric_entries
        .iter()
        .map(|(key, _)| (*key).clone())
        .collect();
    let mut seen = HashSet::new();
    let mut values = Vec::new();
    for (_, entry_values) in lyric_entries {
        for value in entry_values {
            let value = normalize_lrc_text(value);
            if !value.trim().is_empty() && seen.insert(value.clone()) {
                values.push(value);
            }
        }
    }

    let mut content = values.join("\n\n");
    if !content.is_empty() {
        content.push('\n');
    }

    EmbeddedLyrics { tag_keys, content }
}

fn normalize_lrc_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_matches('\n')
        .to_string()
}

fn is_lyric_tag(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "LYRIC"
            | "LYRICS"
            | "SYNCEDLYRIC"
            | "SYNCEDLYRICS"
            | "UNSYNCEDLYRIC"
            | "UNSYNCEDLYRICS"
            | "WMLYRICS"
    )
}

pub async fn read_genre(path: &Path) -> Result<Option<String>> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || read_genre_sync(&path))
        .await
        .context("genre reader task failed")?
}

fn read_genre_sync(path: &Path) -> Result<Option<String>> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;
    let genre = first_text(&tag, &["GENRE"]);
    if genre.is_empty() {
        Ok(None)
    } else {
        Ok(Some(genre))
    }
}

pub async fn write_genre(path: &Path, genre: &str) -> Result<bool> {
    let path = path.to_path_buf();
    let genre = genre.to_string();
    task::spawn_blocking(move || write_genre_sync(&path, &genre))
        .await
        .context("genre writer task failed")?
}

fn write_genre_sync(path: &Path, genre: &str) -> Result<bool> {
    let mut tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;
    let comments = tag.vorbis_comments_mut();
    let changed = set_field(comments, "GENRE", Some(genre));

    if changed {
        tag.save()
            .with_context(|| format!("failed to save FLAC tags {}", path.display()))?;
    }

    Ok(changed)
}

fn first_text(tag: &Tag, keys: &[&str]) -> String {
    for key in keys {
        if let Some(values) = tag.get_vorbis(key) {
            for value in values {
                let text = value.trim();
                if !text.is_empty() {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

fn set_field(
    comments: &mut metaflac::block::VorbisComment,
    key: &str,
    value: Option<&str>,
) -> bool {
    let Some(value) = value else {
        return false;
    };
    let current = comments
        .get(key)
        .and_then(|values| values.first())
        .map(String::as_str);
    if current == Some(value) {
        return false;
    }

    comments.remove(key);
    comments.set(key.to_string(), vec![value.to_string()]);
    true
}

#[cfg(test)]
mod tests {
    use metaflac::block::VorbisComment;

    use super::{extract_embedded_lyrics, is_lyric_tag};

    #[test]
    fn recognizes_common_lyric_tag_names_only() {
        assert!(is_lyric_tag("LYRICS"));
        assert!(is_lyric_tag("unsynced lyrics"));
        assert!(is_lyric_tag("WM/LYRICS"));
        assert!(!is_lyric_tag("LYRICIST"));
        assert!(!is_lyric_tag("LYRICS_LANGUAGE"));
    }

    #[test]
    fn extracts_every_lyric_value_deterministically() {
        let mut comments = VorbisComment::new();
        comments.set("UNSYNCEDLYRICS", vec!["plain\r\nlyrics"]);
        comments.set("LYRICS", vec!["[00:01.00]timed", "plain\r\nlyrics"]);
        comments.set("ARTIST", vec!["Artist"]);

        let lyrics = extract_embedded_lyrics(Some(&comments));

        assert_eq!(lyrics.tag_keys, vec!["LYRICS", "UNSYNCEDLYRICS"]);
        assert_eq!(lyrics.content, "[00:01.00]timed\n\nplain\nlyrics\n");
    }
}
