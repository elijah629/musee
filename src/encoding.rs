use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use metaflac::{Block, Tag};
use serde_json::Value;
use tokio::{fs, task};

use crate::{
    audio::{TrackMetadata, track_from_fields},
    cli::EncodingProfile,
};

const MAX_FRAME_BYTES: u32 = 32 * 1024;
const SONOS_BLOCK_SIZE: u32 = 4096;
const SEEK_POINTS: usize = 100;
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "alac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "wma",
];

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioFingerprint {
    md5: [u8; 16],
    total_samples: u64,
    sample_rate: u32,
    channels: u8,
    bits_per_sample: u8,
}

#[derive(Debug, Clone, Copy)]
struct AudioProbe {
    channels: u8,
    sample_rate: u32,
}

pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|item| item.eq_ignore_ascii_case(ext))
        })
}

pub async fn read_input_track(
    path: &Path,
    profile: Option<EncodingProfile>,
) -> Result<TrackMetadata> {
    if profile.is_none() || is_flac(path) {
        return crate::audio::read_track(path).await;
    }

    let value = ffprobe_json(path).await?;
    let stream_tags = value
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())
        .and_then(|stream| stream.get("tags"));
    let format_tags = value.get("format").and_then(|format| format.get("tags"));

    let tag = |names: &[&str]| {
        find_tag(stream_tags, names)
            .or_else(|| find_tag(format_tags, names))
            .unwrap_or_default()
    };

    Ok(track_from_fields(
        path,
        tag(&["albumartist", "album_artist"]),
        tag(&["artist"]),
        tag(&["album"]),
        tag(&["title"]),
        Some(tag(&["track", "tracknumber"])),
    ))
}

pub async fn encode_to_temp(source: &Path, profile: EncodingProfile) -> Result<PathBuf> {
    match profile {
        EncodingProfile::SonosFlac => encode_sonos_flac(source).await,
    }
}

pub async fn validate_profile(path: &Path, profile: EncodingProfile) -> Result<()> {
    match profile {
        EncodingProfile::SonosFlac => validate_sonos_flac(path).await,
    }
}

pub async fn fingerprint(path: &Path) -> Result<Option<AudioFingerprint>> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || fingerprint_sync(&path))
        .await
        .context("FLAC fingerprint task failed")?
}

async fn encode_sonos_flac(source: &Path) -> Result<PathBuf> {
    let probe = probe_audio(source).await?;
    if probe.channels == 0 {
        bail!("audio stream has no channels: {}", source.display());
    }

    let channels = probe.channels.min(2);
    let sample_rate = nearest_sonos_rate(probe.sample_rate);
    let target = temp_flac_path(source);

    let mut ffmpeg_command = Command::new("ffmpeg");
    ffmpeg_command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-i")
        .arg(source)
        .arg("-map")
        .arg("0:a:0")
        .arg("-map_metadata")
        .arg("0")
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .arg("-c:a")
        .arg("flac")
        .arg("-sample_fmt")
        .arg("s16")
        .arg("-ar")
        .arg(sample_rate.to_string())
        .arg("-ac")
        .arg(channels.to_string())
        .arg("-frame_size")
        .arg(SONOS_BLOCK_SIZE.to_string())
        .arg("-y")
        .arg(&target);
    let ffmpeg = command_output(ffmpeg_command, "ffmpeg").await?;

    if !ffmpeg.status.success() {
        let _ = fs::remove_file(&target).await;
        bail!(
            "ffmpeg failed for {}: {}",
            source.display(),
            String::from_utf8_lossy(&ffmpeg.stderr).trim()
        );
    }

    let mut metaflac_command = Command::new("metaflac");
    metaflac_command
        .arg(format!("--add-seekpoint={SEEK_POINTS}x"))
        .arg(&target);
    let metaflac = command_output(metaflac_command, "metaflac").await?;

    if !metaflac.status.success() {
        let _ = fs::remove_file(&target).await;
        bail!(
            "metaflac failed for {}: {}",
            source.display(),
            String::from_utf8_lossy(&metaflac.stderr).trim()
        );
    }

    if let Err(error) = validate_sonos_flac(&target).await {
        let _ = fs::remove_file(&target).await;
        return Err(error)
            .with_context(|| format!("encoded output failed validation for {}", source.display()));
    }

    Ok(target)
}

async fn validate_sonos_flac(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || validate_sonos_flac_sync(&path))
        .await
        .context("FLAC validation task failed")?
}

