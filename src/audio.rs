use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use metaflac::Tag;
use crate::text::{
    canonical_primary_artist, normalize_text, safe_name, track_prefix,
};

#[derive(Debug, Clone)]
pub struct TrackMetadata {
    pub albumartist: String,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub tracknumber: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TagUpdates {
    pub albumartist: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
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
            && self.title.is_none()
    }
}

pub fn read_track(path: &Path) -> Result<TrackMetadata> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC tags {}", path.display()))?;

    let albumartist = first_text(&tag, &["ALBUMARTIST"]);
    let artist = first_text(&tag, &["ARTIST"]);
    let album = first_text(&tag, &["ALBUM"]);
    let mut title = first_text(&tag, &["TITLE"]);
    if title.is_empty() {
        title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "Unknown".to_string());
    }
    let tracknumber = {
        let value = first_text(&tag, &["TRACKNUMBER"]);
        (!value.is_empty()).then_some(value)
    };

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

    Ok(TrackMetadata {
        albumartist,
        artist,
        album,
        title,
        tracknumber,
    })
}

pub fn canonicalize(track: &TrackMetadata) -> CanonicalTrack {
    let albumartist = canonical_primary_artist(&track.albumartist);
    let artist = normalize_text(&track.artist, false);
    let album = normalize_text(&track.album, false);
    let title = normalize_text(&track.title, false);
    let tag_updates = TagUpdates {
        albumartist: (normalize_text(&track.albumartist, false) != albumartist).then(|| albumartist.clone()),
        artist: (normalize_text(&track.artist, false) != artist).then(|| artist.clone()),
        album: (!track.album.is_empty() && normalize_text(&track.album, false) != album).then(|| album.clone()),
        title: (!track.title.is_empty() && normalize_text(&track.title, false) != title).then(|| title.clone()),
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

pub fn apply_updates(path: &Path, updates: &TagUpdates) -> Result<bool> {
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
    changed |= set_field(comments, "TITLE", updates.title.as_deref());

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
