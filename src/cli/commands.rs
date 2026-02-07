//! Command implementations for cargo-sane CLI.
//!
//! This module contains the implementation of all CLI commands:
//! - `check`: Analyze dependencies and show available updates
//! - `update`: Update dependencies interactively
//! - `fix`: Fix dependency conflicts (not yet implemented)
//! - `clean`: Remove unused dependencies (not yet implemented)
//! - `health`: Check dependency health (not yet implemented)

use crate::analyzer::checker::DependencyChecker;
use crate::cli::output;
use crate::core::dependency::{Dependency, UpdateType};
use crate::core::manifest::Manifest;
use crate::core::workspace::WorkspaceContext;
use crate::updater::DependencyUpdater;
use crate::Result;
use anyhow::Context;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use indicatif::MultiProgress;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Tracks which member manifest contains a dependency.
///
/// Used during workspace-wide updates to know which member Cargo.toml files
/// need to be modified for member-specific (non-workspace) dependencies.
#[derive(Debug, Clone)]
struct MemberDepInfo {
    manifest_path: PathBuf,
}

/// Aggregated dependency info for workspace-wide analysis.
///
/// When analyzing a workspace, the same dependency may appear in multiple
/// member crates. This struct aggregates that information to provide a
/// unified view and enable efficient updates.
#[derive(Debug, Clone)]
struct AggregatedDep {
    /// The dependency with version information
    dep: Dependency,
    /// Names of packages that use this dependency
    used_by: Vec<String>,
    /// For member-specific (non-workspace) deps, the manifests that contain them
    member_manifests: Vec<MemberDepInfo>,
}

/// Format a workspace marker for display (e.g., " [workspace]" or "").
fn workspace_marker(is_inherited: bool) -> String {
    if is_inherited {
        " [workspace]".dimmed().to_string()
    } else {
        String::new()
    }
}

/// Format a workspace marker with member count for aggregated dependencies.
///
/// Returns " [workspace]" for inherited deps or " [N member(s)]" for member-specific deps.
fn aggregated_marker(dep: &Dependency, member_count: usize) -> String {
    if dep.is_workspace_inherited {
        " [workspace]".dimmed().to_string()
    } else {
        format!(" [{} member(s)]", member_count).dimmed().to_string()
    }
}

/// Print common manifest header info (package name, path, workspace info).
fn print_manifest_header(manifest: &Manifest, workspace_ctx: &WorkspaceContext) {
    if let Some(name) = manifest.package_name() {
        output::print_info(&format!("Package: {}", name));
    }
    output::print_info(&format!("Manifest: {}", manifest.path.display()));

    if workspace_ctx.is_workspace() {
        if let Some(root_path) = workspace_ctx.workspace_root_path() {
            output::print_info(&format!(
                "Workspace: {}",
                root_path.parent().unwrap_or(root_path).display()
            ));
        }
    }
    println!();
}

pub fn check_command(manifest_path: Option<String>, verbose: bool, workspace: bool) -> Result<()> {
    output::print_header("🧠 cargo-sane check");
    println!();

    // Load Cargo.toml
    let manifest = Manifest::find(manifest_path)?;

    // Check for workspace context
    let workspace_ctx = WorkspaceContext::new(&manifest.path)?;

    if workspace && workspace_ctx.is_workspace() {
        // Workspace-wide analysis
        check_workspace(&workspace_ctx, verbose)
    } else {
        // Single manifest analysis
        check_single_manifest(&manifest, &workspace_ctx, verbose)
    }
}

fn check_single_manifest(
    manifest: &Manifest,
    workspace_ctx: &WorkspaceContext,
    verbose: bool,
) -> Result<()> {
    print_manifest_header(manifest, workspace_ctx);

    // Check dependencies
    let checker = DependencyChecker::new()?;
    let dependencies = checker.check_dependencies(manifest)?;

    if dependencies.is_empty() {
        output::print_warning("No dependencies found in Cargo.toml");
        return Ok(());
    }

    print_dependency_summary(&dependencies, verbose);

    if dependencies.iter().all(|d| !d.has_update()) {
        output::print_success("All dependencies are up to date! 🎉");
    } else {
        println!(
            "{}",
            "Run `cargo sane update` to update dependencies interactively.".dimmed()
        );
    }

    Ok(())
}

