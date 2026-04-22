use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "musee", about = "Music library organizer and repair tool")]
pub struct Cli {
    /// Music library root on server/NAS
    #[arg(short = 's', long = "server", value_name = "PATH", required = true)]
    pub server: PathBuf,

    /// Show extra progress context
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Add tagged FLAC files into canonical server library
    Add(AddArgs),
    /// Repair entire server library in place
    Repair(RepairArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,

    /// One or more FLAC files/directories to import
    #[arg(required = true, value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RepairArgs {
    /// Apply changes. Default mode is dry-run.
    #[arg(long)]
    pub apply: bool,
}
