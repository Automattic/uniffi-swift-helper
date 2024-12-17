use std::env;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::apple_platform::ApplePlatform;
use crate::build;

#[derive(Parser)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build(BuildArgs),
}

#[derive(Parser)]
struct BuildArgs {
    #[arg(long)]
    package: String,
    #[arg(long)]
    only_ios: bool,
    #[arg(long)]
    only_macos: bool,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    ffi_module_name: String,
}

impl Cli {
    pub fn execute() -> Result<()> {
        let args = Cli::parse();
        match args.command {
            Commands::Build(args) => build(args),
        }
    }
}

fn build(args: BuildArgs) -> Result<()> {
    let apple_platforms = if args.only_ios {
        vec![ApplePlatform::IOS]
    } else if args.only_macos {
        vec![ApplePlatform::MacOS]
    } else if env::consts::OS == "macos" {
        ApplePlatform::all()
    } else {
        vec![]
    };

    build::build(
        args.package,
        args.profile,
        args.ffi_module_name,
        apple_platforms,
    )
}