fn check_workspace(workspace_ctx: &WorkspaceContext, verbose: bool) -> Result<()> {
    let workspace_root = workspace_ctx
        .root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No workspace found"))?;

    output::print_info(&format!(
        "Workspace: {}",
        workspace_root.root_dir().display()
    ));

    // Get all member manifests
    let members = workspace_root.get_member_manifests()?;
    output::print_info(&format!("Members: {}", members.len()));

    // List member names if verbose
    if verbose {
        for manifest in &members {
            if let Some(name) = manifest.package_name() {
                println!("  • {}", name);
            }
        }
    }
    println!();

    // Aggregate dependencies across all members
    let checker = DependencyChecker::new()?;
    let aggregated = aggregate_workspace_deps(&members, &checker, workspace_ctx)?;

    if aggregated.is_empty() {
        output::print_warning("No dependencies found in workspace");
        return Ok(());
    }

    print_workspace_dependency_summary(&aggregated, verbose);

    if aggregated.values().all(|a| !a.dep.has_update()) {
        output::print_success("All workspace dependencies are up to date! 🎉");
    } else {
        println!(
            "{}",
            "Run `cargo sane update --workspace` to update dependencies interactively.".dimmed()
        );
    }

    Ok(())
}

/// Aggregate dependencies across all workspace members (parallel processing)
fn aggregate_workspace_deps(
    members: &[Manifest],
    checker: &DependencyChecker,
    workspace_ctx: &WorkspaceContext,
) -> Result<HashMap<String, AggregatedDep>> {
    // Use a Mutex-wrapped HashMap for thread-safe aggregation
    let aggregated: Mutex<HashMap<String, AggregatedDep>> = Mutex::new(HashMap::new());

    // Create MultiProgress for coordinated progress bar display
    let multi_progress = MultiProgress::new();

    // Process workspace members in parallel with coordinated progress bars
    let errors: Vec<_> = members
        .par_iter()
        .filter_map(|manifest| {
            let package_name = manifest.package_name().unwrap_or("unknown").to_string();

            // Check dependencies with MultiProgress support
            let deps = match checker.check_dependencies_with_progress(
                manifest,
                workspace_ctx,
                Some(&multi_progress),
                Some(&package_name),
            ) {
                Ok(d) => d,
                Err(e) => return Some(e),
            };

            // Aggregate results - lock only during HashMap updates
            let mut agg = aggregated.lock().unwrap();
            for dep in deps {
                let key = dep.name.clone();
                let is_workspace_inherited = dep.is_workspace_inherited;
                let manifest_path = manifest.path.clone();

                agg.entry(key)
                    .and_modify(|entry| {
                        entry.used_by.push(package_name.clone());
                        // Track member manifests for non-workspace deps
                        if !is_workspace_inherited {
                            entry.member_manifests.push(MemberDepInfo {
                                manifest_path: manifest_path.clone(),
                            });
                        }
                        // Keep the one with update info if available
                        if dep.latest_version.is_some() && entry.dep.latest_version.is_none() {
                            entry.dep = dep.clone();
                        }
                    })
                    .or_insert_with(|| {
                        let member_manifests = if is_workspace_inherited {
                            vec![]
                        } else {
                            vec![MemberDepInfo {
                                manifest_path: manifest_path.clone(),
                            }]
                        };
                        AggregatedDep {
                            dep,
                            used_by: vec![package_name.clone()],
                            member_manifests,
                        }
                    });
            }

            None
        })
        .collect();

    // Clear all progress bars
    multi_progress.clear().ok();

    // Return first error if any occurred
    if let Some(err) = errors.into_iter().next() {
        return Err(err);
    }

    Ok(aggregated.into_inner().unwrap())
}

