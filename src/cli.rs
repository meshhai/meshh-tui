use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::config::RuntimeConfig;

/// Command-line arguments for the Meshh terminal client.
#[derive(Debug, Parser)]
#[command(
    name = "meshh",
    version,
    about = "Terminal client for Meshh route deliveries"
)]
pub struct Cli {
    /// Override the Meshh API base URL for this invocation.
    #[arg(long, global = true, value_name = "URL")]
    pub api_base_url: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

/// Supported top-level commands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Authenticate this terminal with Meshh.
    Login,

    /// Open the route delivery terminal UI.
    Tui,
}

/// Runs a parsed CLI command.
pub fn run(cli: Cli) -> Result<()> {
    let _config = RuntimeConfig::load(cli.api_base_url)?;

    match cli.command {
        Command::Login => bail!("`meshh login` is not implemented yet"),
        Command::Tui => bail!("`meshh tui` is not implemented yet"),
    }
}
