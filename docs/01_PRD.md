# Needle — Product Requirements Document

## One-liner

A local-first hybrid search engine that indexes your files, notes, and repos and gives you instant keyword + semantic search — entirely offline, entirely yours.

---

## Problem

Developers and knowledge workers accumulate thousands of files — code repos, markdown notes, docs, configs, journals — spread across directories with no unified way to search them. The existing options are all broken in different ways:

- **grep / ripgrep**: fast keyword match, zero semantic understanding. Searching "the function that retries failed HTTP requests" returns nothing unless those exact words appear.
- **OS-level search (Spotlight, Windows Search)**: slow, shallow indexing, no code awareness, and zero semantic capability.
- **Cloud search (Notion, Google)**: requires uploading your files to someone else's server. Privacy-hostile. Unusable offline.
- **Vector DB wrappers**: semantic-only, miss exact identifier matches. Searching `parseConfigFromYAML` returns vaguely related results instead of the exact function.
- **GitHub code search**: only works on GitHub-hosted repos. Doesn't touch your notes, docs, or local-only code.

No tool does **hybrid** (keyword AND semantic) search, locally, across both code and prose, with incremental indexing that keeps up as you work.

---

## Target users

**Primary**: developers who work across multiple repos and keep local notes/docs.

**Secondary**: researchers, writers, and knowledge workers with large local file collections who want "search my brain" without a cloud dependency.

**Anti-user**: someone who works entirely in Google Docs / Notion and has no local files worth indexing.

---

## Core value proposition

Point Needle at your directories. Within minutes, every file is indexed. Type a query — an exact function name, a vague description of something you half-remember, or a natural-language question — and get ranked results in under 5ms. It works offline, it's free, it never phones home, and it gets smarter results than any single-strategy search by fusing keyword and semantic signals.

---

## Product principles

1. **Speed is the feature.** Sub-5ms query latency or it's not shipping. Users don't "run a search" — they think and results appear.
2. **Offline-only is a feature, not a limitation.** No cloud, no API keys, no telemetry. Privacy by architecture.
3. **Incremental by default.** The first index takes minutes; after that, only changed files are re-processed. The index survives restarts and crashes without corruption.
4. **Hybrid or nothing.** Keyword-only and semantic-only both fail predictable cases. The whole point is fusing them.
5. **Code is a first-class citizen.** AST-aware chunking, not line splitting. A function is a unit. An import block is a unit. A class is a unit.

---

## Functional requirements

### FR-1: File watching and ingestion

- Watch configured directories recursively for file changes (create, modify, delete, rename/move).
- Debounce rapid writes (editors save multiple times per keystroke).
- Support file types: `.rs`, `.py`, `.ts`, `.js`, `.go`, `.java`, `.c`, `.cpp`, `.md`, `.txt`, `.toml`, `.yaml`, `.json`, `.sh`, `.dockerfile`. Extensible via config.
- Ignore patterns: `.git`, `node_modules`, `target/`, `__pycache__`, `.env`, binary files. Configurable.
- Content-hash each chunk to skip re-embedding unchanged content on re-index.

### FR-2: Chunking

- **Code files**: AST-aware chunking via tree-sitter. Split on function/method/class/module boundaries. Each chunk is a semantically complete unit with its signature and docstring.
- **Prose files (md, txt)**: Structure-aware chunking. Split on headings, paragraphs, and logical sections. Sliding window with configurable overlap (default 2 sentences) to preserve cross-boundary context.
- **Config/data files (yaml, json, toml)**: Top-level key blocks as chunks.
- Each chunk stores: content, file path, byte offset, line range, language, chunk type (function / class / paragraph / section / config-block), and a content hash.

### FR-3: Inverted index (keyword search)

- Tokenize and normalize chunks: Unicode-aware lowercasing, ASCII folding, optional stemming (Porter).
- Build postings lists: for each term → list of (chunk_id, term_frequency) pairs.
- Store per-chunk metadata: total token count (for length normalization).
- Score with BM25 (k1=1.2, b=0.75 defaults, configurable).
- Stretch: delta-encode doc IDs + variable-byte compress postings for compact on-disk size.

### FR-4: HNSW vector index (semantic search)

- Embed each chunk using a local sentence-transformer model (all-MiniLM-L6-v2 default, 384-dim; code-specific model for code chunks as stretch goal).
- Run inference via ONNX Runtime — no Python, no GPU required.
- Build a multi-layer HNSW graph from scratch:
  - Exponentially distributed layer assignment per node.
  - Greedy layer-by-layer search from entry point.
  - Neighbor selection with the diversity heuristic (prune candidates closer to existing neighbors than to the query).
  - Parameters: M=16, efConstruction=200, efSearch=50 (configurable).
