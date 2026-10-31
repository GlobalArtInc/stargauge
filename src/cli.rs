use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    about = "Track the stars of a GitHub repository: statistics, charts and an HTML report",
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        "\nBased on Shotstars by Snooppr. Copyright (C) 2024 Snooppr, (C) 2026 GlobalArt.",
        "\nLicense GPL-3.0-or-later <https://www.gnu.org/licenses/gpl-3.0.html>.",
        "\nThis is free software: you are free to change and redistribute it. There is NO WARRANTY.",
    ),
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[arg(help = "Repository as owner/repo or a GitHub URL")]
    pub repo: Option<String>,
    #[arg(long, global = true, help = "Do not open the report in the browser")]
    pub no_open: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Pick a previously scanned repository by number")]
    History,
    #[command(about = "Delete scan data, the config file is kept")]
    Clear {
        #[arg(long, help = "Do not ask for confirmation")]
        yes: bool,
    },
    #[command(about = "Print the data directory and the config file")]
    Paths,
}
