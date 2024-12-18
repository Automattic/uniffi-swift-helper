use std::env;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::apple_platform::ApplePlatform;
use crate::build::BuildExtensions;
use crate::project::Project;
use crate::spm::*;

#[derive(Parser)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build(BuildArgs),
    GeneratePackage(GeneratePackageArgs),
}

#[derive(Parser)]
struct BuildArgs {
    #[arg(long)]
    only_ios: bool,
    #[arg(long)]
    only_macos: bool,
    #[arg(long)]
    profile: String,
}

#[derive(Parser)]
struct GeneratePackageArgs {
    #[arg(long)]
    project_name: String,
    #[arg(long)]
    package_name_map: String,
}

impl Cli {
    pub fn execute() -> Result<()> {
        let args = Cli::parse();
        match args.command {
            Commands::Build(args) => build(args),
            Commands::GeneratePackage(args) => generate_package(args),
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

    let project = Project::new()?;
    project.build(args.profile, apple_platforms)
}

fn generate_package(args: GeneratePackageArgs) -> Result<()> {
    let project = Project::new()?;
    project.generate_swift_package(args.project_name)
}
