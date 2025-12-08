//! Check for dependency updates

use crate::core::dependency::Dependency;
use crate::core::manifest::{DependencySpec, Manifest};
use crate::core::workspace::WorkspaceContext;
use crate::utils::crates_io::CratesIoClient;
use crate::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use semver::Version;
use std::sync::Mutex;

pub struct DependencyChecker {
    client: CratesIoClient,
}

impl DependencyChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: CratesIoClient::new()?,
        })
    }

    /// Analyze all dependencies in a manifest
    pub fn check_dependencies(&self, manifest: &Manifest) -> Result<Vec<Dependency>> {
        // Create workspace context for resolving inherited dependencies
        let workspace_ctx = WorkspaceContext::new(&manifest.path)?;
        self.check_dependencies_with_context(manifest, &workspace_ctx)
    }

    /// Analyze all dependencies in a manifest with workspace context
    pub fn check_dependencies_with_context(
        &self,
        manifest: &Manifest,
        workspace_ctx: &WorkspaceContext,
    ) -> Result<Vec<Dependency>> {
        self.check_dependencies_with_progress(manifest, workspace_ctx, None, None)
    }

    /// Analyze dependencies with optional MultiProgress support for workspace mode
    ///
    /// When `multi_progress` is provided, the progress bar is added to it for coordinated display.
    /// The `package_name` is used as a prefix in workspace mode to identify which package is being checked.
    pub fn check_dependencies_with_progress(
        &self,
        manifest: &Manifest,
        workspace_ctx: &WorkspaceContext,
        multi_progress: Option<&MultiProgress>,
        package_name: Option<&str>,
    ) -> Result<Vec<Dependency>> {
        let deps = manifest.get_dependencies();

        if deps.is_empty() {
            return Ok(Vec::new());
        }

        // Create progress bar - either standalone or attached to MultiProgress
        let pb = match multi_progress {
            Some(mp) => mp.add(ProgressBar::new(deps.len() as u64)),
            None => ProgressBar::new(deps.len() as u64),
        };

        // Use package name in template if provided (workspace mode)
        let template = match package_name {
            Some(name) => format!(
                "{{spinner:.green}} [{{elapsed_precise}}] [{{bar:30.cyan/blue}}] {{pos}}/{{len}} {} ",
                name
            ),
            None => "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} checking dependencies...".to_string(),
        };

        pb.set_style(
            ProgressStyle::default_bar()
                .template(&template)
                .expect("Failed to set progress style")
                .progress_chars("#>-"),
        );

        // Collect warnings to print after progress bars (avoid interleaving)
        let warnings: Mutex<Vec<String>> = Mutex::new(Vec::new());

        // Prepare dependency info for parallel processing
        let dep_infos: Vec<_> = deps
            .into_iter()
            .filter_map(|(name, spec)| {
                // Skip git and path dependencies (but not workspace - we can resolve those)
                if spec.is_git() || spec.is_path() {
                    pb.inc(1);
                    return None;
                }

                // Get current version - resolve from workspace if needed
                let version_str = match Self::resolve_version_static(&name, &spec, workspace_ctx) {
                    Some(v) => v,
                    None => {
                        if spec.is_workspace() {
                            warnings.lock().unwrap().push(format!(
                                "Warning: Could not resolve workspace dependency '{}' - workspace root not found or dependency not defined",
                                name
                            ));
                        }
                        pb.inc(1);
                        return None;
                    }
                };

                // Parse version requirement (remove ^, ~, etc)
                let current_version = match parse_version_req(&version_str) {
                    Some(v) => v,
                    None => {
                        warnings.lock().unwrap().push(format!(
                            "Warning: Could not parse version '{}' for {}",
                            version_str, name
                        ));
                        pb.inc(1);
                        return None;
                    }
                };

                Some((name, spec, current_version))
            })
            .collect();

        // Process dependencies in parallel - this is where the HTTP calls happen
        let results: Vec<Dependency> = dep_infos
            .par_iter()
            .filter_map(|(name, spec, current_version)| {
                // Fetch latest version from crates.io (parallel HTTP requests)
                let latest_version = match self.client.get_latest_version(name) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warnings.lock().unwrap().push(format!(
                            "Warning: Failed to fetch info for {}: {}",
                            name, e
                        ));
                        None
                    }
                };

                // Mark if this dependency is workspace-inherited
                let mut dep = Dependency::new(name.clone(), current_version.clone(), true);
                if spec.is_workspace() {
                    dep = dep.with_workspace_inherited(true);
                }
                if let Some(latest) = latest_version {
                    dep = dep.with_latest(latest);
                }

                pb.inc(1);
                Some(dep)
            })
            .collect();

        pb.finish_and_clear();

        // Print warnings after progress bar is done (only in standalone mode)
        if multi_progress.is_none() {
            for warning in warnings.into_inner().unwrap() {
                eprintln!("{}", warning);
            }
            println!();
        }

        Ok(results)
    }

    /// Static version of resolve_version for use in closures
    fn resolve_version_static(
        name: &str,
        spec: &DependencySpec,
        workspace_ctx: &WorkspaceContext,
    ) -> Option<String> {
        match spec {
            DependencySpec::Simple(v) => Some(v.clone()),
            DependencySpec::Detailed(d) => d.version.clone(),
            DependencySpec::Workspace { workspace: true } => {
                // Resolve from workspace
                workspace_ctx
                    .resolve_dependency_version(name)
                    .map(|s| s.to_string())
            }
            DependencySpec::Workspace { workspace: false } => None,
        }
    }

}

