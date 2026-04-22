use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tokio::fs;

use crate::{
    audio::{apply_updates, canonical_destination, canonicalize, read_track},
    cli::AddArgs,
    fsutil,
    output,
    transfer::{bytes_progress, count_progress, copy_with_progress, file_len, move_with_progress, path_exists},
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

    let sources = collect_sources(&args.sources).await?;
    if sources.is_empty() {
        bail!("no supported FLAC files found in provided sources");
    }

    let scan_pb = count_progress(sources.len() as u64, "planning add");
    let mut plans = Vec::with_capacity(sources.len());
    let mut reserved = HashSet::new();
    for source in &sources {
        let plan = build_plan(source, server, &mut reserved).await?;
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
    for future in plans.iter().flat_map(|plan| plan.sidecars.iter()).map(|sidecar| file_len(&sidecar.source)) {
        sidecar_bytes += future.await.unwrap_or(0);
    }
    let total_bytes = total_bytes + sidecar_bytes;
    let bytes_pb = bytes_progress(total_bytes.max(1), "transfers");
    let files_pb = count_progress(plans.len() as u64, "adding");

    let mut tags_modified = 0_u64;
    let mut sidecars_done = 0_u64;
    for plan in &plans {
        if apply_updates(&plan.source, &plan.tag_updates).await? {
            tags_modified += 1;
        }
        move_with_progress(&plan.source, &plan.target, &bytes_pb).await?;

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

async fn build_plan(source: &Path, server: &Path, reserved: &mut HashSet<PathBuf>) -> Result<AddPlan> {
    let metadata = read_track(source).await?;
    let canonical = canonicalize(&metadata);
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()))
        .unwrap_or_else(|| ".flac".to_string());
    let desired = canonical_destination(server, &canonical, &ext);
    let target = reserve_unique(desired, reserved, Some(source)).await?;
    let collision = target != canonical_destination(server, &canonical, &ext);

    let mut sidecars = Vec::new();
    for ext in TRACK_SIDECAR_EXTENSIONS {
        let candidate = source.with_extension(ext.trim_start_matches('.'));
        if path_exists(&candidate).await? {
            let sidecar_target = reserve_unique(target.with_extension(ext.trim_start_matches('.')), reserved, Some(&candidate)).await?;
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
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("failed to read directory entry in {}", source_dir.display()))?
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let is_file = entry
                .file_type()
                .await
                .with_context(|| format!("failed to read file type for {}", path.display()))?
                .is_file();
            if !is_file || !ALBUM_SIDECAR_NAMES.iter().any(|item| item.eq_ignore_ascii_case(name)) {
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

async fn collect_sources(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
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
            if is_flac(input) {
                results.push(
                    fs::canonicalize(input)
                        .await
                        .with_context(|| format!("failed to canonicalize {}", input.display()))?,
                );
            }
            continue;
        }
        for path in fsutil::collect_files_recursive(input, is_flac).await? {
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

async fn reserve_unique(mut target: PathBuf, reserved: &mut HashSet<PathBuf>, current: Option<&Path>) -> Result<PathBuf> {
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

async fn available(target: &Path, reserved: &HashSet<PathBuf>, current: Option<&Path>) -> Result<bool> {
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
