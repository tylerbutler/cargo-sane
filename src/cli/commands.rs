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

pub fn check_command(manifest_path: Option<String>, verbose: bool) -> Result<()> {
    output::print_header("🧠 cargo-sane check");
    println!();

    // Load Cargo.toml
    let manifest = Manifest::find(manifest_path)?;

    if let Some(name) = manifest.package_name() {
        output::print_info(&format!("Package: {}", name));
    }
    output::print_info(&format!("Manifest: {}", manifest.path.display()));

    // Check for workspace context
    let workspace_ctx = WorkspaceContext::new(&manifest.path)?;
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
    let dependencies = checker.check_dependencies(&manifest)?;

    if dependencies.is_empty() {
        output::print_warning("No dependencies found in Cargo.toml");
        return Ok(());
    }

    // Categorize dependencies
    let mut up_to_date = Vec::new();
    let mut patch_updates = Vec::new();
    let mut minor_updates = Vec::new();
    let mut major_updates = Vec::new();

    for dep in &dependencies {
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

    // Show patch updates
    if !patch_updates.is_empty() {
        println!("{}", "🟢 Patch updates:".green().bold());
        for dep in &patch_updates {
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
                    latest.to_string().green()
                );
                if verbose {
                    println!("    (patch update - likely safe)");
                }
            }
        }
        println!();
    }

    // Show minor updates
    if !minor_updates.is_empty() {
        println!("{}", "🟡 Minor updates:".yellow().bold());
        for dep in &minor_updates {
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
                    latest.to_string().yellow()
                );
                if verbose {
                    println!("    (minor update - should be backwards compatible)");
                }
            }
        }
        println!();
    }

    // Show major updates
    if !major_updates.is_empty() {
        println!("{}", "🔴 Major updates:".red().bold());
        for dep in &major_updates {
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
                    latest.to_string().red()
                );
                if verbose {
                    println!("    (major update - may contain breaking changes)");
                }
            }
        }
        println!();
    }

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

    if patch_updates.is_empty() && minor_updates.is_empty() && major_updates.is_empty() {
        output::print_success("All dependencies are up to date! 🎉");
    } else {
        println!(
            "{}",
            "Run `cargo sane update` to update dependencies interactively.".dimmed()
        );
    }

    Ok(())
}

pub fn update_command(manifest_path: Option<String>, dry_run: bool, all: bool) -> Result<()> {
    output::print_header("🧠 cargo-sane update");
    println!();

    // Load Cargo.toml
    let manifest = Manifest::find(manifest_path)?;

    if let Some(name) = manifest.package_name() {
        output::print_info(&format!("Package: {}", name));
    }
    output::print_info(&format!("Manifest: {}", manifest.path.display()));

    // Check for workspace context
    let workspace_ctx = WorkspaceContext::new(&manifest.path)?;
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
    let dependencies = checker.check_dependencies(&manifest)?;

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
    let mut updater = DependencyUpdater::new(manifest)?;

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
