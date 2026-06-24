//! `needle watch` — watch indexed directories and re-index on file changes.

use colored::Colorize;
use needle::chunking::detect_chunker;
use needle::embedding::EmbeddingModel;
use needle::indexing::{bm25::BM25Index, hnsw::HnswIndex};
use needle::schema::{Chunk, Language};
use needle::storage::Storage;
use needle::Result;
use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub async fn run() -> Result<()> {
    println!("{}", "Needle — watch mode\n".bold());

    let config = match Storage::load_config() {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "{}: No index found. Run: needle init <dirs...> first.",
                "Error".red().bold()
            );
            return Ok(());
        }
    };

    if config.watched_dirs.is_empty() {
        eprintln!("{}: No watched directories in config.", "Error".red().bold());
        return Ok(());
    }

    let storage = Storage::new(Storage::default_index_dir())?;

    // Load all index components into memory
    let mut bm25 = storage.load_bm25()?;
    let mut hnsw = storage.load_hnsw()?;
    let mut chunks = storage.load_chunks()?;
    let mut filemap = storage.load_filemap()?;
    let embedding = EmbeddingModel::new(config.embedding_dim)?;
    let mut next_id: u64 = chunks.keys().max().copied().map(|x| x + 1).unwrap_or(0);

    let dirs: Vec<PathBuf> = config.watched_dirs.iter().map(PathBuf::from).collect();

    println!("  Watching {} directories:", dirs.len());
    for dir in &dirs {
        println!("    {}", dir.display().to_string().cyan());
    }
    println!("\n  Press {} to stop\n", "Ctrl+C".dimmed());

    // Set up notify watcher with sync channel
    let (tx, rx) = mpsc::channel();
    let mut watcher = recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| needle::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|e| needle::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    }

    let mut dirty = false;
    let mut last_save = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(event)) => {
                let paths: Vec<PathBuf> = event.paths.clone();

                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in &paths {
                            if let Some(lang) = file_language(path, &config) {
                                if let Err(e) = reindex_file(
                                    path, lang, &mut bm25, &mut hnsw,
                                    &mut chunks, &mut filemap, &embedding, &mut next_id,
                                ) {
                                    eprintln!(
                                        "  {} {}: {}",
                                        "⚠".yellow(),
                                        path.display(),
                                        e
                                    );
                                } else {
                                    let rel = short_path(path);
                                    println!("  {} {}", "↻".cyan(), rel);
                                    dirty = true;
                                }
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in &paths {
                            let path_str = path.to_string_lossy().to_string();
                            if let Some(old_ids) = filemap.remove(&path_str) {
                                for id in old_ids {
                                    chunks.remove(&id);
                                    let _ = bm25.delete_chunk(id);
                                    let _ = hnsw.delete_node(id);
                                }
                                println!("  {} {}", "✗".red(), short_path(path));
                                dirty = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Err(e)) => {
                eprintln!("  {} Watch error: {}", "⚠".yellow(), e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Flush index to disk at most every 2 seconds
        if dirty && last_save.elapsed() >= Duration::from_secs(2) {
            print!("  Saving index... ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            storage.save_bm25(&bm25)?;
            storage.save_hnsw(&hnsw)?;
            storage.save_chunks(&chunks)?;
            storage.save_filemap(&filemap)?;
            println!("{}", "done".green());
            dirty = false;
            last_save = Instant::now();
        }
    }

    // Final flush
    if dirty {
        storage.save_bm25(&bm25)?;
        storage.save_hnsw(&hnsw)?;
        storage.save_chunks(&chunks)?;
        storage.save_filemap(&filemap)?;
    }

    Ok(())
}

fn file_language(path: &Path, config: &needle::config::Config) -> Option<Language> {
    let name = path.file_name()?.to_string_lossy();
    if config.should_ignore(&name) || config.should_ignore(&path.to_string_lossy()) {
        return None;
    }
    // Skip large files
    if std::fs::metadata(path).ok()?.len() > 1_000_000 {
        return None;
    }
    let ext = path.extension()?.to_str()?;
    Language::from_extension(ext)
}

fn reindex_file(
    path: &Path,
    lang: Language,
    bm25: &mut BM25Index,
    hnsw: &mut HnswIndex,
    chunks: &mut HashMap<u64, Chunk>,
    filemap: &mut HashMap<String, Vec<u64>>,
    embedding: &EmbeddingModel,
    next_id: &mut u64,
) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();

    // Remove old chunks for this file
    if let Some(old_ids) = filemap.remove(&path_str) {
        for id in old_ids {
            chunks.remove(&id);
            bm25.delete_chunk(id)?;
            hnsw.delete_node(id)?;
        }
    }

    // Read + chunk + embed + insert
    let content = std::fs::read_to_string(path)?;
    let chunker = detect_chunker(lang);
    let new_chunks = chunker.chunk(&content, path, lang).unwrap_or_default();

    let mut new_ids = Vec::new();
    for mut chunk in new_chunks {
        chunk.id = *next_id;
        chunk.embedding_id = *next_id;
        *next_id += 1;

        let emb = embedding.embed(&chunk.content);
        new_ids.push(chunk.id);
        bm25.add_chunk(&chunk)?;
        hnsw.add_node(chunk.id, emb)?;
        chunks.insert(chunk.id, chunk);
    }

    filemap.insert(path_str, new_ids);
    Ok(())
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}
