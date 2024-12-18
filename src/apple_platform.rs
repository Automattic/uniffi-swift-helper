use std::{fmt::Display, process::Command};

use crate::spm::DeploymentTargets;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum ApplePlatform {
    MacOS,
    #[allow(clippy::upper_case_acronyms)]
    IOS,
    TvOS,
    WatchOS,
}

impl ApplePlatform {
    pub fn all() -> Vec<Self> {
        vec![Self::MacOS, Self::IOS, Self::TvOS, Self::WatchOS]
    }

    pub fn target_triples(&self) -> Vec<&'static str> {
        match self {
            Self::IOS => vec![
                "aarch64-apple-ios",
                "x86_64-apple-ios",
                "aarch64-apple-ios-sim",
            ],
            Self::MacOS => vec!["x86_64-apple-darwin", "aarch64-apple-darwin"],
            Self::WatchOS => vec![
                "arm64_32-apple-watchos",
                "x86_64-apple-watchos-sim",
                "aarch64-apple-watchos-sim",
            ],
            Self::TvOS => vec!["aarch64-apple-tvos", "aarch64-apple-tvos-sim"],
        }
    }

    pub fn requires_nightly_toolchain(&self) -> bool {
        matches!(self, Self::TvOS | Self::WatchOS)
    }

    pub fn set_deployment_target_env(&self, command: &mut Command) {
        let (key, value) = self.deployment_targets_env();
        command.env(key, value);
    }

    fn deployment_targets_env(&self) -> (&'static str, &'static str) {
        match self {
            Self::IOS => ("IOS_DEPLOYMENT_TARGET", DeploymentTargets::ios()),
            Self::MacOS => ("MACOSX_DEPLOYMENT_TARGET", DeploymentTargets::macos()),
            Self::TvOS => ("TVOS_DEPLOYMENT_TARGET", DeploymentTargets::tvos()),
            Self::WatchOS => ("WATCHOS_DEPLOYMENT_TARGET", DeploymentTargets::watchos()),
        }
    }
}

impl TryFrom<&str> for ApplePlatform {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> std::result::Result<Self, anyhow::Error> {
        match s {
            "darwin" => Ok(ApplePlatform::MacOS),
            "ios" => Ok(ApplePlatform::IOS),
            "tvos" => Ok(ApplePlatform::TvOS),
            "watchos" => Ok(ApplePlatform::WatchOS),
            _ => anyhow::bail!("Unknown Apple platform: {}", s),
        }
    }
}

impl Display for ApplePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ApplePlatform::MacOS => "macos",
            ApplePlatform::IOS => "ios",
            ApplePlatform::TvOS => "tvos",
            ApplePlatform::WatchOS => "watchos",
        };
        write!(f, "{}", name)
    }
}
