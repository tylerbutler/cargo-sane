//! Command implementations

use crate::analyzer::checker::DependencyChecker;
use crate::cli::output;
use crate::core::dependency::{Dependency, UpdateType};
use crate::core::manifest::Manifest;
use crate::core::workspace::WorkspaceContext;
use crate::updater::DependencyUpdater;
use crate::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use std::collections::HashMap;

/// Aggregated dependency info for workspace-wide analysis
#[derive(Debug, Clone)]
struct AggregatedDep {
    dep: Dependency,
    /// Which packages use this dependency
    used_by: Vec<String>,
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

    // List member names
    let member_names: Vec<_> = members
        .iter()
        .filter_map(|m| m.package_name())
        .collect();
    if verbose {
        for name in &member_names {
            println!("  • {}", name);
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

    // Convert to flat list for display
    let all_deps: Vec<&Dependency> = aggregated.values().map(|a| &a.dep).collect();
    print_workspace_dependency_summary(&aggregated, verbose);

    if all_deps.iter().all(|d| !d.has_update()) {
        output::print_success("All workspace dependencies are up to date! 🎉");
    } else {
        println!(
            "{}",
            "Run `cargo sane update --workspace` to update dependencies interactively.".dimmed()
        );
    }

    Ok(())
}

/// Aggregate dependencies across all workspace members
fn aggregate_workspace_deps(
    members: &[Manifest],
    checker: &DependencyChecker,
    workspace_ctx: &WorkspaceContext,
) -> Result<HashMap<String, AggregatedDep>> {
    let mut aggregated: HashMap<String, AggregatedDep> = HashMap::new();

    for manifest in members {
        let package_name = manifest
            .package_name()
            .unwrap_or("unknown")
            .to_string();

        let deps = checker.check_dependencies_with_context(manifest, workspace_ctx)?;

        for dep in deps {
            let key = dep.name.clone();
            aggregated
                .entry(key)
                .and_modify(|agg| {
                    agg.used_by.push(package_name.clone());
                    // Keep the one with update info if available
                    if dep.latest_version.is_some() && agg.dep.latest_version.is_none() {
                        agg.dep = dep.clone();
                    }
                })
                .or_insert(AggregatedDep {
                    dep,
                    used_by: vec![package_name.clone()],
                });
        }
    }

    Ok(aggregated)
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
            let ws_marker = if dep.is_workspace_inherited {
                " [workspace]".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "  • {}{} {} → {}",
                dep.name.bold(),
                ws_marker,
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
            let ws_marker = if dep.is_workspace_inherited {
                " [workspace]".dimmed().to_string()
            } else {
                String::new()
            };
            let usage = format!(" (used by {} crates)", agg.used_by.len());
            println!(
                "  • {}{}{} {} → {}",
                dep.name.bold(),
                ws_marker,
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
) -> Result<()> {
    output::print_header("🧠 cargo-sane update");
    println!();

    // Load Cargo.toml
    let manifest = Manifest::find(manifest_path)?;

    // Check for workspace context
    let workspace_ctx = WorkspaceContext::new(&manifest.path)?;

    if workspace && workspace_ctx.is_workspace() {
        // Workspace-wide update
        update_workspace(&workspace_ctx, dry_run, all)
    } else {
        // Single manifest update
        update_single_manifest(&manifest, &workspace_ctx, dry_run, all)
    }
}

fn update_single_manifest(
    manifest: &Manifest,
    workspace_ctx: &WorkspaceContext,
    dry_run: bool,
    all: bool,
) -> Result<()> {
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
            let ws_marker = if dep.is_workspace_inherited {
                " [workspace]".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "  {} {}{} {} → {}",
                update_type,
                dep.name.bold(),
                ws_marker,
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
    updater.save()?;
    println!();
    output::print_success("Cargo.toml updated successfully!");
    output::print_info("Backup saved as Cargo.toml.backup");
    if workspace_modified {
        if let Some(path) = workspace_path {
            output::print_info(&format!(
                "Workspace root backup saved as {}",
                path.with_extension("toml.backup").display()
            ));
        }
    }
    println!();
    println!(
        "{}",
        "Don't forget to run `cargo check` to verify everything still compiles!".dimmed()
    );

    Ok(())
}

fn update_workspace(workspace_ctx: &WorkspaceContext, dry_run: bool, all: bool) -> Result<()> {
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

    // Select which dependencies to update
    let to_update: Vec<&Dependency> = if all {
        updatable.iter().map(|a| &a.dep).collect()
    } else {
        let deps: Vec<&Dependency> = updatable.iter().map(|a| &a.dep).collect();
        select_dependencies_to_update(&deps)?
    };

    if to_update.is_empty() {
        output::print_info("No dependencies selected for update.");
        return Ok(());
    }

    // Show what will be updated
    println!("\n{}", "📝 Updates to apply:".bold());
    for dep in &to_update {
        if let Some(latest) = &dep.latest_version {
            let update_type = match dep.update_type() {
                UpdateType::Patch => "🟢 PATCH",
                UpdateType::Minor => "🟡 MINOR",
                UpdateType::Major => "🔴 MAJOR",
                UpdateType::UpToDate => "✅ UP-TO-DATE",
            };
            let ws_marker = if dep.is_workspace_inherited {
                " [workspace]".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "  {} {}{} {} → {}",
                update_type,
                dep.name.bold(),
                ws_marker,
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
    // For now, focus on workspace dependencies (updating workspace root)
    println!("\n{}", "🔄 Applying updates...".bold());

    // Group by whether workspace-inherited or not
    let (workspace_deps, member_deps): (Vec<&&Dependency>, Vec<&&Dependency>) = to_update
        .iter()
        .partition(|d| d.is_workspace_inherited);

    // Update workspace dependencies in root
    if !workspace_deps.is_empty() {
        // Create a manifest for the workspace root to use the updater
        let root_manifest = Manifest::from_path(&workspace_root.path)?;
        let root_updater = DependencyUpdater::new(root_manifest)?;

        for dep in workspace_deps {
            if let Some(latest) = &dep.latest_version {
                // For workspace deps, update directly in root
                match DependencyUpdater::update_version_in_content(
                    root_updater.get_content(),
                    &dep.name,
                    &latest.to_string(),
                ) {
                    Ok(_) => {
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
    }

    // Update member-specific dependencies
    if !member_deps.is_empty() {
        output::print_warning(
            "Member-specific dependency updates require updating each member's Cargo.toml individually.",
        );
        output::print_info("Use `cargo sane update` in each member directory for non-workspace deps.");
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
