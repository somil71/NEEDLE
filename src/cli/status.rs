//! `needle status` — display index health metrics.

use needle::storage::{human_size, Storage};
use needle::Result;
use colored::Colorize;

pub async fn run() -> Result<()> {
    println!("{}", "Needle v0.1.0 — index status\n".bold());

    if !Storage::index_exists() {
        println!("  {}", "No index found. Run: needle init <dirs...>".yellow());
        return Ok(());
    }

    let index_dir = Storage::default_index_dir();
    let storage = Storage::new(index_dir.clone())?;

    let meta = match storage.load_metadata() {
        Ok(m) => m,
        Err(_) => {
            println!("  {}", "Index metadata not found. Run: needle reindex".yellow());
            return Ok(());
        }
    };

    // Watched directories
    println!("  {}:", "Watched directories".bold());
    if meta.watched_dirs.is_empty() {
        println!("    (none configured)");
    } else {
        for dir in &meta.watched_dirs {
            println!("    {}", dir.cyan());
        }
    }

    // Index health
    println!();
    println!("  {}:", "Index health".bold());
    println!(
        "    Chunks:     {} active  ·  {} tombstoned",
        meta.total_chunks.to_string().green(),
        meta.tombstoned_chunks.to_string().dimmed()
    );
    println!(
        "    Files:      {}",
        meta.total_files.to_string().green()
    );

    let total_bytes = storage.index_size_bytes();
    let chunks_bytes = storage.file_size_bytes("chunks.json");
    let bm25_bytes = storage.file_size_bytes("bm25.bin");
    let hnsw_bytes = storage.file_size_bytes("hnsw.bin");

    println!(
        "    Disk:       {} total  (chunks: {}  bm25: {}  hnsw: {})",
        human_size(total_bytes).cyan(),
        human_size(chunks_bytes),
        human_size(bm25_bytes),
        human_size(hnsw_bytes),
    );

    if meta.last_update_ts > 0 {
        let now = chrono::Utc::now().timestamp() as u64;
        let secs = now.saturating_sub(meta.last_update_ts);
        let ago = if secs < 60 {
            format!("{} seconds ago", secs)
        } else if secs < 3600 {
            format!("{} minutes ago", secs / 60)
        } else if secs < 86400 {
            format!("{} hours ago", secs / 3600)
        } else {
            format!("{} days ago", secs / 86400)
        };
        println!("    Last update: {}", ago.dimmed());
    }

    // HNSW stats
    println!();
    println!("  {}:", "HNSW".bold());
    println!(
        "    M={}  efConstruction={}  efSearch={}",
        meta.hnsw_m, meta.hnsw_ef_construction, 50
    );
    println!("    Model: {}", meta.embedding_model.dimmed());
    println!("    Dim: {}", meta.embedding_dim);

    // BM25 stats
    println!();
    println!("  {}:", "BM25".bold());
    println!(
        "    k1={}  b={}",
        meta.bm25_k1, meta.bm25_b
    );
    if meta.avg_chunk_length > 0.0 {
        println!(
            "    avg chunk length: {:.0} tokens",
            meta.avg_chunk_length
        );
    }

    Ok(())
}
