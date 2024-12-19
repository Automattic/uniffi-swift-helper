use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use cargo_metadata::camino::Utf8PathBuf;
use rinja::Template;
use uniffi_bindgen::bindings::SwiftBindingsOptions;

use crate::project::UniffiPackage;
use crate::utils::*;
use crate::{apple_platform::ApplePlatform, project::Project};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoProfile {
    Dev,
    Release,
}

pub trait BuildExtensions {
    fn build(&self, profile: CargoProfile, apple_platforms: Vec<ApplePlatform>) -> Result<()>;
}

impl BuildExtensions for Project {
    fn build(&self, profile: CargoProfile, apple_platforms: Vec<ApplePlatform>) -> Result<()> {
        let package = &self.package.name;

        let targets = if apple_platforms.is_empty() {
            vec![PlatformTarget {
                package: package.clone(),
                profile,
                platform: None,
            }]
        } else {
            apple_platforms
                .iter()
                .map(|platform| PlatformTarget {
                    package: package.clone(),
                    profile,
                    platform: Some(*platform),
                })
                .collect()
        };
        for target in &targets {
            target.build_uniffi_package()?;
            target.generate_bindings(
                &self.cargo_metadata.target_directory,
                &self.ffi_module_name,
            )?;
        }

        if apple_platforms.is_empty() {
            let target_dir = &targets[0].built_dirs(&self.cargo_metadata.target_directory)[0];
            self.create_linux_library(target_dir)?;
        } else {
            crate::xcframework::create_xcframework(
                self.cargo_metadata.target_directory.as_std_path(),
                apple_platforms
                    .iter()
                    .flat_map(|p| p.target_triples())
                    .map(|s| s.to_string())
                    .collect(),
                profile,
                &self.ffi_module_name,
                self.xcframework_path().as_std_path(),
                self.swift_wrapper_dir().as_std_path(),
            )?;
        }

        self.update_swift_wrappers()?;

        Ok(())
    }
}

impl Project {
    fn update_swift_wrappers(&self) -> Result<()> {
        for (path, package) in self.swift_wrapper_files_iter() {
            self.update_swift_wrapper(path, package)?;
        }

        Ok(())
    }

    fn update_swift_wrapper(&self, path: Utf8PathBuf, package: &UniffiPackage) -> Result<()> {
        let tempdir = self.cargo_metadata.target_directory.join("tmp");
        if !tempdir.exists() {
            std::fs::create_dir(&tempdir)?;
        }

        let tempfile_path = tempdir.join("temp.swift");
        if tempfile_path.exists() {
            std::fs::remove_file(&tempfile_path)?;
        }

        let mut tempfile = File::create_new(&tempfile_path)?;

        let content = self.swift_wrapper_prefix(package)?;
        writeln!(tempfile, "{}\n", content)?;

        let original = BufReader::new(File::open(&path)?);
        for line in original.lines() {
            let mut line = line?;
            if line == "protocol UniffiForeignFutureTask {" {
                line = "fileprivate protocol UniffiForeignFutureTask {".to_string()
            }

            writeln!(tempfile, "{}", line)?;
        }

        tempfile.sync_all()?;
        std::mem::drop(tempfile);

        std::fs::rename(tempfile_path, path)?;

        Ok(())
    }

    fn swift_wrapper_prefix(&self, package: &UniffiPackage) -> Result<String> {
        let mut modules_to_import: Vec<String> = vec![];

        package
            .iter()
            .filter(|p| p.name != package.name)
            .for_each(|p| modules_to_import.push(p.internal_module_name().unwrap()));

        let project_ffi_module_name = self.ffi_module_name.clone();
        if package.ffi_module_name()? != project_ffi_module_name {
            modules_to_import.push(project_ffi_module_name);
        }

        Ok(PrefixTemplate { modules_to_import }.render()?)
    }

    fn create_linux_library(&self, target_dir: &Utf8PathBuf) -> Result<()> {
        let mut static_lib = target_dir.files_with_extension("a")?;
        if static_lib.len() != 1 {
            anyhow::bail!("Expected 1 static library, found {:?}", static_lib)
        }
        let static_lib = static_lib.pop().unwrap();

        let headers_dir = target_dir.join("swift-bindings/Headers");
        if !headers_dir.exists() {
            anyhow::bail!("Headers directory not found: {}", &headers_dir)
        }

        let linux_library_dir = self.linux_library_path();
        fs::copy_dir(&headers_dir, &linux_library_dir)?;

        let static_lib_dest = linux_library_dir.join(format!("{}.a", self.ffi_module_name));
        std::fs::copy(&static_lib, &static_lib_dest)?;

        Ok(())
    }
}

