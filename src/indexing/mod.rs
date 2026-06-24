//! Indexing layer: BM25 inverted index and HNSW vector index.

pub mod bm25;
pub mod hnsw;

use crate::schema::Chunk;
use std::collections::HashMap;

/// Combined index: BM25 + HNSW + in-memory chunk store.
pub struct Index {
    pub inverted: bm25::BM25Index,
    pub hnsw: hnsw::HnswIndex,
    pub chunk_store: HashMap<u64, Chunk>,
    next_chunk_id: u64,
}

impl Index {
    pub fn new(embedding_dim: usize) -> Self {
        Self {
            inverted: bm25::BM25Index::new(),
            hnsw: hnsw::HnswIndex::new(embedding_dim),
            chunk_store: HashMap::new(),
            next_chunk_id: 0,
        }
    }

    pub fn with_params(
        embedding_dim: usize,
        hnsw_m: usize,
        hnsw_ef_construction: usize,
        bm25_k1: f32,
        bm25_b: f32,
    ) -> Self {
        Self {
            inverted: bm25::BM25Index::with_params(bm25_k1, bm25_b),
            hnsw: hnsw::HnswIndex::with_params(embedding_dim, hnsw_m, hnsw_ef_construction),
            chunk_store: HashMap::new(),
            next_chunk_id: 0,
        }
    }

    /// Assign the next available chunk ID.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_chunk_id;
        self.next_chunk_id += 1;
        id
    }

    pub fn set_next_id(&mut self, id: u64) {
        self.next_chunk_id = id;
    }

    pub fn add_chunk(&mut self, chunk: Chunk, embedding: Vec<f32>) -> crate::Result<()> {
        self.chunk_store.insert(chunk.id, chunk.clone());
        self.inverted.add_chunk(&chunk)?;
        self.hnsw.add_node(chunk.id, embedding)?;
        Ok(())
    }

    pub fn delete_chunk(&mut self, chunk_id: u64) -> crate::Result<()> {
        self.chunk_store.remove(&chunk_id);
        self.inverted.delete_chunk(chunk_id)?;
        self.hnsw.delete_node(chunk_id)?;
        Ok(())
    }

    pub fn total_chunks(&self) -> usize {
        self.chunk_store.len()
    }
}
