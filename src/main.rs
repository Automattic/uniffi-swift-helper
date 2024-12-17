mod apple_platform;
mod build;
mod cli;
mod spm;
mod utils;
mod xcframework;

fn main() -> anyhow::Result<()> {
    cli::Cli::execute()
}
