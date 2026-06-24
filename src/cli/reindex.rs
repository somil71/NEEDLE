//! `needle reindex` — force full rebuild of all indexes.

use needle::storage::Storage;
use needle::Result;
use colored::Colorize;

pub async fn run() -> Result<()> {
    println!("{}", "Needle v0.1.0 — reindexing\n".bold());

    let config = match Storage::load_config() {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "{}: No configuration found.\n  Run: needle init <dirs...> first.",
                "Error".red().bold()
            );
            return Err(needle::Error::ConfigError("no config".to_string()));
        }
    };

    if config.watched_dirs.is_empty() {
        eprintln!("{}: No watched directories in config.", "Error".red().bold());
        return Ok(());
    }

    println!(
        "  Rebuilding index for {} directories...",
        config.watched_dirs.len()
    );
    for dir in &config.watched_dirs {
        println!("    {}", dir.cyan());
    }
    println!();

    // Delegate to the init pipeline (full rebuild)
    super::init::run(config.watched_dirs).await
}
