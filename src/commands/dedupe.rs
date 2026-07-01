use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tokio::fs;

use crate::{
    audio::{canonical_destination, canonicalize, read_track},
    cli::DedupeArgs,
    encoding::{AudioFingerprint, fingerprint},
    fsutil, output,
    transfer::{count_progress, path_exists},
};

const TRACK_SIDECAR_EXTENSIONS: &[&str] = &["lrc", "txt"];
const ALBUM_SIDECAR_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "folder.jpg",
    "folder.jpeg",
    "folder.png",
    "album.nfo",
];

#[derive(Debug)]
struct DuplicateGroup {
    keeper: PathBuf,
    duplicates: Vec<PathBuf>,
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    canonical: bool,
}

pub async fn run(server: &Path, args: &DedupeArgs) -> Result<()> {
    ensure_server_root(server).await?;
    let mut files = fsutil::collect_files_recursive(server, is_flac).await?;
    files.sort();
    if files.is_empty() {
        bail!("no FLAC files found under {}", server.display());
    }

    let scan_pb = count_progress(files.len() as u64, "finding duplicates");
    let mut by_fingerprint: HashMap<AudioFingerprint, Vec<Candidate>> = HashMap::new();
    let mut missing_fingerprints = 0_usize;
    for file in &files {
        match fingerprint(file).await? {
            Some(value) => {
                let track = canonicalize(&read_track(file).await?);
                let canonical = canonical_destination(server, &track, ".flac") == *file;
                by_fingerprint.entry(value).or_default().push(Candidate {
                    path: file.clone(),
                    canonical,
                });
            }
            None => missing_fingerprints += 1,
        }
        scan_pb.inc(1);
    }
    scan_pb.finish_and_clear();

    let mut groups = by_fingerprint
        .into_values()
        .filter(|candidates| candidates.len() > 1)
        .map(|mut candidates| {
            candidates.sort_by(|left, right| {
                right
                    .canonical
                    .cmp(&left.canonical)
                    .then_with(|| left.path.cmp(&right.path))
            });
            DuplicateGroup {
                keeper: candidates.remove(0).path,
                duplicates: candidates
                    .into_iter()
                    .map(|candidate| candidate.path)
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| left.keeper.cmp(&right.keeper));

    let removals = groups
        .iter()
        .flat_map(|group| group.duplicates.iter().cloned())
        .collect::<HashSet<_>>();
    let redundant_album_dirs = redundant_album_dirs(&files, &removals);

    output::headline(
        if args.apply { "apply" } else { "dry-run" },
        "dedupe",
        server,
    );
    output::note(format!(
        "duplicate_groups {} tracks_to_remove {} redundant_albums {} unverified {}",
        groups.len(),
        removals.len(),
        redundant_album_dirs.len(),
        missing_fingerprints
    ));

    for group in &groups {
        for duplicate in &group.duplicates {
            println!(
                "REMOVE {} (duplicate of {})",
                duplicate.display(),
                group.keeper.display()
            );
        }
    }
    if !args.apply {
        return Ok(());
    }

    let remove_pb = count_progress(removals.len() as u64, "removing duplicates");
    let mut sidecars_removed = 0_usize;
    for duplicate in &removals {
        fs::remove_file(duplicate)
            .await
            .with_context(|| format!("failed to remove duplicate {}", duplicate.display()))?;
        for extension in TRACK_SIDECAR_EXTENSIONS {
            let sidecar = duplicate.with_extension(extension);
            if path_exists(&sidecar).await? {
                fs::remove_file(&sidecar).await.with_context(|| {
                    format!("failed to remove duplicate sidecar {}", sidecar.display())
                })?;
                sidecars_removed += 1;
            }
        }
        remove_pb.inc(1);
    }
    remove_pb.finish_and_clear();

    for directory in &redundant_album_dirs {
        remove_known_album_sidecars(directory).await?;
    }
    remove_empty_dirs(server).await?;

    output::note(format!(
        "done tracks_removed {} track_sidecars_removed {} redundant_albums_cleaned {}",
        removals.len(),
        sidecars_removed,
        redundant_album_dirs.len()
    ));
    Ok(())
}

fn redundant_album_dirs(files: &[PathBuf], removals: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut files_by_dir: HashMap<PathBuf, Vec<&PathBuf>> = HashMap::new();
    for file in files {
        if let Some(parent) = file.parent() {
            files_by_dir
                .entry(parent.to_path_buf())
                .or_default()
                .push(file);
        }
    }
    let mut directories = files_by_dir
        .into_iter()
        .filter_map(|(directory, tracks)| {
            tracks
                .iter()
                .all(|track| removals.contains(*track))
                .then_some(directory)
        })
        .collect::<Vec<_>>();
    directories.sort();
    directories
}

async fn remove_known_album_sidecars(directory: &Path) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .await
        .with_context(|| format!("failed to read directory {}", directory.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if entry.file_type().await?.is_file()
            && ALBUM_SIDECAR_NAMES
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(name))
        {
            fs::remove_file(&path).await.with_context(|| {
                format!(
                    "failed to remove redundant album sidecar {}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

async fn remove_empty_dirs(server: &Path) -> Result<()> {
    let mut directories = fsutil::collect_dirs_recursive(server).await?;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        let mut entries = fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read directory {}", directory.display()))?;
        if entries.next_entry().await?.is_none() {
            fs::remove_dir(&directory).await.with_context(|| {
                format!("failed to remove empty directory {}", directory.display())
            })?;
        }
    }
    Ok(())
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
    use std::{collections::HashSet, path::PathBuf};

    use super::redundant_album_dirs;

    #[test]
    fn recognizes_an_album_when_every_track_is_removed() {
        let first = PathBuf::from("Artist/Copy/01.flac");
        let second = PathBuf::from("Artist/Copy/02.flac");
        let keep = PathBuf::from("Artist/Original/01.flac");
        let removals = HashSet::from([first.clone(), second.clone()]);

        assert_eq!(
            redundant_album_dirs(&[first, second, keep], &removals),
            vec![PathBuf::from("Artist/Copy")]
        );
    }
}
