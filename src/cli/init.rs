//! `needle init <dirs...>` — scan directories, build indexes, save to disk.

use needle::chunking::detect_chunker;
use needle::config::Config;
use needle::embedding::{EmbeddingModel, OLLAMA_DEFAULT_URL, OLLAMA_EMBED_MODEL};
use needle::graph;
use needle::indexing::Index;
use needle::schema::{IndexMetadata, Language};
use needle::storage::{human_size, Storage};
use needle::Result;
use chrono::Utc;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub async fn run(directories: Vec<String>) -> Result<()> {
    println!("{}", "Needle v0.1.0 — initializing index".bold());
    println!();

    // 1. Validate directories
    let mut valid_dirs: Vec<PathBuf> = Vec::new();
    for dir in &directories {
        let p = PathBuf::from(dir);
        if !p.exists() {
            eprintln!(
                "{}: {} does not exist\n  Check the path and try: needle init <dir>",
                "Error".red().bold(),
                dir
            );
            return Err(needle::Error::InvalidPath(dir.clone()));
        }
        let canonical = clean_path(p.canonicalize()?);
        valid_dirs.push(canonical);
    }

    // 2. Build config
    let config = Config {
        watched_dirs: valid_dirs.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        ..Config::default()
    };

    // 3. Set up storage
    let index_dir = Storage::default_index_dir();
    let storage = Storage::new(index_dir)?;
    Storage::save_config(&config)?;

    // 4. Walk and collect files
    println!("Scanning directories...");
    let mut all_files: Vec<(PathBuf, Language)> = Vec::new();

    for (i, dir) in valid_dirs.iter().enumerate() {
        let prefix = if i + 1 == valid_dirs.len() { "└──" } else { "├──" };
        let files = collect_files(dir, &config);
        println!(
            "  {}  {}    {} files",
            prefix,
            dir.display(),
            files.len().to_string().cyan()
        );
        all_files.extend(files);
    }

    if all_files.is_empty() {
        println!("\n{} No supported files found.", "Warning:".yellow());
        return Ok(());
    }

    println!();

    // 5. Chunking phase (parallel via rayon)
    let chunk_bar = ProgressBar::new(all_files.len() as u64);
    chunk_bar.set_style(
        ProgressStyle::default_bar()
            .template("  Chunking  {bar:32.cyan/dim}  {pos}/{len} files  [{elapsed}]")
            .unwrap()
            .progress_chars("█▓░"),
    );

    // Collect (file_path, language, content) entries
    let file_entries: Vec<(PathBuf, Language, String)> = all_files
        .iter()
        .filter_map(|(path, lang)| {
            std::fs::read_to_string(path)
                .ok()
                .map(|content| (path.clone(), *lang, content))
        })
        .collect();

    // Parallel chunking
    let raw_chunks: Vec<needle::schema::Chunk> = file_entries
        .par_iter()
        .flat_map(|(path, lang, content)| {
            chunk_bar.inc(1);
            let chunker = detect_chunker(*lang);
            chunker.chunk(content, path, *lang).unwrap_or_default()
        })
        .collect();

    chunk_bar.finish_with_message("done");

    let total_chunks = raw_chunks.len();
    let total_files = file_entries.len();

    // 6. Assign chunk IDs and build indexes
    // Auto-detect Ollama for real semantic embeddings; fall back to hash-projection.
    print!("  Checking for Ollama ({})... ", OLLAMA_EMBED_MODEL);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let embed_model = match EmbeddingModel::try_ollama(OLLAMA_DEFAULT_URL, OLLAMA_EMBED_MODEL) {
        Some(m) => {
            println!("{}", format!("found ({}d real embeddings)", m.dim()).green());
            m
        }
        None => {
            println!("{}", "not found — using hash-projection".yellow());
            println!(
                "  {} For better semantic search: ollama pull {}\n",
                "tip:".dimmed(), OLLAMA_EMBED_MODEL
            );
            EmbeddingModel::new(config.embedding_dim)?
        }
    };

    let embed_bar = ProgressBar::new(total_chunks as u64);
    embed_bar.set_style(
        ProgressStyle::default_bar()
            .template("  Embedding {bar:32.magenta/dim}  {pos}/{len} chunks  [{elapsed}]")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut index = Index::with_params(
        config.embedding_dim,
        config.hnsw_m as usize,
        config.hnsw_ef_construction as usize,
        config.bm25_k1,
        config.bm25_b,
    );

    let mut filemap: HashMap<String, Vec<u64>> = HashMap::new();

    for mut chunk in raw_chunks {
        let chunk_id = index.next_id();
        chunk.id = chunk_id;
        chunk.embedding_id = chunk_id;

        // block_in_place lets tokio know we may block (Ollama HTTP call)
        let embedding = tokio::task::block_in_place(|| embed_model.embed(&chunk.content));

        filemap
            .entry(chunk.file_path.clone())
            .or_default()
            .push(chunk_id);

        index.add_chunk(chunk, embedding)?;
        embed_bar.inc(1);
    }

    embed_bar.finish_with_message("done");

    // 7. Save everything to disk
    print!("  Building inverted index... ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let bm25_start = std::time::Instant::now();
    storage.save_bm25(&index.inverted)?;
    println!("done [{:.1}s]", bm25_start.elapsed().as_secs_f64());

    print!("  Building HNSW graph...      ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let hnsw_start = std::time::Instant::now();
    storage.save_hnsw(&index.hnsw)?;
    println!("done [{:.1}s]", hnsw_start.elapsed().as_secs_f64());

    storage.save_chunks(&index.chunk_store)?;
    storage.save_filemap(&filemap)?;

    print!("  Building knowledge graph...  ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let graph_start = std::time::Instant::now();
    let code_graph = graph::extract(&file_entries);
    storage.save_graph(&code_graph)?;
    println!(
        "done [{:.1}s]  {} nodes  {} edges",
        graph_start.elapsed().as_secs_f64(),
        code_graph.stats.total_nodes,
        code_graph.stats.total_edges,
    );

    let meta = IndexMetadata {
        total_chunks: total_chunks as u64,
        total_files: total_files as u64,
        last_update_ts: Utc::now().timestamp() as u64,
        embedding_model: embed_model.model_string(),
        embedding_dim: config.embedding_dim as u32,
        hnsw_m: config.hnsw_m,
        hnsw_ef_construction: config.hnsw_ef_construction,
        bm25_k1: config.bm25_k1,
        bm25_b: config.bm25_b,
        watched_dirs: config.watched_dirs.clone(),
        avg_chunk_length: index.inverted.avg_doc_length(),
        ..IndexMetadata::default()
    };
    storage.save_metadata(&meta)?;

    // 8. Summary
    let disk_bytes = storage.index_size_bytes();
    let vocab = index.inverted.vocabulary_size();

    println!();
    println!("{}", "✓ Index ready".green().bold());
    println!(
        "  {}  ·  {}  ·  {} on disk",
        format!("{} chunks", total_chunks).cyan(),
        format!("{} files", total_files).cyan(),
        human_size(disk_bytes).cyan(),
    );
    println!(
        "  {} terms in vocabulary  ·  HNSW M={} efC={}",
        vocab,
        config.hnsw_m,
        config.hnsw_ef_construction,
    );
    println!(
        "  Model: hash-projection-{}",
        config.embedding_dim
    );
    println!("  Stored at: {}", Storage::default_index_dir().display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Directory walker
// ---------------------------------------------------------------------------

fn collect_files(dir: &Path, config: &Config) -> Vec<(PathBuf, Language)> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden dirs and configured ignore patterns
            !name.starts_with('.')
                && !config.should_ignore(&name)
                && !config.should_ignore(&e.path().to_string_lossy())
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = clean_path(e.path().to_path_buf());
            let ext = path.extension()?.to_str()?;
            let lang = Language::from_extension(ext)?;
            // Skip very large files (>1MB)
            let meta = std::fs::metadata(&path).ok()?;
            if meta.len() > 1_000_000 {
                return None;
            }
            Some((path, lang))
        })
        .collect()
}

/// Strip Windows extended-length path prefix `\\?\` so paths work everywhere.
fn clean_path(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}
