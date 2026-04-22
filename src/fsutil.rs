use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub fn collect_files_recursive<F>(root: &Path, mut predicate: F) -> Result<Vec<PathBuf>>
where
    F: FnMut(&Path) -> bool,
{
    let mut out = Vec::new();
    visit(root, &mut predicate, &mut out)?;
    Ok(out)
}

pub fn collect_dirs_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    visit_dirs(root, &mut out)?;
    Ok(out)
}

fn visit<F>(path: &Path, predicate: &mut F, out: &mut Vec<PathBuf>) -> Result<()>
where
    F: FnMut(&Path) -> bool,
{
    let entries = fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("failed to read directory entry in {}", path.display()))?;
        let child = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", child.display()))?;
        if file_type.is_dir() {
            visit(&child, predicate, out)?;
        } else if file_type.is_file() && predicate(&child) {
            out.push(child);
        }
    }
    Ok(())
}

fn visit_dirs(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("failed to read directory entry in {}", path.display()))?;
        let child = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", child.display()))?;
        if file_type.is_dir() {
            visit_dirs(&child, out)?;
            out.push(child);
        }
    }
    Ok(())
}
