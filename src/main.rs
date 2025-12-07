use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo-sane",
    bin_name = "cargo",
    version,
    about = "🧠 Stop losing your mind over Rust dependency conflicts",
    long_about = "cargo-sane helps you manage Rust dependencies intelligently.\n\
                  It checks for updates, resolves conflicts, and keeps your Cargo.toml clean."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze your dependencies and show update availability
    #[command(alias = "c")]
    Check {
        /// Path to Cargo.toml (default: current directory)
        #[arg(short, long)]
        manifest_path: Option<String>,

        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,

        /// Analyze all workspace members
        #[arg(short, long)]
        workspace: bool,
    },

    /// Update dependencies interactively
    #[command(alias = "u")]
    Update {
        /// Path to Cargo.toml
        #[arg(short, long)]
        manifest_path: Option<String>,

        /// Perform a dry run without making changes
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Update all dependencies without prompting
        #[arg(short, long)]
        all: bool,

        /// Update all workspace members
        #[arg(short, long)]
        workspace: bool,
    },

    /// Fix dependency conflicts
    #[command(alias = "f")]
    Fix {
        /// Path to Cargo.toml
        #[arg(short, long)]
        manifest_path: Option<String>,

        /// Automatically apply fixes without prompting
        #[arg(short, long)]
        auto: bool,
    },

    /// Clean unused dependencies
    #[command(alias = "cl")]
    Clean {
        /// Path to Cargo.toml
        #[arg(short, long)]
        manifest_path: Option<String>,

        /// Perform a dry run
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Check dependency health (security, maintenance status)
    #[command(alias = "h")]
    Health {
        /// Path to Cargo.toml
        #[arg(short, long)]
        manifest_path: Option<String>,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    // Parse CLI arguments
    // Note: cargo passes "sane" as first arg when called as "cargo sane"
    let args = std::env::args().collect::<Vec<_>>();
    let args = if args.get(1).map(|s| s.as_str()) == Some("sane") {
        // Remove "sane" subcommand
        [&args[..1], &args[2..]].concat()
    } else {
        args
    };

    let cli = Cli::parse_from(args);

    // Import commands module
    use cargo_sane::cli::commands;

    match cli.command {
        Commands::Check {
            manifest_path,
            verbose,
            workspace,
        } => commands::check_command(manifest_path, verbose, workspace),
        Commands::Update {
            manifest_path,
            dry_run,
            all,
            workspace,
        } => commands::update_command(manifest_path, dry_run, all, workspace),
        Commands::Fix {
            manifest_path,
            auto,
        } => commands::fix_command(manifest_path, auto),
        Commands::Clean {
            manifest_path,
            dry_run,
        } => commands::clean_command(manifest_path, dry_run),
        Commands::Health {
            manifest_path,
            json,
        } => commands::health_command(manifest_path, json),
    }
}
