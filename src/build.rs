use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use cargo_metadata::camino::Utf8PathBuf;
use rinja::Template;
use tempfile::tempdir;
use uniffi_bindgen::bindings::SwiftBindingsOptions;

use crate::utils::*;
use crate::{apple_platform::ApplePlatform, project::Project};

pub trait BuildExtensions {
    fn build(&self, profile: String, apple_platforms: Vec<ApplePlatform>) -> Result<()>;
}

impl BuildExtensions for Project {
    fn build(&self, profile: String, apple_platforms: Vec<ApplePlatform>) -> Result<()> {
        if profile != "release" && profile != "dev" {
            anyhow::bail!(
                "Profile must be either 'release' or 'dev', found {}",
                profile
            )
        }

        let package = self.uniffi_package()?.name;

        let target_dirs: Vec<_> = if apple_platforms.is_empty() {
            self.build_uniffi_package(&package, &profile, None)?
        } else {
            apple_platforms
                .iter()
                .map(|platform| self.build_uniffi_package(&package, &profile, Some(*platform)))
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

            self.generate_bindings(&libraries[0])?;
        }

        if apple_platforms.is_empty() {
            // TODO: Linux
            unimplemented!("Not implemented for Linux yet")
        } else {
            crate::xcframework::create_xcframework(
                self.cargo_metadata.target_directory.as_std_path(),
                apple_platforms
                    .iter()
                    .flat_map(|p| p.target_triples())
                    .map(|s| s.to_string())
                    .collect(),
                profile.to_string(),
                &self.ffi_module_name,
                self.xcframework_path().as_std_path(),
                self.swift_wrapper_dir().as_std_path(),
            )
        }
    }
}

impl Project {
    fn build_uniffi_package(
        &self,
        package: &str,
        profile: &str,
        platform: Option<ApplePlatform>,
    ) -> Result<Vec<Utf8PathBuf>> {
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

        let cargo_target_dir = &self.cargo_metadata.target_directory;
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

    fn generate_bindings(&self, library_path: &Path) -> Result<PathBuf> {
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

        self.reorganize_binding_files(&out_dir)?;
        self.fix_swift_bindings(&out_dir)?;

        Ok(out_dir)
    }

    fn reorganize_binding_files(&self, bindings_dir: &Path) -> Result<()> {
        #[derive(Template)]
        #[template(path = "module.modulemap", escape = "none")]
        struct ModuleMapTemplate {
            ffi_module_name: String,
            header_files: Vec<String>,
        }

        let headers_dir = bindings_dir.join("Headers");
        fs::recreate_dir(&headers_dir)?;

        let mut header_files = vec![];
        for entry in std::fs::read_dir(bindings_dir)? {
            let entry = entry?;
            if entry.path().extension() == Some("h".as_ref()) {
                header_files.push(entry.file_name().into_string().unwrap());
                fs::move_file(&entry.path(), &headers_dir)?;
            }
        }

        let template = ModuleMapTemplate {
            ffi_module_name: self.ffi_module_name.clone(),
            header_files,
        };
        let content = template.render()?;
        let mut modulemap = File::create_new(headers_dir.join("module.modulemap"))?;
        modulemap.write_all(content.as_bytes())?;

        Ok(())
    }

    fn fix_swift_bindings(&self, dir: &Path) -> Result<()> {
        let swift_files = dir.files_with_extension("swift")?;
        let tempdir = tempdir()?;

        #[derive(Template)]
        #[template(path = "binding-prefix.swift", escape = "none")]
        struct PrefixTemplate {
            ffi_module_name: String,
        }
        let prefix = PrefixTemplate {
            ffi_module_name: self.ffi_module_name.clone(),
        }
        .render()?;

        for path in swift_files {
            let reader = BufReader::new(File::open(&path)?);
            let tempfile_path = tempdir.path().join("temp.swift");
            let mut tempfile = File::create(&tempfile_path)?;

            writeln!(tempfile, "{}\n", prefix)?;

            for line in reader.lines() {
                let mut line = line?;
                if line == "protocol UniffiForeignFutureTask {" {
                    line = "fileprivate protocol UniffiForeignFutureTask {".to_string()
                }

                writeln!(tempfile, "{}", line)?;
            }

            tempfile.sync_all()?;
            std::mem::drop(tempfile);

            std::fs::rename(tempfile_path, path)?
        }

        Ok(())
    }
}
