use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::sensor::ProviderError;

pub(crate) fn read_trimmed(path: &Path) -> Result<String, ProviderError> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|source| ProviderError::Io {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn read_number<T: FromStr>(path: &Path) -> Result<T, ProviderError> {
    let text = read_trimmed(path)?;
    text.parse().map_err(|_| ProviderError::Parse {
        path: path.to_path_buf(),
        value: text,
    })
}

/// Sorted subdirectories of `dir`, following symlinks; empty when `dir` is unreadable.
pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

pub(crate) fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}
