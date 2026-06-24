# Needle Project Setup

Complete Rust project structure for the Needle hybrid search engine, organized according to the PRD.

## What's Created

### Core Files
- `Cargo.toml` — Dependencies and build configuration
- `src/lib.rs` — Library root with module exports
- `src/main.rs` — CLI entry point
- `src/error.rs` — Error handling

### Modules

#### `src/schema.rs`
Data structures following the schema document:
- `Chunk` — atomic indexing unit
- `Language` enum — supported file types
- `ChunkType` enum — semantic categories
- `SearchResult` — query results
- `IndexMetadata` — index state

#### `src/config.rs`
Configuration management:
- `Config` struct with defaults
- Load/save from TOML
- BM25 and HNSW parameters

#### `src/chunking/`
Splitting content into chunks:
- `mod.rs` — trait and dispatcher
- `code.rs` — AST-aware tree-sitter chunking (placeholder)
- `prose.rs` — paragraph-based markdown/text chunking

#### `src/indexing/`
Two search indexes:
- `mod.rs` — unified Index struct
- `bm25.rs` — inverted index with BM25 scoring
- `hnsw.rs` — HNSW graph for semantic k-NN

#### `src/query/`
Query execution:
- `mod.rs` — QueryEngine struct
- `fusion.rs` — Reciprocal Rank Fusion combining results

#### `src/storage/`
Index persistence:
- Memory-mapped file I/O
- Directory structure management
- Load/save paths

#### `src/embedding/`
ONNX Runtime integration:
- `EmbeddingModel` for sentence-transformers
- Text → 384-dim vectors
- Batch embedding

#### `src/watcher/`
Filesystem watching for incremental indexing:
- `FileWatcher` for directory events
- Event types: Created, Modified, Deleted, Renamed
- Debouncing (ready for implementation)

#### `src/cli/`
Command-line interface:
- `mod.rs` — command definitions
- `init.rs` — `needle init <dirs...>` initialization
- `search.rs` — `needle search <query>` hybrid search
- `status.rs` — `needle status` index inspection
- `reindex.rs` — `needle reindex` full re-index
- `config.rs` — `needle config [view|edit]` settings
- `bench.rs` — `needle bench` performance measurement

### Tests & Benchmarks
- `tests/` — Integration tests (ready for tests)
- `benches/hnsw_bench.rs` — HNSW insertion and search
- `benches/bm25_bench.rs` — BM25 scoring
- `benches/embedding_bench.rs` — Embedding throughput

### Documentation
- `docs/01_PRD.md` — Product requirements document
- `docs/02_TECH_STACK_AND_FLOWS.md` — Architecture and flows
- `docs/03_SCHEMA.md` — Data structures and storage
- `docs/04_DESIGN.md` — CLI and UI design
- `design/needle-web-ui/` — Web UI prototype (HTML/CSS/JS)
- `README.md` — Project overview and quick start

## Architecture Highlights

### Chunking
- **Code files** (`.rs`, `.py`, `.ts`, `.js`, `.go`, `.java`, `.c`, `.cpp`): AST-aware via tree-sitter
- **Prose files** (`.md`, `.txt`): Structure-aware paragraph-based
- **Config files** (`.toml`, `.yaml`, `.json`): Top-level key blocks

### Indexing
**BM25 (Keyword Search)**
- Tokenization with Unicode normalization
- Postings lists with term frequencies
- BM25 formula scoring (k1=1.2, b=0.75)

**HNSW (Semantic Search)**
- Multi-layer navigable small world graph
- Diversity heuristic for neighbor selection
- Brute-force k-NN at query time (can be upgraded to greedy traversal)

### Query Fusion
Reciprocal Rank Fusion (k=60) combines:
- Top-50 BM25 results
- Top-50 HNSW results
- Returns hybrid score per chunk

### Storage
- Memory-mapped files for O(1) access
- Write-ahead log (WAL) for crash safety
- Content-hash dedup for incremental indexing
- Periodic compaction rebuilds indexes

## Next Steps

### 1. Install Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Build the Project
```bash
cd d:\NEEDLE
cargo build
```

### 3. Run a Command
```bash
cargo run -- status
```

### 4. Implement Core Logic
Priority order:
1. **Chunking** (chunking/code.rs, prose.rs) — tree-sitter integration
2. **Embedding** (embedding/mod.rs) — ONNX model loading
3. **Indexing** (indexing/bm25.rs, hnsw.rs) — upgrade placeholders to full implementations
4. **Query** (query/mod.rs) — tie together BM25 + HNSW + RRF
5. **CLI** (cli/*.rs) — wire up init, search, status commands

### 5. Add Tests
Create integration tests in `tests/` that verify:
- File watching and chunking
- Index building and persistence
- Search quality and latency

### 6. Benchmarks
Run with:
```bash
cargo bench
```

## Design Philosophy

- **Speed is the feature** — target <5ms p50 queries
- **Offline-only** — no cloud, no dependencies
- **Incremental** — watch for changes, update efficiently
- **Hybrid** — keyword AND semantic, never one alone
- **Code-aware** — functions/classes are units, not lines

## Key Files to Start With

1. **Understanding the scope**: Read [docs/01_PRD.md](docs/01_PRD.md)
2. **Understanding the flows**: Read [docs/02_TECH_STACK_AND_FLOWS.md](docs/02_TECH_STACK_AND_FLOWS.md)
3. **Understanding the data**: Read [docs/03_SCHEMA.md](docs/03_SCHEMA.md)
4. **Implementing chunking**: Start with `src/chunking/code.rs`
5. **Implementing indexing**: `src/indexing/bm25.rs` and `hnsw.rs`

---

Complete project scaffolding ready. All modules are organized, typed, and documented. Begin implementation in order of priority.
