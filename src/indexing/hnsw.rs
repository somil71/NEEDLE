//! HNSW (Hierarchical Navigable Small World) graph index for approximate nearest-neighbor search.
//!
//! Implements the algorithm from "Efficient and robust approximate nearest neighbor search
//! using Hierarchical Navigable Small World graphs" (Malkov & Yashunin, 2018).
//!
//! Key design choices:
//! - Pre-normalized unit embeddings → cosine distance = 1 - dot product (no sqrt needed)
//! - Diversity heuristic (Algorithm 4 from the paper) for neighbor selection
//! - Soft-delete with tombstones; compaction on `reindex`
//! - Flat Vec<f32> for embeddings (cache-friendly, O(1) slot access)
//! - Bidirectional edge insertion with pruning when over M/M_max0 capacity

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// A (distance, node_id) pair ordered by distance for use in a max-heap.
/// Larger distance = higher priority so we can `.pop()` the farthest element.
#[derive(Clone, PartialEq)]
struct DistNode {
    dist: f32,
    id: u64,
}

impl Eq for DistNode {}

impl PartialOrd for DistNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DistNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Max-heap: larger dist = higher priority (so farthest is on top)
        self.dist
            .partial_cmp(&other.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.id.cmp(&other.id))
    }
}

// ---------------------------------------------------------------------------
// Per-node graph data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HnswNodeData {
    /// Maximum layer this node participates in (0 = bottom layer only)
    layer: u8,
    /// neighbors[l] = list of chunk_ids that are neighbors at layer l
    neighbors: Vec<Vec<u64>>,
}

// ---------------------------------------------------------------------------
// HNSW index
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct HnswIndex {
    /// Per-node graph adjacency
    nodes: HashMap<u64, HnswNodeData>,

    /// Flat embedding store: slot s → embeddings_flat[s*dim .. (s+1)*dim]
    embeddings_flat: Vec<f32>,
    /// chunk_id → slot in embeddings_flat
    chunk_to_slot: HashMap<u64, usize>,
    /// slot → chunk_id
    slot_to_chunk: Vec<u64>,

    /// Soft-deleted nodes (still in graph for traversal, excluded from results)
    tombstones: HashSet<u64>,

    /// The node with the highest layer — starting point for all searches
    entry_point: Option<u64>,
    max_layer: u8,

    // Graph parameters
    pub m: usize,
    pub m_max0: usize,
    pub ef_construction: usize,
    pub ml: f64, // level generation factor = 1/ln(M)
    pub dim: usize,
}

impl HnswIndex {
    pub fn new(dim: usize) -> Self {
        Self::with_params(dim, 16, 200)
    }

    pub fn with_params(dim: usize, m: usize, ef_construction: usize) -> Self {
        Self {
            nodes: HashMap::new(),
            embeddings_flat: Vec::new(),
            chunk_to_slot: HashMap::new(),
            slot_to_chunk: Vec::new(),
            tombstones: HashSet::new(),
            entry_point: None,
            max_layer: 0,
            m,
            m_max0: m * 2,
            ef_construction,
            ml: 1.0 / (m as f64).ln(),
            dim,
        }
    }

    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn add_node(&mut self, id: u64, embedding: Vec<f32>) -> crate::Result<()> {
        if embedding.len() != self.dim {
            return Err(crate::error::Error::EmbeddingError(format!(
                "Expected dim={}, got {}",
                self.dim,
                embedding.len()
            )));
        }

        // Allocate slot and store embedding
        let slot = self.slot_to_chunk.len();
        self.slot_to_chunk.push(id);
        self.chunk_to_slot.insert(id, slot);
        self.embeddings_flat.extend_from_slice(&embedding);

        // Sample layer for the new node
        let new_layer = self.sample_layer();

        // First node is its own entry point
        let Some(mut ep) = self.entry_point else {
            self.nodes.insert(
                id,
                HnswNodeData {
                    layer: new_layer,
                    neighbors: vec![Vec::new(); new_layer as usize + 1],
                },
            );
            self.entry_point = Some(id);
            self.max_layer = new_layer;
            return Ok(());
        };

        let ep_layer = self.max_layer;

        // ---- Phase 1: greedy descent from top layer to new_layer+1 ----
        // Just find the closest entry point in that region (ef=1, no insertion)
        for layer in (new_layer + 1..=ep_layer).rev() {
            let results = self.search_layer(&embedding, ep, 1, layer);
            if let Some(&(closest, _)) = results.first() {
                ep = closest;
            }
        }

        // ---- Phase 2: insert at layers new_layer down to 0 ----
        let mut neighbors_per_layer: Vec<Vec<u64>> = vec![Vec::new(); new_layer as usize + 1];

        for layer in (0..=new_layer).rev() {
            let m_at_layer = if layer == 0 { self.m_max0 } else { self.m };

            // Find ef_construction nearest candidates at this layer
            let candidates = self.search_layer(&embedding, ep, self.ef_construction, layer);

            // Select M diverse neighbors via the heuristic
            let selected = self.select_neighbors_heuristic(&embedding, &candidates, m_at_layer);

            neighbors_per_layer[layer as usize] = selected.clone();

            // Create bidirectional edges: for each selected neighbor, add id to its list
            for &neighbor_id in &selected {
                if let Some(neighbor_data) = self.nodes.get_mut(&neighbor_id) {
                    let l = layer as usize;
                    if l < neighbor_data.neighbors.len() {
                        neighbor_data.neighbors[l].push(id);

                        // Prune neighbor's edges if over capacity
                        if neighbor_data.neighbors[l].len() > m_at_layer {
                            // Collect all of neighbor's current neighbors as candidates
                            let n_neighbors = neighbor_data.neighbors[l].clone();
                            let n_emb = self.get_embedding(neighbor_id).to_vec();
                            let cands_with_dist: Vec<(u64, f32)> = n_neighbors
                                .iter()
                                .map(|&nid| (nid, self.cosine_dist(&n_emb, nid)))
                                .collect();
                            let pruned = self.select_neighbors_heuristic(
                                &n_emb,
                                &cands_with_dist,
                                m_at_layer,
                            );
                            if let Some(nd) = self.nodes.get_mut(&neighbor_id) {
                                nd.neighbors[l] = pruned;
                            }
                        }
                    }
                }
            }

            // Update entry point for next layer (closest found at this layer)
            if let Some(&(closest, _)) = candidates.first() {
                ep = closest;
            }
        }

        self.nodes.insert(
            id,
            HnswNodeData {
                layer: new_layer,
                neighbors: neighbors_per_layer,
            },
        );

        // Update graph entry point if new node has a higher layer
        if new_layer > ep_layer {
            self.max_layer = new_layer;
            self.entry_point = Some(id);
        }

        Ok(())
    }

