use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::task;
use walkdir::WalkDir;

pub async fn collect_files_recursive(
    root: &Path,
    predicate: fn(&Path) -> bool,
) -> Result<Vec<PathBuf>> {
    let root = root.to_path_buf();
    task::spawn_blocking(move || {
        let mut out = Vec::new();
        for entry in WalkDir::new(&root) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err)
                    if err
                        .io_error()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied) =>
                {
                    continue;
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed while walking {}", root.display()));
                }
            };

            let path = entry.path();
            if entry.file_type().is_file() && predicate(path) {
                out.push(path.to_path_buf());
            }
        }
        Ok(out)
    })
    .await
    .context("file walker task failed")?
}

pub async fn collect_dirs_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let root = root.to_path_buf();
    task::spawn_blocking(move || {
        let mut out = Vec::new();
        for entry in WalkDir::new(&root).min_depth(1) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err)
                    if err
                        .io_error()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied) =>
                {
                    continue;
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed while walking {}", root.display()));
                }
            };

            if entry.file_type().is_dir() {
                out.push(entry.into_path());
            }
        }
        Ok(out)
    })
    .await
    .context("directory walker task failed")?
}