- Soft-delete with tombstones; periodic compaction rebuilds the graph.
- Persistence: memory-mapped file, zero-copy reload on startup.

### FR-5: Hybrid query engine

- On each query:
  1. Tokenize + normalize the query string → BM25 lookup → ranked list A.
  2. Embed the query string → HNSW k-NN search → ranked list B.
  3. Fuse with Reciprocal Rank Fusion (k=60): `score(d) = Σ 1/(k + rank_i(d))` across both lists.
- Return top-N results (default 10) with: file path, line range, snippet with highlighted matches, score, and which signal(s) contributed (keyword, semantic, or both).
- Query latency budget: < 5ms p50, < 15ms p99 over 100k chunks.

### FR-6: Incremental indexing and crash safety

- On file change: re-chunk only the changed file, diff chunks by content hash, update only changed/new/deleted chunks in both indexes.
- Write-ahead log (WAL) for index mutations: write intent → apply → commit. On crash recovery, replay uncommitted WAL entries or discard.
- Atomic index snapshots: periodic background flush of the full index state to disk. On startup, load latest snapshot + replay WAL tail.

### FR-7: Interface

- **Primary**: CLI with sub-commands:
  - `needle init <dirs...>` — configure watched directories, run first index.
  - `needle search <query>` — hybrid search, print ranked results with snippets.
  - `needle status` — index health: total chunks, last update, watch status.
  - `needle reindex` — force full re-index.
  - `needle config` — view/edit settings (watched dirs, ignore patterns, BM25 params, HNSW params).
- **Secondary (stretch)**: lightweight local web UI — a single search bar that streams results as you type, with file previews. Served on localhost, no external dependencies.
- **Tertiary (stretch)**: editor integrations — VS Code extension that calls Needle as a backend for workspace-wide semantic search.

### FR-8: Observability and benchmarking

- Built-in `needle bench` command:
  - Reports: index size on disk, total chunks, embedding throughput (chunks/sec), query latency (p50/p95/p99), HNSW recall@10 vs brute-force exact k-NN.
- Structured logging (JSON) for all indexing and query operations.
- Expose internal metrics: chunks indexed, queries served, cache hit rate, watch events processed.

---

## Non-functional requirements

| Requirement | Target |
|---|---|
| Query latency (p50) | < 5ms |
| Query latency (p99) | < 15ms |
| HNSW recall@10 | ≥ 95% vs exact brute-force |
| Cold start (load index from disk) | < 500ms for 100k chunks |
| Incremental re-index (single file change) | < 200ms |
| Memory usage (idle, 100k chunks indexed) | < 300MB RSS |
| Disk footprint (index, 100k chunks) | < 500MB |
| Crash recovery | < 2s (snapshot + WAL replay) |

---

## Scope boundaries

### In scope (v1)

- CLI interface
- Python, TypeScript, JavaScript, Rust, Go, Markdown, plain text file support
- Tree-sitter chunking for supported languages
- Hand-rolled inverted index with BM25
- Hand-rolled HNSW with diversity heuristic
- Incremental indexing with filesystem watcher
- WAL-based crash safety
- Memory-mapped persistence
- RRF-based hybrid fusion
- Built-in benchmarking

### Out of scope (v1)

- GUI / web UI (stretch goal)
- PDF / DOCX parsing
- Multi-machine / distributed indexing
- GPU-accelerated embedding
- LLM-based answer synthesis
- Real-time collaboration
- Cloud sync

---

## Success metrics

1. **Recall quality**: ≥ 95% recall@10 on HNSW vs exact, measured on SIFT1M benchmark AND on a self-curated test set of real queries over the user's own files.
2. **Latency**: sub-5ms p50 queries consistently on a corpus of 100k+ chunks.
3. **Daily usability**: the developer (you) actually uses Needle as their primary search tool for at least 30 days.
4. **Portfolio signal**: a reviewer reading the README and skimming the HNSW implementation says "wait, an undergrad wrote that?"

---

## Milestones (high level)

| Phase | Deliverable | Duration |
|---|---|---|
| 0 — Scaffold | Project structure, CI, test harness, brute-force baseline | 1 week |
| 1 — HNSW | Working HNSW with diversity heuristic, persistence, recall benchmarks | 3 weeks |
| 2 — Inverted index | BM25 index, tokenizer, postings lists | 1.5 weeks |
| 3 — Chunking | Tree-sitter code chunking + prose chunking | 1.5 weeks |
| 4 — Fusion + CLI | RRF fusion, CLI interface, end-to-end search | 1 week |
| 5 — Incremental + crash safety | File watcher, WAL, atomic snapshots | 2 weeks |
| 6 — Polish + bench | Benchmarking suite, README, perf tuning | 1 week |

**Total: ~11 weeks**
