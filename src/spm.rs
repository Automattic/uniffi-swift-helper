use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use cargo_metadata::{DependencyKind, MetadataCommand, Package};
use pathdiff::diff_paths;
use rinja::Template;

use crate::utils::ExecuteCommand;

#[derive(Template)]
#[template(path = "Package.swift", escape = "none")]
struct PackageTemplate {
    package_name: String,
    ffi_module_name: String,
    project_name: String,
    targets: Vec<Target>,
}

struct Target {
    name: String,
    library_source_path: String,
    test_source_path: String,
    dependencies: Vec<String>,
    has_test_resources: bool,
}

impl Target {
    fn new(name: String, package: &Package, root_dir: &Path) -> Result<Self> {
        let root_dir = root_dir.canonicalize()?;

        if !package.id.repr.starts_with("git+") && !package.id.repr.starts_with("path+") {
            anyhow::bail!("Unsupported package id: {}. We can only find Swift source code when package is integrated as a git repo or a local path.", package.id.repr)
        }

        let metadata = MetadataCommand::new()
            .manifest_path(&package.manifest_path)
            .exec()
            .with_context(|| format!("Can't get cargo metadata for package {}", package.name))?;

        let swift_code_dir = metadata
            .workspace_root
            .join("native/swift")
            .canonicalize()?;
        if !swift_code_dir.is_dir() {
            anyhow::bail!(
                "Swift code for package {} is not a directory at {}",
                package.name,
                &swift_code_dir.display()
            )
        }

        // There could be 'Sources' and 'Tests' directories in the swift code directory.
        // We need the 'Sources' directory.
        let sources_dir = get_only_subdir(&swift_code_dir.join("Sources"))?;
        let tests_dir = get_only_subdir(&swift_code_dir.join("Tests"))?;

        let library_source_path = relative_path(&sources_dir, &root_dir);
        let test_source_path = relative_path(&tests_dir, &root_dir);

        Ok(Self {
            name,
            library_source_path,
            test_source_path,
            dependencies: vec![],
            has_test_resources: tests_dir.join("Resources").exists(),
        })
    }
}

pub fn generate_swift_package(
    top_level_package: String,
    ffi_module_name: String,
    project_name: String,
    packages: HashMap<String, String>,
) -> Result<()> {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .with_context(|| "Can't get cargo metadata")?;

    if metadata.workspace_root.as_std_path() != std::env::current_dir()? {
        anyhow::bail!("The current directory is not the cargo root directory")
    }

    let uniffi_packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|p| packages.contains_key(&p.name))
        .collect();
    println!("Found {} uniffi packages", uniffi_packages.len());
    for pkg in &uniffi_packages {
        println!("  - {}", pkg.name);
    }

    let mut targets: Vec<Target> = vec![];
    for package in &uniffi_packages {
        let name = packages
            .get(&package.name)
            .context(format!(
                "No module name specified for package {}",
                &package.name
            ))?
            .clone();
        let mut target = Target::new(name, package, metadata.workspace_root.as_std_path())?;
        target.dependencies = package
            .dependencies
            .iter()
            .filter(|d| d.name == target.name && !d.optional && d.kind == DependencyKind::Normal)
            .map(|d| {
                let spm_target_name = packages
                    .get(&d.name)
                    .context("No module name specified for dependency")?;
                Ok(spm_target_name.clone())
            })
            .collect::<Result<Vec<_>>>()?;
        targets.push(target);
    }

    let template = PackageTemplate {
        package_name: packages.get(&top_level_package).unwrap().clone(),
        ffi_module_name,
        project_name,
        targets,
    };
    let content = template.render()?;
    let dest = metadata.workspace_root.join("Package.swift");
    File::create(&dest)?.write_all(content.as_bytes())?;

    Command::new("swift")
        .args(["format", "--in-place"])
        .arg(&dest)
        .successful_output()?;

    Ok(())
}

fn get_only_subdir(path: &Path) -> Result<PathBuf> {
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
