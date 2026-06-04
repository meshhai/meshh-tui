use std::io;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::{
    api::{ApiClient, ApiClientConfig},
    config::RuntimeConfig,
    credentials::FileCredentialStore,
    login::{self, LoginError, LoginOptions, TokioLoginSleeper},
    tui,
};

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
pub async fn run(cli: Cli) -> Result<()> {
    let config = RuntimeConfig::load(cli.api_base_url)?;

    match cli.command {
        Command::Login => run_login_command(&config).await,
        Command::Tui => tui::run(&config).await.map_err(Into::into),
    }
}

async fn run_login_command(config: &RuntimeConfig) -> Result<()> {
    let api = ApiClient::new(ApiClientConfig::from_runtime(config));
    let credentials = FileCredentialStore::new_default()?;
    let sleeper = TokioLoginSleeper;
    let mut output = io::stdout();

    tokio::select! {
        result = login::run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default(),
        ) => {
            result?;
            Ok(())
        }
        signal = tokio::signal::ctrl_c() => {
            signal.context("could not listen for terminal interrupt")?;
            Err(LoginError::Interrupted.into())
        }
    }
}
