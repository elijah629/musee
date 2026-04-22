use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    audio::{apply_updates, canonical_destination, canonicalize, read_track},
    cli::AddArgs,
    fsutil,
    output,
    transfer::{bytes_progress, count_progress, copy_with_progress, move_with_progress},
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

pub fn run(server: &Path, args: &AddArgs) -> Result<()> {
    ensure_server_root(server)?;

    let sources = collect_sources(&args.sources)?;
    if sources.is_empty() {
        bail!("no supported FLAC files found in provided sources");
    }

    let mut scan_pb = count_progress(sources.len() as u64, "planning add");
    let mut plans = Vec::with_capacity(sources.len());
    let mut reserved = HashSet::new();
    for source in &sources {
        let plan = build_plan(source, server, &mut reserved)?;
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

    let total_bytes: u64 = plans.iter().map(|plan| plan.bytes).sum::<u64>()
        + plans
            .iter()
            .flat_map(|plan| plan.sidecars.iter())
            .filter_map(|sidecar| fs::metadata(&sidecar.source).ok().map(|meta| meta.len()))
            .sum::<u64>();
    let mut bytes_pb = bytes_progress(total_bytes.max(1), "transfers");
    let mut files_pb = count_progress(plans.len() as u64, "adding");

    let mut tags_modified = 0_u64;
    let mut sidecars_done = 0_u64;
    for plan in &plans {
        if apply_updates(&plan.source, &plan.tag_updates)? {
            tags_modified += 1;
        }
        move_with_progress(&plan.source, &plan.target, &mut bytes_pb)?;

        for sidecar in &plan.sidecars {
            if sidecar.copy_only {
                copy_with_progress(&sidecar.source, &sidecar.target, &mut bytes_pb)?;
            } else {
                move_with_progress(&sidecar.source, &sidecar.target, &mut bytes_pb)?;
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

fn build_plan(source: &Path, server: &Path, reserved: &mut HashSet<PathBuf>) -> Result<AddPlan> {
    let metadata = read_track(source)?;
    let canonical = canonicalize(&metadata);
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()))
        .unwrap_or_else(|| ".flac".to_string());
    let desired = canonical_destination(server, &canonical, &ext);
    let target = reserve_unique(desired, reserved, Some(source));
    let collision = target != canonical_destination(server, &canonical, &ext);

    let mut sidecars = Vec::new();
    for ext in TRACK_SIDECAR_EXTENSIONS {
        let candidate = source.with_extension(ext.trim_start_matches('.'));
        if candidate.exists() {
            let sidecar_target = reserve_unique(target.with_extension(ext.trim_start_matches('.')), reserved, Some(&candidate));
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
        for entry in fs::read_dir(source_dir)
            .with_context(|| format!("failed to read directory {}", source_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("failed to read directory entry in {}", source_dir.display())
            })?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !path.is_file() || !ALBUM_SIDECAR_NAMES.iter().any(|item| item.eq_ignore_ascii_case(name)) {
                continue;
            }
            let target_path = album_dir.join(name);
            if !target_path.exists() && !reserved.contains(&target_path) {
                reserved.insert(target_path.clone());
                sidecars.push(SidecarPlan {
                    source: path,
                    target: target_path,
                    copy_only: true,
                });
            }
        }
    }

    let bytes = fs::metadata(source)
        .with_context(|| format!("failed to stat {}", source.display()))?
        .len();

    Ok(AddPlan {
        source: source.to_path_buf(),
        target,
        sidecars,
        tag_updates: canonical.tag_updates,
        collision,
        bytes,
    })
}

fn collect_sources(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    for input in inputs {
        if !input.exists() {
            bail!("source not found: {}", input.display());
        }
        if input.is_file() {
            if is_flac(input) {
                results.push(
                    fs::canonicalize(input)
                        .with_context(|| format!("failed to canonicalize {}", input.display()))?,
                );
            }
            continue;
        }
        for path in fsutil::collect_files_recursive(input, is_flac)? {
            results.push(
                fs::canonicalize(&path)
                    .with_context(|| format!("failed to canonicalize {}", path.display()))?,
            );
        }
    }
    results.sort();
    results.dedup();
    Ok(results)
}

fn reserve_unique(mut target: PathBuf, reserved: &mut HashSet<PathBuf>, current: Option<&Path>) -> PathBuf {
    if available(&target, reserved, current) {
        reserved.insert(target.clone());
        return target;
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
        if available(&target, reserved, current) {
            reserved.insert(target.clone());
            return target;
        }
        index += 1;
    }
}

fn available(target: &Path, reserved: &HashSet<PathBuf>, current: Option<&Path>) -> bool {
    if current.is_some_and(|path| path == target) {
        return true;
    }
    !target.exists() && !reserved.contains(target)
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
}

fn ensure_server_root(server: &Path) -> Result<()> {
    if !server.exists() {
        bail!("server root not found: {}", server.display());
    }
    if !server.is_dir() {
        bail!("server root is not directory: {}", server.display());
    }
    Ok(())
}
