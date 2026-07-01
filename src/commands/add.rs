use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tokio::fs;

use crate::{
    audio::{CanonicalTrack, TrackMetadata, apply_updates, canonical_destination, canonicalize},
    cli::AddArgs,
    encoding::{encode_to_temp, is_supported_audio, read_input_track, validate_profile},
    fsutil, output,
    transfer::{
        bytes_progress, copy_with_progress, count_progress, file_len, move_with_progress,
        path_exists,
    },
};

const TRACK_SIDECAR_EXTENSIONS: &[&str] = &[".lrc", ".txt"];
const ALBUM_SIDECAR_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "folder.jpg",
    "folder.jpeg",
    "folder.png",
    "album.nfo",
];

#[derive(Debug, Clone)]
struct SidecarPlan {
    source: PathBuf,
    target: PathBuf,
    copy_only: bool,
}

#[derive(Debug, Clone)]
struct AddPlan {
    source: PathBuf,
    target: PathBuf,
    sidecars: Vec<SidecarPlan>,
    tag_updates: crate::audio::TagUpdates,
    collision: bool,
    bytes: u64,
}

pub async fn run(server: &Path, args: &AddArgs) -> Result<()> {
    ensure_server_root(server).await?;

    let sources = collect_sources(&args.sources, args.encoding.is_some()).await?;
    if sources.is_empty() {
        bail!("no supported audio files found in provided sources");
    }

    let scan_pb = count_progress(sources.len() as u64, "planning add");
    let mut plans = Vec::with_capacity(sources.len());
    let mut reserved = HashSet::new();
    for source in &sources {
        let plan = build_plan(source, server, &mut reserved, args).await?;
        scan_pb.inc(1);
        plans.push(plan);
    }
    scan_pb.finish_and_clear();

    output::headline(if args.apply { "apply" } else { "dry-run" }, "add", server);
    output::note(format!(
        "files {} collisions {}",
        plans.len(),
        plans.iter().filter(|plan| plan.collision).count()
    ));

    if !args.apply {
        for plan in &plans {
            println!("ADD {} -> {}", plan.source.display(), plan.target.display());
        }
        return Ok(());
    }

    let total_bytes: u64 = plans.iter().map(|plan| plan.bytes).sum();
    let mut sidecar_bytes = 0_u64;
    for future in plans
        .iter()
        .flat_map(|plan| plan.sidecars.iter())
        .map(|sidecar| file_len(&sidecar.source))
    {
        sidecar_bytes += future.await.unwrap_or(0);
    }
    let total_bytes = total_bytes + sidecar_bytes;
    let bytes_pb = bytes_progress(total_bytes.max(1), "transfers");
    let files_pb = count_progress(plans.len() as u64, "adding");

    let mut tags_modified = 0_u64;
    let mut sidecars_done = 0_u64;
    for plan in &plans {
        let tags_changed = if let Some(profile) = args.encoding {
            apply_encoded_track(plan, profile, &bytes_pb).await?
        } else {
            let changed = apply_updates(&plan.source, &plan.tag_updates).await?;
            move_with_progress(&plan.source, &plan.target, &bytes_pb).await?;
            changed
        };
        if tags_changed {
            tags_modified += 1;
        }

        for sidecar in &plan.sidecars {
            if sidecar.copy_only {
                copy_with_progress(&sidecar.source, &sidecar.target, &bytes_pb).await?;
            } else {
                move_with_progress(&sidecar.source, &sidecar.target, &bytes_pb).await?;
            }
            sidecars_done += 1;
        }
        files_pb.inc(1);
    }

    files_pb.finish_and_clear();
    bytes_pb.finish_and_clear();
    output::note(format!(
        "done files {} tags {} sidecars {}",
        plans.len(),
        tags_modified,
        sidecars_done
    ));
    Ok(())
}

