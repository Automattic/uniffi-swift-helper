use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use cargo_metadata::{camino::Utf8PathBuf, DependencyKind, Metadata, MetadataCommand, Package};
use pathdiff::diff_paths;
use rinja::Template;

use crate::utils::{fs, ExecuteCommand};

#[derive(Template)]
#[template(path = "Package.swift", escape = "none")]
struct PackageTemplate {
    package_name: String,
    ffi_module_name: String,
    project_name: String,
    targets: Vec<Target>,
    internal_targets: Vec<InternalTarget>,
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

fn relative_path<P, B>(path: P, base: B) -> String
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    diff_paths(path, base)
        .as_ref()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap()
}

struct InternalTarget {
    name: String,
    swift_wrapper_dir: String,
    source_file: String,
    dependencies: Vec<String>,
}

#[derive(Debug)]
struct UniffiPackage {
    name: String,
    manifest_path: Utf8PathBuf,
    dependencies: Vec<UniffiPackage>,
}

pub fn generate_swift_package2(
    ffi_module_name: String,
    project_name: String,
    packages: HashMap<String, String>,
) -> Result<()> {
    let metadata = MetadataCommand::new()
        .exec()
        .with_context(|| "Can't get cargo metadata")?;

    if metadata.workspace_root.as_std_path() != std::env::current_dir()? {
        anyhow::bail!("The current directory is not the cargo root directory")
    }

    let uniffi_packages = metadata
        .packages
        .iter()
        .filter(|p| is_uniffi_package(p))
        .collect::<Vec<_>>();
    for package in &uniffi_packages {
        if !package.id.repr.starts_with("git+") && !package.id.repr.starts_with("path+") {
            anyhow::bail!("Unsupported package id: {}. We can only find Swift source code when package is integrated as a git repo or a local path.", package.id.repr)
        }
    }

    let uniffi_packages = uniffi_packages
        .iter()
        .map(|p| UniffiPackage::new(p, &uniffi_packages))
        .collect::<Vec<_>>();
    let top_level_package = uniffi_packages
        .iter()
        .find(|p| {
            !uniffi_packages
                .iter()
                .any(|other| other.depends_on(&p.name))
        })
        .unwrap();

    let resolver = SPMResolver {
        metadata: ProjectMetadata {
            ffi_module_name,
            cargo_metadata: metadata.clone(),
        },
        cargo_package_to_spm_target_map: packages,
    };

    let targets = uniffi_packages
        .iter()
        .map(|p| resolver.public_target(p))
        .collect::<Result<Vec<_>>>()?;
    let internal_targets = uniffi_packages
        .iter()
        .map(|p| resolver.internal_target(p))
        .collect::<Result<Vec<_>>>()?;

    let template = PackageTemplate {
        package_name: resolver.spm_target_name(&top_level_package.name),
        ffi_module_name: resolver.metadata.ffi_module_name.clone(),
        project_name,
        targets,
        internal_targets,
    };
    let content = template.render()?;
    let dest = resolver.swift_package_manifest_file_path();
    File::create(&dest)?.write_all(content.as_bytes())?;

    Command::new("swift")
        .args(["format", "--in-place"])
        .arg(&dest)
        .successful_output()?;

    Ok(())
}

fn is_uniffi_package(package: &Package) -> bool {
    let depends_on_uniffi = package
        .dependencies
        .iter()
        .any(|d| d.name == "uniffi" && !d.optional && d.kind == DependencyKind::Normal);
    let has_uniffi_toml = package.manifest_path.with_file_name("uniffi.toml").exists();
    depends_on_uniffi && has_uniffi_toml
}

impl UniffiPackage {
    fn new(package: &Package, all_uniffi_packages: &Vec<&Package>) -> Self {
        let dependencies: Vec<_> = package
            .dependencies
            .iter()
            .filter_map(|d| {
                all_uniffi_packages
                    .iter()
                    .find(|p| p.name == d.name)
                    .map(|p| Self::new(p, all_uniffi_packages))
            })
            .collect();

        UniffiPackage {
            name: package.name.clone(),
            manifest_path: package.manifest_path.clone(),
            dependencies,
        }
    }

    fn depends_on(&self, other: &str) -> bool {
        self.dependencies.iter().any(|d| d.name == other)
    }

    fn swift_wrapper_file_name(&self) -> String {
        format!("{}.swift", self.name)
    }
}

struct ProjectMetadata {
    ffi_module_name: String,
    cargo_metadata: Metadata,
}

impl ProjectMetadata {
    fn swift_wrapper_dir(&self) -> Utf8PathBuf {
        self.cargo_metadata
            .target_directory
            .join(&self.ffi_module_name)
            .join("swift-wrapper")
    }
}

struct SPMResolver {
    metadata: ProjectMetadata,
    cargo_package_to_spm_target_map: HashMap<String, String>,
}

impl SPMResolver {
    fn swift_package_manifest_file_path(&self) -> Utf8PathBuf {
        self.metadata
            .cargo_metadata
            .workspace_root
            .join("Package.swift")
    }

    fn spm_target_name(&self, cargo_package_name: &str) -> String {
        self.cargo_package_to_spm_target_map
            .get(cargo_package_name)
            .unwrap_or_else(|| {
                panic!(
                    "No SPM target name specified for cargo package {}",
                    cargo_package_name
                )
            })
            .to_string()
    }

    fn public_target_name(&self, package: &UniffiPackage) -> String {
        self.spm_target_name(&package.name)
    }

    fn internal_target_name(&self, package: &UniffiPackage) -> String {
        format!("{}Internal", self.spm_target_name(&package.name))
    }

    fn internal_target(&self, package: &UniffiPackage) -> Result<InternalTarget> {
        let swift_wrapper_dir = self.metadata.swift_wrapper_dir();
        let source_file_name = package.swift_wrapper_file_name();
        let binding_file = swift_wrapper_dir.join(&source_file_name);
        if !binding_file.exists() {
            anyhow::bail!(
                "Swift wrapper file is not found at {}. Need to build xcframework first.",
                binding_file
            )
        }

        let dependencies = package
            .dependencies
            .iter()
            .map(|p| self.internal_target_name(p))
            .collect::<Vec<_>>();

        Ok(InternalTarget {
            name: self.internal_target_name(package),
            swift_wrapper_dir: relative_path(
                swift_wrapper_dir,
                &self.metadata.cargo_metadata.workspace_root,
            ),
            source_file: source_file_name,
            dependencies,
        })
    }

    fn public_target(&self, package: &UniffiPackage) -> Result<Target> {
        let swift_code_dir = self.vend_swift_source_code(package)?;

        // There could be 'Sources' and 'Tests' directories in the swift code directory.
        // We need the 'Sources' directory.
        let sources_dir = get_only_subdir(swift_code_dir.join("Sources"))?;
        let tests_dir = get_only_subdir(swift_code_dir.join("Tests"))?;

        let root_dir = &self.metadata.cargo_metadata.workspace_root;
        let library_source_path = relative_path(&sources_dir, root_dir);
        let test_source_path = relative_path(&tests_dir, root_dir);

        let dependencies = package
            .dependencies
            .iter()
            .map(|p| self.spm_target_name(&p.name))
            .collect();

        Ok(Target {
            name: self.public_target_name(package),
            library_source_path,
            test_source_path,
            dependencies,
            has_test_resources: tests_dir.join("Resources").exists(),
        })
    }

    fn vend_swift_source_code(&self, package: &UniffiPackage) -> Result<Utf8PathBuf> {
        let root_dir = &self.metadata.cargo_metadata.workspace_root;
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
