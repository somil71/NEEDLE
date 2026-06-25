//! BM25 inverted index for keyword search.
//!
//! Implements the Okapi BM25 scoring function with proper IDF weighting,
//! per-term frequency counting, and incremental soft-delete.

use crate::schema::{Chunk, PostingsEntry};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BM25Index {
    /// term → list of (chunk_id, term_freq) pairs
    postings: HashMap<String, Vec<PostingsEntry>>,
    /// term → number of chunks containing this term
    doc_freqs: HashMap<String, u32>,
    /// chunk_id → token count (for BM25 length normalization)
    chunk_lengths: HashMap<u64, u32>,
    /// soft-deleted chunk ids (filtered from results, compacted on reindex)
    deleted_chunks: HashSet<u64>,
    pub total_docs: u64,
    pub avg_doc_length: f32,
    pub k1: f32,
    pub b: f32,
}

impl Default for BM25Index {
    fn default() -> Self {
        Self::new()
    }
}

impl BM25Index {
    pub fn new() -> Self {
        Self::with_params(1.2, 0.75)
    }

    pub fn with_params(k1: f32, b: f32) -> Self {
        Self {
            postings: HashMap::new(),
            doc_freqs: HashMap::new(),
            chunk_lengths: HashMap::new(),
            deleted_chunks: HashSet::new(),
            total_docs: 0,
            avg_doc_length: 0.0,
            k1,
            b,
        }
    }

    pub fn add_chunk(&mut self, chunk: &Chunk) -> crate::Result<()> {
        let tokens = tokenize(&chunk.content);
        let token_count = tokens.len() as u32;

        // Count per-term frequencies in this chunk
        let mut tf_map: HashMap<String, u16> = HashMap::new();
        for token in &tokens {
            *tf_map.entry(token.clone()).or_insert(0) += 1;
        }

        // Write to postings lists
        for (term, tf) in &tf_map {
            self.postings
                .entry(term.clone())
                .or_default()
                .push(PostingsEntry {
                    chunk_id: chunk.id,
                    term_freq: *tf,
                });
            *self.doc_freqs.entry(term.clone()).or_insert(0) += 1;
        }

        self.chunk_lengths.insert(chunk.id, token_count);

        // Welford-style incremental mean update
        let new_total = self.total_docs + 1;
        self.avg_doc_length = (self.avg_doc_length * self.total_docs as f32
            + token_count as f32)
            / new_total as f32;
        self.total_docs = new_total;

        Ok(())
    }

    pub fn delete_chunk(&mut self, chunk_id: u64) -> crate::Result<()> {
        self.deleted_chunks.insert(chunk_id);
        self.chunk_lengths.remove(&chunk_id);
        self.total_docs = self.total_docs.saturating_sub(1);
        Ok(())
    }

    /// Returns (chunk_id, bm25_score) pairs sorted by score descending.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(u64, f32)> {
        if self.total_docs == 0 || self.avg_doc_length <= 0.0 {
            return Vec::new();
        }

        // Deduplicate query terms
        let query_terms: HashSet<String> = tokenize(query).into_iter().collect();
        let mut scores: HashMap<u64, f32> = HashMap::new();

        for term in &query_terms {
            let Some(postings) = self.postings.get(term) else {
                continue;
            };

            let df = self.doc_freqs.get(term).copied().unwrap_or(1) as f32;
            let n = self.total_docs as f32;

            // Robertson-Sparck Jones IDF (smooth). Clamp at 0: after soft-deletes
            // `total_docs` (n) shrinks but `doc_freqs` (df) is not decremented until
            // the next reindex, so df can exceed n and make the raw IDF negative —
            // which would wrongly *penalize* documents that contain the term.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.0);

            for entry in postings {
                if self.deleted_chunks.contains(&entry.chunk_id) {
                    continue;
                }
                let dl = self
                    .chunk_lengths
                    .get(&entry.chunk_id)
                    .copied()
                    .unwrap_or(self.avg_doc_length as u32) as f32;
                let tf = entry.term_freq as f32;

                // BM25 term score
                let tf_norm = (tf * (self.k1 + 1.0))
                    / (tf + self.k1 * (1.0 - self.b + self.b * dl / self.avg_doc_length));
                *scores.entry(entry.chunk_id).or_insert(0.0) += idf * tf_norm;
            }
        }

        let mut results: Vec<(u64, f32)> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    pub fn vocabulary_size(&self) -> usize {
        self.postings.len()
    }

    pub fn deleted_count(&self) -> usize {
        self.deleted_chunks.len()
    }

    pub fn avg_doc_length(&self) -> f32 {
        self.avg_doc_length
    }
}

/// Tokenize text into normalized BM25 terms.
///
/// Splits on non-alphanumeric-or-underscore, lowercases, unicode-normalizes.
/// For snake_case identifiers, emits both the whole token AND each part so that
/// searching "retry backoff" and "retry_with_backoff" both work.
pub fn tokenize(content: &str) -> Vec<String> {
    let normalized: String = content.nfkd().collect::<String>().to_lowercase();
    let mut tokens: Vec<String> = Vec::new();

    for raw_word in normalized.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if raw_word.len() < 2 {
            continue;
        }
        // Emit the whole identifier (handles exact matches like `retry_with_backoff`)
        if !is_stopword(raw_word) {
            tokens.push(raw_word.to_string());
        }
        // Also split on underscore so "retry backoff" matches "retry_with_backoff"
        if raw_word.contains('_') {
            for part in raw_word.split('_') {
                if part.len() >= 2 && !is_stopword(part) {
                    tokens.push(part.to_string());
                }
            }
        }
    }

    tokens
}

/// Returns the matched query terms found in a text (for snippet highlighting).
pub fn match_terms<'a>(text: &str, query_terms: &'a HashSet<String>) -> Vec<&'a str> {
    let lower = text.to_lowercase();
    query_terms
        .iter()
        .filter(|t| lower.contains(t.as_str()))
        .map(|t| t.as_str())
        .collect()
}

fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        "the" | "a" | "an" | "is" | "it" | "in" | "on" | "at" | "to" | "for"
            | "of" | "and" | "or" | "but" | "not" | "this" | "that" | "with"
            | "from" | "by" | "as" | "be" | "was" | "are" | "have" | "has"
            | "do" | "if" | "then" | "else" | "true" | "false" | "null"
            | "new" | "var" | "let" | "use" | "type" | "where" | "when"
    )
}