async fn apply_encoded_track(
    plan: &AddPlan,
    profile: crate::cli::EncodingProfile,
    bytes_pb: &indicatif::ProgressBar,
) -> Result<bool> {
    let temporary = encode_to_temp(&plan.source, profile).await?;
    let result = async {
        let changed = apply_updates(&temporary, &plan.tag_updates).await?;
        validate_profile(&temporary, profile).await?;
        install_encoded(&temporary, &plan.source, &plan.target, bytes_pb).await?;
        Ok::<bool, anyhow::Error>(changed)
    }
    .await;

    if result.is_err() && path_exists(&temporary).await.unwrap_or(false) {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

async fn install_encoded(
    encoded: &Path,
    source: &Path,
    target: &Path,
    bytes_pb: &indicatif::ProgressBar,
) -> Result<()> {
    if source != target {
        move_with_progress(encoded, target, bytes_pb).await?;
        fs::remove_file(source)
            .await
            .with_context(|| format!("failed to remove encoded source {}", source.display()))?;
        return Ok(());
    }

    let backup = target.with_extension(format!("musee-backup-{}", std::process::id()));
    if path_exists(&backup).await? {
        bail!("temporary backup already exists: {}", backup.display());
    }
    fs::rename(source, &backup)
        .await
        .with_context(|| format!("failed to back up {} before replacement", source.display()))?;

    if let Err(error) = move_with_progress(encoded, target, bytes_pb).await {
        if path_exists(target).await.unwrap_or(false) {
            let _ = fs::remove_file(target).await;
        }
        fs::rename(&backup, source)
            .await
            .with_context(|| format!("failed to restore {} after: {error:#}", source.display()))?;
        return Err(error);
    }

    fs::remove_file(&backup)
        .await
        .with_context(|| format!("failed to remove backup {}", backup.display()))?;
    Ok(())
}

async fn build_plan(
    source: &Path,
    server: &Path,
    reserved: &mut HashSet<PathBuf>,
    args: &AddArgs,
) -> Result<AddPlan> {
    let metadata = read_input_track(source, args.encoding).await?;
    let canonical =
        canonicalize_for_add(metadata, args.unreleased, args.unreleased_artist.as_deref());
    let ext = if args.encoding.is_some() {
        ".flac".to_string()
    } else {
        source
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!(".{}", ext.to_ascii_lowercase()))
            .unwrap_or_else(|| ".flac".to_string())
    };
    let desired = canonical_destination(server, &canonical, &ext);
    let target = reserve_unique(desired, reserved, Some(source)).await?;
    let collision = target != canonical_destination(server, &canonical, &ext);

    let mut sidecars = Vec::new();
    for ext in TRACK_SIDECAR_EXTENSIONS {
        let candidate = source.with_extension(ext.trim_start_matches('.'));
        if path_exists(&candidate).await? {
            let sidecar_target = reserve_unique(
                target.with_extension(ext.trim_start_matches('.')),
                reserved,
                Some(&candidate),
            )
            .await?;
            sidecars.push(SidecarPlan {
                source: candidate,
                target: sidecar_target,
                copy_only: false,
            });
        }
    }

    let album_dir = target
        .parent()
        .context("target missing parent directory")?
        .to_path_buf();
    if let Some(source_dir) = source.parent() {
        let mut entries = fs::read_dir(source_dir)
            .await
            .with_context(|| format!("failed to read directory {}", source_dir.display()))?;
        while let Some(entry) = entries.next_entry().await.with_context(|| {
            format!("failed to read directory entry in {}", source_dir.display())
        })? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let is_file = entry
                .file_type()
                .await
                .with_context(|| format!("failed to read file type for {}", path.display()))?
                .is_file();
            if !is_file
                || !ALBUM_SIDECAR_NAMES
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let target_path = album_dir.join(name);
            if !path_exists(&target_path).await? && !reserved.contains(&target_path) {
                reserved.insert(target_path.clone());
                sidecars.push(SidecarPlan {
                    source: path,
                    target: target_path,
                    copy_only: true,
                });
            }
        }
    }

    let bytes = file_len(source).await?;

    Ok(AddPlan {
        source: source.to_path_buf(),
        target,
        sidecars,
        tag_updates: canonical.tag_updates,
        collision,
        bytes,
    })
}

