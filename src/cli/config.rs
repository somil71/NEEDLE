//! `needle config [view|edit]` — view or edit configuration.

use needle::storage::Storage;
use needle::Result;
use colored::Colorize;

#[derive(clap::Subcommand)]
pub enum ConfigAction {
    /// View current configuration
    View,
    /// Print the path to the config file for editing
    Edit,
}

pub async fn run(action: Option<ConfigAction>) -> Result<()> {
    let config_path = Storage::config_path();

    match action {
        Some(ConfigAction::View) | None => {
            println!("{}", "Needle configuration\n".bold());

            match Storage::load_config() {
                Ok(config) => {
                    println!("  File: {}", config_path.display().to_string().dimmed());
                    println!();
                    println!("  Watched directories:");
                    if config.watched_dirs.is_empty() {
                        println!("    (none — run: needle init <dirs...>)");
                    } else {
                        for dir in &config.watched_dirs {
                            println!("    {}", dir.cyan());
                        }
                    }
                    println!();
                    println!("  BM25:  k1={}  b={}", config.bm25_k1, config.bm25_b);
                    println!(
                        "  HNSW:  M={}  efConstruction={}  efSearch={}",
                        config.hnsw_m, config.hnsw_ef_construction, config.hnsw_ef_search
                    );
                    println!("  Embedding dim: {}", config.embedding_dim);
                    println!("  Default limit: {}", config.default_limit);
                    println!();
                    println!("  Ignore patterns:");
                    for pat in &config.ignore_patterns {
                        println!("    {}", pat.dimmed());
                    }
                }
                Err(_) => {
                    println!("  {}", "No configuration found.".yellow());
                    println!("  Run: needle init <dirs...> to create it.");
                }
            }
        }

        Some(ConfigAction::Edit) => {
            println!("  Config file: {}", config_path.display().to_string().cyan());
            println!();
            if !config_path.exists() {
                println!(
                    "  {}",
                    "File does not exist yet. Run: needle init <dirs...>".yellow()
                );
            } else {
                println!("  Open this file in your editor to modify settings.");
                println!("  After editing, run: needle reindex");
            }
        }
    }

    Ok(())
}
