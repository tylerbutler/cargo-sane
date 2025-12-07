//! Update dependencies in Cargo.toml

use crate::core::dependency::Dependency;
use crate::core::manifest::Manifest;
use crate::Result;
use anyhow::Context;
use regex::Regex;
use std::fs;

pub struct DependencyUpdater {
    manifest: Manifest,
    original_content: String,
}

impl DependencyUpdater {
    pub fn new(manifest: Manifest) -> Result<Self> {
        let original_content =
            fs::read_to_string(&manifest.path).context("Failed to read Cargo.toml")?;

        Ok(Self {
            manifest,
            original_content,
        })
    }

    /// Update a single dependency to a new version
    pub fn update_dependency(&mut self, dep: &Dependency, new_version: &str) -> Result<()> {
        let dep_name = &dep.name;

        // Strategy 1: Detailed format - name = { version = "x.y.z", ... }
        // Capture: everything up to and including opening quote, version, closing quote
        let detailed_pattern = format!(
            r#"(?m)^(\s*{}\s*=\s*\{{\s*version\s*=\s*")([^"]+)(")"#,
            regex::escape(dep_name)
        );

        if let Ok(re) = Regex::new(&detailed_pattern) {
            if re.is_match(&self.original_content) {
                let new_content = re.replace(&self.original_content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_version, &caps[3])
                });
                self.original_content = new_content.to_string();
                return Ok(());
            }
        }

        // Strategy 2: Simple format - name = "x.y.z"
        let simple_pattern = format!(r#"(?m)^(\s*{}\s*=\s*")([^"]+)(")"#, regex::escape(dep_name));

        if let Ok(re) = Regex::new(&simple_pattern) {
            if re.is_match(&self.original_content) {
                let new_content = re.replace(&self.original_content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_version, &caps[3])
                });
                self.original_content = new_content.to_string();
                return Ok(());
            }
        }

        anyhow::bail!("Could not find dependency {} in Cargo.toml", dep_name);
    }

    /// Save the updated Cargo.toml
    pub fn save(&self) -> Result<()> {
        // Create backup
        let backup_path = self.manifest.path.with_extension("toml.backup");
        fs::copy(&self.manifest.path, &backup_path).context("Failed to create backup")?;

        // Write updated content
        fs::write(&self.manifest.path, &self.original_content)
            .context("Failed to write updated Cargo.toml")?;

        Ok(())
    }

    /// Get the current content (for dry-run)
    pub fn get_content(&self) -> &str {
        &self.original_content
    }
}