fn validate_sonos_flac_sync(path: &Path) -> Result<()> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read encoded FLAC metadata {}", path.display()))?;
    let stream = tag
        .get_streaminfo()
        .with_context(|| format!("missing FLAC STREAMINFO in {}", path.display()))?;

    if stream.bits_per_sample != 16 {
        bail!("expected 16-bit FLAC, found {}-bit", stream.bits_per_sample);
    }
    if !(1..=2).contains(&stream.num_channels) {
        bail!(
            "expected mono or stereo FLAC, found {} channels",
            stream.num_channels
        );
    }
    if !matches!(stream.sample_rate, 44_100 | 48_000) {
        bail!(
            "expected 44.1 or 48 kHz FLAC, found {} Hz",
            stream.sample_rate
        );
    }
    if stream.min_frame_size == 0 || stream.max_frame_size == 0 {
        bail!("STREAMINFO does not contain minimum and maximum frame sizes");
    }
    if stream.max_frame_size > MAX_FRAME_BYTES {
        bail!(
            "maximum FLAC frame is {} bytes; limit is {} bytes",
            stream.max_frame_size,
            MAX_FRAME_BYTES
        );
    }
    let uncompressed_frame_bytes = u64::from(stream.max_block_size)
        * u64::from(stream.bits_per_sample)
        * u64::from(stream.num_channels)
        / 8;
    if uncompressed_frame_bytes > u64::from(MAX_FRAME_BYTES) {
        bail!(
            "block size can produce a {} byte frame; limit is {} bytes",
            uncompressed_frame_bytes,
            MAX_FRAME_BYTES
        );
    }

    let seek_points = tag
        .blocks()
        .filter_map(|block| match block {
            Block::SeekTable(table) => Some(table.seekpoints.len()),
            _ => None,
        })
        .sum::<usize>();
    if seek_points < SEEK_POINTS {
        bail!("expected {SEEK_POINTS} FLAC seek points, found {seek_points}");
    }

    Ok(())
}

async fn probe_audio(path: &Path) -> Result<AudioProbe> {
    let value = ffprobe_json(path).await?;
    let stream = value
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())
        .with_context(|| format!("no audio stream found in {}", path.display()))?;
    let channels = stream
        .get("channels")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .with_context(|| format!("missing channel count in {}", path.display()))?;
    let sample_rate = stream
        .get("sample_rate")
        .and_then(|value| {
            value
                .as_str()
                .and_then(|value| value.parse().ok())
                .or_else(|| value.as_u64())
        })
        .and_then(|value| u32::try_from(value).ok())
        .with_context(|| format!("missing sample rate in {}", path.display()))?;
    Ok(AudioProbe {
        channels,
        sample_rate,
    })
}

async fn ffprobe_json(path: &Path) -> Result<Value> {
    let mut command = Command::new("ffprobe");
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_streams")
        .arg("-show_format")
        .arg("-of")
        .arg("json")
        .arg(path);
    let output = command_output(command, "ffprobe").await?;
    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("ffprobe returned invalid JSON for {}", path.display()))
}

async fn command_output(mut command: Command, program: &'static str) -> Result<Output> {
    task::spawn_blocking(move || command.output())
        .await
        .with_context(|| format!("{program} process task failed"))?
        .with_context(|| {
            format!(
                "failed to run {program}; install the required audio tools or use the Nix package"
            )
        })
}

fn find_tag(tags: Option<&Value>, names: &[&str]) -> Option<String> {
    let tags = tags?.as_object()?;
    tags.iter().find_map(|(key, value)| {
        names
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            .then(|| value.as_str().map(str::to_owned))
            .flatten()
    })
}

fn fingerprint_sync(path: &Path) -> Result<Option<AudioFingerprint>> {
    let tag = Tag::read_from_path(path)
        .with_context(|| format!("failed to read FLAC fingerprint {}", path.display()))?;
    let stream = tag
        .get_streaminfo()
        .with_context(|| format!("missing FLAC STREAMINFO in {}", path.display()))?;
    let Ok(md5) = <[u8; 16]>::try_from(stream.md5.as_slice()) else {
        return Ok(None);
    };
    if md5.iter().all(|byte| *byte == 0) || stream.total_samples == 0 {
        return Ok(None);
    }
    Ok(Some(AudioFingerprint {
        md5,
        total_samples: stream.total_samples,
        sample_rate: stream.sample_rate,
        channels: stream.num_channels,
        bits_per_sample: stream.bits_per_sample,
    }))
}

fn nearest_sonos_rate(sample_rate: u32) -> u32 {
    if sample_rate.abs_diff(44_100) <= sample_rate.abs_diff(48_000) {
        44_100
    } else {
        48_000
    }
}

fn temp_flac_path(source: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("track");
    std::env::temp_dir().join(format!(
        "musee-{}-{sequence}-{stem}.flac",
        std::process::id()
    ))
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
}

#[cfg(test)]
mod tests {
    use super::nearest_sonos_rate;

    #[test]
    fn chooses_nearest_supported_sample_rate() {
        assert_eq!(nearest_sonos_rate(44_100), 44_100);
        assert_eq!(nearest_sonos_rate(96_000), 48_000);
        assert_eq!(nearest_sonos_rate(32_000), 44_100);
    }
}