impl Default for DependencyChecker {
    fn default() -> Self {
        Self::new().expect("Failed to create DependencyChecker")
    }
}

/// Parse a version requirement string and extract a concrete version
/// Examples:
///   "1.0.5" -> Some(1.0.5)
///   "1.0" -> Some(1.0.0)
///   "1" -> Some(1.0.0)
///   "^1.0.5" -> Some(1.0.5)
///   "~1.0.5" -> Some(1.0.5)
///   ">=1.0.5" -> Some(1.0.5)
fn parse_version_req(req: &str) -> Option<Version> {
    // Remove common version requirement prefixes
    let cleaned = req
        .trim()
        .trim_start_matches('^')
        .trim_start_matches('~')
        .trim_start_matches('=')
        .trim_start_matches('>')
        .trim_start_matches('<')
        .trim();

    // Try to parse the cleaned version directly
    if let Ok(v) = Version::parse(cleaned) {
        return Some(v);
    }

    // If it fails, try to normalize the version
    // "1.0" -> "1.0.0"
    // "1" -> "1.0.0"
    let normalized = normalize_version(cleaned);
    Version::parse(&normalized).ok()
}

/// Normalize a version string to major.minor.patch format
/// Examples:
///   "1" -> "1.0.0"
///   "1.0" -> "1.0.0"
///   "1.0.5" -> "1.0.5"
fn normalize_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();

    match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("1"), "1.0.0");
        assert_eq!(normalize_version("1.0"), "1.0.0");
        assert_eq!(normalize_version("1.0.5"), "1.0.5");
        assert_eq!(normalize_version("1.35"), "1.35.0");
    }

    #[test]
    fn test_parse_version_req() {
        assert_eq!(parse_version_req("1.0.5"), Some(Version::new(1, 0, 5)));
        assert_eq!(parse_version_req("1.0"), Some(Version::new(1, 0, 0)));
        assert_eq!(parse_version_req("1"), Some(Version::new(1, 0, 0)));
        assert_eq!(parse_version_req("^1.0.5"), Some(Version::new(1, 0, 5)));
        assert_eq!(parse_version_req("~1.0.5"), Some(Version::new(1, 0, 5)));
        assert_eq!(parse_version_req("1.35"), Some(Version::new(1, 35, 0)));
    }
}
