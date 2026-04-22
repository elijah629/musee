use std::{io, path::Path};

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
};

pub async fn move_with_progress(source: &Path, target: &Path, pb: &ProgressBar) -> Result<()> {
    let size = file_len(source).await.unwrap_or(0);
    if path_exists(target).await? && target != source {
        bail!("destination already exists: {}", target.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    match fs::rename(source, target).await {
        Ok(()) => {
            pb.inc(size);
            Ok(())
        }
        Err(_) => {
            copy_file(source, target, pb).await?;
            fs::remove_file(source)
                .await
                .with_context(|| format!("failed to remove source file {}", source.display()))?;
            Ok(())
        }
    }
}

pub async fn copy_with_progress(source: &Path, target: &Path, pb: &ProgressBar) -> Result<()> {
    if path_exists(target).await? && target != source {
        bail!("destination already exists: {}", target.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    copy_file(source, target, pb).await
}

pub fn bytes_progress(total: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_draw_target(ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::with_template(
            "{msg:20} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {binary_bytes_per_sec} {eta}",
        )
        .expect("valid progress template")
        .progress_chars("=> "),
    );
    pb.set_message(message.to_string());
    pb
}

pub fn count_progress(total: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_draw_target(ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::with_template("{msg:20} [{bar:40.green/white}] {pos}/{len} {eta}")
            .expect("valid progress template")
            .progress_chars("=> "),
    );
    pb.set_message(message.to_string());
    pb
}

pub async fn file_len(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len())
}

pub async fn path_exists(path: &Path) -> Result<bool> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to stat {}", path.display())),
    }
}

async fn copy_file(source: &Path, target: &Path, pb: &ProgressBar) -> Result<()> {
    let mut input = File::open(source)
        .await
        .with_context(|| format!("failed to open source file {}", source.display()))?;
    let mut output = File::create(target)
        .await
        .with_context(|| format!("failed to create target file {}", target.display()))?;

    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read source file {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .await
            .with_context(|| format!("failed to write target file {}", target.display()))?;
        pb.inc(read as u64);
    }
    output
        .flush()
        .await
        .with_context(|| format!("failed to flush target file {}", target.display()))?;
    Ok(())
}
