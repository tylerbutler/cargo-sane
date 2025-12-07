//! Workspace detection and value resolution
//!
//! This module provides functionality to:
//! 1. Detect workspace roots by walking up the directory tree
//! 2. Parse workspace configuration ([workspace.package] and [workspace.dependencies])
//! 3. Resolve inherited values from workspace root
//! 4. Discover all workspace members (with glob pattern support)

use anyhow::{Context, Result};
use glob::glob;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::manifest::{DependencySpec, Manifest};

/// Represents a parsed workspace root Cargo.toml
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    /// Path to the workspace root Cargo.toml
    pub path: PathBuf,
    /// Package defaults that can be inherited ([workspace.package])
    pub package: Option<WorkspacePackage>,
    /// Dependencies that can be inherited ([workspace.dependencies])
    pub dependencies: Option<HashMap<String, DependencySpec>>,
    /// Members of the workspace
    pub members: Vec<String>,
}

/// Package fields that can be inherited from workspace
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspacePackage {
    pub version: Option<String>,
    pub edition: Option<String>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    pub documentation: Option<String>,
    pub readme: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    #[serde(rename = "license-file")]
    pub license_file: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub categories: Option<Vec<String>>,
    pub publish: Option<bool>,
    #[serde(rename = "rust-version")]
    pub rust_version: Option<String>,
    pub exclude: Option<Vec<String>>,
    pub include: Option<Vec<String>>,
}

/// Raw workspace Cargo.toml structure for deserialization
#[derive(Debug, Deserialize)]
struct WorkspaceToml {
    workspace: Option<WorkspaceSection>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceSection {
    members: Option<Vec<String>>,
    package: Option<WorkspacePackage>,
    dependencies: Option<HashMap<String, DependencySpec>>,
}

impl WorkspaceRoot {
    /// Find the workspace root starting from a given path
    ///
    /// Walks up the directory tree looking for a Cargo.toml that contains
    /// a [workspace] section. Returns None if no workspace root is found.
    pub fn find(start_path: &Path) -> Result<Option<Self>> {
        // Start from the directory containing the manifest
        let start_dir = if start_path.is_file() {
            start_path
                .parent()
                .context("Failed to get parent directory")?
        } else {
            start_path
        };

        let mut current = start_dir.to_path_buf();

        loop {
            let cargo_toml = current.join("Cargo.toml");

            if cargo_toml.exists() {
                // Check if this Cargo.toml has a [workspace] section
                if let Some(workspace_root) = Self::try_parse(&cargo_toml)? {
                    return Ok(Some(workspace_root));
                }
            }

            // Move up to parent directory
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break, // Reached filesystem root
            }
        }

        Ok(None)
    }

    /// Try to parse a Cargo.toml as a workspace root
    ///
    /// Returns Some(WorkspaceRoot) if the file contains a [workspace] section,
    /// None otherwise.
    fn try_parse(path: &Path) -> Result<Option<Self>> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let toml: WorkspaceToml =
            toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;

        match toml.workspace {
            Some(workspace) => Ok(Some(Self {
                path: path.to_path_buf(),
                package: workspace.package,
                dependencies: workspace.dependencies,
                members: workspace.members.unwrap_or_default(),
            })),
            None => Ok(None),
        }
    }

    /// Get a package field value by name
    pub fn get_package_field(&self, field: &str) -> Option<String> {
        let pkg = self.package.as_ref()?;
        match field {
            "version" => pkg.version.clone(),
            "edition" => pkg.edition.clone(),
            "description" => pkg.description.clone(),
            "documentation" => pkg.documentation.clone(),
            "readme" => pkg.readme.clone(),
            "homepage" => pkg.homepage.clone(),
            "repository" => pkg.repository.clone(),
            "license" => pkg.license.clone(),
            "license-file" => pkg.license_file.clone(),
            "rust-version" => pkg.rust_version.clone(),
            _ => None,
        }
    }

    /// Get a dependency specification from workspace dependencies
    pub fn get_dependency(&self, name: &str) -> Option<&DependencySpec> {
        self.dependencies.as_ref()?.get(name)
    }

    /// Get the version string for a workspace dependency
    pub fn get_dependency_version(&self, name: &str) -> Option<&str> {
        self.get_dependency(name)?.version()
    }

    /// Get the workspace root directory
    pub fn root_dir(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Discover all workspace member directories by expanding glob patterns
    ///
    /// Workspace members can be specified as:
    /// - Exact paths: "member1", "crates/foo"
    /// - Glob patterns: "crates/*", "packages/*"
    pub fn discover_member_paths(&self) -> Result<Vec<PathBuf>> {
        let root_dir = self.root_dir();
        let mut member_paths = Vec::new();

        for pattern in &self.members {
            let full_pattern = root_dir.join(pattern);
            let pattern_str = full_pattern.to_string_lossy();

            // Check if this is a glob pattern or a direct path
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                // Expand glob pattern
                for entry in glob(&pattern_str)
                    .with_context(|| format!("Invalid glob pattern: {}", pattern))?
                {
                    match entry {
                        Ok(path) => {
                            if path.is_dir() && path.join("Cargo.toml").exists() {
                                member_paths.push(path);
                            }
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to read glob entry: {}", e);
                        }
                    }
                }
            } else {
                // Direct path
                let member_dir = root_dir.join(pattern);
                if member_dir.is_dir() && member_dir.join("Cargo.toml").exists() {
                    member_paths.push(member_dir);
                }
            }
        }

        // Sort for consistent ordering
        member_paths.sort();
        Ok(member_paths)
    }

    /// Get all member manifests in the workspace
    pub fn get_member_manifests(&self) -> Result<Vec<Manifest>> {
        let member_paths = self.discover_member_paths()?;
        let mut manifests = Vec::new();

        for path in member_paths {
            let manifest_path = path.join("Cargo.toml");
            match Manifest::from_path(&manifest_path) {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to parse {}: {}",
                        manifest_path.display(),
                        e
                    );
                }
            }
        }

        Ok(manifests)
    }

    /// Check if the workspace root itself is also a package (has [package] section)
    pub fn is_also_package(&self) -> bool {
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        content.contains("[package]")
    }
}

