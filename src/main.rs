mod audio;
mod cli;
mod commands;
mod encoding;
mod fsutil;
mod genre;
mod output;
mod text;
mod transfer;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, TagTarget};
use tokio::runtime::Builder;

fn main() -> Result<()> {
    Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add(args) => {
            let server = args
                .server
                .as_deref()
                .context("`musee add` requires -s, --server <PATH>")?;
            commands::add::run(server, &args).await
        }
        Commands::Dedupe(args) => {
            let server = args
                .server
                .as_deref()
                .context("`musee dedupe` requires -s, --server <PATH>")?;
            commands::dedupe::run(server, &args).await
        }
        Commands::Repair(args) => {
            let server = args
                .server
                .as_deref()
                .context("`musee repair` requires -s, --server <PATH>")?;
            commands::repair::run(server, &args).await
        }
        Commands::Tag(command) => match command.target {
            TagTarget::Genre(args) => commands::tag::run(&args).await,
        },
    }
}
