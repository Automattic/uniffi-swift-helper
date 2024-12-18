use anyhow::{Context, Result};
use cargo_metadata::{camino::Utf8PathBuf, DependencyKind, Metadata, MetadataCommand, Package};

pub struct Project {
    pub ffi_module_name: String,
    pub cargo_metadata: Metadata,
}

impl Project {
    pub fn new(ffi_module_name: String) -> Result<Self> {
        let cargo_metadata = MetadataCommand::new()
            .exec()
            .with_context(|| "Can't get cargo metadata")?;

        if cargo_metadata.workspace_root.as_std_path() != std::env::current_dir()? {
            anyhow::bail!("The current directory is not the cargo root directory")
        }

        Ok(Self {
            ffi_module_name,
            cargo_metadata,
        })
    }

    pub fn uniffi_package(&self) -> Result<UniffiPackage> {
        let is_uniffi_package = |package: &Package| {
            let depends_on_uniffi = package
                .dependencies
                .iter()
                .any(|d| d.name == "uniffi" && !d.optional && d.kind == DependencyKind::Normal);
            let has_uniffi_toml = package.manifest_path.with_file_name("uniffi.toml").exists();
            depends_on_uniffi && has_uniffi_toml
        };
        let uniffi_packages = self
            .cargo_metadata
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

    pub fn xcframework_path(&self) -> Utf8PathBuf {
        self.cargo_metadata
            .target_directory
            .join(&self.ffi_module_name)
            .join(format!("{}.xcframework", &self.ffi_module_name))
    }

    pub fn swift_wrapper_dir(&self) -> Utf8PathBuf {
        self.cargo_metadata
            .target_directory
            .join(&self.ffi_module_name)
            .join("swift-wrapper")
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
}