fn print_dependency_summary(dependencies: &[Dependency], verbose: bool) {
    let mut up_to_date = Vec::new();
    let mut patch_updates = Vec::new();
    let mut minor_updates = Vec::new();
    let mut major_updates = Vec::new();

    for dep in dependencies {
        match dep.update_type() {
            UpdateType::UpToDate => up_to_date.push(dep),
            UpdateType::Patch => patch_updates.push(dep),
            UpdateType::Minor => minor_updates.push(dep),
            UpdateType::Major => major_updates.push(dep),
        }
    }

    // Print summary
    println!("📊 Update Summary:");
    println!("  {} Up to date: {}", "✅".green(), up_to_date.len());
    println!(
        "  {} Patch updates available: {}",
        "🟢".green(),
        patch_updates.len()
    );
    println!(
        "  {} Minor updates available: {}",
        "🟡".yellow(),
        minor_updates.len()
    );
    println!(
        "  {} Major updates available: {}",
        "🔴".red(),
        major_updates.len()
    );
    println!();

    print_update_list("🟢 Patch updates:", &patch_updates, verbose, |s| s.green());
    print_update_list("🟡 Minor updates:", &minor_updates, verbose, |s| s.yellow());
    print_update_list("🔴 Major updates:", &major_updates, verbose, |s| s.red());

    // Show up to date if verbose
    if verbose && !up_to_date.is_empty() {
        println!("{}", "✅ Up to date:".green().bold());
        for dep in up_to_date {
            println!(
                "  • {} {}",
                dep.name,
                dep.current_version.to_string().green()
            );
        }
        println!();
    }
}

fn print_workspace_dependency_summary(
    aggregated: &HashMap<String, AggregatedDep>,
    verbose: bool,
) {
    let mut up_to_date = Vec::new();
    let mut patch_updates = Vec::new();
    let mut minor_updates = Vec::new();
    let mut major_updates = Vec::new();

    for agg in aggregated.values() {
        match agg.dep.update_type() {
            UpdateType::UpToDate => up_to_date.push(agg),
            UpdateType::Patch => patch_updates.push(agg),
            UpdateType::Minor => minor_updates.push(agg),
            UpdateType::Major => major_updates.push(agg),
        }
    }

    // Print summary
    println!("📊 Workspace Update Summary:");
    println!(
        "  {} Total unique dependencies: {}",
        "📦".cyan(),
        aggregated.len()
    );
    println!("  {} Up to date: {}", "✅".green(), up_to_date.len());
    println!(
        "  {} Patch updates available: {}",
        "🟢".green(),
        patch_updates.len()
    );
    println!(
        "  {} Minor updates available: {}",
        "🟡".yellow(),
        minor_updates.len()
    );
    println!(
        "  {} Major updates available: {}",
        "🔴".red(),
        major_updates.len()
    );
    println!();

    print_workspace_update_list("🟢 Patch updates:", &patch_updates, verbose, |s| s.green());
    print_workspace_update_list("🟡 Minor updates:", &minor_updates, verbose, |s| s.yellow());
    print_workspace_update_list("🔴 Major updates:", &major_updates, verbose, |s| s.red());

    // Show up to date if verbose
    if verbose && !up_to_date.is_empty() {
        println!("{}", "✅ Up to date:".green().bold());
        for agg in up_to_date {
            println!(
                "  • {} {} (used by {})",
                agg.dep.name,
                agg.dep.current_version.to_string().green(),
                agg.used_by.len()
            );
        }
        println!();
    }
}

fn print_update_list<F>(header: &str, deps: &[&Dependency], verbose: bool, colorize: F)
where
    F: Fn(String) -> colored::ColoredString,
{
    if deps.is_empty() {
        return;
    }

    println!("{}", header.bold());
    for dep in deps {
        if let Some(latest) = &dep.latest_version {
            println!(
                "  • {}{} {} → {}",
                dep.name.bold(),
                workspace_marker(dep.is_workspace_inherited),
                dep.current_version.to_string().dimmed(),
                colorize(latest.to_string())
            );
            if verbose {
                let update_type = match dep.update_type() {
                    UpdateType::Patch => "patch update - likely safe",
                    UpdateType::Minor => "minor update - should be backwards compatible",
                    UpdateType::Major => "major update - may contain breaking changes",
                    UpdateType::UpToDate => "up to date",
                };
                println!("    ({})", update_type);
            }
        }
    }
    println!();
}

fn print_workspace_update_list<F>(
    header: &str,
    deps: &[&AggregatedDep],
    verbose: bool,
    colorize: F,
) where
    F: Fn(String) -> colored::ColoredString,
{
    if deps.is_empty() {
        return;
    }

    println!("{}", header.bold());
    for agg in deps {
        let dep = &agg.dep;
        if let Some(latest) = &dep.latest_version {
            let usage = format!(" (used by {} crates)", agg.used_by.len());
            println!(
                "  • {}{}{} {} → {}",
                dep.name.bold(),
                workspace_marker(dep.is_workspace_inherited),
                usage.dimmed(),
                dep.current_version.to_string().dimmed(),
                colorize(latest.to_string())
            );
            if verbose {
                println!("    Used by: {}", agg.used_by.join(", "));
            }
        }
    }
    println!();
}

