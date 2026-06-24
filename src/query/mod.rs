//! Query engine: BM25 + HNSW + Reciprocal Rank Fusion.

pub mod fusion;

use crate::embedding::EmbeddingModel;
use crate::indexing::{bm25::BM25Index, hnsw::HnswIndex};
use crate::schema::{Chunk, Language, SearchResult};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub struct QueryEngine {
    pub bm25: BM25Index,
    pub hnsw: HnswIndex,
    pub chunks: HashMap<u64, Chunk>,
    pub embedding: EmbeddingModel,
    pub ef_search: usize,
    pub rrf_k: usize,
    pub top_candidates: usize,
}

#[derive(Debug, Clone)]
pub struct QueryTiming {
    pub bm25_ms: f64,
    pub embed_ms: f64,
    pub hnsw_ms: f64,
    pub fusion_ms: f64,
    pub total_ms: f64,
}

impl QueryEngine {
    pub fn new(
        bm25: BM25Index,
        hnsw: HnswIndex,
        chunks: HashMap<u64, Chunk>,
        embedding: EmbeddingModel,
    ) -> Self {
        Self {
            bm25,
            hnsw,
            chunks,
            embedding,
            ef_search: 50,
            rrf_k: 60,
            top_candidates: 50,
        }
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
        lang_filter: Option<Language>,
    ) -> crate::Result<(Vec<SearchResult>, QueryTiming)> {
        let total_start = Instant::now();

        // Pull extra candidates when filtering so post-filter limit is still met
        let candidates = if lang_filter.is_some() {
            (self.top_candidates * 3).max(200)
        } else {
            self.top_candidates
        };

        // --- Path A: BM25 keyword search ---
        let bm25_start = Instant::now();
        let bm25_raw = self.bm25.search(query, candidates);
        let bm25_ms = bm25_start.elapsed().as_secs_f64() * 1000.0;

        // --- Path B: Embed query + HNSW kNN ---
        let embed_start = Instant::now();
        let query_emb = self.embedding.embed(query);
        let embed_ms = embed_start.elapsed().as_secs_f64() * 1000.0;

        let hnsw_start = Instant::now();
        let hnsw_raw = self.hnsw.search_knn(&query_emb, candidates, self.ef_search);
        let hnsw_ms = hnsw_start.elapsed().as_secs_f64() * 1000.0;

        // --- Fusion via RRF ---
        let fusion_start = Instant::now();

        let bm25_ids: HashSet<u64> = bm25_raw.iter().map(|(id, _)| *id).collect();
        let hnsw_ids: HashSet<u64> = hnsw_raw.iter().map(|(id, _)| *id).collect();

        let fused = fusion::reciprocal_rank_fusion(&bm25_raw, &hnsw_raw, self.rrf_k);
        let fusion_ms = fusion_start.elapsed().as_secs_f64() * 1000.0;

        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

        // --- Enrich, filter, deduplicate ---
        let mut results = Vec::with_capacity(limit);
        let mut seen: Vec<(String, u32, u32)> = Vec::new(); // (file, line_start, line_end)

        for (chunk_id, rrf_score) in fused.into_iter() {
            if results.len() >= limit {
                break;
            }
            let Some(chunk) = self.chunks.get(&chunk_id) else {
                continue;
            };

            // Language filter
            if let Some(lf) = lang_filter {
                if chunk.language != lf {
                    continue;
                }
            }

            // Deduplication: skip if an already-selected chunk overlaps >50% by line range
            let overlap = seen.iter().any(|(f, s, e)| {
                f == &chunk.file_path
                    && line_overlap_ratio(*s, *e, chunk.line_start, chunk.line_end) > 0.5
            });
            if overlap {
                continue;
            }
            seen.push((chunk.file_path.clone(), chunk.line_start, chunk.line_end));

            let signal = fusion::classify_signal(chunk_id, &bm25_ids, &hnsw_ids);

            results.push(SearchResult {
                chunk_id,
                file_path: chunk.file_path.clone(),
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                language: chunk.language,
                chunk_type: chunk.chunk_type,
                content: chunk.content.clone(),
                score: rrf_score,
                signals: signal,
            });
        }

        Ok((results, QueryTiming { bm25_ms, embed_ms, hnsw_ms, fusion_ms, total_ms }))
    }

