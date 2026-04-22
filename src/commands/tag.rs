use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tokio::fs;

use crate::{
    audio::{TrackMetadata, read_genre, read_track, write_genre},
    cli::TagArgs,
    fsutil,
    genre::GenreLookup,
    output,
    text::{canonical_primary_artist, normalize_text},
    transfer::{count_progress, path_exists},
};

#[derive(Debug)]
struct TagPlan {
    path: PathBuf,
    genre: String,
    existing_genre: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AlbumKey {
    albumartist: String,
    album: String,
}

#[derive(Debug)]
struct AlbumTrack {
    path: PathBuf,
    existing_genre: Option<String>,
}

#[derive(Debug)]
struct AlbumGroup {
    metadata: TrackMetadata,
    tracks: Vec<AlbumTrack>,
}

pub async fn run(args: &TagArgs) -> Result<()> {
    let sources = resolve_sources(args).await?;
    let tracks = collect_sources(&sources).await?;
    if tracks.is_empty() {
        bail!("no supported FLAC files found in provided sources");
    }

    let scan_pb = count_progress(tracks.len() as u64, "planning tag");
    let mut albums: BTreeMap<AlbumKey, AlbumGroup> = BTreeMap::new();

    for path in &tracks {
        let existing_genre = read_genre(path).await?;
        let metadata = read_track(path).await?;
        let key = album_key(&metadata);

        albums
            .entry(key)
            .and_modify(|group| {
                group.tracks.push(AlbumTrack {
                    path: path.clone(),
                    existing_genre: existing_genre.clone(),
                });
            })
            .or_insert_with(|| AlbumGroup {
                metadata,
                tracks: vec![AlbumTrack {
                    path: path.clone(),
                    existing_genre,
                }],
            });

        scan_pb.inc(1);
    }
    scan_pb.finish_and_clear();

    let mut lookup = GenreLookup::new()?;
    let lookup_pb = count_progress(albums.len() as u64, "looking up album genre");
    let mut plans = Vec::new();
    let mut matched_albums = 0_u64;
    let mut unmatched_albums = 0_u64;
    let mut skipped_existing_albums = 0_u64;
    let mut skipped_existing_files = 0_u64;

    for group in albums.values() {
        if !args.retag && group.tracks.iter().all(|track| track.existing_genre.is_some()) {
            skipped_existing_albums += 1;
            skipped_existing_files += group.tracks.len() as u64;
            lookup_pb.inc(1);
            continue;
        }

        let Some(genre) = lookup.genre_for_album(&group.metadata).await? else {
            unmatched_albums += 1;
            lookup_pb.inc(1);
            continue;
        };

        matched_albums += 1;

        for track in &group.tracks {
            if track.existing_genre.is_some() && !args.retag {
                skipped_existing_files += 1;
                continue;
            }

            plans.push(TagPlan {
                path: track.path.clone(),
                genre: genre.clone(),
                existing_genre: track.existing_genre.clone(),
            });
        }

        lookup_pb.inc(1);
    }
    lookup_pb.finish_and_clear();

    let scope = describe_scope(args);
    output::headline(if args.apply { "apply" } else { "dry-run" }, "tag", &scope);
    output::note(format!(
        "albums {} matched {} skipped_existing {} unmatched {}",
        albums.len(),
        matched_albums,
        skipped_existing_albums,
        unmatched_albums
    ));
    output::note(format!(
        "files {} tagged {} skipped_existing {}",
        tracks.len(),
        plans.len(),
        skipped_existing_files,
    ));

    if !args.apply {
        for plan in &plans {
            match &plan.existing_genre {
                Some(existing) => println!(
                    "TAG {} {} -> {}",
                    plan.path.display(),
                    existing,
                    plan.genre
                ),
                None => println!("TAG {} -> {}", plan.path.display(), plan.genre),
            }
        }
        return Ok(());
    }

    let apply_pb = count_progress(plans.len() as u64, "writing genre");
    let mut updated = 0_u64;
    for plan in &plans {
        if write_genre(&plan.path, &plan.genre).await? {
            updated += 1;
        }
        apply_pb.inc(1);
    }
    apply_pb.finish_and_clear();

    output::note(format!(
        "done updated {} skipped_existing {} unmatched_albums {}",
        updated, skipped_existing_files, unmatched_albums
    ));
    Ok(())
}

async fn resolve_sources(args: &TagArgs) -> Result<Vec<PathBuf>> {
    if args.all {
        let server = args
            .server
            .as_ref()
            .context("--all requires -s, --server <PATH>")?;
        ensure_existing_dir(server, "server root").await?;
        return Ok(vec![server.clone()]);
    }

    if !args.sources.is_empty() {
        return Ok(args.sources.clone());
    }

    if let Some(server) = &args.server {
        ensure_existing_dir(server, "server root").await?;
        return Ok(vec![server.clone()]);
    }

    bail!("provide one or more SOURCE paths, or use -s <PATH>, or pass --all with -s")
}

async fn collect_sources(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for input in inputs {
        if !path_exists(input).await? {
            bail!("source not found: {}", input.display());
        }
        if fs::metadata(input)
            .await
            .with_context(|| format!("failed to stat {}", input.display()))?
            .is_file()
        {
            if is_flac(input) {
                let canonical = fs::canonicalize(input)
                    .await
                    .with_context(|| format!("failed to canonicalize {}", input.display()))?;
                if seen.insert(canonical.clone()) {
                    results.push(canonical);
                }
            }
            continue;
        }

        for path in fsutil::collect_files_recursive(input, is_flac).await? {
            let canonical = fs::canonicalize(&path)
                .await
                .with_context(|| format!("failed to canonicalize {}", path.display()))?;
            if seen.insert(canonical.clone()) {
                results.push(canonical);
            }
        }
    }

    results.sort();
    Ok(results)
}

fn describe_scope(args: &TagArgs) -> PathBuf {
    if let Some(server) = &args.server {
        if args.all || args.sources.is_empty() {
            return server.clone();
        }
    }

    if args.sources.len() == 1 {
        return args.sources[0].clone();
    }

    PathBuf::from(format!("{} sources", args.sources.len()))
}

async fn ensure_existing_dir(path: &Path, label: &str) -> Result<()> {
    if !path_exists(path).await? {
        bail!("{label} not found: {}", path.display());
    }
    if !fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?
        .is_dir()
    {
        bail!("{label} is not directory: {}", path.display());
    }
    Ok(())
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
}

fn album_key(metadata: &TrackMetadata) -> AlbumKey {
    AlbumKey {
        albumartist: normalize_text(&canonical_primary_artist(&metadata.albumartist), false),
        album: normalize_text(&metadata.album, false),
    }
}
