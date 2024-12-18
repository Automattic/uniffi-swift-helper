use std::collections::HashMap;
use std::env;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::apple_platform::ApplePlatform;
use crate::build;
use crate::spm;

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

#[derive(Parser)]
struct GeneratePackageArgs {
    #[arg(long)]
    package: String,
    #[arg(long)]
    ffi_module_name: String,
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

    build::build(
        args.package,
        args.profile,
        args.ffi_module_name,
        apple_platforms,
    )
}

fn generate_package(args: GeneratePackageArgs) -> Result<()> {
    let map = args
        .package_name_map
        .split(',')
        .map(|pair| {
            let mut iter = pair.split(':');
            let key = iter.next().unwrap();
            let value = iter.next().unwrap();
            (key.to_string(), value.to_string())
        })
        .collect::<HashMap<String, String>>();

    // spm::generate_swift_package(&args.package, map)
    // spm::generate_swift_package(args.package, args.ffi_module_name, args.project_name, map)
    spm::generate_swift_package2(args.ffi_module_name, args.project_name, map)
}
