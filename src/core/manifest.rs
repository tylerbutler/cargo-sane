//! Cargo.toml manifest handling

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Manifest {
    pub path: PathBuf,
    pub content: ManifestContent,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestContent {
    pub package: Option<Package>,
    pub dependencies: Option<HashMap<String, DependencySpec>>,
    #[serde(rename = "dev-dependencies")]
    pub dev_dependencies: Option<HashMap<String, DependencySpec>>,
    #[serde(rename = "build-dependencies")]
    pub build_dependencies: Option<HashMap<String, DependencySpec>>,
}

/// Represents a field that can either be a value or inherited from workspace
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InheritableField<T> {
    Value(T),
    Workspace { workspace: bool },
}

impl<T> InheritableField<T> {
    /// Get the value if it's a direct value (not workspace-inherited)
    pub fn as_value(&self) -> Option<&T> {
        match self {
            InheritableField::Value(v) => Some(v),
            InheritableField::Workspace { .. } => None,
        }
    }

    /// Check if this field is inherited from workspace
    pub fn is_workspace(&self) -> bool {
        matches!(self, InheritableField::Workspace { .. })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: InheritableField<String>,
    #[serde(default)]
    pub edition: Option<InheritableField<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    Simple(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailedDependency {
    pub version: Option<String>,
    pub git: Option<String>,
    pub path: Option<String>,
    pub features: Option<Vec<String>>,
    pub optional: Option<bool>,
    #[serde(rename = "default-features")]
    pub default_features: Option<bool>,
    // Ignore other fields
    #[serde(flatten)]
    pub other: Option<HashMap<String, toml::Value>>,
}

impl Manifest {
    /// Find Cargo.toml in current directory or specified path
    pub fn find(path: Option<String>) -> Result<Self> {
        let manifest_path = if let Some(p) = path {
            PathBuf::from(p)
        } else {
            // Look for Cargo.toml in current directory
            let current = std::env::current_dir().context("Failed to get current directory")?;
            current.join("Cargo.toml")
        };

        Self::from_path(&manifest_path)
    }

    /// Load manifest from specific path
    pub fn from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("Cargo.toml not found at: {}", path.display());
        }

        let content_str = fs::read_to_string(path)
            .context(format!("Failed to read Cargo.toml at {}", path.display()))?;

        let content: ManifestContent =
            toml::from_str(&content_str).context("Failed to parse Cargo.toml")?;

        Ok(Self {
            path: path.to_path_buf(),
            content,
        })
    }

    /// Get all dependencies (direct only)
    pub fn get_dependencies(&self) -> Vec<(String, DependencySpec)> {
        let mut deps = Vec::new();

        if let Some(ref dependencies) = self.content.dependencies {
            for (name, spec) in dependencies {
                deps.push((name.clone(), spec.clone()));
            }
        }

        deps
    }

    /// Get package name
    pub fn package_name(&self) -> Option<&str> {
        self.content.package.as_ref().map(|p| p.name.as_str())
    }
}

impl DependencySpec {
    /// Get version string if available
    pub fn version(&self) -> Option<&str> {
        match self {
            DependencySpec::Simple(v) => Some(v.as_str()),
            DependencySpec::Detailed(d) => d.version.as_deref(),
        }
    }

    /// Check if this is a git dependency
    pub fn is_git(&self) -> bool {
        match self {
            DependencySpec::Simple(_) => false,
            DependencySpec::Detailed(d) => d.git.is_some(),
        }
    }

    /// Check if this is a path dependency
    pub fn is_path(&self) -> bool {
        match self {
            DependencySpec::Simple(_) => false,
            DependencySpec::Detailed(d) => d.path.is_some(),
        }
    }

    /// Check if this is from crates.io (not git or path)
    pub fn is_crates_io(&self) -> bool {
        !self.is_git() && !self.is_path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_workspace_inherited_version() {
        // Create a temp directory and file with workspace inheritance
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        let toml_content = r#"
[package]
name = "test-package"
version.workspace = true
edition.workspace = true

[dependencies]
serde = "1.0"
"#;

        fs::write(&manifest_path, toml_content).unwrap();

        // This should not panic or fail - it should parse successfully
        let result = Manifest::from_path(&manifest_path);

        assert!(
            result.is_ok(),
            "Should parse workspace-inherited fields without error"
        );

        let manifest = result.unwrap();
        assert_eq!(manifest.package_name(), Some("test-package"));
    }

    #[test]
    fn test_parse_workspace_inherited_dependency() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        let toml_content = r#"
[package]
name = "test-package"
version = "0.1.0"

[dependencies]
serde.workspace = true
tokio = "1.0"
"#;

        fs::write(&manifest_path, toml_content).unwrap();

        // This should parse successfully
        let result = Manifest::from_path(&manifest_path);

        assert!(
            result.is_ok(),
            "Should parse workspace-inherited dependencies"
        );

        let manifest = result.unwrap();
        let deps = manifest.get_dependencies();

        // Should have 2 dependencies
        assert_eq!(deps.len(), 2);
    }
}
