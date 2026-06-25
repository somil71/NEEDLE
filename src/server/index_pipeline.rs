//! Reusable indexing pipeline — called by the background indexer for cloud repos.

use crate::{
    chunking::detect_chunker,
    config::Config,
    embedding::EmbeddingModel,
    graph,
    indexing::Index,
    schema::{IndexMetadata, Language},
    storage::Storage,
};
use chrono::Utc;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Index `source_dir` and store the result at `index_dir`.
/// Silent (no progress bars) — designed to run in a background task.
pub fn run(source_dir: &Path, index_dir: &Path) -> crate::Result<IndexStats> {
    let config = Config {
        watched_dirs: vec![source_dir.to_string_lossy().to_string()],
        ..Config::default()
    };

    let storage = Storage::new(index_dir.to_path_buf())?;

    // Collect files
    let all_files = collect_files(source_dir, &config);
    if all_files.is_empty() {
        return Err(crate::Error::InvalidPath("No supported files found".to_string()));
    }

    // Read file contents (PDFs get text extracted, others read as UTF-8)
    let file_entries: Vec<(PathBuf, Language, String)> = all_files
        .iter()
        .filter_map(|(path, lang)| {
            let content = if *lang == Language::Pdf {
                pdf_extract::extract_text(path).ok()?
            } else {
                std::fs::read_to_string(path).ok()?
            };
            Some((path.clone(), *lang, content))
        })
        .collect();

    // Chunk (parallel)
    let raw_chunks: Vec<crate::schema::Chunk> = file_entries
        .par_iter()
        .flat_map(|(path, lang, content)| {
            let chunker = detect_chunker(*lang);
            chunker.chunk(content, path, *lang).unwrap_or_default()
        })
        .collect();

    let total_chunks = raw_chunks.len();
    let total_files = file_entries.len();

    // Use hash-projection (no Ollama on cloud)
    let embed_model = EmbeddingModel::new(config.embedding_dim)?;

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
        let embedding = embed_model.embed(&chunk.content);
        filemap.entry(chunk.file_path.clone()).or_default().push(chunk_id);
        index.add_chunk(chunk, embedding)?;
    }

    // Save to disk
    storage.save_bm25(&index.inverted)?;
    storage.save_hnsw(&index.hnsw)?;
    storage.save_chunks(&index.chunk_store)?;
    storage.save_filemap(&filemap)?;

    let code_graph = graph::extract(&file_entries);
    storage.save_graph(&code_graph)?;

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

    Ok(IndexStats { total_chunks, total_files })
}

pub struct IndexStats {
    pub total_chunks: usize,
    pub total_files:  usize,
}

fn collect_files(dir: &Path, config: &Config) -> Vec<(PathBuf, Language)> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && !config.should_ignore(&name)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.path().to_path_buf();
            let ext = path.extension()?.to_str()?;
            let lang = Language::from_extension(ext)?;
            let meta = std::fs::metadata(&path).ok()?;
            let size_limit = if lang == Language::Pdf { 20_000_000 } else { 1_000_000 };
            if meta.len() > size_limit { return None; }
            Some((path, lang))
        })
        .collect()
}
