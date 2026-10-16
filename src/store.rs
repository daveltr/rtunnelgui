use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{de::DeserializeOwned, Serialize};

pub fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "kratos", "ssh-tunnel-manager").context("no home directory")?;
    Ok(dirs.config_dir().to_path_buf())
}

pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "kratos", "ssh-tunnel-manager").context("no home directory")?;
    Ok(dirs.data_dir().to_path_buf())
}

/// Write `*.tmp`, fsync, rename — a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Missing file => default. Corrupt file is an error (don't silently overwrite user data).
pub fn load<T: DeserializeOwned + Default>(name: &str) -> Result<T> {
    let path = config_dir()?.join(name);
    match fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save<T: Serialize>(name: &str, value: &T) -> Result<()> {
    write_atomic(&config_dir()?.join(name), &serde_json::to_vec_pretty(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("stm-test-{}", uuid::Uuid::new_v4()));
        let p = dir.join("a.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert!(!p.with_extension("tmp").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