/// Context for resolving workspace-inherited values
#[derive(Debug)]
pub struct WorkspaceContext {
    /// The workspace root, if found
    pub root: Option<WorkspaceRoot>,
    /// Path to the member manifest being analyzed
    pub member_path: PathBuf,
}

impl WorkspaceContext {
    /// Create a new workspace context for a manifest
    pub fn new(manifest_path: &Path) -> Result<Self> {
        let root = WorkspaceRoot::find(manifest_path)?;
        Ok(Self {
            root,
            member_path: manifest_path.to_path_buf(),
        })
    }

    /// Check if we're in a workspace
    pub fn is_workspace(&self) -> bool {
        self.root.is_some()
    }

    /// Check if the member is at the workspace root
    pub fn is_root_manifest(&self) -> bool {
        match &self.root {
            Some(root) => root.path == self.member_path,
            None => false,
        }
    }

    /// Resolve a package field value
    ///
    /// Returns the inherited value from the workspace root, or None if not available.
    pub fn resolve_package_field(&self, field: &str) -> Option<String> {
        self.root.as_ref()?.get_package_field(field)
    }

    /// Resolve a dependency version from workspace
    ///
    /// Returns the version string if the dependency is defined in workspace.dependencies.
    pub fn resolve_dependency_version(&self, name: &str) -> Option<&str> {
        self.root.as_ref()?.get_dependency_version(name)
    }

    /// Get the full dependency spec from workspace
    pub fn resolve_dependency(&self, name: &str) -> Option<&DependencySpec> {
        self.root.as_ref()?.get_dependency(name)
    }