pub fn update_command(
    manifest_path: Option<String>,
    dry_run: bool,
    all: bool,
    workspace: bool,
    no_backup: bool,
) -> Result<()> {
    output::print_header("🧠 cargo-sane update");
    println!();

    // Load Cargo.toml
    let manifest = Manifest::find(manifest_path)?;

    // Check for workspace context
    let workspace_ctx = WorkspaceContext::new(&manifest.path)?;

    if workspace && workspace_ctx.is_workspace() {
        // Workspace-wide update
        update_workspace(&workspace_ctx, dry_run, all, no_backup)
    } else {
        // Single manifest update
        update_single_manifest(&manifest, &workspace_ctx, dry_run, all, no_backup)
    }
}

fn update_single_manifest(
    manifest: &Manifest,
    workspace_ctx: &WorkspaceContext,
    dry_run: bool,
    all: bool,
    no_backup: bool,
) -> Result<()> {
    print_manifest_header(manifest, workspace_ctx);

    // Check dependencies
    let checker = DependencyChecker::new()?;
    let dependencies = checker.check_dependencies(manifest)?;

    // Filter only dependencies with updates
    let updatable: Vec<&Dependency> = dependencies.iter().filter(|d| d.has_update()).collect();

    if updatable.is_empty() {
        output::print_success("All dependencies are up to date! 🎉");
        return Ok(());
    }

    println!(
        "Found {} dependencies with updates available.\n",
        updatable.len()
    );

    // Select which dependencies to update
    let to_update = if all {
        updatable
    } else {
        select_dependencies_to_update(&updatable)?
    };

    if to_update.is_empty() {
        output::print_info("No dependencies selected for update.");
        return Ok(());
    }

    // Show what will be updated
    println!("\n{}", "📝 Updates to apply:".bold());
    let has_workspace_deps = to_update.iter().any(|d| d.is_workspace_inherited);
    for dep in &to_update {
        if let Some(latest) = &dep.latest_version {
            let update_type = match dep.update_type() {
                UpdateType::Patch => "🟢 PATCH",
                UpdateType::Minor => "🟡 MINOR",
                UpdateType::Major => "🔴 MAJOR",
                UpdateType::UpToDate => "✅ UP-TO-DATE",
            };
            println!(
                "  {} {}{} {} → {}",
                update_type,
                dep.name.bold(),
                workspace_marker(dep.is_workspace_inherited),
                dep.current_version.to_string().dimmed(),
                latest.to_string().cyan()
            );
        }
    }
    if has_workspace_deps {
        println!(
            "\n{}",
            "Note: [workspace] dependencies will be updated in the workspace root Cargo.toml"
                .dimmed()
        );
    }
    println!();

    // Confirm unless --all flag is used
    if !all && !dry_run {
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Apply these updates?")
            .default(true)
            .interact()?;

        if !confirm {
            output::print_info("Update cancelled.");
            return Ok(());
        }
    }

    if dry_run {
        output::print_info("Dry-run mode: No changes will be made.");
        return Ok(());
    }

    // Create updater
    let mut updater = DependencyUpdater::new(manifest.clone())?;

    // Apply updates
    println!("\n{}", "🔄 Applying updates...".bold());
    for dep in to_update {
        if let Some(latest) = &dep.latest_version {
            match updater.update_dependency(dep, &latest.to_string()) {
                Ok(_) => {
                    println!(
                        "  ✓ Updated {} to {}",
                        dep.name.green(),
                        latest.to_string().cyan()
                    );
                }
                Err(e) => {
                    eprintln!("  ✗ Failed to update {}: {}", dep.name.red(), e);
                }
            }
        }
    }

    // Save changes
    let workspace_modified = updater.workspace_modified();
    let workspace_path = updater.workspace_root_path().cloned();
    updater.save(no_backup)?;
    println!();
    output::print_success("Cargo.toml updated successfully!");
    if !no_backup {
        output::print_info("Backup saved as Cargo.toml.backup");
        if workspace_modified {
            if let Some(path) = workspace_path {
                output::print_info(&format!(
                    "Workspace root backup saved as {}",
                    path.with_extension("toml.backup").display()
                ));
            }
        }
    }
    println!();
    println!(
        "{}",
        "Don't forget to run `cargo check` to verify everything still compiles!".dimmed()
    );

    Ok(())
}

