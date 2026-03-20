mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "alchemy",
    version,
    about = "A fast npm package manager written in Rust"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install dependencies from package.json
    Install,
    /// Add a package to dependencies
    Add {
        /// Package name (optionally with version: pkg@version)
        package: String,
        /// Add as dev dependency
        #[arg(short = 'D', long)]
        dev: bool,
    },
    /// Remove a package from dependencies
    Remove {
        /// Package name to remove
        package: String,
    },
    /// Initialize a new package.json
    Init,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Install => commands::install::run().await?,
        Commands::Add { package, dev } => commands::add::run(&package, dev).await?,
        Commands::Remove { package } => commands::remove::run(&package).await?,
        Commands::Init => commands::init::run()?,
    }

    Ok(())
}
