use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::{
    api::{ApiClient, ApiClientConfig},
    config::RuntimeConfig,
    credentials::FileCredentialStore,
    login::{self, LoginError, LoginOptions, TokioLoginSleeper},
    tui, update,
};

/// Command-line arguments for the MESHH terminal client.
#[derive(Debug, Parser)]
#[command(
    name = "meshh",
    version,
    about = "Terminal client for MESHH route deliveries"
)]
pub struct Cli {
    /// Override the MESHH API base URL for this invocation.
    #[arg(long, global = true, value_name = "URL")]
    pub api_base_url: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

/// Supported top-level commands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Authenticate this terminal with MESHH.
    Login,

    /// Open the route delivery terminal UI.
    Tui,

    /// Check for or install the latest release.
    Update {
        /// Only check for a newer release; do not install it.
        #[arg(long)]
        check: bool,

        /// Install without prompting for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
}

/// Runs a parsed CLI command.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Login => {
            let config = RuntimeConfig::load(cli.api_base_url)?;
            run_login_command(&config).await
        }
        Command::Tui => {
            let config = RuntimeConfig::load(cli.api_base_url)?;
            tui::run(&config).await.map_err(Into::into)
        }
        Command::Update { check, yes } => run_update_command(check, yes).await,
    }
}

async fn run_update_command(check_only: bool, yes: bool) -> Result<()> {
    let check = update::check_for_update().await?;
    let mut output = io::stdout();

    update::write_check_report(&mut output, &check)?;

    if check_only || !check.update_available() {
        return Ok(());
    }

    let install_dir = update::current_install_dir()?;
    writeln!(output)?;
    writeln!(output, "Install directory: {}", install_dir.display())?;

    if !yes {
        write!(output, "Install update now? [y/N] ")?;
        output.flush()?;

        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;

        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes") {
            writeln!(output, "Update cancelled.")?;
            return Ok(());
        }
    }

    update::install_checked_release(&check, install_dir).await?;

    Ok(())
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