fn update_workspace(
    workspace_ctx: &WorkspaceContext,
    dry_run: bool,
    all: bool,
    no_backup: bool,
) -> Result<()> {
    let workspace_root = workspace_ctx
        .root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No workspace found"))?;

    output::print_info(&format!(
        "Workspace: {}",
        workspace_root.root_dir().display()
    ));

    // Get all member manifests
    let members = workspace_root.get_member_manifests()?;
    output::print_info(&format!("Members: {}", members.len()));
    println!();

    // Aggregate dependencies across all members
    let checker = DependencyChecker::new()?;
    let aggregated = aggregate_workspace_deps(&members, &checker, workspace_ctx)?;

    // Filter only those with updates
    let updatable: Vec<_> = aggregated
        .values()
        .filter(|a| a.dep.has_update())
        .collect();

    if updatable.is_empty() {
        output::print_success("All workspace dependencies are up to date! 🎉");
        return Ok(());
    }

    println!(
        "Found {} unique dependencies with updates available.\n",
        updatable.len()
    );

    // Select which dependencies to update (keep AggregatedDep to access member_manifests)
    let to_update: Vec<&AggregatedDep> = if all {
        updatable.into_iter().collect()
    } else {
        select_aggregated_deps_to_update(&updatable)?
    };

    if to_update.is_empty() {
        output::print_info("No dependencies selected for update.");
        return Ok(());
    }

    // Show what will be updated
    println!("\n{}", "📝 Updates to apply:".bold());
    for agg in &to_update {
        let dep = &agg.dep;
        if let Some(latest) = &dep.latest_version {
            let update_type = match dep.update_type() {
                UpdateType::Patch => "🟢 PATCH",
                UpdateType::Minor => "🟡 MINOR",
                UpdateType::Major => "🔴 MAJOR",
                UpdateType::UpToDate => "✅ UP-TO-DATE",
            };
            println!(
                "  {} {}{} {} → {}",
                update_type,
                dep.name.bold(),
                aggregated_marker(dep, agg.member_manifests.len()),
                dep.current_version.to_string().dimmed(),
                latest.to_string().cyan()
            );
        }
    }
    println!(
        "\n{}",
        "Note: Workspace dependencies will be updated in workspace root. Member-specific deps will be updated in each member."
            .dimmed()
    );
    println!();

    // Confirm unless --all flag is used
    if !all && !dry_run {
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Apply these updates?")
            .default(true)
            .interact()?;

        if !confirm {
            output::print_info("Update cancelled.");
            return Ok(());
        }
    }

    if dry_run {
        output::print_info("Dry-run mode: No changes will be made.");
        return Ok(());
    }

    // For workspace updates, we need to update both workspace root and members
    println!("\n{}", "🔄 Applying updates...".bold());

    // Group by whether workspace-inherited or not
    let (workspace_aggs, member_aggs): (Vec<&&AggregatedDep>, Vec<&&AggregatedDep>) = to_update
        .iter()
        .partition(|a| a.dep.is_workspace_inherited);

    // Update workspace dependencies in root
    if !workspace_aggs.is_empty() {
        let mut root_content =
            std::fs::read_to_string(&workspace_root.path).context("Failed to read workspace root")?;

        for agg in &workspace_aggs {
            let dep = &agg.dep;
            if let Some(latest) = &dep.latest_version {
                match DependencyUpdater::update_version_in_content(
                    &root_content,
                    &dep.name,
                    &latest.to_string(),
                ) {
                    Ok(updated) => {
                        root_content = updated;
                        println!(
                            "  ✓ Updated {} to {} [workspace root]",
                            dep.name.green(),
                            latest.to_string().cyan()
                        );
                    }
                    Err(e) => {
                        eprintln!("  ✗ Failed to update {}: {}", dep.name.red(), e);
                    }
                }
            }
        }

        // Save the workspace root changes
        if !no_backup {
            let backup_path = workspace_root.path.with_extension("toml.backup");
            std::fs::copy(&workspace_root.path, &backup_path).context("Failed to create backup")?;
        }
        std::fs::write(&workspace_root.path, &root_content)
            .context("Failed to write workspace root")?;
    }

    // Update member-specific dependencies
    if !member_aggs.is_empty() {
        // Group updates by manifest path
        let mut updates_by_manifest: HashMap<PathBuf, Vec<(&str, String)>> = HashMap::new();

        for agg in &member_aggs {
            let dep = &agg.dep;
            if let Some(latest) = &dep.latest_version {
                for member_info in &agg.member_manifests {
                    updates_by_manifest
                        .entry(member_info.manifest_path.clone())
                        .or_default()
                        .push((&dep.name, latest.to_string()));
                }
            }
        }

        // Apply updates to each member manifest
        for (manifest_path, updates) in updates_by_manifest {
            let mut content =
                std::fs::read_to_string(&manifest_path).context("Failed to read member manifest")?;
            let member_name = manifest_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            for (dep_name, new_version) in &updates {
                match DependencyUpdater::update_version_in_content(&content, dep_name, new_version)
                {
                    Ok(updated) => {
                        content = updated;
                        println!(
                            "  ✓ Updated {} to {} [{}]",
                            dep_name.green(),
                            new_version.cyan(),
                            member_name.dimmed()
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "  ✗ Failed to update {} in {}: {}",
                            dep_name.red(),
                            member_name,
                            e
                        );
                    }
                }
            }

            // Save the member manifest changes
            if !no_backup {
                let backup_path = manifest_path.with_extension("toml.backup");
                std::fs::copy(&manifest_path, &backup_path).context("Failed to create backup")?;
            }
            std::fs::write(&manifest_path, &content).context("Failed to write member manifest")?;
        }
    }

    println!();
    output::print_success("Workspace update complete!");
    println!(
        "{}",
        "Don't forget to run `cargo check` to verify everything still compiles!".dimmed()
    );

    Ok(())
}