    pub fn delete_node(&mut self, id: u64) -> crate::Result<()> {
        self.tombstones.insert(id);
        // Update entry point if we just tombstoned it
        if self.entry_point == Some(id) {
            // Find another node at the highest layer
            self.entry_point = self
                .nodes
                .iter()
                .filter(|(nid, _)| !self.tombstones.contains(nid))
                .max_by_key(|(_, data)| data.layer)
                .map(|(&nid, _)| nid);
            self.max_layer = self
                .entry_point
                .and_then(|ep| self.nodes.get(&ep))
                .map(|d| d.layer)
                .unwrap_or(0);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Approximate k-NN search using HNSW graph traversal.
    /// Returns (chunk_id, cosine_distance) pairs sorted by distance ascending.
    pub fn search_knn(&self, q: &[f32], k: usize, ef_search: usize) -> Vec<(u64, f32)> {
        let Some(ep) = self.entry_point else {
            return Vec::new();
        };

        let ef = ef_search.max(k);
        let mut ep_id = ep;

        // Phase 1: greedy descent to layer 1
        for layer in (1..=self.max_layer).rev() {
            let results = self.search_layer(q, ep_id, 1, layer);
            if let Some(&(closest, _)) = results.first() {
                ep_id = closest;
            }
        }

        // Phase 2: beam search at layer 0 with ef candidates
        let mut results = self.search_layer(q, ep_id, ef, 0);
        results.truncate(k);
        results
    }

    /// Exact brute-force k-NN. Used for recall benchmarking and as baseline.
    pub fn exact_knn(&self, q: &[f32], k: usize) -> Vec<(u64, f32)> {
        let mut dists: Vec<(u64, f32)> = self
            .slot_to_chunk
            .iter()
            .enumerate()
            .filter(|(_, id)| !self.tombstones.contains(id))
            .map(|(slot, &id)| {
                let emb = &self.embeddings_flat[slot * self.dim..(slot + 1) * self.dim];
                (id, dot_distance(q, emb))
            })
            .collect();

        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        dists.truncate(k);
        dists
    }

    pub fn node_count(&self) -> usize {
        self.slot_to_chunk.len().saturating_sub(self.tombstones.len())
    }

    pub fn total_slots(&self) -> usize {
        self.slot_to_chunk.len()
    }

    pub fn entry_point(&self) -> Option<u64> {
        self.entry_point
    }

    pub fn max_layer(&self) -> u8 {
        self.max_layer
    }

    // -----------------------------------------------------------------------
    // Core algorithm: layer search
    // -----------------------------------------------------------------------

    /// Search a single layer of the graph for `ef` nearest neighbors of `q`
    /// starting from entry point `ep`.
    ///
    /// Returns list of (chunk_id, distance) sorted by distance ascending.
    fn search_layer(&self, q: &[f32], ep: u64, ef: usize, layer: u8) -> Vec<(u64, f32)> {
        let mut visited: HashSet<u64> = HashSet::new();

        // W = results (max-heap: farthest on top for O(1) eviction)
        let mut w: BinaryHeap<DistNode> = BinaryHeap::new();
        // Candidates (min-heap: closest on top for greedy expansion)
        let mut candidates: BinaryHeap<Reverse<DistNode>> = BinaryHeap::new();

        let ep_dist = self.dist_to_q(q, ep);
        visited.insert(ep);

        let ep_node = DistNode { dist: ep_dist, id: ep };
        candidates.push(Reverse(ep_node.clone()));
        if !self.tombstones.contains(&ep) {
            w.push(ep_node);
        }

        loop {
            let Reverse(c) = match candidates.pop() {
                Some(x) => x,
                None => break,
            };

            // Termination: if closest candidate is farther than worst result, stop
            let f_dist = w.peek().map(|n| n.dist).unwrap_or(f32::MAX);
            if c.dist > f_dist && w.len() >= ef {
                break;
            }

            // Expand this candidate's neighbors at this layer
            let neighbors = self
                .nodes
                .get(&c.id)
                .and_then(|nd| nd.neighbors.get(layer as usize))
                .cloned()
                .unwrap_or_default();

            for neighbor_id in neighbors {
                if visited.contains(&neighbor_id) {
                    continue;
                }
                visited.insert(neighbor_id);

                let f_dist = w.peek().map(|n| n.dist).unwrap_or(f32::MAX);
                let nd = self.dist_to_q(q, neighbor_id);

                if nd < f_dist || w.len() < ef {
                    candidates.push(Reverse(DistNode { dist: nd, id: neighbor_id }));

                    // Only add live nodes to the result set W
                    if !self.tombstones.contains(&neighbor_id) {
                        w.push(DistNode { dist: nd, id: neighbor_id });
                        if w.len() > ef {
                            w.pop(); // evict farthest
                        }
                    }
                }
            }
        }

        let mut results: Vec<(u64, f32)> =
            w.into_iter().map(|n| (n.id, n.dist)).collect();
        // Sort closest first
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    // -----------------------------------------------------------------------
    // Core algorithm: diversity heuristic (Algorithm 4 from the HNSW paper)
    // -----------------------------------------------------------------------

    /// Select at most `m` neighbors from `candidates` using the diversity heuristic.
    ///
    /// Candidates must be (chunk_id, distance_to_q) sorted by distance ascending.
    ///
    /// The heuristic rejects a candidate `c` if there's already a selected neighbor `s`
    /// such that `dist(c, s) < dist(c, q)` — meaning `c` is "covered" by `s`.
    /// This keeps neighbors spread out in space rather than clustering in one direction.
    fn select_neighbors_heuristic(
        &self,
        _q_emb: &[f32],
        candidates: &[(u64, f32)],
        m: usize,
    ) -> Vec<u64> {
        let mut selected: Vec<(u64, f32)> = Vec::with_capacity(m);

        'outer: for &(cand_id, dist_to_q) in candidates {
            if selected.len() >= m {
                break;
            }

            // Reject if any already-selected neighbor is closer to cand than q is
            for &(sel_id, _) in &selected {
                let dist_cand_to_sel = self.cosine_dist_between(cand_id, sel_id);
                if dist_cand_to_sel < dist_to_q {
                    continue 'outer; // c is covered by sel — skip it
                }
            }

            selected.push((cand_id, dist_to_q));
        }

        selected.into_iter().map(|(id, _)| id).collect()
    }

    // -----------------------------------------------------------------------
    // Distance helpers (all assume pre-normalized unit vectors)
    // -----------------------------------------------------------------------

    /// Cosine distance from query vector q to the stored embedding of node `id`.
    fn dist_to_q(&self, q: &[f32], id: u64) -> f32 {
        let emb = self.get_embedding(id);
        dot_distance(q, emb)
    }

    /// Cosine distance between two stored nodes.
    fn cosine_dist_between(&self, id_a: u64, id_b: u64) -> f32 {
        let emb_b = self.get_embedding(id_b).to_vec();
        self.cosine_dist(&emb_b, id_a)
    }

    /// Cosine distance from an owned vector to a stored node.
    fn cosine_dist(&self, v: &[f32], id: u64) -> f32 {
        let emb = self.get_embedding(id);
        dot_distance(v, emb)
    }

    /// Get embedding slice for a node id.
    fn get_embedding(&self, id: u64) -> &[f32] {
        let slot = self.chunk_to_slot[&id];
        &self.embeddings_flat[slot * self.dim..(slot + 1) * self.dim]
    }

    // -----------------------------------------------------------------------
    // Layer sampling
    // -----------------------------------------------------------------------

    /// Sample a layer assignment using the geometric distribution.
    /// Most nodes land at layer 0; probability of layer k ∝ e^{-k/mL}.
    fn sample_layer(&self) -> u8 {
        let r: f64 = rand::thread_rng().gen::<f64>();
        // Clamp to a reasonable max (e.g. 16) to avoid pathological cases
        ((-r.ln() * self.ml).floor() as u8).min(16)
    }
}

// ---------------------------------------------------------------------------
// Distance function
// ---------------------------------------------------------------------------

/// Cosine distance for pre-normalized (unit-length) vectors.
/// cosine_distance = 1 - cos(θ) = 1 - dot(a, b)   [for unit vectors]
/// Range [0, 2]: 0 = identical, 1 = orthogonal, 2 = opposite.
#[inline]
fn dot_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    (1.0 - dot).max(0.0) // clamp numerical noise below zero
}
