use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::Path;

use crate::error::Result;

pub const QUERYFORGE_CODEGEN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(pub String);

impl Fingerprint {
    pub fn from_text(text: &str) -> Self {
        // FNV-1a 64-bit. Good enough for a dependency-free starter; replace with
        // sha256 before publishing the crate.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self(format!("fnv1a64:{hash:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::from_text(&text))
    }

    pub fn from_files(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut text = String::new();
        for path in paths {
            let path = path.as_ref();
            let contents = fs::read_to_string(path)?;
            text.push_str(&path.display().to_string());
            text.push('\n');
            text.push_str(&contents);
            text.push('\n');
        }
        Ok(Self::from_text(&text))
    }

    pub fn from_paths(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut files = Vec::new();
        for path in paths {
            collect_files(path.as_ref(), &mut files)?;
        }
        files.sort();
        Self::from_files(&files)
    }
}

fn collect_files(path: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_files(&entry?.path(), files)?;
        }
        return Ok(());
    }

    Ok(())
}

impl Display for Fingerprint {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
