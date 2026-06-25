//! `needle bench` — benchmarking suite for HNSW recall and query latency.

use needle::embedding::EmbeddingModel;
use needle::query::QueryEngine;
use needle::storage::{human_size, Storage};
use needle::Result;
use colored::Colorize;

pub async fn run() -> Result<()> {
    println!("{}", "Needle v0.1.0 — benchmarks\n".bold());

    if !Storage::index_exists() {
        println!("  {}", "No index found. Run: needle init <dirs...>".yellow());
        return Ok(());
    }

    let index_dir = Storage::default_index_dir();
    let storage = Storage::new(index_dir)?;
    let config = Storage::load_config()?;
    let meta = storage.load_metadata()?;

    let n_chunks = meta.total_chunks as usize;
    println!("  Running benchmarks on {} chunks...\n", n_chunks.to_string().cyan());

    if n_chunks == 0 {
        println!("  {}", "No chunks in index. Run: needle init <dirs...>".yellow());
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Load index
    // -----------------------------------------------------------------------
    let hnsw = storage.load_hnsw()?;
    let bm25 = storage.load_bm25()?;
    let chunks = storage.load_chunks()?;
    let embedding = EmbeddingModel::new(config.embedding_dim)?;

    // -----------------------------------------------------------------------
    // HNSW recall benchmark
    // -----------------------------------------------------------------------
    println!("  {}:", "HNSW recall (1000 random queries)".bold());

    let n_recall_queries = 1000.min(n_chunks);
    let chunk_ids: Vec<u64> = chunks.keys().copied().collect();
    let mut recalls_at_10: Vec<f64> = Vec::with_capacity(n_recall_queries);
    let mut rng_state: u64 = 0xdeadbeef_cafebabe;

    for _ in 0..n_recall_queries {
        // Pick a random chunk as the query vector
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let idx = (rng_state >> 32) as usize % chunk_ids.len();
        let qid = chunk_ids[idx];

        let Some(chunk) = chunks.get(&qid) else { continue };
        let q_emb = embedding.embed(&chunk.content);

        let approx: std::collections::HashSet<u64> = hnsw
            .search_knn(&q_emb, 10, config.hnsw_ef_search as usize)
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        let exact: std::collections::HashSet<u64> = hnsw
            .exact_knn(&q_emb, 10)
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        let intersection = approx.intersection(&exact).count();
        recalls_at_10.push(intersection as f64 / 10.0);
    }

    let mean_recall = recalls_at_10.iter().sum::<f64>() / recalls_at_10.len() as f64;
    let mut sorted_recalls = recalls_at_10.clone();
    sorted_recalls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min_recall = sorted_recalls.first().copied().unwrap_or(0.0);
    let p5_idx = (sorted_recalls.len() as f64 * 0.05) as usize;
    let p5_recall = sorted_recalls.get(p5_idx).copied().unwrap_or(0.0);

    let _recall_color = if mean_recall >= 0.95 { "green" } else { "yellow" };
    println!(
        "    recall@10:  {:.1}%  (min: {:.1}%  p5: {:.1}%)",
        mean_recall * 100.0,
        min_recall * 100.0,
        p5_recall * 100.0,
    );
    println!(
        "    {}",
        if mean_recall >= 0.95 {
            "✓ meets ≥95% target".green().to_string()
        } else {
            "⚠ below 95% target — increase M or efConstruction".yellow().to_string()
        }
    );

    // -----------------------------------------------------------------------
    // Query latency benchmark (100 hybrid queries)
    // -----------------------------------------------------------------------
    println!();
    println!("  {}:", "Query latency (100 hybrid queries)".bold());

    let mut engine = QueryEngine::new(bm25, hnsw, chunks.clone(), embedding);
    engine.ef_search = config.hnsw_ef_search as usize;

    // Pick 100 sample content fragments as queries
    let sample_queries: Vec<String> = chunk_ids
        .iter()
        .step_by((chunk_ids.len() / 100).max(1))
        .take(100)
        .filter_map(|id| chunks.get(id))
        .map(|c| {
            // Use the first ~50 chars of content as a query
            c.content
                .split_whitespace()
                .take(8)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();

    let n_queries = sample_queries.len();
    let mut latencies_ms: Vec<f64> = Vec::with_capacity(n_queries);
    let mut bm25_ms_total = 0.0f64;
    let mut hnsw_ms_total = 0.0f64;
    let mut embed_ms_total = 0.0f64;
    let mut fuse_ms_total = 0.0f64;

    // Warmup
    for q in sample_queries.iter().take(5) {
        let _ = engine.search(q, 10, None);
    }

    // Measured runs
    for q in &sample_queries {
        let (_, timing) = engine.search(q, 10, None)?;
        latencies_ms.push(timing.total_ms);
        bm25_ms_total += timing.bm25_ms;
        hnsw_ms_total += timing.hnsw_ms;
        embed_ms_total += timing.embed_ms;
        fuse_ms_total += timing.fusion_ms;
    }

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = percentile(&latencies_ms, 50);
    let p95 = percentile(&latencies_ms, 95);
    let p99 = percentile(&latencies_ms, 99);

    println!("    p50:   {:.2}ms", p50);
    println!("    p95:   {:.2}ms", p95);
    println!("    p99:   {:.2}ms", p99);
    println!(
        "    breakdown:  BM25 {:.2}ms · HNSW {:.2}ms · embed {:.2}ms · fuse {:.2}ms",
        bm25_ms_total / n_queries as f64,
        hnsw_ms_total / n_queries as f64,
        embed_ms_total / n_queries as f64,
        fuse_ms_total / n_queries as f64,
    );
    println!(
        "    {}",
        if p50 < 5.0 {
            "✓ meets <5ms p50 target".green().to_string()
        } else {
            "⚠ above 5ms p50 target".yellow().to_string()
        }
    );

    // -----------------------------------------------------------------------
    // Index size
    // -----------------------------------------------------------------------
    println!();
    println!("  {}:", "Index size".bold());
    let total = storage.index_size_bytes();
    let chunks_bytes = storage.file_size_bytes("chunks.json");
    let bm25_bytes = storage.file_size_bytes("bm25.bin");
    let hnsw_bytes = storage.file_size_bytes("hnsw.bin");
    let filemap_bytes = storage.file_size_bytes("filemap.json");
    let meta_bytes = storage.file_size_bytes("meta.json");

    println!("    Total: {}", human_size(total).cyan());
    println!("    ├── chunks.json     {}", human_size(chunks_bytes));
    println!("    ├── bm25.bin        {}", human_size(bm25_bytes));
    println!("    ├── hnsw.bin        {}", human_size(hnsw_bytes));
    println!("    ├── filemap.json    {}", human_size(filemap_bytes));
    println!("    └── meta.json       {}", human_size(meta_bytes));

    // -----------------------------------------------------------------------
    // Vocabulary stats
    // -----------------------------------------------------------------------
    println!();
    println!("  {}:", "BM25 stats".bold());
    println!(
        "    Vocabulary: {} terms",
        engine.bm25.vocabulary_size().to_string().dimmed()
    );
    println!(
        "    avg chunk length: {:.0} tokens",
        meta.avg_chunk_length
    );

    Ok(())
}

fn percentile(sorted: &[f64], p: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
    sorted[idx]
}
