//! `needle search <query>` — run hybrid search and render colored result cards.

use needle::embedding::EmbeddingModel;
use needle::indexing::bm25::tokenize;
use needle::query::QueryEngine;
use needle::schema::{Language, SearchSignal};
use needle::storage::Storage;
use needle::Result;
use colored::Colorize;
use std::collections::HashSet;

pub async fn run(
    query: String,
    limit: usize,
    _all: bool,
    no_color: bool,
    compact: bool,
    lang: Option<String>,
) -> Result<()> {
    if no_color {
        colored::control::set_override(false);
    }

    // Load index from disk
    if !Storage::index_exists() {
        eprintln!(
            "{}: No index found.\n  Run: needle init <dirs...>",
            "Error".red().bold()
        );
        return Err(needle::Error::IndexNotFound(
            Storage::default_index_dir().to_string_lossy().to_string(),
        ));
    }

    let index_dir = Storage::default_index_dir();
    let storage = Storage::new(index_dir)?;
    let config = Storage::load_config()?;

    let bm25 = storage.load_bm25()?;
    let hnsw = storage.load_hnsw()?;
    let chunks = storage.load_chunks()?;
    let embedding = EmbeddingModel::new(config.embedding_dim)?;

    let mut engine = QueryEngine::new(bm25, hnsw, chunks, embedding);
    engine.ef_search = config.hnsw_ef_search as usize;
    engine.rrf_k = config.rrf_k;

    let lang_filter = lang.as_deref().and_then(Language::from_short);
    let (results, timing) = engine.search(&query, limit, lang_filter)?;

    // Header line
    println!(
        "\n  {} in {:.1}ms  (BM25: {:.1}ms  HNSW: {:.1}ms  embed: {:.1}ms  fuse: {:.1}ms)\n",
        format!("{} results", results.len()).bold(),
        timing.total_ms,
        timing.bm25_ms,
        timing.hnsw_ms,
        timing.embed_ms,
        timing.fusion_ms,
    );

    if results.is_empty() {
        println!("  {}", format!("No results for '{}'", query).dimmed());
        return Ok(());
    }

    // Query terms for snippet highlighting
    let query_terms: HashSet<String> = tokenize(&query).into_iter().collect();

    // Rank symbols: ❶ ❷ ❸ … fall back to numbers after 9
    let rank_symbols = ["❶", "❷", "❸", "❹", "❺", "❻", "❼", "❽", "❾", "❿"];

    for (i, result) in results.iter().enumerate() {
        let rank = rank_symbols.get(i).copied().unwrap_or("•");

        // --- Header line ---
        let path_display = truncate_path(&result.file_path, 60);
        let line_range = format!(":{}:{}", result.line_start, result.line_end);
        let badge = result.chunk_type.badge();

        let signal_colored = match result.signals {
            SearchSignal::Hybrid => format!("[{}]", "HYBRID").green().bold().to_string(),
            SearchSignal::Keyword => format!("[{}]", "KW").yellow().to_string(),
            SearchSignal::Semantic => format!("[{}]", "SEM").magenta().to_string(),
        };

        let score_str = if compact {
            String::new()
        } else {
            format!("  {}", format!("{:.4}", result.score).dimmed())
        };

        println!(
            " {} {}{}  {}  {}{}",
            rank.bold(),
            path_display.blue(),
            line_range.dimmed(),
            badge.cyan(),
            signal_colored,
            score_str,
        );

        if !compact {
            // --- Snippet ---
            let bar = "│".dimmed();
            let snippet = make_snippet(&result.content, &query_terms, config.snippet_lines);
            for line in &snippet {
                println!(" {} {}", bar, highlight_terms(line, &query_terms));
            }
        }

        println!();
    }

    // Footer
    let meta = storage.load_metadata().ok();
    let (total_chunks, total_files) = meta
        .as_ref()
        .map(|m| (m.total_chunks, m.total_files))
        .unwrap_or((0, 0));

    println!(
        "  {}",
        format!(
            "{} results  ·  index: {} chunks across {} files",
            results.len(),
            total_chunks,
            total_files
        )
        .dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Snippet rendering
// ---------------------------------------------------------------------------

/// Extract the most relevant window of lines from a chunk's content.
fn make_snippet(content: &str, query_terms: &HashSet<String>, max_lines: usize) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    if lines.len() <= max_lines {
        return lines.iter().map(|l| l.to_string()).collect();
    }

    // Find the line with the most query term hits to center the window
    let best_line = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let lower = l.to_lowercase();
            let hits = query_terms.iter().filter(|t| lower.contains(t.as_str())).count();
            (i, hits)
        })
        .max_by_key(|(_, hits)| *hits)
        .map(|(i, _)| i)
        .unwrap_or(0);

    let half = max_lines / 2;
    let start = best_line.saturating_sub(half);
    let end = (start + max_lines).min(lines.len());
    let start = end.saturating_sub(max_lines);

    let mut result: Vec<String> = lines[start..end].iter().map(|l| l.to_string()).collect();
    if end < lines.len() {
        result.push("  ...".to_string());
    }
    result
}

/// Highlight query terms in a line by making them bold + yellow.
fn highlight_terms(line: &str, query_terms: &HashSet<String>) -> String {
    if query_terms.is_empty() {
        return line.to_string();
    }

    // Simple scan: for each term occurrence, wrap it
    let mut result = line.to_string();
    for term in query_terms {
        let lower = result.to_lowercase();
        if let Some(pos) = lower.find(term.as_str()) {
            let end = pos + term.len();
            let highlighted = result[pos..end].yellow().bold().to_string();
            result = format!("{}{}{}", &result[..pos], highlighted, &result[end..]);
            // Only highlight the first occurrence per term per line (ANSI escape
            // length messes up subsequent byte-offset calculations)
            break;
        }
    }

    result
}

/// Truncate a path from the left if it's too long, adding "…/" prefix.
/// Also strips Windows extended-length prefix \\?\ .
fn truncate_path(path: &str, max_len: usize) -> String {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    if path.len() <= max_len {
        return path.to_string();
    }
    let keep = &path[path.len() - (max_len - 2)..];
    format!("…/{}", keep)
}
