//! config.toml.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub base_url: String,
    pub title: String,
    pub description: String,
    pub default_language: String,
    pub generate_feeds: bool,
    pub taxonomies: Vec<Taxonomy>,
    pub markdown: Markdown,
    /// Stays a table so a new key in config.toml needs no change here.
    pub extra: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Taxonomy {
    pub name: String,
    pub feed: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Markdown {
    pub smart_punctuation: bool,
    pub highlighting: Highlighting,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Highlighting {
    pub style: String,
    pub theme: String,
}

pub fn load(path: &Path) -> Result<Config> {
    let name = path.to_string_lossy().replace('\\', "/");
    let text = std::fs::read_to_string(path).map_err(|e| Error::new(&name, e.to_string()))?;
    toml::from_str(&text).map_err(|e| Error::new(&name, e.to_string()))
}
