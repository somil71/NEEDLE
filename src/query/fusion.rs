//! Reciprocal Rank Fusion (RRF) for combining keyword and semantic ranked lists.
//!
//! Score formula: rrf(d) = Σ 1/(k + rank_i(d))
//! where k=60 smooths out rank differences at the top.

use std::collections::HashMap;

/// Fuse two ranked lists (BM25 and HNSW) via RRF.
///
/// Each input is a Vec of (chunk_id, score) sorted by score descending.
/// Returns (chunk_id, rrf_score) sorted by rrf_score descending.
pub fn reciprocal_rank_fusion(
    bm25_results: &[(u64, f32)],
    hnsw_results: &[(u64, f32)],
    rrf_k: usize,
) -> Vec<(u64, f32)> {
    let mut scores: HashMap<u64, f32> = HashMap::new();
    let k = rrf_k as f32;

    for (rank, (chunk_id, _)) in bm25_results.iter().enumerate() {
        *scores.entry(*chunk_id).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }

    for (rank, (chunk_id, _)) in hnsw_results.iter().enumerate() {
        *scores.entry(*chunk_id).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }

    let mut results: Vec<(u64, f32)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Determine the signal source for a chunk based on whether it appeared
/// in the BM25 results, HNSW results, or both.
pub fn classify_signal(
    chunk_id: u64,
    bm25_ids: &std::collections::HashSet<u64>,
    hnsw_ids: &std::collections::HashSet<u64>,
) -> crate::schema::SearchSignal {
    let in_bm25 = bm25_ids.contains(&chunk_id);
    let in_hnsw = hnsw_ids.contains(&chunk_id);
    match (in_bm25, in_hnsw) {
        (true, true) => crate::schema::SearchSignal::Hybrid,
        (true, false) => crate::schema::SearchSignal::Keyword,
        (false, true) => crate::schema::SearchSignal::Semantic,
        (false, false) => crate::schema::SearchSignal::Keyword, // shouldn't happen
    }
}
