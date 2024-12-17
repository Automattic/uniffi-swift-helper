mod apple_platform;
mod build;
mod cli;
mod spm;
mod utils;
mod xcframework;

pub fn cli_main() -> anyhow::Result<()> {
    cli::Cli::execute()
}