/// Interactive selection of dependencies to update
fn select_dependencies_to_update<'a>(deps: &[&'a Dependency]) -> Result<Vec<&'a Dependency>> {
    let items: Vec<String> = deps
        .iter()
        .map(|d| {
            let update_type = match d.update_type() {
                UpdateType::Patch => "🟢",
                UpdateType::Minor => "🟡",
                UpdateType::Major => "🔴",
                UpdateType::UpToDate => "✅",
            };
            let ws_marker = if d.is_workspace_inherited {
                " [ws]"
            } else {
                ""
            };
            format!(
                "{} {}{} {} → {}",
                update_type,
                d.name,
                ws_marker,
                d.current_version,
                d.latest_version.as_ref().unwrap()
            )
        })
        .collect();

    let selections = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select dependencies to update (Space to select, Enter to confirm)")
        .items(&items)
        .interact()?;

    let selected: Vec<&Dependency> = selections.iter().map(|&i| deps[i]).collect();
    Ok(selected)
}

/// Interactive selection of aggregated dependencies to update (for workspace mode)
fn select_aggregated_deps_to_update<'a>(
    aggs: &[&'a AggregatedDep],
) -> Result<Vec<&'a AggregatedDep>> {
    let items: Vec<String> = aggs
        .iter()
        .map(|a| {
            let d = &a.dep;
            let update_type = match d.update_type() {
                UpdateType::Patch => "🟢",
                UpdateType::Minor => "🟡",
                UpdateType::Major => "🔴",
                UpdateType::UpToDate => "✅",
            };
            let ws_marker = if d.is_workspace_inherited {
                " [ws]".to_string()
            } else {
                format!(" [{}m]", a.member_manifests.len())
            };
            format!(
                "{} {}{} {} → {}",
                update_type,
                d.name,
                ws_marker,
                d.current_version,
                d.latest_version.as_ref().unwrap()
            )
        })
        .collect();

    let selections = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select dependencies to update (Space to select, Enter to confirm)")
        .items(&items)
        .interact()?;

    let selected: Vec<&AggregatedDep> = selections.iter().map(|&i| aggs[i]).collect();
    Ok(selected)
}

pub fn fix_command(manifest_path: Option<String>, auto: bool) -> Result<()> {
    let _ = (manifest_path, auto);
    output::print_warning("Fix command not yet implemented");
    Ok(())
}

pub fn clean_command(manifest_path: Option<String>, dry_run: bool) -> Result<()> {
    let _ = (manifest_path, dry_run);
    output::print_warning("Clean command not yet implemented");
    Ok(())
}

pub fn health_command(manifest_path: Option<String>, json: bool) -> Result<()> {
    let _ = (manifest_path, json);
    output::print_warning("Health command not yet implemented");
    Ok(())
}