struct PlatformTarget {
    package: String,
    profile: CargoProfile,
    platform: Option<ApplePlatform>,
}

impl PlatformTarget {
    fn build_uniffi_package(&self) -> Result<()> {
        let mut build = vec!["cargo"];

        if self
            .platform
            .as_ref()
            .map_or(false, |p| p.requires_nightly_toolchain())
        {
            // TODO: Use a specific nightly toolchain?
            build.extend(["+nightly", "-Z", "build-std=panic_abort,std"]);
        }

        // Include debug symbols.
        let config_debug = format!("profile.{}.debug=true", self.profile.as_str());
        // Abort on panic to include Rust backtrace in crash reports.
        let config_panic = format!(r#"profile.{}.panic="abort""#, self.profile.as_str());
        build.extend(["--config", &config_debug, "--config", &config_panic]);

        build.extend([
            "build",
            "--package",
            self.package.as_str(),
            "--profile",
            self.profile.as_str(),
        ]);

        if let Some(platform) = self.platform {
            for target_triple in platform.target_triples() {
                let mut cmd = Command::new(build[0]);
                platform.set_deployment_target_env(&mut cmd);
                cmd.args(&build[1..]);
                cmd.args(["--target", target_triple]);

                println!("$ {:?}", cmd);
                if !cmd.spawn()?.wait()?.success() {
                    anyhow::bail!(
                        "Failed to build package {} for target {}",
                        self.package,
                        target_triple
                    )
                }
            }
        } else {
            let mut cmd = Command::new(build[0]);
            cmd.args(&build[1..]);

            println!("$ {:?}", cmd);
            if !cmd.spawn()?.wait()?.success() {
                anyhow::bail!("Failed to build package {}", self.package)
            }
        }

        Ok(())
    }

    fn built_dirs(&self, cargo_target_dir: &Utf8PathBuf) -> Vec<Utf8PathBuf> {
        if let Some(platform) = self.platform {
            platform
                .target_triples()
                .into_iter()
                .map(|target| cargo_target_dir.join(target).join(self.profile.dir_name()))
                .collect()
        } else {
            vec![cargo_target_dir.join(self.profile.as_str())]
        }
    }

    fn generate_bindings(
        &self,
        cargo_target_dir: &Utf8PathBuf,
        ffi_module_name: &str,
    ) -> Result<()> {
        for target_dir in self.built_dirs(cargo_target_dir) {
            let libraries = target_dir.files_with_extension("a")?;
            if libraries.len() != 1 {
                anyhow::bail!("Expected 1 library in target dir, found {:?}", libraries)
            }

            let library_path = &libraries[0];
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

            self.reorganize_binding_files(&out_dir, ffi_module_name.to_string())?;
        }

        Ok(())
    }

    fn reorganize_binding_files(&self, bindings_dir: &Path, ffi_module_name: String) -> Result<()> {
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
            ffi_module_name,
            header_files,
        };
        let content = template.render()?;
        let mut modulemap = File::create_new(headers_dir.join("module.modulemap"))?;
        modulemap.write_all(content.as_bytes())?;

        Ok(())
    }
}

#[derive(Template)]
#[template(path = "binding-prefix.swift", escape = "none")]
struct PrefixTemplate {
    modules_to_import: Vec<String>,
}

impl CargoProfile {
    pub fn as_str(&self) -> &str {
        match self {
            CargoProfile::Dev => "dev",
            CargoProfile::Release => "release",
        }
    }

    pub fn dir_name(&self) -> &str {
        match self {
            CargoProfile::Dev => "debug",
            CargoProfile::Release => "release",
        }
    }
}

impl TryFrom<&str> for CargoProfile {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "dev" => Ok(CargoProfile::Dev),
            "release" => Ok(CargoProfile::Release),
            _ => anyhow::bail!("Invalid profile: {}", value),
        }
    }
}

impl TryFrom<String> for CargoProfile {
    type Error = anyhow::Error;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}
