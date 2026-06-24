# Needle — Local-first Hybrid Search Engine

A fast, offline-only hybrid search engine that indexes your files, notes, and repos with instant keyword + semantic search under 5ms.

## Features

- **Hybrid Search**: Combine keyword (BM25) and semantic (HNSW) search via Reciprocal Rank Fusion
- **Local-First**: Runs entirely offline — no cloud, no telemetry, no API keys
- **Fast**: Sub-5ms query latency (p50) on 100k+ chunks
- **Incremental**: Watch directories for changes and update the index automatically
- **Crash Safe**: Write-ahead logging and atomic snapshots ensure data integrity
- **Code-Aware**: Tree-sitter based AST chunking treats functions, classes, and modules as units

## Project Structure

```
needle/
├── Cargo.toml                 # Rust dependencies and build config
├── src/
│   ├── main.rs                # CLI entry point
│   ├── lib.rs                 # Library root
│   ├── error.rs               # Error types
│   ├── config.rs              # Configuration management
│   ├── schema.rs              # Data structures (Chunk, etc)
│   ├── storage/               # Index persistence layer
│   ├── chunking/              # Code and prose chunking
│   │   ├── mod.rs
│   │   ├── code.rs            # Tree-sitter based code chunking
│   │   └── prose.rs           # Paragraph-based prose chunking
│   ├── indexing/              # BM25 and HNSW indexes
│   │   ├── mod.rs
│   │   ├── bm25.rs            # Inverted index with BM25 scoring
│   │   └── hnsw.rs            # HNSW graph index for semantic search
│   ├── query/                 # Query engine
│   │   ├── mod.rs
│   │   └── fusion.rs          # Reciprocal Rank Fusion
│   ├── embedding/             # ONNX Runtime embedding inference
│   ├── watcher/               # File system watcher
│   └── cli/                   # Command-line interface
│       ├── mod.rs
│       ├── init.rs            # `needle init` command
│       ├── search.rs          # `needle search` command
│       ├── status.rs          # `needle status` command
│       ├── reindex.rs         # `needle reindex` command
│       ├── config.rs          # `needle config` command
│       └── bench.rs           # `needle bench` command
├── tests/                     # Integration tests
├── benches/                   # Criterion benchmarks
│   ├── hnsw_bench.rs
│   ├── bm25_bench.rs
│   └── embedding_bench.rs
├── docs/                      # Project documentation
│   ├── 01_PRD.md              # Product requirements
│   ├── 02_TECH_STACK_AND_FLOWS.md
│   ├── 03_SCHEMA.md
│   └── 04_DESIGN.md
└── design/                    # Design artifacts
    ├── needle-web-ui/         # Web UI prototype
    └── README.md
```

## Quick Start

### Initialize Index

```bash
cargo run -- init ~/code ~/notes ~/docs
```

This will:
1. Scan all files in the directories
2. Chunk content (AST-aware for code, paragraph-based for prose)
3. Embed chunks using sentence-transformers (all-MiniLM-L6-v2)
4. Build BM25 inverted index for keyword search
5. Build HNSW graph for semantic search
6. Set up file watcher for incremental updates

### Search

```bash
cargo run -- search "function that retries HTTP requests"
```

Output will show top-10 results with:
- File path and line range
- Chunk type (function, section, paragraph, etc)
- Signal source: [HYBRID], [KW] (keyword only), [SEM] (semantic only)
- Score and snippet with highlighted matches

### Check Status

```bash
cargo run -- status
```

Shows index health, chunk count, disk usage, and HNSW recall metrics.

### Run Benchmarks

```bash
cargo run -- bench
```

Measures:
- Query latency (p50, p95, p99)
- HNSW recall@10 vs brute-force k-NN
- Indexing throughput
- Index size breakdown

## Architecture

### Core Components

**Chunking Layer**
- Code files: AST-aware tree-sitter chunking (functions, classes, modules as units)
- Prose files: Paragraph-based with sliding window overlap

**BM25 Inverted Index**
- Tokenization with Unicode normalization
- Porter stemming (optional)
- Postings lists with term frequencies
- Scoring via BM25 formula (k1=1.2, b=0.75 defaults)

**HNSW Vector Index**
- Multi-layer navigable small world graph
- Diversity heuristic for neighbor selection
- Brute-force k-NN search at query time
- Soft-delete with periodic compaction

**Query Fusion**
- Reciprocal Rank Fusion (RRF) combines BM25 and HNSW results
- Handles keyword-only, semantic-only, and hybrid matches

**Storage**
- Memory-mapped files for zero-copy index loading
- Write-ahead log for crash safety
- Periodic snapshots with WAL replay on recovery

**File Watching**
- Filesystem events trigger incremental re-indexing
- Content-hash based dedup skips unchanged chunks
- Background compaction rebuilds indexes periodically

## Development

### Run Tests

```bash
cargo test
```

### Run with Logging

```bash
RUST_LOG=needle=debug cargo run -- search "query"
```

### Build Release Binary

```bash
cargo build --release
```

### Run Benchmarks

```bash
cargo bench
```

## Design Files

See [design/README.md](design/README.md) for the web UI prototype and design handoff.

## Documentation

- [01_PRD.md](docs/01_PRD.md) — Complete product requirements
- [02_TECH_STACK_AND_FLOWS.md](docs/02_TECH_STACK_AND_FLOWS.md) — Architecture and flows
- [03_SCHEMA.md](docs/03_SCHEMA.md) — Data structures and storage layout
- [04_DESIGN.md](docs/04_DESIGN.md) — CLI and UI design spec

## Performance Targets

| Metric | Target |
|---|---|
| Query latency (p50) | < 5ms |
| Query latency (p99) | < 15ms |
| HNSW recall@10 | ≥ 95% vs exact k-NN |
| Cold start | < 500ms for 100k chunks |
| Incremental re-index | < 200ms per file change |
| Memory (100k chunks) | < 300MB RSS |
| Disk footprint | < 500MB |
| Crash recovery | < 2s |

## Dependencies

### Core
- `clap` — CLI argument parsing
- `tree-sitter` + language grammars — Code parsing
- `ort` — ONNX Runtime for embeddings
- `tokenizers` — HuggingFace tokenization
- `notify` — File system watcher
- `memmap2` — Memory-mapped file I/O
- `xxhash-rust` — Content hashing
- `rkyv` — Zero-copy serialization

### Utilities
- `rayon` — Parallelism
- `crossbeam` — Channels
- `tracing` — Structured logging
- `tokio` — Async runtime
- `serde` / `toml` — Configuration

## License

MIT
# NEEDLE
