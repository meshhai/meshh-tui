use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = meshh_tui::cli::Cli::parse();
    meshh_tui::cli::run(cli)
}
