use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Command,
};

use std::io::Write;

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand};
use uniffi_bindgen::bindings::SwiftBindingsOptions;

use crate::utils::*;
use crate::{apple_platform::ApplePlatform, xcframework::create_xcframework};

pub fn build(
    package: String,
    profile: String,
    ffi_module_name: String,
    apple_platforms: Vec<ApplePlatform>,
) -> Result<()> {
    if profile != "release" && profile != "dev" {
        anyhow::bail!(
            "Profile must be either 'release' or 'dev', found {}",
            profile
        )
    }

    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .with_context(|| "Can't get cargo metadata")?;
    let package = metadata
        .packages
        .iter()
        .find(|p| p.name == package)
        .context(format!(
            "Package {} not found. Make sure run this command in the cargo root directory",
            package
        ))?;

    let target_dirs: Vec<_> = if apple_platforms.is_empty() {
        build_uniffi_package(&package.name, &profile, None, &metadata)?
    } else {
        apple_platforms
            .iter()
            .map(|platform| {
                build_uniffi_package(&package.name, &profile, Some(*platform), &metadata)
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect()
    };

    for target_dir in target_dirs {
        let libraries = target_dir.files_with_extension("a")?;
        if libraries.len() != 1 {
            anyhow::bail!("Expected 1 library in target dir, found {:?}", libraries)
        }

        generate_bindings(&libraries[0], &ffi_module_name)?;
    }

    if apple_platforms.is_empty() {
        // TODO: Linux
        unimplemented!("Not implemented for Linux yet")
    } else {
        create_xcframework(
            metadata.target_directory.as_std_path(),
            apple_platforms
                .iter()
                .flat_map(|p| p.target_triples())
                .map(|s| s.to_string())
                .collect(),
            profile.to_string(),
            &ffi_module_name,
        )
    }
}

fn build_uniffi_package(
    package: &str,
    profile: &str,
    platform: Option<ApplePlatform>,
    metadata: &Metadata,
) -> Result<Vec<PathBuf>> {
    let profile_dirname = match profile {
        "release" => "release",
        "dev" => "debug",
        _ => anyhow::bail!("Invalid profile: {}", profile),
    };

    let mut build = vec!["cargo"];

    if platform
        .as_ref()
        .map_or(false, |p| p.requires_nightly_toolchain())
    {
        // TODO: Use a specific nightly toolchain?
        build.extend(["+nightly", "-Z", "build-std=panic_abort,std"]);
    }

    // Include debug symbols.
    let config_debug = format!("profile.{}.debug=true", profile);
    // Abort on panic to include Rust backtrace in crash reports.
    let panic_config = format!(r#"profile.{}.panic="abort""#, profile);
    build.extend(["--config", &config_debug, "--config", &panic_config]);

    build.extend(["build", "--package", package, "--profile", profile]);

    let cargo_target_dir: &Path = metadata.target_directory.as_std_path();

    let targets = platform.as_ref().map_or(vec![], |p| p.target_triples());
    if targets.is_empty() {
        let mut cmd = Command::new(build[0]);
        cmd.args(&build[1..]);

        println!("$ {:?}", cmd);
        if !cmd.spawn()?.wait()?.success() {
            anyhow::bail!("Failed to build package {}", package)
        }

        let target_dir = cargo_target_dir.join(profile_dirname);
        Ok(vec![target_dir])
    } else {
        targets
            .into_iter()
            .map(|target| {
                let mut cmd = Command::new(build[0]);
                cmd.args(&build[1..]);
                cmd.args(["--target", target]);

                println!("$ {:?}", cmd);
                if !cmd.spawn()?.wait()?.success() {
                    anyhow::bail!("Failed to build package {} for target {}", package, target)
                }

                let target_dir = cargo_target_dir.join(target).join(profile_dirname);

                Ok(target_dir)
            })
            .collect()
    }
}

fn generate_bindings(library_path: &Path, module_name: &str) -> Result<PathBuf> {
    let out_dir = library_path.parent().unwrap().join("swift-bindings");
    fs::recreate_dir(&out_dir)?;

    let options = SwiftBindingsOptions {
        generate_swift_sources: true,
        generate_headers: true,
        generate_modulemap: false,
        library_path: library_path.to_path_buf().try_into()?,
        out_dir: out_dir.clone().try_into()?,
        xcframework: false,
        module_name: None,
        modulemap_filename: None,
        metadata_no_deps: false,
    };
    uniffi_bindgen::bindings::generate_swift_bindings(options)?;

    reorganize_binding_files(&out_dir, module_name)?;

    Ok(out_dir)
}

fn reorganize_binding_files(bindings_dir: &Path, module_name: &str) -> Result<()> {
    let headers_dir = bindings_dir.join("Headers");
    fs::recreate_dir(&headers_dir)?;

    let modulemap_path = headers_dir.join("module.modulemap");
    let mut modulemap = File::create_new(modulemap_path)?;
    writeln!(modulemap, r#"module {} {{"#, module_name)?;
    for entry in std::fs::read_dir(bindings_dir)? {
        let entry = entry?;
        if entry.path().extension() == Some("h".as_ref()) {
            writeln!(
                modulemap,
                r#"    header "{}""#,
                entry.file_name().to_str().unwrap()
            )?;
            fs::move_file(&entry.path(), &headers_dir)?;
        }
    }
    writeln!(modulemap, r#"    export *"#)?;
    writeln!(modulemap, r#"}}"#)?;

    // TODO: https://github.com/mozilla/uniffi-rs/pull/2341
    // sed -i '' 's/^protocol UniffiForeignFutureTask /fileprivate protocol UniffiForeignFutureTask /' ".swift"

    Ok(())
}
