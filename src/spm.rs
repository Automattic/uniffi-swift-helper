use std::{
    ffi::OsStr,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use cargo_metadata::{camino::Utf8PathBuf, MetadataCommand};
use rinja::Template;

use crate::project::*;
use crate::utils::*;

pub struct DeploymentTargets;

impl DeploymentTargets {
    pub fn ios() -> &'static str {
        "13.0"
    }

    pub fn macos() -> &'static str {
        "11.0"
    }

    pub fn tvos() -> &'static str {
        "13.0"
    }

    pub fn watchos() -> &'static str {
        "8.0"
    }
}

#[derive(Template)]
#[template(path = "Package.swift", escape = "none")]
struct PackageTemplate {
    package_name: String,
    ffi_module_name: String,
    project_name: String,
    targets: Vec<Target>,
    internal_targets: Vec<InternalTarget>,

    ios_version: &'static str,
    macos_version: &'static str,
    tvos_version: &'static str,
    watchos_version: &'static str,
}

struct Target {
    name: String,
    library_source_path: String,
    test_source_path: String,
    dependencies: Vec<String>,
    has_test_resources: bool,
}

fn get_only_subdir<P>(path: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let subdirs = path
        .read_dir()?
        .map(|p| p.context("Can't read directory entry"))
        .collect::<Result<Vec<_>>>()?;
    if subdirs.len() != 1 {
        anyhow::bail!(
            "Expected 1 subdirectory in {}, found {:?}",
            path.display(),
            subdirs
        )
    }
    Ok(subdirs[0].path())
}

struct InternalTarget {
    name: String,
    swift_wrapper_dir: String,
    source_file: String,
    excluded_source_files: Vec<String>,
    dependencies: Vec<String>,
}

pub trait SPMExtension {
    fn generate_swift_package(&self, project_name: String) -> Result<()>;
}

impl SPMExtension for Project {
    fn generate_swift_package(&self, project_name: String) -> Result<()> {
        let top_level_package = &self.package;

        let targets = top_level_package
            .iter()
            .map(|p| self.public_target(p))
            .collect::<Result<Vec<_>>>()?;
        let internal_targets = top_level_package
            .iter()
            .map(|p| self.internal_target(p))
            .collect::<Result<Vec<_>>>()?;

        let template = PackageTemplate {
            package_name: top_level_package.public_module_name()?,
            ffi_module_name: self.ffi_module_name.clone(),
            project_name,
            targets,
            internal_targets,
            ios_version: DeploymentTargets::ios(),
            macos_version: DeploymentTargets::macos(),
            tvos_version: DeploymentTargets::tvos(),
            watchos_version: DeploymentTargets::watchos(),
        };
        let content = template.render()?;
        let dest = self.swift_package_manifest_file_path();
        File::create(&dest)?.write_all(content.as_bytes())?;

        Command::new("swift")
            .args(["format", "--in-place"])
            .arg(&dest)
            .successful_output()?;

        Ok(())
    }
}

impl Project {
    fn swift_package_manifest_file_path(&self) -> Utf8PathBuf {
        self.cargo_metadata.workspace_root.join("Package.swift")
    }

    fn spm_target_name(&self, cargo_package_name: &str) -> Result<String> {
        self.package(cargo_package_name)
            .context(format!("Can't find package {}", cargo_package_name))?
            .public_module_name()
    }

    fn internal_target(&self, package: &UniffiPackage) -> Result<InternalTarget> {
        let swift_wrapper_dir = self.swift_wrapper_dir();
        let source_file_name = package.swift_wrapper_file_name();
        let binding_file = swift_wrapper_dir.join(&source_file_name);
        if !binding_file.exists() {
            anyhow::bail!(
                "Swift wrapper file is not found at {}. Need to build xcframework first.",
                binding_file
            )
        }

        let excluded_source_files = swift_wrapper_dir
            .files_with_extension("swift")?
            .iter()
            .filter(|f| f.file_name() != Some(OsStr::new(&source_file_name)))
            .map(|f| f.file_name().unwrap().to_str().unwrap().to_string())
            .collect::<Vec<_>>();

        let dependencies = package
            .dependencies
            .iter()
            .map(|p| p.internal_module_name())
            .collect::<Result<Vec<_>>>()?;

        Ok(InternalTarget {
            name: package.internal_module_name()?,
            swift_wrapper_dir: fs::relative_path(
                &swift_wrapper_dir,
                &self.cargo_metadata.workspace_root,
            ),
            source_file: source_file_name.clone(),
            excluded_source_files,
            dependencies,
        })
    }

    fn public_target(&self, package: &UniffiPackage) -> Result<Target> {
        let swift_code_dir = self.vend_swift_source_code(package)?;

        // There could be 'Sources' and 'Tests' directories in the swift code directory.
        // We need the 'Sources' directory.
        let sources_dir = get_only_subdir(swift_code_dir.join("Sources"))?;
        let tests_dir = get_only_subdir(swift_code_dir.join("Tests"))?;

        let root_dir = &self.cargo_metadata.workspace_root;
        let library_source_path = fs::relative_path(&sources_dir, root_dir);
        let test_source_path = fs::relative_path(&tests_dir, root_dir);

        let dependencies = package
            .dependencies
            .iter()
            .map(|p| self.spm_target_name(&p.name))
            .collect::<Result<Vec<_>>>()?;

        Ok(Target {
            name: package.public_module_name()?,
            library_source_path,
            test_source_path,
            dependencies,
            has_test_resources: tests_dir.join("Resources").exists(),
        })
    }

    fn vend_swift_source_code(&self, package: &UniffiPackage) -> Result<Utf8PathBuf> {
        let root_dir = &self.cargo_metadata.workspace_root;
        if !root_dir.is_absolute() {
            anyhow::bail!(
                "Cargo workspace root dir is not an absolute path: {}",
                root_dir
            )
        }

        let metadata = MetadataCommand::new()
            .manifest_path(&package.manifest_path)
            .exec()
            .with_context(|| format!("Can't get cargo metadata for package {}", package.name))?;

        let mut swift_code_dir = metadata.workspace_root.join("native/swift");
        if !swift_code_dir.is_dir() {
            anyhow::bail!(
                "Swift code for package {} is not a directory at {}",
                package.name,
                &swift_code_dir
            )
        }

        if swift_code_dir.starts_with(root_dir) {
            return Ok(swift_code_dir);
        }

        println!(
            "{} swift code directory is outside of the cargo root directory.",
            package.name
        );
        println!(
            "⚠️ Remember to run the command again when {} cargo dependency is updated.",
            package.name
        );

        println!("Copying swift code directory to the cargo root directory");

        let cargo_target_dir = root_dir.join("target");
        let vendor_path = cargo_target_dir.join("uniffi-swift-helper/vendor");
        let new_path = vendor_path.join(&package.name);
        fs::recreate_dir(&new_path)?;

        println!("  - from: {}", swift_code_dir);
        println!("  - to: {}", new_path);

        fs::copy_dir(&swift_code_dir, &new_path)?;

        swift_code_dir = new_path;

        Ok(swift_code_dir)
    }
}