    /// Get the path to workspace root Cargo.toml
    pub fn workspace_root_path(&self) -> Option<&Path> {
        self.root.as_ref().map(|r| r.path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_workspace_structure(temp_dir: &TempDir) -> (PathBuf, PathBuf) {
        // Create workspace root Cargo.toml
        let root_toml = temp_dir.path().join("Cargo.toml");
        let root_content = r#"
[workspace]
members = ["member1", "member2"]

[workspace.package]
version = "1.0.0"
edition = "2021"
authors = ["Test Author"]
license = "MIT"

[workspace.dependencies]
serde = { version = "1.0.193", features = ["derive"] }
tokio = "1.35.0"
anyhow = "1.0"
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
edition.workspace = true

[dependencies]
serde.workspace = true
tokio = "1.0"
"#;
        fs::write(&member_toml, member_content).unwrap();

        (root_toml, member_toml)
    }

    #[test]
    fn test_find_workspace_root() {
        let temp_dir = TempDir::new().unwrap();
        let (root_toml, member_toml) = create_workspace_structure(&temp_dir);

        // Find workspace from member
        let workspace = WorkspaceRoot::find(&member_toml)
            .expect("Should not error")
            .expect("Should find workspace root");

        assert_eq!(workspace.path, root_toml);
        assert_eq!(workspace.members, vec!["member1", "member2"]);
    }

    #[test]
    fn test_workspace_package_fields() {
        let temp_dir = TempDir::new().unwrap();
        let (_, member_toml) = create_workspace_structure(&temp_dir);

        let context = WorkspaceContext::new(&member_toml).expect("Should create context");

        assert!(context.is_workspace());
        assert!(!context.is_root_manifest());

        // Resolve package fields
        assert_eq!(
            context.resolve_package_field("version"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            context.resolve_package_field("edition"),
            Some("2021".to_string())
        );
        assert_eq!(
            context.resolve_package_field("license"),
            Some("MIT".to_string())
        );
    }

    #[test]
    fn test_workspace_dependency_resolution() {
        let temp_dir = TempDir::new().unwrap();
        let (_, member_toml) = create_workspace_structure(&temp_dir);

        let context = WorkspaceContext::new(&member_toml).expect("Should create context");

        // Resolve dependency versions
        assert_eq!(
            context.resolve_dependency_version("serde"),
            Some("1.0.193")
        );
        assert_eq!(context.resolve_dependency_version("tokio"), Some("1.35.0"));
        assert_eq!(context.resolve_dependency_version("anyhow"), Some("1.0"));

        // Unknown dependency
        assert_eq!(context.resolve_dependency_version("unknown"), None);
    }

    #[test]
    fn test_no_workspace() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        // Create a non-workspace Cargo.toml
        let content = r#"
[package]
name = "standalone"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#;
        fs::write(&manifest_path, content).unwrap();

        let context = WorkspaceContext::new(&manifest_path).expect("Should create context");

        assert!(!context.is_workspace());
        assert_eq!(context.resolve_package_field("version"), None);
        assert_eq!(context.resolve_dependency_version("serde"), None);
    }

    #[test]
    fn test_workspace_root_is_member() {
        let temp_dir = TempDir::new().unwrap();
        let (root_toml, _) = create_workspace_structure(&temp_dir);

        let context = WorkspaceContext::new(&root_toml).expect("Should create context");

        assert!(context.is_workspace());
        assert!(context.is_root_manifest());
    }

    #[test]
    fn test_discover_member_paths() {
        let temp_dir = TempDir::new().unwrap();
        let (root_toml, _) = create_workspace_structure(&temp_dir);

        // Create member2 directory
        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir_all(&member2_dir).unwrap();
        fs::write(
            member2_dir.join("Cargo.toml"),
            r#"[package]
name = "member2"
version = "0.1.0"
"#,
        )
        .unwrap();

        let workspace = WorkspaceRoot::find(&root_toml)
            .expect("Should not error")
            .expect("Should find workspace");

        let members = workspace.discover_member_paths().expect("Should discover members");

        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|p| p.ends_with("member1")));
        assert!(members.iter().any(|p| p.ends_with("member2")));
    }

    #[test]
    fn test_discover_member_paths_with_glob() {
        let temp_dir = TempDir::new().unwrap();

        // Create workspace with glob pattern
        let root_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &root_toml,
            r#"
[workspace]
members = ["crates/*"]
"#,
        )
        .unwrap();

        // Create crates directory with multiple members
        let crates_dir = temp_dir.path().join("crates");
        fs::create_dir_all(&crates_dir).unwrap();

        for name in ["foo", "bar", "baz"] {
            let crate_dir = crates_dir.join(name);
            fs::create_dir_all(&crate_dir).unwrap();
            fs::write(
                crate_dir.join("Cargo.toml"),
                format!(
                    r#"[package]
name = "{}"
version = "0.1.0"
"#,
                    name
                ),
            )
            .unwrap();
        }

        let workspace = WorkspaceRoot::find(&root_toml)
            .expect("Should not error")
            .expect("Should find workspace");

        let members = workspace.discover_member_paths().expect("Should discover members");

        assert_eq!(members.len(), 3);
        assert!(members.iter().any(|p| p.ends_with("foo")));
        assert!(members.iter().any(|p| p.ends_with("bar")));
        assert!(members.iter().any(|p| p.ends_with("baz")));
    }

    #[test]
    fn test_get_member_manifests() {
        let temp_dir = TempDir::new().unwrap();
        let (root_toml, _) = create_workspace_structure(&temp_dir);

        // Create member2
        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir_all(&member2_dir).unwrap();
        fs::write(
            member2_dir.join("Cargo.toml"),
            r#"[package]
name = "member2"
version = "0.1.0"

[dependencies]
log = "0.4"
"#,
        )
        .unwrap();

        let workspace = WorkspaceRoot::find(&root_toml)
            .expect("Should not error")
            .expect("Should find workspace");

        let manifests = workspace.get_member_manifests().expect("Should get manifests");

        assert_eq!(manifests.len(), 2);

        let names: Vec<_> = manifests
            .iter()
            .filter_map(|m| m.package_name())
            .collect();
        assert!(names.contains(&"member1"));
        assert!(names.contains(&"member2"));
    }
}