fn canonicalize_for_add(
    mut metadata: TrackMetadata,
    unreleased: bool,
    unreleased_artist: Option<&str>,
) -> CanonicalTrack {
    if !unreleased {
        return canonicalize(&metadata);
    }

    let artist = unreleased_artist.unwrap_or("Ye").to_string();
    metadata.albumartist = artist.clone();
    metadata.artist = artist;
    metadata.album = "Unreleased".to_string();

    let mut canonical = canonicalize(&metadata);
    canonical.tag_updates.albumartist = Some(canonical.albumartist.clone());
    canonical.tag_updates.artist = Some(metadata.artist);
    canonical.tag_updates.album = Some(canonical.album.clone());
    canonical.tag_updates.title = Some(canonical.title.clone());
    canonical
}

async fn collect_sources(inputs: &[PathBuf], accept_supported_audio: bool) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    for input in inputs {
        if !path_exists(input).await? {
            bail!("source not found: {}", input.display());
        }
        if fs::metadata(input)
            .await
            .with_context(|| format!("failed to stat {}", input.display()))?
            .is_file()
        {
            if accepted_input(input, accept_supported_audio) {
                results.push(
                    fs::canonicalize(input)
                        .await
                        .with_context(|| format!("failed to canonicalize {}", input.display()))?,
                );
            }
            continue;
        }
        let predicate = if accept_supported_audio {
            is_supported_audio
        } else {
            is_flac
        };
        for path in fsutil::collect_files_recursive(input, predicate).await? {
            results.push(
                fs::canonicalize(&path)
                    .await
                    .with_context(|| format!("failed to canonicalize {}", path.display()))?,
            );
        }
    }
    results.sort();
    results.dedup();
    Ok(results)
}

fn accepted_input(path: &Path, accept_supported_audio: bool) -> bool {
    if accept_supported_audio {
        is_supported_audio(path)
    } else {
        is_flac(path)
    }
}

async fn reserve_unique(
    mut target: PathBuf,
    reserved: &mut HashSet<PathBuf>,
    current: Option<&Path>,
) -> Result<PathBuf> {
    if available(&target, reserved, current).await? {
        reserved.insert(target.clone());
        return Ok(target);
    }

    let stem = target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = target
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default();
    let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut index = 1;
    loop {
        target = parent.join(format!("{stem} ({index}){ext}"));
        if available(&target, reserved, current).await? {
            reserved.insert(target.clone());
            return Ok(target);
        }
        index += 1;
    }
}

async fn available(
    target: &Path,
    reserved: &HashSet<PathBuf>,
    current: Option<&Path>,
) -> Result<bool> {
    if current.is_some_and(|path| path == target) {
        return Ok(true);
    }
    Ok(!path_exists(target).await? && !reserved.contains(target))
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
}

async fn ensure_server_root(server: &Path) -> Result<()> {
    if !path_exists(server).await? {
        bail!("server root not found: {}", server.display());
    }
    if !fs::metadata(server)
        .await
        .with_context(|| format!("failed to stat {}", server.display()))?
        .is_dir()
    {
        bail!("server root is not directory: {}", server.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::audio::{TrackMetadata, canonical_destination};

    use super::canonicalize_for_add;

    #[test]
    fn unreleased_defaults_to_ye_and_writes_missing_tags() {
        let metadata = TrackMetadata {
            albumartist: "Downloads".to_string(),
            artist: "Downloads".to_string(),
            album: String::new(),
            title: "Never See Me Again".to_string(),
            tracknumber: None,
        };

        let track = canonicalize_for_add(metadata, true, None);

        assert_eq!(
            canonical_destination(Path::new("Music"), &track, ".flac"),
            Path::new("Music/Ye/Unreleased/Never See Me Again.flac")
        );
        assert_eq!(track.tag_updates.albumartist.as_deref(), Some("Ye"));
        assert_eq!(track.tag_updates.artist.as_deref(), Some("Ye"));
        assert_eq!(track.tag_updates.album.as_deref(), Some("Unreleased"));
        assert_eq!(
            track.tag_updates.title.as_deref(),
            Some("Never See Me Again")
        );
    }
}
