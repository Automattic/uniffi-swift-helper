use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::apple_platform::ApplePlatform;
use crate::build::CargoProfile;
use crate::utils::*;

pub fn create_xcframework(
    cargo_target_dir: &Path,
    targets: Vec<String>,
    profile: CargoProfile,
    name: &str,
    xcframework: &Path,
    swift_wrapper: &Path,
) -> Result<()> {
    let temp_dir = cargo_target_dir.join("tmp/wp-rs-xcframework");
    fs::recreate_dir(&temp_dir)?;
    XCFramework::new(&targets, profile)?.create(
        cargo_target_dir,
        name,
        &temp_dir,
        xcframework,
        swift_wrapper,
    )?;

    std::fs::remove_dir_all(&temp_dir).ok();

    Ok(())
}

// Represent a xcframework that contains static libraries for multiple platforms.
//
// Since `xcodebuild -create-xcframework` command requires its `-libraray` not
// having duplicated platform. This type along with `LibraryGroup` and `Slice`
// work together to make it easier to create a xcframework.
struct XCFramework {
    libraries: Vec<LibraryGroup>,
}

// Represent a group of static libraries that are built for the same platform.
struct LibraryGroup {
    id: LibraryGroupId,
    slices: Vec<Slice>,
}

// Represent a thin static library which is built with `cargo build --target <target> --profile <profile>`
struct Slice {
    target: String,
    profile: CargoProfile,
}

impl XCFramework {
    fn new(targets: &Vec<String>, profile: CargoProfile) -> Result<Self> {
        let mut groups = HashMap::<LibraryGroupId, LibraryGroup>::new();
        for target in targets {
            let id = LibraryGroupId::from_target(target)?;
            let id_clone = id.clone();
            groups
                .entry(id)
                .or_insert(LibraryGroup {
                    id: id_clone,
                    slices: Vec::new(),
                })
                .slices
                .push(Slice {
                    target: target.clone(),
                    profile: profile.to_owned(),
                });
        }

        Ok(Self {
            libraries: groups.into_values().collect(),
        })
    }

    fn create(
        &self,
        cargo_target_dir: &Path,
        library_file_name: &str,
        temp_dir: &Path,
        dest: &Path,
        swift_wrapper_dir: &Path,
    ) -> Result<()> {
        self.preview();

        let temp_dest = self.create_xcframework(cargo_target_dir, library_file_name, temp_dir)?;
        self.patch_xcframework(&temp_dest, library_file_name)?;

        fs::recreate_dir(dest)?;
        std::fs::rename(temp_dest, dest).with_context(|| "Failed to move xcframework")?;
        println!("xcframework created at {}", &dest.display());

        // It's okay to use the first element (or any element), since Swift binding files in all
        // targets should be exactly the same.
        fs::recreate_dir(swift_wrapper_dir)?;
        for file in self.libraries[0].swift_binding_files(cargo_target_dir)? {
            let dest = swift_wrapper_dir.join(file.file_name().unwrap());
            std::fs::copy(&file, &dest).with_context(|| {
                format!("Failed to copy {} to {}", file.display(), dest.display())
            })?;
        }
        println!("Swift bindings created at {}", &swift_wrapper_dir.display());

        Ok(())
    }

    fn preview(&self) {
        println!("Creating xcframework to include the following targets:");
        for lib in &self.libraries {
            println!("  Platform: {}", lib.id);
            for slice in &lib.slices {
                println!("    - {}", slice.target);
            }
        }
    }

    fn create_xcframework(
        &self,
        cargo_target_dir: &Path,
        library_file_name: &str,
        temp_dir: &Path,
    ) -> Result<PathBuf> {
        let temp_dest = temp_dir.join(format!("{}.xcframework", library_file_name));
        std::fs::remove_dir_all(&temp_dest).ok();

        let library_args: Result<Vec<(PathBuf, PathBuf)>> = self
            .libraries
            .iter()
            .map(|library| {
                let lib = library.create(cargo_target_dir, library_file_name, temp_dir)?;
                let header = library.headers_dir(cargo_target_dir)?;
                Ok((lib, header))
            })
            .collect();
        let library_args = library_args?;

        let library_args = library_args.iter().flat_map(|(lib, headers)| {
            [
                "-library".as_ref(),
                lib.as_os_str(),
                "-headers".as_ref(),
                headers.as_os_str(),
            ]
        });
        Command::new("xcodebuild")
            .arg("-create-xcframework")
            .args(library_args)
            .arg("-output")
            .arg(&temp_dest)
            .successful_output()?;

        Ok(temp_dest)
    }

    // Fixes an issue including the XCFramework in an Xcode project that already contains an XCFramework: https://github.com/jessegrosjean/module-map-error
    fn patch_xcframework(&self, temp_dir: &Path, module_name: &str) -> Result<()> {
        println!("Patching XCFramework to have a unique header directory");

        for dir_entry in std::fs::read_dir(temp_dir)? {
            let path = dir_entry.expect("Invalid Path").path();
            if path.is_dir() {
                let headers_dir = temp_dir.join(&path).join("Headers");
                let non_lib_files: Vec<PathBuf> = std::fs::read_dir(&headers_dir)?
                    .flat_map(|f| f.ok())
                    .filter_map(|f| {
                        if f.path().ends_with(".a") {
                            None
                        } else {
                            Some(f.path())
                        }
                    })
                    .collect();

                let new_headers_dir = headers_dir.join(module_name);
                fs::recreate_dir(&new_headers_dir)?;

                for file in non_lib_files {
                    std::fs::rename(&file, new_headers_dir.join(file.file_name().unwrap()))?;
                }
            }
        }

        Ok(())
    }
}

