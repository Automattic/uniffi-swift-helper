use std::str::FromStr;

use anyhow::{Context, Result};
use cargo_metadata::{camino::Utf8PathBuf, DependencyKind, Metadata, MetadataCommand, Package};
use toml::Table;

pub struct Project {
    pub package: UniffiPackage,
    pub cargo_metadata: Metadata,
}

impl Project {
    pub fn new() -> Result<Self> {
        let cargo_metadata = MetadataCommand::new()
            .exec()
            .with_context(|| "Can't get cargo metadata")?;

        if cargo_metadata.workspace_root.as_std_path() != std::env::current_dir()? {
            anyhow::bail!("The current directory is not the cargo root directory")
        }

        Ok(Self {
            package: Self::uniffi_package(&cargo_metadata)?,
            cargo_metadata,
        })
    }

    fn uniffi_package(metadata: &Metadata) -> Result<UniffiPackage> {
        let is_uniffi_package = |package: &Package| {
            let depends_on_uniffi = package
                .dependencies
                .iter()
                .any(|d| d.name == "uniffi" && !d.optional && d.kind == DependencyKind::Normal);
            let has_uniffi_toml = package.manifest_path.with_file_name("uniffi.toml").exists();
            depends_on_uniffi && has_uniffi_toml
        };
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

        let mut uniffi_packages = uniffi_packages
            .iter()
            .map(|p| UniffiPackage::new(p, &uniffi_packages))
            .collect::<Vec<_>>();
        let top_level_packages = uniffi_packages
            .iter()
            .enumerate()
            .filter(|p| {
                !uniffi_packages
                    .iter()
                    .any(|other| other.depends_on(&p.1.name))
            })
            .collect::<Vec<_>>();

        if top_level_packages.len() != 1 {
            anyhow::bail!(
                "Expected 1 top-level package, found {:?}",
                top_level_packages
                    .iter()
                    .map(|(_, p)| p.name.to_string())
                    .collect::<Vec<_>>()
            )
        }

        let index = top_level_packages[0].0;
        Ok(uniffi_packages.remove(index))
    }

    pub fn packages_iter(&self) -> impl Iterator<Item = &UniffiPackage> {
        self.package.iter()
    }

    pub fn package(&self, name: &str) -> Option<&UniffiPackage> {
        self.packages_iter().find(|p| p.name == name)
    }

    pub fn ffi_module_name(&self) -> Result<String> {
        self.package.ffi_module_name()
    }

    pub fn linux_library_path(&self) -> Result<Utf8PathBuf> {
        let ffi_module_name = self.ffi_module_name()?;
        Ok(self
            .cargo_metadata
            .target_directory
            .join(&ffi_module_name)
            .join("linux"))
    }

    pub fn xcframework_path(&self) -> Result<Utf8PathBuf> {
        let ffi_module_name = self.ffi_module_name()?;
        Ok(self
            .cargo_metadata
            .target_directory
            .join(&ffi_module_name)
            .join(format!("{}.xcframework", &ffi_module_name)))
    }

    pub fn swift_wrapper_dir(&self) -> Result<Utf8PathBuf> {
        Ok(self
            .cargo_metadata
            .target_directory
            .join(self.ffi_module_name()?)
            .join("swift-wrapper"))
    }

    pub fn swift_wrapper_files_iter(
        &self,
    ) -> impl Iterator<Item = Result<(Utf8PathBuf, &UniffiPackage)>> {
        self.packages_iter()
            .map(|pkg| {
                let file_name = format!("{}.swift", pkg.name);
                let path = self.swift_wrapper_dir()?.join(file_name);
                if path.exists() {
                    Ok((path, pkg))
                } else {
                    anyhow::bail!("Swift wrapper file {} not found. Please run the build command first", path);
                }
            })
    }
}

#[derive(Debug)]
pub struct UniffiPackage {
    pub name: String,
    pub manifest_path: Utf8PathBuf,
    pub dependencies: Vec<UniffiPackage>,
}

impl UniffiPackage {
    pub fn new(package: &Package, all_uniffi_packages: &Vec<&Package>) -> Self {
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

    pub fn depends_on(&self, other: &str) -> bool {
        self.dependencies.iter().any(|d| d.name == other)
    }

    pub fn swift_wrapper_file_name(&self) -> String {
        format!("{}.swift", self.name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &UniffiPackage> {
        let mut result: Vec<&UniffiPackage> = vec![];

        let mut queue: Vec<&UniffiPackage> = vec![];
        queue.push(self);
        while let Some(package) = queue.pop() {
            result.push(package);

            for dep in &package.dependencies {
                queue.push(dep);
            }
        }

        result.into_iter()
    }

    pub fn ffi_module_name(&self) -> Result<String> {
        self.uniffi_toml_swift_configuration("ffi_module_name")
    }

    pub fn public_module_name(&self) -> Result<String> {
        self.uniffi_toml_swift_configuration("wp_spm_public_module_name")
    }

    pub fn internal_module_name(&self) -> Result<String> {
        Ok(format!("{}Internal", self.public_module_name()?))
    }

    fn uniffi_toml(&self) -> Result<Table> {
        let uniffi_toml_path = self.manifest_path.with_file_name("uniffi.toml");
        let content = std::fs::read(uniffi_toml_path)
            .with_context(|| format!("Can't read the uniffi.toml of package {}", self.name))?;
        let str = String::from_utf8(content).with_context(|| {
            format!(
                "The uniffi.toml of package {} is not uft-8 encoded",
                self.name
            )
        })?;
        let table = Table::from_str(&str)
            .with_context(|| format!("The uniffi.toml of package {} is invalid", self.name))?;
        Ok(table)
    }

    fn uniffi_toml_swift_configuration(&self, key: &str) -> Result<String> {
        self.uniffi_toml()?
            .get("bindings")
            .and_then(|t| t.get("swift"))
            .and_then(|t| t.get(key))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .context(format!(
                "{} not found in the uniffi.toml of package {}",
                key, self.name
            ))
    }
}
