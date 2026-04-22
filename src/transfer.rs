use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Result, bail, Context};

const BAR_WIDTH: usize = 40;
const REDRAW_INTERVAL: Duration = Duration::from_millis(80);

pub struct ProgressBar {
    label: String,
    unit: Unit,
    total: u64,
    position: u64,
    started: Instant,
    last_draw: Instant,
    drawn: bool,
}

enum Unit {
    Count,
    Bytes,
}

impl ProgressBar {
    pub fn inc(&mut self, amount: u64) {
        self.position = self.position.saturating_add(amount).min(self.total);
        let now = Instant::now();
        if now.duration_since(self.last_draw) >= REDRAW_INTERVAL || self.position == self.total {
            self.draw(false);
            self.last_draw = now;
        }
    }

    pub fn finish_and_clear(&mut self) {
        if self.drawn {
            let mut stderr = io::stderr().lock();
            let _ = write!(stderr, "\r{:width$}\r", "", width = 100);
            let _ = stderr.flush();
            self.drawn = false;
        }
    }

    fn draw(&mut self, final_draw: bool) {
        let filled = if self.total == 0 {
            BAR_WIDTH
        } else {
            ((self.position as f64 / self.total as f64) * BAR_WIDTH as f64)
                .round()
                .clamp(0.0, BAR_WIDTH as f64) as usize
        };

        let mut bar = String::with_capacity(BAR_WIDTH);
        for index in 0..BAR_WIDTH {
            bar.push(if index < filled { '=' } else { ' ' });
        }

        let elapsed = self.started.elapsed();
        let suffix = match self.unit {
            Unit::Count => format!(
                "{}/{} {}",
                self.position,
                self.total,
                format_duration(eta(self.position, self.total, elapsed))
            ),
            Unit::Bytes => format!(
                "{}/{} {}/s {}",
                format_bytes(self.position),
                format_bytes(self.total),
                format_bytes(rate(self.position, elapsed)),
                format_duration(eta(self.position, self.total, elapsed))
            ),
        };

        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "\r{:<20} [{}] {}", self.label, bar, suffix);
        if final_draw {
            let _ = writeln!(stderr);
        }
        let _ = stderr.flush();
        self.drawn = true;
    }
}

pub fn move_with_progress(source: &Path, target: &Path, pb: &mut ProgressBar) -> Result<()> {
    let size = source.metadata().map(|meta| meta.len()).unwrap_or(0);
    if target.exists() && target != source {
        bail!("destination already exists: {}", target.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    match fs::rename(source, target) {
        Ok(()) => {
            pb.inc(size);
            Ok(())
        }
        Err(_) => {
            copy_file(source, target, pb)?;
            fs::remove_file(source)
                .with_context(|| format!("failed to remove source file {}", source.display()))?;
            Ok(())
        }
    }
}

pub fn copy_with_progress(source: &Path, target: &Path, pb: &mut ProgressBar) -> Result<()> {
    if target.exists() && target != source {
        bail!("destination already exists: {}", target.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    copy_file(source, target, pb)
}

pub fn bytes_progress(total: u64, message: &str) -> ProgressBar {
    ProgressBar {
        label: message.to_string(),
        unit: Unit::Bytes,
        total,
        position: 0,
        started: Instant::now(),
        last_draw: Instant::now(),
        drawn: false,
    }
}

pub fn count_progress(total: u64, message: &str) -> ProgressBar {
    ProgressBar {
        label: message.to_string(),
        unit: Unit::Count,
        total,
        position: 0,
        started: Instant::now(),
        last_draw: Instant::now(),
        drawn: false,
    }
}

fn copy_file(source: &Path, target: &Path, pb: &mut ProgressBar) -> Result<()> {
    let mut input = fs::File::open(source)
        .with_context(|| format!("failed to open source file {}", source.display()))?;
    let mut output = fs::File::create(target)
        .with_context(|| format!("failed to create target file {}", target.display()))?;

    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("failed to read source file {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("failed to write target file {}", target.display()))?;
        pb.inc(read as u64);
    }
    output
        .flush()
        .with_context(|| format!("failed to flush target file {}", target.display()))?;
    Ok(())
}

fn eta(position: u64, total: u64, elapsed: Duration) -> Duration {
    if position == 0 || position >= total {
        return Duration::ZERO;
    }
    let nanos_per_unit = elapsed.as_nanos() / position as u128;
    let remaining = total.saturating_sub(position) as u128;
    Duration::from_nanos((nanos_per_unit.saturating_mul(remaining)).min(u64::MAX as u128) as u64)
}

fn rate(position: u64, elapsed: Duration) -> u64 {
    let secs = elapsed.as_secs_f64();
    if secs <= f64::EPSILON {
        0
    } else {
        (position as f64 / secs) as u64
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{}{}", bytes, UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}
