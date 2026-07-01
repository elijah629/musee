use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "musee",
    version,
    about = "Music library organizer and repair tool"
)]
pub struct Cli {
    /// Music library root on server/NAS
    #[arg(short = 's', long = "server", value_name = "PATH", global = true)]
    pub server: Option<PathBuf>,

    /// Show extra progress context
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Add tagged audio files into canonical server library. Requires -s, --server.
    Add(AddArgs),
    /// Remove duplicate tracks and redundant album directories. Requires -s, --server.
    Dedupe(DedupeArgs),
    /// Repair entire server library in place. Requires -s, --server.
    Repair(RepairArgs),
    /// Tag music files with derived metadata.
    Tag(TagCommand),
}

#[derive(Debug, Args)]
pub struct TagCommand {
    #[command(subcommand)]
    pub target: TagTarget,
}

#[derive(Debug, Subcommand)]
pub enum TagTarget {
    /// Look up genres and tag matching FLAC files. Use --all with -s, or pass explicit paths.
    Genre(TagArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Music library root on server/NAS
    #[arg(from_global)]
    pub server: Option<PathBuf>,

    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,

    /// Transcode every input with the selected encoding profile before import
    #[arg(long, value_enum, value_name = "PROFILE")]
    pub encoding: Option<EncodingProfile>,

    /// Import tracks as an Unreleased album; missing metadata is replaced with Ye defaults
    #[arg(long)]
    pub unreleased: bool,

    /// Override the artist used by --unreleased (default: Ye)
    #[arg(long, value_name = "NAME", requires = "unreleased")]
    pub unreleased_artist: Option<String>,

    /// One or more audio files/directories to import
    #[arg(required = true, value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EncodingProfile {
    /// Sonos-compatible FLAC: 16-bit, <=2 channels, 44.1/48 kHz, 100 seek points
    SonosFlac,
}

#[derive(Debug, Args)]
pub struct DedupeArgs {
    /// Music library root on server/NAS
    #[arg(from_global)]
    pub server: Option<PathBuf>,

    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Args)]
pub struct RepairArgs {
    /// Music library root on server/NAS
    #[arg(from_global)]
    pub server: Option<PathBuf>,

    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Args)]
pub struct TagArgs {
    /// Music library root on server/NAS
    #[arg(from_global)]
    pub server: Option<PathBuf>,

    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,

    /// Tag every FLAC file under --server
    #[arg(long, conflicts_with = "sources")]
    pub all: bool,

    /// Replace existing genre tags instead of skipping them
    #[arg(long)]
    pub retag: bool,

    /// One or more FLAC files/directories to tag
    #[arg(value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,
}
