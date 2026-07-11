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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub async fn run(directories: Vec<String>) -> Result<()> {
    println!("{}", "Needle v0.1.0 — initializing index".bold());
    println!();

    let (valid_dirs, config, storage) = init_workspace(directories)?;
    let file_entries = scan_files(&valid_dirs, &config);

    if file_entries.is_empty() {
        println!("\n{} No supported files found.", "Warning:".yellow());
        return Ok(());
    }

    let (index, filemap) = build_index(&file_entries, &config).await?;
    let code_graph = extract_graph(&file_entries);
    persist_artifacts(&storage, &index, &filemap, &code_graph, &config, file_entries.len())?;
    warn_if_not_gitignored();
    Ok(())
}

fn init_workspace(directories: Vec<String>) -> Result<(Vec<PathBuf>, Config, Storage)> {
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
        valid_dirs.push(clean_path(p.canonicalize()?));
    }

    let config = Config {
        watched_dirs: valid_dirs.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        ..Config::default()
    };

    let index_dir = Storage::default_index_dir();
    let storage = Storage::new(index_dir)?;
    Storage::save_config(&config)?;
    Ok((valid_dirs, config, storage))
}

fn scan_files(valid_dirs: &[PathBuf], config: &Config) -> Vec<(PathBuf, Language, String)> {
    let mut all_files: Vec<(PathBuf, Language)> = Vec::new();
    println!("Scanning directories...");

    for (i, dir) in valid_dirs.iter().enumerate() {
        let prefix = if i + 1 == valid_dirs.len() { "└──" } else { "├──" };
        let files = collect_files(dir, config);
        println!(
            "  {}  {}    {} files",
            prefix,
            dir.display(),
            files.len().to_string().cyan()
        );
        all_files.extend(files);
    }
    println!();

    all_files
        .iter()
        .filter_map(|(path, lang)| {
            let content = if *lang == Language::Pdf {
                pdf_extract::extract_text(path).ok()?
            } else {
                std::fs::read_to_string(path).ok()?
            };
            Some((path.clone(), *lang, content))
        })
        .collect()
}

async fn build_index(
    file_entries: &[(PathBuf, Language, String)],
    config: &Config,
) -> Result<(Index, HashMap<String, Vec<u64>>)> {
    // Sequential chunking — rayon workers have 1 MB stack on Windows which
    // overflows on large files; tree-sitter runs on the main thread instead.
    let chunk_bar = ProgressBar::new(file_entries.len() as u64);
    chunk_bar.set_style(
        ProgressStyle::default_bar()
            .template("  Chunking  {bar:32.cyan/dim}  {pos}/{len} files  [{elapsed}]")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let raw_chunks: Vec<needle::schema::Chunk> = file_entries
        .iter()
        .flat_map(|(path, lang, content)| {
            chunk_bar.inc(1);
            let chunker = detect_chunker(*lang);
            chunker.chunk(content, path, *lang).unwrap_or_default()
        })
        .collect();
    chunk_bar.finish_with_message("done");

    let total_chunks = raw_chunks.len();

    print!("  Checking for Ollama ({})... ", OLLAMA_EMBED_MODEL);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let embed_model = match tokio::task::block_in_place(|| {
        EmbeddingModel::try_ollama(OLLAMA_DEFAULT_URL, OLLAMA_EMBED_MODEL)
    }) {
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
        let embedding = tokio::task::block_in_place(|| embed_model.embed(&chunk.content));
        filemap.entry(chunk.file_path.clone()).or_default().push(chunk_id);
        index.add_chunk(chunk, embedding)?;
        embed_bar.inc(1);
    }
    embed_bar.finish_with_message("done");

    Ok((index, filemap))
}

fn extract_graph(file_entries: &[(PathBuf, Language, String)]) -> needle::graph::CodeGraph {
    print!("  Building knowledge graph...  ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let start = std::time::Instant::now();
    // Spawn a thread with a large stack — recursive tree-sitter traversal
    // overflows the default 1 MB Windows stack on deeply nested source files.
    let code_graph = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn_scoped(s, || graph::extract(file_entries))
            .expect("thread spawn failed")
            .join()
            .expect("graph extraction panicked")
    });
    println!(
        "done [{:.1}s]  {} nodes  {} edges",
        start.elapsed().as_secs_f64(),
        code_graph.stats.total_nodes,
        code_graph.stats.total_edges,
    );
    code_graph
}

fn persist_artifacts(
    storage: &Storage,
    index: &Index,
    filemap: &HashMap<String, Vec<u64>>,
    code_graph: &needle::graph::CodeGraph,
    config: &Config,
    total_files: usize,
) -> Result<()> {
    print!("  Building inverted index... ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let t = std::time::Instant::now();
    storage.save_bm25(&index.inverted)?;
    println!("done [{:.1}s]", t.elapsed().as_secs_f64());

    print!("  Building HNSW graph...      ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let t = std::time::Instant::now();
    storage.save_hnsw(&index.hnsw)?;
    println!("done [{:.1}s]", t.elapsed().as_secs_f64());

    storage.save_chunks(&index.chunk_store)?;
    storage.save_filemap(filemap)?;
    storage.save_graph(code_graph)?;

    let meta = IndexMetadata {
        total_chunks: (index.chunk_store.len()) as u64,
        total_files: total_files as u64,
        last_update_ts: Utc::now().timestamp() as u64,
        embedding_model: String::from("hash-projection"),
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

    let disk_bytes = storage.index_size_bytes();
    let vocab = index.inverted.vocabulary_size();
    println!();
    println!("{}", "✓ Index ready".green().bold());
    println!(
        "  {}  ·  {}  ·  {} on disk",
        format!("{} chunks", meta.total_chunks).cyan(),
        format!("{} files", total_files).cyan(),
        human_size(disk_bytes).cyan(),
    );
    println!(
        "  {} terms in vocabulary  ·  HNSW M={} efC={}",
        vocab, config.hnsw_m, config.hnsw_ef_construction,
    );
    println!("  Model: hash-projection-{}", config.embedding_dim);
    println!("  Stored at: {}", Storage::default_index_dir().display());
    Ok(())
}

fn warn_if_not_gitignored() {
    let index_dir = Storage::default_index_dir();
    let Some(needle_dir) = index_dir.parent() else { return };
    let Some(repo_root) = needle_dir.parent() else { return };
    if !repo_root.join(".git").exists() {
        return;
    }
    let gitignore_path = repo_root.join(".gitignore");
    let already_ignored = std::fs::read_to_string(&gitignore_path)
        .map(|s| s.lines().any(|l| l.trim() == ".needle/" || l.trim() == ".needle"))
        .unwrap_or(false);
    if !already_ignored {
        println!();
        println!(
            "  {} .needle/ isn't in this repo's .gitignore yet — add it so the index doesn't get committed.",
            "tip:".yellow().bold()
        );
    }
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
            let meta = std::fs::metadata(&path).ok()?;
            let size_limit = if lang == Language::Pdf { 20_000_000 } else { 1_000_000 };
            if meta.len() > size_limit {
                return None;
            }
            Some((path, lang))
        })
        .collect()
}

fn clean_path(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}
