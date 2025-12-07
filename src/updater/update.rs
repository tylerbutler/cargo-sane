//! Update dependencies in Cargo.toml

use crate::core::dependency::Dependency;
use crate::core::manifest::Manifest;
use crate::core::workspace::WorkspaceContext;
use crate::Result;
use anyhow::Context;
use regex::Regex;
use std::fs;
use std::path::PathBuf;

pub struct DependencyUpdater {
    manifest: Manifest,
    original_content: String,
    /// Content of workspace root Cargo.toml (if different from manifest)
    workspace_content: Option<String>,
    /// Path to workspace root Cargo.toml
    workspace_path: Option<PathBuf>,
}

impl DependencyUpdater {
    pub fn new(manifest: Manifest) -> Result<Self> {
        let original_content =
            fs::read_to_string(&manifest.path).context("Failed to read Cargo.toml")?;

        // Create workspace context
        let workspace_ctx = WorkspaceContext::new(&manifest.path)?;

        // Load workspace root content if we're in a workspace and not at the root
        let (workspace_content, workspace_path) = if workspace_ctx.is_workspace()
            && !workspace_ctx.is_root_manifest()
        {
            if let Some(root_path) = workspace_ctx.workspace_root_path() {
                let content = fs::read_to_string(root_path)
                    .context("Failed to read workspace root Cargo.toml")?;
                (Some(content), Some(root_path.to_path_buf()))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // workspace_ctx is used for detection, but we only need the paths after
        let _ = &workspace_ctx; // Acknowledge usage

        Ok(Self {
            manifest,
            original_content,
            workspace_content,
            workspace_path,
        })
    }

    /// Update a single dependency to a new version
    ///
    /// For workspace-inherited dependencies, this updates the workspace root Cargo.toml.
    /// For regular dependencies, this updates the member Cargo.toml.
    pub fn update_dependency(&mut self, dep: &Dependency, new_version: &str) -> Result<()> {
        if dep.is_workspace_inherited {
            self.update_workspace_dependency(&dep.name, new_version)
        } else {
            self.update_local_dependency(&dep.name, new_version)
        }
    }

    /// Update a dependency in the local manifest
    fn update_local_dependency(&mut self, dep_name: &str, new_version: &str) -> Result<()> {
        self.original_content =
            Self::update_version_in_content(&self.original_content, dep_name, new_version)?;
        Ok(())
    }

    /// Update a dependency in the workspace root manifest
    fn update_workspace_dependency(&mut self, dep_name: &str, new_version: &str) -> Result<()> {
        let content = self
            .workspace_content
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No workspace root found for inherited dependency"))?;

        let updated = Self::update_version_in_content(content, dep_name, new_version)?;
        self.workspace_content = Some(updated);
        Ok(())
    }

    /// Update a version string in TOML content using regex
    pub fn update_version_in_content(
        content: &str,
        dep_name: &str,
        new_version: &str,
    ) -> Result<String> {
        // Strategy 1: Detailed format - name = { version = "x.y.z", ... }
        // Capture: everything up to and including opening quote, version, closing quote
        let detailed_pattern = format!(
            r#"(?m)^(\s*{}\s*=\s*\{{\s*version\s*=\s*")([^"]+)(")"#,
            regex::escape(dep_name)
        );

        if let Ok(re) = Regex::new(&detailed_pattern) {
            if re.is_match(content) {
                let new_content = re.replace(content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_version, &caps[3])
                });
                return Ok(new_content.to_string());
            }
        }

        // Strategy 2: Simple format - name = "x.y.z"
        let simple_pattern = format!(r#"(?m)^(\s*{}\s*=\s*")([^"]+)(")"#, regex::escape(dep_name));

        if let Ok(re) = Regex::new(&simple_pattern) {
            if re.is_match(content) {
                let new_content = re.replace(content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_version, &caps[3])
                });
                return Ok(new_content.to_string());
            }
        }

