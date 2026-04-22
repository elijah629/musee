use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "musee", about = "Music library organizer and repair tool")]
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
    /// Add tagged FLAC files into canonical server library. Requires -s, --server.
    Add(AddArgs),
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

    /// One or more FLAC files/directories to import
    #[arg(required = true, value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,
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