    /// Pure semantic similarity search — embeds the given snippet and queries
    /// HNSW only (no BM25, no RRF). Used by the "Find Similar" feature.
    pub fn search_similar(
        &self,
        snippet: &str,
        limit: usize,
        exclude_id: Option<u64>,
    ) -> crate::Result<Vec<SearchResult>> {
        let query_emb = self.embedding.embed(snippet);
        let candidates = (limit * 4).max(40);
        let hnsw_raw = self.hnsw.search_knn(&query_emb, candidates, self.ef_search);

        let mut results = Vec::with_capacity(limit);
        let mut seen: Vec<(String, u32, u32)> = Vec::new();

        for (chunk_id, dist) in hnsw_raw.into_iter() {
            if results.len() >= limit { break; }
            if Some(chunk_id) == exclude_id { continue; }

            let Some(chunk) = self.chunks.get(&chunk_id) else { continue };

            let overlap = seen.iter().any(|(f, s, e)| {
                f == &chunk.file_path
                    && line_overlap_ratio(*s, *e, chunk.line_start, chunk.line_end) > 0.5
            });
            if overlap { continue; }
            seen.push((chunk.file_path.clone(), chunk.line_start, chunk.line_end));

            results.push(SearchResult {
                chunk_id,
                file_path: chunk.file_path.clone(),
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                language: chunk.language,
                chunk_type: chunk.chunk_type,
                content: chunk.content.clone(),
                score: 1.0 / (1.0 + dist),
                signals: crate::schema::SearchSignal::Semantic,
            });
        }
        Ok(results)
    }

    /// Scan all chunks for TODO/FIXME/HACK/XXX annotations.
    pub fn scan_todos(&self) -> Vec<TodoItem> {
        const TAGS: &[&str] = &["TODO", "FIXME", "HACK", "XXX", "NOCOMMIT", "DEPRECATED"];
        let mut items = Vec::new();

        for chunk in self.chunks.values() {
            for (idx, line) in chunk.content.lines().enumerate() {
                let upper = line.to_uppercase();
                for &tag in TAGS {
                    if upper.contains(tag) {
                        items.push(TodoItem {
                            file_path: chunk.file_path.clone(),
                            line: chunk.line_start + idx as u32,
                            kind: tag.to_string(),
                            text: line.trim().to_string(),
                            language: chunk.language.short_name().to_string(),
                        });
                        break;
                    }
                }
            }
        }

        items.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.line.cmp(&b.line)));
        items
    }

    /// Return all indexed files with chunk counts and language.
    pub fn file_list(&self) -> Vec<FileEntry> {
        let mut map: std::collections::HashMap<String, (u32, String)> = std::collections::HashMap::new();
        for chunk in self.chunks.values() {
            let e = map.entry(chunk.file_path.clone()).or_insert((0, chunk.language.short_name().to_string()));
            e.0 += 1;
        }
        let mut files: Vec<FileEntry> = map.into_iter().map(|(path, (chunks, lang))| FileEntry { path, chunks, lang }).collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }
}

#[derive(Debug, Clone)]
pub struct TodoItem {
    pub file_path: String,
    pub line: u32,
    pub kind: String,
    pub text: String,
    pub language: String,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub chunks: u32,
    pub lang: String,
}

fn line_overlap_ratio(s1: u32, e1: u32, s2: u32, e2: u32) -> f32 {
    let overlap_start = s1.max(s2);
    let overlap_end = e1.min(e2);
    if overlap_end <= overlap_start {
        return 0.0;
    }
    let overlap = (overlap_end - overlap_start) as f32;
    let shorter = ((e1.saturating_sub(s1)).min(e2.saturating_sub(s2))) as f32;
    if shorter == 0.0 {
        return 0.0;
    }
    overlap / shorter
}
