//! Session logging to a file (FR-O3). Plain text, appended: the file gets
//! exactly what the log area shows (ANSI-stripped output plus the local echo
//! of sent lines), so re-opening the same path keeps earlier sessions.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub struct LogFile {
    file: File,
    path: PathBuf,
}

impl LogFile {
    /// Open (creating it and any missing parent directories) `path` for
    /// appending. A leading `~/` expands to the home directory.
    pub fn open(path: &str) -> Result<Self> {
        let path = path.trim();
        if path.is_empty() {
            anyhow::bail!("Enter a log file path");
        }
        let path = expand_home(path);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(Self { file, path })
    }

    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.file.write_all(data)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").zip(dirs::home_dir()) {
        Some((rest, home)) => home.join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rtc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn creates_parent_dirs_and_appends() {
        let dir = temp_dir("append");
        let path = dir.join("nested").join("session.log");
        let path_str = path.to_str().unwrap();

        let mut log = LogFile::open(path_str).unwrap();
        log.write(b"first\n").unwrap();
        drop(log);
        let mut log = LogFile::open(path_str).unwrap();
        log.write(b"second\n").unwrap();
        drop(log);

        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_empty_path() {
        assert!(LogFile::open("   ").is_err());
    }
}
