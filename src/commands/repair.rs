use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    audio::{apply_updates, canonical_destination, canonicalize, read_track},
    cli::RepairArgs,
    fsutil,
    output,
    transfer::{bytes_progress, count_progress, move_with_progress},
};

const MOVE_SIDECAR_EXTENSIONS: &[&str] = &[".lrc", ".txt"];

#[derive(Debug, Clone)]
struct RepairPlan {
    source: PathBuf,
    target: PathBuf,
    sidecars: Vec<(PathBuf, PathBuf)>,
    tag_updates: crate::audio::TagUpdates,
    bytes: u64,
}

pub fn run(server: &Path, args: &RepairArgs) -> Result<()> {
    ensure_server_root(server)?;

    let files = collect_library(server)?;
    if files.is_empty() {
        bail!("no FLAC files found under {}", server.display());
    }

    let mut scan_pb = count_progress(files.len() as u64, "planning repair");
    let mut reserved = HashSet::new();
    let mut sidecar_reserved = HashSet::new();
    let mut plans = Vec::with_capacity(files.len());
    for file in &files {
        let plan = build_plan(file, server, &mut reserved, &mut sidecar_reserved)?;
        scan_pb.inc(1);
        plans.push(plan);
    }
    scan_pb.finish_and_clear();

    let move_count = plans.iter().filter(|plan| plan.source != plan.target).count();
    let tag_count = plans.iter().filter(|plan| !plan.tag_updates.is_empty()).count();

    output::headline(if args.apply { "apply" } else { "dry-run" }, "repair", server);
    output::note(format!("files {} moves {} tags {}", plans.len(), move_count, tag_count));

    if !args.apply {
        for plan in &plans {
            if plan.source != plan.target {
                println!("MOVE {} -> {}", plan.source.display(), plan.target.display());
            }
        }
        return Ok(());
    }

    let total_bytes: u64 = plans
        .iter()
        .filter(|plan| plan.source != plan.target)
        .map(|plan| plan.bytes)
        .sum::<u64>()
        + plans
            .iter()
            .flat_map(|plan| plan.sidecars.iter())
            .filter(|(source, target)| source != target)
            .filter_map(|(source, _)| fs::metadata(source).ok().map(|meta| meta.len()))
            .sum::<u64>();
    let mut bytes_pb = bytes_progress(total_bytes.max(1), "transfers");
    let mut files_pb = count_progress(plans.len() as u64, "repairing");

    let mut tags_modified = 0_u64;
    let mut files_moved = 0_u64;
    let mut sidecars_moved = 0_u64;

    for plan in &plans {
        if apply_updates(&plan.source, &plan.tag_updates)? {
            tags_modified += 1;
        }
        if plan.source != plan.target {
            move_with_progress(&plan.source, &plan.target, &mut bytes_pb)?;
            files_moved += 1;
        }
        for (source, target) in &plan.sidecars {
            if source != target && source.exists() {
                move_with_progress(source, target, &mut bytes_pb)?;
                sidecars_moved += 1;
            }
        }
        files_pb.inc(1);
    }

    files_pb.finish_and_clear();
    bytes_pb.finish_and_clear();
    remove_empty_dirs(server)?;

    output::note(format!(
        "done files_moved {} tags {} sidecars {}",
        files_moved, tags_modified, sidecars_moved
    ));
    Ok(())
}

fn build_plan(
    source: &Path,
    server: &Path,
    reserved: &mut HashSet<PathBuf>,
    sidecar_reserved: &mut HashSet<PathBuf>,
) -> Result<RepairPlan> {
    let metadata = read_track(source)?;
    let canonical = canonicalize(&metadata);
    let desired = canonical_destination(server, &canonical, ".flac");
    let target = reserve_unique(desired, reserved, Some(source));

    let mut sidecars = Vec::new();
    for ext in MOVE_SIDECAR_EXTENSIONS {
        let candidate = source.with_extension(ext.trim_start_matches('.'));
        if candidate.exists() {
            let desired_sidecar = target.with_extension(ext.trim_start_matches('.'));
            let target_sidecar = reserve_unique(desired_sidecar, sidecar_reserved, Some(&candidate));
            sidecars.push((candidate, target_sidecar));
        }
    }

    let bytes = fs::metadata(source)
        .with_context(|| format!("failed to stat {}", source.display()))?
        .len();

    Ok(RepairPlan {
        source: source.to_path_buf(),
        target,
        sidecars,
        tag_updates: canonical.tag_updates,
        bytes,
    })
}

fn collect_library(server: &Path) -> Result<Vec<PathBuf>> {
    let mut results = fsutil::collect_files_recursive(server, is_flac)?;
    results.sort();
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

fn remove_empty_dirs(server: &Path) -> Result<()> {
    let mut dirs = fsutil::collect_dirs_recursive(server)?;
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

    for dir in dirs {
        if fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory {}", dir.display()))?
            .next()
            .is_none()
        {
            fs::remove_dir(&dir)
                .with_context(|| format!("failed to remove empty directory {}", dir.display()))?;
        }
    }
    Ok(())
}
