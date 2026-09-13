//! Connection profiles (FR-O4). Optional convenience layer on top of the CLI
//! flags in `main.rs`: `--save-profile NAME` stores the resolved connection
//! parameters, `--profile NAME` loads them back on a later run.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Profile {
    Serial {
        port_name: String,
        baud_rate: u32,
        newline: String,
    },
    Ssh {
        host: String,
        port: u16,
        username: String,
        newline: String,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    profiles: HashMap<String, Profile>,
}

fn config_path() -> Result<PathBuf> {
    let mut dir = dirs::config_dir().context("could not determine config directory for this OS")?;
    dir.push("rust-term-console");
    fs::create_dir_all(&dir).context("failed to create config directory")?;
    dir.push("profiles.toml");
    Ok(dir)
}

fn load_file() -> Result<ProfileFile> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(ProfileFile::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn load_profile(name: &str) -> Result<Profile> {
    let file = load_file()?;
    file.profiles
        .get(name)
        .cloned()
        .with_context(|| format!("no saved profile named '{name}'"))
}

pub fn save_profile(name: &str, profile: Profile) -> Result<()> {
    let path = config_path()?;
    let mut file = load_file()?;
    file.profiles.insert(name.to_string(), profile);
    let text = toml::to_string_pretty(&file).context("failed to serialize profiles")?;
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

pub fn list_profiles() -> Result<Vec<String>> {
    let file = load_file()?;
    let mut names: Vec<String> = file.profiles.into_keys().collect();
    names.sort();
    Ok(names)
}
