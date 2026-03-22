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
    /// Clean install from lockfile (like npm ci)
    Ci,
    /// Import lockfile from another package manager
    Import {
        /// Source format: npm
        #[arg(default_value = "npm")]
        source: String,
    },
    /// Link a local package for development
    Link {
        /// Package name to link (omit to register current package)
        package: Option<String>,
    },
    /// Remove a local package link
    Unlink {
        /// Package name to unlink
        package: String,
    },
    /// Show outdated packages
    Outdated,
    /// Update packages to latest versions
    Update {
        /// Specific packages to update (omit for all)
        packages: Vec<String>,
    },
    /// Run a script defined in package.json
    Run {
        /// Script name to run
        script: String,
        /// Additional arguments to pass to the script
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
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
        Commands::Ci => commands::ci::run().await?,
        Commands::Import { source } => commands::import::run(&source)?,
        Commands::Link { package } => commands::link::run(package.as_deref())?,
        Commands::Unlink { package } => commands::link::unlink(&package)?,
        Commands::Outdated => commands::outdated::run().await?,
        Commands::Update { packages } => commands::update::run(&packages).await?,
        Commands::Run { script, args } => {
            let code = commands::run::run(&script, &args)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }

    Ok(())
}
