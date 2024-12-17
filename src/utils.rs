use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result};

#[allow(dead_code)]
pub(crate) trait ExecuteCommand {
    fn successful_output(&mut self) -> Result<Output>;
}

impl ExecuteCommand for Command {
    fn successful_output(&mut self) -> Result<Output> {
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

pub(crate) trait FileSystemExtensions {
    fn files_with_extension(&self, ext: &str) -> Result<Vec<PathBuf>>;
}

impl FileSystemExtensions for Path {
    fn files_with_extension(&self, ext: &str) -> Result<Vec<PathBuf>> {
        let files = std::fs::read_dir(self)?
            .filter_map(|f| f.ok())
            .map(|f| f.path())
            .filter(|p| p.is_file() && p.extension().map_or(false, |e| e == ext))
            .collect();
        Ok(files)
    }
}

pub(crate) mod fs {

    use std::path::PathBuf;

    use super::*;

    pub fn recreate_dir(dir: &Path) -> Result<()> {
        if dir.exists() {
            std::fs::remove_dir_all(dir)
                .with_context(|| format!("Failed to remove directory at {:?}", dir))?;
        }

        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {:?}", dir))
    }

    pub fn move_file(src: &Path, dst: &Path) -> Result<PathBuf> {
        assert!(src.exists(), "Source file does not exist: {:?}", src);
        assert!(src.is_file(), "Source is not a file: {:?}", src);

        let destination: PathBuf = if dst.is_dir() {
            dst.join(src.file_name().unwrap())
        } else {
            dst.to_path_buf()
        };

        std::fs::rename(src, &destination)
            .with_context(|| format!("Failed to move directory from {:?} to {:?}", src, dst))?;

        Ok(destination)
    }
}
