//! Terminal output formatting

use colored::Colorize;

pub fn print_header(text: &str) {
    println!("\n{}", text.bold().cyan());
}

pub fn print_success(text: &str) {
    println!("{} {}", "✓".green().bold(), text);
}

pub fn print_warning(text: &str) {
    println!("{} {}", "⚠".yellow().bold(), text);
}

pub fn print_info(text: &str) {
    println!("{} {}", "ℹ".blue().bold(), text);
}
