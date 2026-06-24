# Needle — Tech Stack & Logical Flows

---

## Tech stack

### Core language: Rust

The entire engine is Rust. This isn't a style choice — the project's hard parts (HNSW inner loops, mmap'd index traversal, concurrent indexing, zero-copy deserialization) are exactly where Rust pays off. "I built a hybrid search engine with a from-scratch HNSW in Rust" is categorically different from the same sentence in Python.

### Dependencies (intentionally minimal)

| Layer | Crate / Tool | Why |
|---|---|---|
| **CLI framework** | `clap` | Derive-based arg parsing, sub-commands, shell completions |
| **AST parsing** | `tree-sitter` + language grammars (`tree-sitter-python`, `tree-sitter-typescript`, `tree-sitter-rust`, `tree-sitter-go`) | Code-aware chunking on function/class boundaries |
| **Embeddings** | `ort` (ONNX Runtime Rust bindings) | Local inference of sentence-transformer, no Python needed |
| **Embedding model** | `all-MiniLM-L6-v2` (ONNX export, 384-dim, ~80MB) | Small, fast, good general-purpose sentence embeddings |
| **Tokenizer** | `tokenizers` (HuggingFace Rust) | WordPiece tokenization for the embedding model |
| **File watching** | `notify` | Cross-platform fs events (inotify / FSEvents / ReadDirectoryChanges) |
| **Memory mapping** | `memmap2` | Zero-copy index loading, OS page cache handles hot/cold |
| **Hashing** | `xxhash-rust` | Content hashing for chunk dedup (xxh3, ~30GB/s) |
| **Serialization** | `rkyv` (zero-copy) | Serialize index structures to disk without deserialization overhead |
| **Logging** | `tracing` + `tracing-subscriber` | Structured JSON logs, span-based instrumentation |
| **Testing** | `criterion` | Microbenchmarks for HNSW, BM25, embedding throughput |
| **Concurrency** | `rayon` (data parallelism) + `crossbeam` (channels) | Parallel chunking/embedding, watcher-to-indexer channel |
| **Unicode** | `unicode-normalization` + `unicode-segmentation` | Correct tokenization for BM25 |
| **Stemmer** | hand-rolled Porter stemmer OR `rust-stemmers` | Optional stemming for keyword search |

### What is NOT a dependency

- No vector database (Qdrant, Pinecone, etc.) — HNSW is hand-rolled.
- No search engine library (Tantivy, Meilisearch) — inverted index is hand-rolled.
- No Python — embedding runs via ONNX Runtime in-process.
- No GPU required — CPU inference is fast enough for the embedding model at indexing time.
- No network stack — no HTTP server in v1 (CLI only). Stretch web UI uses a lightweight embedded server (`axum`).

### Build and CI

| Tool | Purpose |
|---|---|
| `cargo` | Build, test, bench |
| `cargo-nextest` | Faster test runner |
| GitHub Actions | CI: `cargo clippy`, `cargo test`, `cargo bench`, HNSW recall regression |
| `cargo-release` | Versioned releases with changelogs |

### Stretch stack (not v1)

| Component | Tool | When |
|---|---|---|
| Web UI | `axum` (backend) + vanilla HTML/JS (frontend) | After CLI is solid |
| VS Code extension | TypeScript + Needle as subprocess | After web UI |
| Code-specific embeddings | `codesage-small` or `unixcoder` ONNX export | After general embeddings prove out |

---

## Logical flows

### Flow 1: First-time initialization (`needle init`)

```
User runs: needle init ~/code ~/notes ~/docs

  1. CLI parses directory arguments, validates they exist.
  2. Create ~/.needle/ directory structure:
     ├── config.toml          (watched dirs, ignore patterns, params)
     ├── index/
     │   ├── inverted.idx     (BM25 inverted index, mmap'd)
     │   ├── hnsw.idx         (HNSW graph, mmap'd)
     │   ├── chunks.store     (chunk content + metadata, mmap'd)
     │   ├── embeddings.bin   (raw f32 vectors, mmap'd)
     │   └── wal/             (write-ahead log segments)
     └── models/
         └── minilm-l6-v2.onnx  (downloaded on first init if missing)

  3. If embedding model not present:
     → Download all-MiniLM-L6-v2.onnx from bundled URL or local cache.
     → Verify checksum.

  4. Walk all configured directories recursively.
     → Apply ignore patterns (.git, node_modules, etc.).
     → Collect file list with paths + modification timestamps.

  5. For each file (parallelized via rayon):
     a. Detect language from extension.
     b. Read file content.
     c. Route to appropriate chunker:
        - Code file → tree-sitter AST chunker
        - Markdown  → structure-aware prose chunker
        - Plain text → sliding-window paragraph chunker
        - Config    → top-level key-block chunker
     d. For each chunk produced:
        i.   Compute content hash (xxh3).
        ii.  Assign chunk_id (monotonic u64).
        iii. Write chunk metadata to chunks.store.

  6. Embedding pass (parallelized, batched):
     → Batch chunks into groups of 32.
     → Run ONNX inference: text → 384-dim f32 vector.
     → Write vectors to embeddings.bin (append, position = chunk_id × 384 × 4 bytes).

  7. Build inverted index:
     → For each chunk: tokenize → normalize → stem (optional).
     → Build in-memory postings: HashMap<Term, Vec<(ChunkId, TermFreq)>>.
     → Compute IDF for each term.
     → Serialize to inverted.idx via rkyv.

  8. Build HNSW graph:
     → Initialize empty graph with entry point = first vector.
     → For each vector (sequential, order matters for graph quality):
        a. Sample layer from geometric distribution: floor(-ln(rand()) × mL), mL = 1/ln(M).
        b. Starting from entry point at top layer, greedy search down to the node's layer.
        c. At each layer from the node's layer down to 0:
           - Search for efConstruction nearest neighbors.
           - Select M neighbors using diversity heuristic:
             For each candidate c (sorted by distance to new node):
               If c is closer to the new node than to ALL already-selected neighbors → keep.
               Else → prune (it's redundant, another neighbor covers that direction).
           - Create bidirectional edges.
        d. If new node's layer > current max layer → update entry point.
     → Serialize graph (adjacency lists per layer) to hnsw.idx via rkyv.

  9. Flush WAL, write index snapshot.
  10. Print summary: X files, Y chunks, Z seconds, index size on disk.
```

### Flow 2: Incremental indexing (background, continuous)

```
File watcher (notify) emits events on configured directories.

  Event arrives: FileCreated | FileModified | FileDeleted | FileRenamed

  1. Debounce: buffer events for 300ms, collapse multiple events on the same
     path into a single "changed" event.

  2. For each debounced event:

     [FileCreated or FileModified]
       a. Read new file content.
       b. Chunk the file (same logic as init).
       c. Compute content hash for each new chunk.
       d. Load existing chunk hashes for this file path from chunks.store.
       e. Diff:
          - Unchanged chunks (hash match) → skip entirely.
          - New chunks (no matching hash) → embed, add to both indexes.
          - Deleted chunks (old hash not in new set) → tombstone in both indexes.
       f. Write WAL entry: { file_path, added_chunks[], deleted_chunk_ids[] }
       g. Apply mutations:
          - Inverted index: update postings lists (add new terms, decrement old).
          - HNSW: insert new vectors, tombstone deleted vectors.
          - Chunks.store: append new chunks, mark deleted as tombstoned.
       h. Commit WAL entry.

     [FileDeleted]
       a. Load all chunk_ids for this file path.
       b. Tombstone all in both indexes.
       c. WAL entry + commit.

     [FileRenamed]
       a. Update file_path on all chunks for old path → new path.
       b. No re-chunking, no re-embedding (content unchanged).
       c. WAL entry + commit.

  3. Periodic compaction (background, every N minutes or on threshold):
     → Rebuild HNSW graph without tombstoned nodes.
     → Compact inverted index (remove dead postings).
     → Compact chunks.store (remove tombstoned chunks).
     → Write new snapshot, truncate WAL.
```

### Flow 3: Search query (`needle search`)

```
User runs: needle search "function that retries HTTP requests"

  1. Parse query string.

  2. Fork into two parallel paths:

     PATH A — Keyword (BM25):
       a. Tokenize query: ["function", "that", "retries", "http", "requests"]
       b. Remove stopwords: ["function", "retries", "http", "requests"]
       c. Normalize + stem: ["function", "retri", "http", "request"]
       d. For each query term:
          → Look up postings list in inverted index.
          → Score each chunk in the postings list:
             BM25(q, d) = Σ IDF(t) × (tf(t,d) × (k1+1)) / (tf(t,d) + k1 × (1 - b + b × |d|/avgdl))
       e. Accumulate scores across query terms per chunk.
       f. Return top-50 by BM25 score → Ranked List A.

     PATH B — Semantic (HNSW k-NN):
       a. Embed query string → 384-dim vector.
       b. Search HNSW graph:
          → Start at entry point, top layer.
          → At each layer: maintain a dynamic candidate list of size efSearch.
             Greedily explore neighbors, keep the closest efSearch candidates.
          → At layer 0: return top-K nearest neighbors (K=50).
       c. Return top-50 by cosine similarity → Ranked List B.

  3. Fuse with Reciprocal Rank Fusion:
     For each unique chunk_id across both lists:
       rrf_score = 0
       If present in List A at position r_a: rrf_score += 1 / (60 + r_a)
       If present in List B at position r_b: rrf_score += 1 / (60 + r_b)
     Sort all chunk_ids by rrf_score descending.

  4. Take top-N results (default 10).

  5. For each result, enrich:
     → Load chunk content from chunks.store.
     → Extract snippet: highlight matching keywords (bold in terminal).
     → Annotate signal source: [KW] keyword only, [SEM] semantic only, [HYBRID] both.
     → Include: file_path, line_range, language, chunk_type, rrf_score.

  6. Render to terminal:
     ┌─ [HYBRID] src/http/retry.rs:42-67 (function) ───────────
     │  pub async fn retry_with_backoff<F, T>(f: F, max: u32) -> Result<T>
     │  where F: Fn() -> Future<Output = Result<T>> {
     │      for attempt in 0..max {
     │          match f().await {
     │              Ok(v) => return Ok(v),
     │              Err(_) if attempt < max - 1 => {
     │                  sleep(backoff(attempt)).await;  // ← **retries** **HTTP** **requests**
     │  ...
     └─ score: 0.0312  (BM25: rank #3, HNSW: rank #1)
```

### Flow 4: Crash recovery

```
Needle process starts (after unclean shutdown).

  1. Locate latest snapshot files in ~/.needle/index/:
     → inverted.idx, hnsw.idx, chunks.store, embeddings.bin
     → Each has a snapshot_sequence_number in its header.

  2. Scan WAL directory for segment files newer than snapshot_sequence_number.

  3. For each WAL segment (in order):
     → Read entries: { operation, file_path, chunk_mutations[] }
     → Check commit marker:
        - Committed → replay: apply mutations to in-memory index structures.
        - Uncommitted (no commit marker) → discard (incomplete write).

  4. After replay, indexes are consistent with the last committed state.

  5. Re-verify watched directories:
     → Walk file tree, compare modification timestamps against chunks.store.
     → Any file modified after the last WAL entry → re-chunk and re-index.
     → This catches changes that happened while Needle was down.

  6. Resume normal file watching.
  7. Schedule compaction if WAL is large.
```

### Flow 5: HNSW graph construction (detailed internal flow)

```
Input: vector V with assigned layer L

  1. Sample L = floor(-ln(uniform_random()) × mL)
     where mL = 1 / ln(M)
     → Most nodes get L=0. Probability of L=k decreases exponentially.

  2. ep = current entry point (the node with highest layer)
     ep_layer = max layer in the graph

  3. Phase 1 — Descend from top to L+1 (greedy, no insertions):
     For layer = ep_layer down to L+1:
       → From ep, greedily walk to the nearest neighbor of V in this layer.
       → ep = the closest node found.
     (This positions us near V's neighborhood before we start connecting.)

  4. Phase 2 — Insert at layers L down to 0:
     For layer = L down to 0:
       a. Search: starting from ep, find efConstruction nearest candidates of V.
          → Maintain a min-heap (candidates) and max-heap (results).
          → BFS-like expansion: pop closest unvisited candidate, check its neighbors,
             add closer ones to both heaps.
          → Stop when closest candidate is farther than farthest result.

       b. Select neighbors (DIVERSITY HEURISTIC — the hard part):
          → Sort candidates by distance to V (ascending).
          → selected = []
          → For each candidate c:
               skip = false
               For each already-selected neighbor s:
                 If dist(c, s) < dist(c, V):
                   skip = true   // c is closer to an existing neighbor than to V
                   break          // → it's redundant, that neighbor already covers c's direction
               If not skip:
                 selected.push(c)
               If len(selected) == M:
                 break
          → This ensures selected neighbors are SPREAD OUT around V,
             not clustered in one direction. This is what gives HNSW its
             navigability and high recall.

       c. Connect: create bidirectional edges V ↔ each selected neighbor.
          → If any neighbor now has > M_max edges, prune its connections
             using the same diversity heuristic (keep the most diverse M).

       d. ep = closest node found in this layer's search (for next layer down).

  5. If L > ep_layer:
     → V becomes the new entry point.
     → (Rare event — most nodes are layer 0.)
```

### Flow 6: Benchmarking (`needle bench`)

```
User runs: needle bench

  1. Report index stats:
     → Total chunks, total files, index size on disk, model size.
     → Chunks by type: function / class / paragraph / section / config.
     → Chunks by language.

  2. HNSW recall benchmark:
     → Sample 1000 random query vectors from the index.
     → For each: run brute-force exact k-NN (k=10) and HNSW k-NN (k=10).
     → Compute recall@10 = |exact ∩ hnsw| / 10, averaged.
     → Report: mean recall, min recall, p5 recall.

  3. Query latency benchmark:
     → Sample 100 real query strings (from a curated test set or recent queries).
     → For each: time the full hybrid pipeline (BM25 + HNSW + RRF).
     → Report: p50, p95, p99 latency.
     → Break down: BM25 time, HNSW time, embedding time, fusion time.

  4. Indexing throughput benchmark:
     → Re-index a known corpus (or the current index).
     → Report: chunks/sec (chunking), embeddings/sec (ONNX), total wall time.

  5. Output as structured JSON + human-readable table.
```