        anyhow::bail!("Could not find dependency {} in Cargo.toml", dep_name)
    }

    /// Save the updated Cargo.toml(s)
    pub fn save(&self) -> Result<()> {
        // Create backup and save member manifest
        let backup_path = self.manifest.path.with_extension("toml.backup");
        fs::copy(&self.manifest.path, &backup_path).context("Failed to create backup")?;

        fs::write(&self.manifest.path, &self.original_content)
            .context("Failed to write updated Cargo.toml")?;

        // Save workspace root if it was modified
        if let (Some(ref content), Some(ref path)) = (&self.workspace_content, &self.workspace_path)
        {
            let backup_path = path.with_extension("toml.backup");
            fs::copy(path, &backup_path).context("Failed to create workspace backup")?;

            fs::write(path, content).context("Failed to write updated workspace Cargo.toml")?;
        }

        Ok(())
    }

    /// Check if workspace root was modified
    pub fn workspace_modified(&self) -> bool {
        self.workspace_content.is_some()
    }

    /// Get the path to workspace root if it exists and was modified
    pub fn workspace_root_path(&self) -> Option<&PathBuf> {
        if self.workspace_content.is_some() {
            self.workspace_path.as_ref()
        } else {
            None
        }
    }

    /// Get the current content (for dry-run)
    pub fn get_content(&self) -> &str {
        &self.original_content
    }

    /// Get the workspace content (for dry-run)
    pub fn get_workspace_content(&self) -> Option<&str> {
        self.workspace_content.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dependency::Dependency;
    use semver::Version;
    use std::fs;
    use tempfile::TempDir;

    fn create_workspace_structure(temp_dir: &TempDir) -> (PathBuf, PathBuf) {
        // Create workspace root Cargo.toml
        let root_toml = temp_dir.path().join("Cargo.toml");
        let root_content = r#"
[workspace]
members = ["member1"]

[workspace.package]
version = "1.0.0"
edition = "2021"

[workspace.dependencies]
serde = { version = "1.0.193", features = ["derive"] }
tokio = "1.35.0"
"#;
        fs::write(&root_toml, root_content).unwrap();

        // Create member directory
        let member_dir = temp_dir.path().join("member1");
        fs::create_dir_all(&member_dir).unwrap();

        // Create member Cargo.toml with workspace inheritance
        let member_toml = member_dir.join("Cargo.toml");
        let member_content = r#"
[package]
name = "member1"
version.workspace = true

[dependencies]
serde.workspace = true
anyhow = "1.0"
"#;
        fs::write(&member_toml, member_content).unwrap();

        (root_toml, member_toml)
    }

    #[test]
    fn test_update_version_in_content_simple() {
        let content = r#"
[dependencies]
serde = "1.0.0"
tokio = "1.0"
"#;
        let result = DependencyUpdater::update_version_in_content(content, "serde", "1.0.200");
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains(r#"serde = "1.0.200""#));
        // Ensure tokio is untouched
        assert!(updated.contains(r#"tokio = "1.0""#));
    }

    #[test]
    fn test_update_version_in_content_detailed() {
        let content = r#"
[workspace.dependencies]
serde = { version = "1.0.0", features = ["derive"] }
tokio = "1.0"
"#;
        let result = DependencyUpdater::update_version_in_content(content, "serde", "1.0.200");
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains(r#"serde = { version = "1.0.200", features = ["derive"] }"#));
    }

    #[test]
    fn test_update_local_dependency() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        let content = r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0.0"
"#;
        fs::write(&manifest_path, content).unwrap();

        let manifest = Manifest::from_path(&manifest_path).unwrap();
        let mut updater = DependencyUpdater::new(manifest).unwrap();

        let dep = Dependency::new("serde".to_string(), Version::new(1, 0, 0), true);
        updater.update_dependency(&dep, "1.0.200").unwrap();

        assert!(updater.get_content().contains(r#"serde = "1.0.200""#));
        assert!(!updater.workspace_modified());
    }

    #[test]
    fn test_update_workspace_dependency() {
        let temp_dir = TempDir::new().unwrap();
        let (_, member_toml) = create_workspace_structure(&temp_dir);

        let manifest = Manifest::from_path(&member_toml).unwrap();
        let mut updater = DependencyUpdater::new(manifest).unwrap();

        // Simulate a workspace-inherited dependency
        let mut dep = Dependency::new("serde".to_string(), Version::new(1, 0, 193), true);
        dep = dep.with_workspace_inherited(true);

        updater.update_dependency(&dep, "1.0.200").unwrap();

        // The workspace content should be modified
        assert!(updater.workspace_modified());
        let ws_content = updater.get_workspace_content().unwrap();
        assert!(ws_content.contains("1.0.200"));
    }

    #[test]
    fn test_mixed_updates() {
        let temp_dir = TempDir::new().unwrap();
        let (_, member_toml) = create_workspace_structure(&temp_dir);

        let manifest = Manifest::from_path(&member_toml).unwrap();
        let mut updater = DependencyUpdater::new(manifest).unwrap();

        // Update workspace-inherited serde
        let mut serde_dep = Dependency::new("serde".to_string(), Version::new(1, 0, 193), true);
        serde_dep = serde_dep.with_workspace_inherited(true);
        updater.update_dependency(&serde_dep, "1.0.200").unwrap();

        // Update local anyhow
        let anyhow_dep = Dependency::new("anyhow".to_string(), Version::new(1, 0, 0), true);
        updater.update_dependency(&anyhow_dep, "1.0.100").unwrap();

        // Both should be updated in their respective locations
        assert!(updater.workspace_modified());
        assert!(updater.get_workspace_content().unwrap().contains("1.0.200"));
        assert!(updater.get_content().contains(r#"anyhow = "1.0.100""#));
    }
}
