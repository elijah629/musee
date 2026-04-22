mod audio;
mod cli;
mod commands;
mod fsutil;
mod output;
mod text;
mod transfer;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add(args) => commands::add::run(&cli.server, &args),
        Commands::Repair(args) => commands::repair::run(&cli.server, &args),
    }
}