impl LibraryGroup {
    fn create(
        &self,
        cargo_target_dir: &Path,
        library_file_name: &str,
        temp_dir: &Path,
    ) -> Result<PathBuf> {
        let mut libraries: Vec<PathBuf> = Vec::new();
        for slice in &self.slices {
            libraries.push(slice.create(cargo_target_dir, library_file_name, temp_dir)?);
        }

        let dir = temp_dir.join(self.id.to_string());
        fs::recreate_dir(&dir)?;

        let dest = dir.join(format!("{}.a", library_file_name));
        Command::new("xcrun")
            .arg("lipo")
            .arg("-create")
            .args(libraries)
            .arg("-output")
            .arg(&dest)
            .successful_output()?;

        Ok(dest)
    }

    fn swift_bindings_dir(&self, cargo_target_dir: &Path) -> Result<PathBuf> {
        let slice = self
            .slices
            .first()
            .with_context(|| "No slices in library group")?;
        let path = slice
            .built_product_dir(cargo_target_dir)
            .join("swift-bindings");
        if !path.exists() {
            anyhow::bail!("Headers not found: {}", path.display())
        }
        Ok(path)
    }

    fn headers_dir(&self, cargo_target_dir: &Path) -> Result<PathBuf> {
        let path = self.swift_bindings_dir(cargo_target_dir)?.join("headers");
        if !path.exists() {
            anyhow::bail!("Headers not found: {}", path.display())
        }
        Ok(path)
    }

    fn swift_binding_files(&self, cargo_target_dir: &Path) -> Result<Vec<PathBuf>> {
        self.swift_bindings_dir(cargo_target_dir)?
            .files_with_extension("swift")
    }
}

impl Slice {
    fn create(
        &self,
        cargo_target_dir: &Path,
        library_file_name: &str,
        temp_dir: &Path,
    ) -> Result<PathBuf> {
        let libs = self.built_libraries(cargo_target_dir)?;

        // If there are more static libraries (a.k.a cargo packages), we'll
        // need to bundle them together into one static library.
        // At the moment, we only have one libwp_api, so we can just copy it.
        assert!(
            libs.len() == 1,
            "Expected exactly one library for each slice. Found: {:?}",
            libs
        );

        let lib = &libs[0];
        if !lib.exists() {
            anyhow::bail!("Library not found: {}", lib.display())
        }

        let dir = temp_dir.join(&self.target);
        fs::recreate_dir(&dir)?;

        let dest = dir.join(format!("{}.a", library_file_name));
        std::fs::copy(lib, &dest)
            .with_context(|| format!("Failed to copy {} to {}", lib.display(), dest.display()))?;

        Ok(dest)
    }

    /// Returns the directory where the built static libraries are located.
    fn built_product_dir(&self, cargo_target_dir: &Path) -> PathBuf {
        cargo_target_dir
            .join(&self.target)
            .join(self.profile.dir_name())
    }

    fn built_libraries(&self, cargo_target_dir: &Path) -> Result<Vec<PathBuf>> {
        self.built_product_dir(cargo_target_dir)
            .files_with_extension("a")
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct LibraryGroupId {
    os: ApplePlatform,
    is_sim: bool,
}

impl LibraryGroupId {
    fn from_target(target: &str) -> Result<Self> {
        let mut parts = target.split('-');
        _ /* arch */= parts.next();
        if parts.next() != Some("apple") {
            anyhow::bail!("{} is not an Apple platform", target)
        }

        let os: ApplePlatform = parts
            .next()
            .with_context(|| format!("No OS in target: {}", target))?
            .try_into()?;

        let output = Command::new("rustc")
            .env("RUSTC_BOOTSTRAP", "1")
            .args([
                "-Z",
                "unstable-options",
                "--print",
                "target-spec-json",
                "--target",
            ])
            .arg(target)
            .successful_output()?;
        let json = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .with_context(|| "Failed to parse command output as JSON")?;
        let llvm_target = json
            .get("llvm-target")
            .and_then(|t| t.as_str())
            .with_context(|| "No llvm-target in command output")?;

        Ok(Self {
            os,
            is_sim: llvm_target.ends_with("-simulator"),
        })
    }
}

impl Display for LibraryGroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.os)?;

        if self.is_sim {
            write!(f, "-sim")
        } else {
            Ok(())
        }
    }
}

trait ExecuteCommand {
    fn successful_output(&mut self) -> Result<std::process::Output>;
}

impl ExecuteCommand for Command {
    fn successful_output(&mut self) -> Result<std::process::Output> {
        let output = self
            .output()
            .with_context(|| format!("Command failed: $ {:?}", self))?;
        if output.status.success() {
            Ok(output)
        } else {
            anyhow::bail!(
                "Command failed with exit code: {}\nstdout: {:?}\nstderr: {:?}\n$ {:?}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                self
            )
        }
    }
}
