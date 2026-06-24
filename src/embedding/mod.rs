//! Embedding model — two strategies selectable at `needle init` time:
//!
//! 1. **Hash-projection** (default fallback): xxh64-based random projection.
//!    Fast, no dependencies, but can only match overlapping tokens — "Ollama"
//!    and "local LLM" will NOT be similar.
//!
//! 2. **Ollama** (`nomic-embed-text`): real transformer embeddings via the
//!    Ollama local API. Understands semantic similarity. Requires Ollama running
//!    with `nomic-embed-text` pulled (`ollama pull nomic-embed-text`).

use serde::Deserialize;
use xxhash_rust::xxh64::xxh64;

pub const DEFAULT_DIM: usize = 384;
pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";
pub const OLLAMA_EMBED_MODEL: &str = "nomic-embed-text";

pub struct EmbeddingModel {
    strategy: Strategy,
    pub dim: usize,
}

enum Strategy {
    Hash,
    Ollama {
        base_url: String,
        model: String,
        client: reqwest::blocking::Client,
    },
}

#[derive(Deserialize)]
struct OllamaEmbedResp {
    embedding: Vec<f32>,
}

impl EmbeddingModel {
    /// Hash-projection fallback — always works, no external deps.
    pub fn new(dim: usize) -> crate::Result<Self> {
        Ok(Self { strategy: Strategy::Hash, dim })
    }

    /// Try to connect to Ollama and use it for embeddings.
    /// Returns `None` if Ollama isn't reachable or the model isn't pulled.
    pub fn try_ollama(base_url: &str, model: &str) -> Option<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .ok()?;

        // Probe: also tells us the output dim (768 for nomic-embed-text)
        let emb = Self::call_ollama(&client, base_url, model, "needle probe").ok()?;

        Some(Self {
            dim: emb.len(),
            strategy: Strategy::Ollama {
                base_url: base_url.to_string(),
                model: model.to_string(),
                client,
            },
        })
    }

    /// Restore an Ollama model from stored metadata (serve.rs startup).
    /// Falls back to hash-projection with matching dim if Ollama is down.
    pub fn from_metadata(embedding_model: &str, embedding_dim: usize) -> crate::Result<Self> {
        if let Some(model_name) = embedding_model.strip_prefix("ollama:") {
            if let Some(m) = Self::try_ollama(OLLAMA_DEFAULT_URL, model_name) {
                return Ok(m);
            }
            eprintln!(
                "Warning: index was built with Ollama ({}) but Ollama is not running.\n  \
                 Semantic search quality degraded. Run `ollama serve` and restart.",
                model_name
            );
            // Fall back to hash with the same dim so vector distances are consistent
            Self::new(embedding_dim)
        } else {
            Self::new(embedding_dim)
        }
    }

    pub fn dim(&self) -> usize { self.dim }

    pub fn model_string(&self) -> String {
        match &self.strategy {
            Strategy::Hash => format!("hash-projection-{}", self.dim),
            Strategy::Ollama { model, .. } => format!("ollama:{}", model),
        }
    }

    pub fn is_ollama(&self) -> bool {
        matches!(&self.strategy, Strategy::Ollama { .. })
    }

    /// Embed text to a unit-normalized f32 vector.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        match &self.strategy {
            Strategy::Hash => hash_embed(text, self.dim),
            Strategy::Ollama { client, base_url, model } => {
                Self::call_ollama(client, base_url, model, text)
                    .unwrap_or_else(|_| hash_embed(text, self.dim))
            }
        }
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    fn call_ollama(
        client: &reqwest::blocking::Client,
        base_url: &str,
        model: &str,
        text: &str,
    ) -> crate::Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", base_url);
        let body = serde_json::json!({ "model": model, "prompt": text });

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| crate::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        let data: OllamaEmbedResp = resp
            .json()
            .map_err(|e| crate::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        let mut emb = data.embedding;
        l2_normalize(&mut emb);
        Ok(emb)
    }
}

// ── Hash projection ──────────────────────────────────────────────────────────

fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut acc = vec![0.0f32; dim];
    let tokens = tokenize_for_embedding(text);
    if tokens.is_empty() { return acc; }

    for token in &tokens {
        let bytes = token.as_bytes();
        let h0 = xxh64(bytes, 0) as usize;
        let h1 = xxh64(bytes, 1) as usize;
        let h2 = xxh64(bytes, 2);
        let sign0 = if h2 & 1 == 0 { 1.0f32 } else { -1.0 };
        let sign1 = if h2 & 2 == 0 { 1.0f32 } else { -1.0 };
        acc[h0 % dim] += sign0;
        acc[h1 % dim] += sign1;
    }
    l2_normalize(&mut acc);
    acc
}

fn tokenize_for_embedding(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() >= 2)
        .flat_map(|word| {
            let mut parts = vec![word.to_string()];
            if word.contains('_') {
                parts.extend(
                    word.split('_').filter(|p| p.len() >= 2).map(|p| p.to_string()),
                );
            }
            parts
        })
        .collect()
}

pub fn l2_normalize(v: &mut Vec<f32>) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-8 {
        for x in v.iter_mut() { *x /= norm; }
    }
}
