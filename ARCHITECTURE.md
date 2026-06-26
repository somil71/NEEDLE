# Needle — Architecture & Design Reference

_Full technical reference covering every component, data flow, schema, and design decision._

---

## 1. Problem Statement

| Tool | Problem |
|------|---------|
| GitHub code search | Keyword-only, requires internet, no graph |
| Sourcegraph | Cloud upload required, expensive |
| ctags / ripgrep | Regex only, no semantic understanding |
| Copilot / Cursor | No persistent memory of your specific codebase |
| Language servers (LSP) | Single-file scope, no cross-repo graph |

**Needle's solution**: a local-first binary that builds a hybrid search index + call graph from any codebase, serves a web UI, exposes an MCP server, and ships as a native desktop app — with zero cloud dependency.

---

## 2. Feature List

### Search
- [x] BM25 inverted index (keyword search, exact token matching)
- [x] HNSW vector index (semantic search, embedding similarity)
- [x] Reciprocal Rank Fusion (hybrid ranking, k=60)
- [x] Per-language filter (`--lang rust`, `--lang python`, etc.)
- [x] Limit parameter (top-N results)
- [x] Score signal badges (BM25-only, semantic-only, hybrid)

### Indexing
- [x] Tree-sitter AST chunking (Rust, Python, TS/JS, Go, Java, C, C++)
- [x] Prose chunking (Markdown, plain text, TOML, YAML, JSON, Shell, Dockerfile)
- [x] PDF text extraction + paragraph chunking
- [x] Sliding-window fallback for unknown file types
- [x] Content-hash deduplication via xxHash3 (skip unchanged files)
- [x] JSON persistence with WAL for crash-safe incremental updates
- [x] notify-based file watcher (live reindex on file changes)

### Graph
- [x] Definition extraction: function, method, class, struct, trait, module, enum
- [x] Endpoint detection: Axum `.route()`, Express `app.get/post`, FastAPI decorators
- [x] Call edge extraction with same-file disambiguation
- [x] Import edge extraction
- [x] Contains edge (module → member)
- [x] D3 force-directed interactive visualization (web UI)
- [x] Standalone D3 HTML export (`needle graph`)
- [x] Node detail panel (file path, line range, HTTP method, neighbors)
- [x] Filter by node kind (module/function/method/class/struct/endpoint)
- [x] Filter by edge type (calls/imports/contains)
- [x] Zoom/pan, drag-to-pin, node search with highlight

### Analysis (`needle report`)
- [x] God nodes — degree centrality ranking
- [x] Community detection — label propagation on call/import edges
- [x] Surprise edges — cross-community call detection
- [x] Markdown output

### MCP Server (11 tools)
- [x] `search_code` — hybrid search
- [x] `find_callers` — reverse call lookup
- [x] `find_callees` — forward call lookup
- [x] `find_similar` — semantic similarity (k-NN)
- [x] `get_god_nodes` — top-N by degree
- [x] `get_endpoints` — all HTTP routes
- [x] `get_communities` — label propagation clusters
- [x] `get_surprises` — cross-community edges
- [x] `get_file_structure` — directory tree
- [x] `get_stats` — index summary
- [x] `explain` — LLM-based explanation (requires API key)

### CLI Commands
- [x] `needle init` — build index for one or more directories
- [x] `needle serve` — web UI + REST API (default when no command given)
- [x] `needle search` — terminal search (`--limit`, `--lang`, `--compact`, `--all`)
- [x] `needle reindex` — full re-index of all watched directories
- [x] `needle watch` — watch directories and reindex on file changes
- [x] `needle status` — index health and stats
- [x] `needle graph` — export standalone D3 HTML visualization
- [x] `needle report` — generate Markdown architecture report
- [x] `needle bench` — performance benchmarks
- [x] `needle config` — view/edit configuration
- [x] `needle mcp` — start MCP server over stdio

### Desktop App
- [x] Tauri v2 wrapper — native window, no browser required
- [x] System tray icon (Open / Quit)
- [x] Close minimizes to tray, server keeps running
- [x] NSIS installer (`Needle_0.1.0_x64-setup.exe`)

### Cloud Mode
- [x] GitHub OAuth (`/auth/github`, `/auth/callback`)
- [x] Per-user sessions (cookie-based, SQLite)
- [x] Per-user API key storage (Anthropic, OpenAI, Groq)
- [x] Multi-repo support (isolated index namespaces)
- [x] Docker deployment (Railway, Render, any Docker host)

---

## 3. Tech Stack

### Core Language: Rust

The entire engine is Rust. The project's hard parts — HNSW inner loops, concurrent indexing, zero-copy index access, memory-mapped storage — are exactly where Rust pays off.

### Dependencies

| Layer | Crate | Why |
|---|---|---|
| CLI | `clap` | Derive-based arg parsing, subcommands |
| AST parsing | `tree-sitter` + grammars | Code-aware chunking on function/class boundaries |
| Embeddings | Hash-projection (built-in) | 384-dim offline embeddings, no model download, no ONNX |
| File watching | `notify` | Cross-platform fs events (inotify / FSEvents / ReadDirectoryChanges) |
| Memory mapping | `memmap2` | Zero-copy index loading, OS page cache handles hot/cold |
| Hashing | `xxhash-rust` (xxh3) | Content dedup (~30 GB/s) |
| Serialization | `serde` + `serde_json` + `bincode` | Index persistence |
| Web server | `axum` | Async HTTP server for the web UI + REST API |
| HTTP client | `reqwest` | Ollama/LLM API calls |
| Database | `rusqlite` (bundled) | Sessions, per-user API keys (cloud mode) |
| Async | `tokio` (full) | Async runtime |
| Logging | `tracing` + `tracing-subscriber` | Structured logs, env-filter |
| Parallelism | `rayon` + `crossbeam` | Parallel chunking, watcher-to-indexer channels |
| PDF | `pdf-extract` | Text extraction |
| Progress | `indicatif` | Terminal progress bars |
| Terminal | `colored` | Colored search output |
| Desktop | `tauri v2` | Native window wrapping the web UI |

### What is NOT a dependency

- No vector database (Qdrant, Pinecone, Weaviate) — HNSW is hand-rolled
- No search engine library (Tantivy, Meilisearch) — BM25 inverted index is hand-rolled
- No ONNX Runtime, no model download — embeddings use hash-projection
- No GPU required — everything runs on CPU

---

## 4. Repository Layout

```
NEEDLE/
├── src/
│   ├── main.rs              # CLI entry point (clap subcommands)
│   ├── lib.rs               # Library crate root (pub mods)
│   ├── schema.rs            # Chunk, Language, NodeKind, Edge types
│   ├── config.rs            # needle.toml + env config
│   ├── error.rs             # Error / Result types (thiserror)
│   ├── chunking/
│   │   ├── code.rs          # Tree-sitter AST chunking
│   │   └── prose.rs         # Markdown, PDF, plain-text chunking
│   ├── indexing/
│   │   ├── bm25.rs          # Inverted index, BM25 scoring
│   │   └── hnsw.rs          # HNSW approximate nearest-neighbour graph
│   ├── query/
│   │   ├── mod.rs           # QueryEngine
│   │   └── fusion.rs        # Reciprocal Rank Fusion
│   ├── embedding/
│   │   └── mod.rs           # Hash-projection 384-dim embeddings (offline)
│   ├── graph/
│   │   └── mod.rs           # CodeGraph: extraction, communities, god nodes
│   ├── storage/
│   │   └── mod.rs           # JSON persistence, WAL
│   ├── server/              # Axum HTTP server + all REST routes
│   ├── watcher/             # notify-based file watcher (live reindex)
│   ├── llm/                 # LLM routing (Anthropic → OpenAI → Groq → Ollama)
│   ├── cli/                 # One file per subcommand
│   │   ├── init.rs
│   │   ├── serve.rs         # Main server entry, opens browser
│   │   ├── search.rs
│   │   ├── mcp.rs           # stdio MCP server
│   │   ├── graph.rs         # D3 HTML export
│   │   ├── report.rs        # Markdown analysis
│   │   ├── bench.rs
│   │   ├── watch.rs
│   │   ├── status.rs
│   │   ├── reindex.rs
│   │   └── config.rs
│   └── assets/
│       └── ui.html          # Web UI — single-file SPA, embedded at compile time
│
├── src-tauri/               # Tauri v2 desktop app
│   ├── src/lib.rs           # Spawns needle binary, opens WebviewWindow
│   ├── src/main.rs          # Entry point (no console window in release)
│   ├── tauri.conf.json      # App identity, bundle config, icon paths
│   ├── icons/               # App icons (32x32, 128x128, ico, icns)
│   ├── frontend/            # Placeholder dist (window actually loads localhost:7700)
│   ├── capabilities/        # Tauri v2 permission set
│   └── WebView2Loader.dll   # Bundled for GNU toolchain (not needed with MSVC)
│
├── needle-vscode/           # VS Code extension
│   ├── src/extension.ts     # Activates, spawns needle serve, opens WebviewPanel
│   └── package.json         # Extension manifest
│
├── benches/                 # criterion benchmarks (HNSW, BM25, embedding)
├── Cargo.toml               # Workspace root — members: [".","src-tauri"]
├── Dockerfile               # Two-stage Rust build → Debian slim
└── docs/                    # Additional detailed design docs
```

---

## 5. Deployment Modes

| Mode | Entry Point | Notes |
|------|-------------|-------|
| Desktop app | `Needle_0.1.0_x64-setup.exe` | Tauri wraps needle server in native window |
| Web UI | `needle serve` | Axum on localhost:7700, opens browser |
| CLI | `needle search`, `needle init`, … | No server, terminal only |
| MCP server | `needle mcp` | stdio, connects to Claude Code / Cursor / Windsurf |
| VS Code | `.vsix` extension | WebviewPanel embedding the web UI |
| Cloud / Docker | `docker run needle` | GitHub OAuth, multi-user, Railway/Render |

---

## 6. Indexing Pipeline

```
Files on disk
     │
     ▼
[needle init / Watcher]
  notify events + recursive directory walk
     │
     ▼
[Chunker]  ───────────────────────────────────────────
  code.rs:    tree-sitter visitor → function/class/struct chunks
              each chunk = {file, line_start, line_end, kind, content}
  prose.rs:   heading-aware paragraph splits (Markdown)
              pdf-extract → paragraph chunks (PDF)
              sliding-window fallback (plain text / config)
     │
     ▼
[Deduplication]
  xxHash3(content) → compare with stored hash for this file
  unchanged chunks → skip entirely
     │
     ▼
[Embedder]
  hash-projection: tokenize → n-grams → stable hash → 384-dim vector → L2-normalize
  no model file, no network, deterministic
     │
     ├──► [BM25 Indexer]          inverted index, per-term postings + IDF
     ├──► [HNSW Indexer]          ANN graph, bidirectional edges
     └──► [CodeGraph]             call + import + contains edges
              │
              ▼
         [Storage] → ~/.needle/ (JSON + WAL)
```

---

## 7. Query Pipeline

```
User query string
       │
       ├──► [BM25]
       │      tokenize → normalize → lookup postings
       │      score = Σ IDF(t) × tf(t,d)×(k1+1) / (tf(t,d) + k1×(1 - b + b×|d|/avgdl))
       │      k1=1.2, b=0.75
       │      → Ranked List A (top 50)
       │
       └──► [HNSW k-NN]
              embed query → 384-dim vector
              greedy layer descent from entry point, ef_search candidate pool
              → Ranked List B (top 50 by cosine similarity)
                     │
                     ▼
            [Reciprocal Rank Fusion]
            For each unique chunk across both lists:
              rrf_score += 1 / (60 + rank_in_list)
            Sort descending → top-N results
                     │
                     ▼
            Enrich: load content, snippet, signal badge [KW] / [SEM] / [HYBRID]
```

---

## 8. Embedding Strategy

No model download. Hash-projection embeddings:

1. Tokenize input into character n-grams (n=2,3)
2. Each n-gram → stable xxHash3 → maps to one of 384 dimensions
3. Accumulate: `vec[hash % 384] += 1.0`
4. L2-normalize the resulting vector

**Tradeoff**: lower semantic recall than a transformer model, but zero latency, zero disk space, works fully offline. Sufficient for code — identifiers are already semantically distinct tokens.

---

## 9. CodeGraph

**Nodes**: functions, methods, classes, structs, traits, modules, enums, HTTP endpoints

**Edges**:
| Type | Meaning |
|------|---------|
| `calls` | Function A calls function B |
| `imports` | Module A imports module B |
| `contains` | Module/class contains member |

**Extraction**: tree-sitter visitor per language. Explicit patterns for call expressions, method calls, and import statements.

**Self-loop suppression**: recursive calls filtered. Common stdlib names (`len`, `print`, `fmt`, `new`, `clone`, …) suppressed via deny-list to prevent false cross-module edges.

**Same-file disambiguation**: when resolving a call target, prefer definitions in the same file before falling back to imported names.

**Communities**: label-propagation algorithm on the call+import subgraph. Each node starts as its own community; iteratively adopts the most common community among its neighbors until convergence.

**God nodes**: degree centrality = in-degree + out-degree. Top-N surfaced in `needle report` and MCP `get_god_nodes`.

**Surprise edges**: call edges that cross community boundaries. Indicates unexpected coupling between architectural modules.

---

## 10. HNSW Construction (detailed)

```
Input: vector V to insert

  1. Sample insertion layer L:
     L = floor(-ln(uniform_random()) × mL)
     mL = 1 / ln(M)          (default M=16)
     → Most nodes get L=0. Probability of L=k decreases exponentially.

  2. Phase 1 — Descend to layer L+1 (greedy, no insertions):
     From entry point (highest layer), greedily walk toward V at each layer.
     → Positions entry point near V's neighborhood.

  3. Phase 2 — Insert at layers L..=0:
     For each layer from L down to 0:
       a. Search: find efConstruction nearest candidates of V (default ef=200).
          BFS-like: maintain min-heap of candidates, max-heap of results.
          Expand closest unvisited candidates until no improvement.

       b. Select M neighbors (DIVERSITY HEURISTIC):
          Sort candidates by dist(c, V) ascending.
          selected = []
          For each candidate c:
            If dist(c, s) < dist(c, V) for ANY already-selected s → skip
              (c is redundant: s already covers that direction of space)
            Else → add to selected
            Stop at M.
          → Ensures selected neighbors are spread around V, not clustered.

       c. Create bidirectional edges V ↔ each selected neighbor.
          If any neighbor now has > Mmax edges → prune using same heuristic.

  4. If L > current max layer → V becomes new entry point.
```

**Search (query time)**:
```
Start at entry point, top layer.
For each layer top..=0:
  Maintain candidate set of size ef_search.
  Greedily expand: pop closest, check neighbors, add closer ones.
  Pass closest node as entry to next layer.
At layer 0: return top-K by distance.
```

---

## 11. Data Schema

### Chunk (atomic unit of indexing)

```
Chunk {
    id:             u64,            # monotonic ChunkId
    file_path:      String,         # relative path, e.g. "src/http/retry.rs"
    byte_offset:    u64,
    byte_length:    u32,
    line_start:     u32,            # 1-indexed
    line_end:       u32,            # inclusive
    language:       Language,       # Rust | Python | TypeScript | Go | Java | Cpp | Markdown | Pdf | …
    chunk_type:     ChunkType,      # Function | Class | Method | Module | Struct |
                                    # Paragraph | Section | ConfigBlock | Import
    content_hash:   u64,            # xxh3 for dedup
    token_count:    u32,            # BM25 length normalization
    content:        String,         # raw text
    embedding:      [f32; 384],     # hash-projection vector
    status:         Active | Tombstoned,
}
```

### Edge (CodeGraph)

```
Edge {
    source:     NodeId,             # (file_path, symbol_name)
    target:     NodeId,
    kind:       Calls | Imports | Contains,
    call_site:  Option<u32>,        # line number
}
```

### Storage layout

```
~/.needle/
├── config.toml          # watched dirs, chunk params, server port, LLM config
├── index.json           # serialized BM25 inverted index + chunk store
├── hnsw.json            # HNSW graph (adjacency lists)
├── graph.json           # CodeGraph nodes + edges
└── wal/                 # write-ahead log segments
    └── segment_N.wal
```

### WAL entry

```
WalEntry {
    sequence:       u64,
    entry_type:     AddChunks | DeleteChunks | UpdatePath | Checkpoint,
    file_path:      Option<String>,
    added_chunks:   Vec<ChunkId>,
    deleted_chunks: Vec<ChunkId>,
    checksum:       u32,            # CRC32
    committed:      bool,           # written AFTER successful apply
}

Write protocol:
  1. Append entry with committed=false
  2. Apply mutations to in-memory indexes
  3. Set committed=true
  4. fsync

Recovery: replay committed entries, discard uncommitted.
```

---

## 12. Desktop App (Tauri v2)

```
Needle_0.1.0_x64-setup.exe (NSIS installer)
    │
    installs → C:\Program Files\Needle\
                   needle-app.exe       # Tauri wrapper
                   needle.exe           # Bundled as resource
                   WebView2Loader.dll   # Bundled (GNU toolchain requirement)

needle-app.exe on launch:
    │
    ├── std::process::Command::new("needle.exe")
    │     .args(["serve", "--port", "7700", "--no-open"])
    │     .spawn()
    │
    ├── Poll http://127.0.0.1:7700/ every 200ms (max 50 attempts = 10s)
    │
    └── WebviewWindowBuilder::new("main", WebviewUrl::External("http://localhost:7700"))
          .title("Needle").inner_size(1280, 820).build()

System tray: Open → show/focus window | Quit → kill needle.exe + exit
WindowEvent::CloseRequested → hide window (don't kill server)
```

**GNU toolchain note**: `WebView2Loader.dll` needs to be in the same directory as the exe for Windows DLL loader to find it. MSVC builds statically link it; GNU builds require the DLL. Bundled via Tauri resources → placed in `$INSTDIR` by NSIS.

**Dev workflow**:
```bash
cargo build --release --package needle      # build the server binary first
cargo-tauri dev                             # opens native window (uses release binary)
cargo-tauri build                           # produces NSIS + MSI installers
```

---

## 13. MCP Server

Runs over stdio (`needle mcp`). Implements the Model Context Protocol spec.

Connect from any MCP client:
```json
{
  "mcpServers": {
    "needle": { "command": "needle", "args": ["mcp"] }
  }
}
```

| Tool | Input | Output |
|------|-------|--------|
| `search_code` | `query`, `limit`, `lang?` | Ranked chunks with snippets |
| `find_callers` | `symbol` | All call sites into this symbol |
| `find_callees` | `symbol` | All symbols called by this symbol |
| `find_similar` | `query`, `limit` | k-NN by embedding distance |
| `get_god_nodes` | `limit` | Top-N by degree centrality |
| `get_endpoints` | — | All HTTP routes with method + path |
| `get_communities` | — | Cluster labels for all nodes |
| `get_surprises` | `limit` | Top-N cross-community edges |
| `get_file_structure` | `path?` | Directory tree from index |
| `get_stats` | — | Chunk count, node count, edge count, index size |
| `explain` | `symbol` | LLM explanation (routes to configured provider) |

---

## 14. LLM Routing

`needle explain` and the `/api/ask` web UI endpoint route to the first available provider:

```
1. Anthropic (ANTHROPIC_API_KEY)   → claude-haiku-4-5-20251001
2. OpenAI    (OPENAI_API_KEY)      → gpt-4o-mini
3. Groq      (GROQ_API_KEY)        → llama3-8b-8192
4. Ollama    (OLLAMA_URL or default http://localhost:11434) → configured model
```

The query includes retrieved code chunks as context (RAG). No code leaves the machine unless the user configures a cloud LLM provider.

---

## 15. Cloud Mode (Docker)

Activated when `RAILWAY_ENVIRONMENT` or `DOCKER` env var is set.

```bash
docker build -t needle .
docker run -p 8080:8080 \
  -e GITHUB_CLIENT_ID=... \
  -e GITHUB_CLIENT_SECRET=... \
  -e SESSION_SECRET=... \
  -v needle_data:/data \
  needle
```

Adds on top of local mode:
- GitHub OAuth login (`/auth/github` → `/auth/callback`)
- Per-user SQLite sessions table
- Per-user encrypted API key storage
- Index namespacing per user (`/data/<user_id>/`)

Dockerfile: two-stage build (Rust builder image → Debian slim runtime). Port 8080.

---

## 16. Benchmarking (`needle bench`)

```
1. Index stats
   → Total chunks, files, languages, index size, chunk type distribution

2. HNSW recall@10
   → Sample 1000 random query vectors from index
   → For each: run brute-force exact k-NN and HNSW k-NN (k=10)
   → recall@10 = |exact ∩ hnsw| / 10, averaged
   → Report: mean, min, p5

3. Query latency
   → 100 representative queries through full hybrid pipeline
   → Report: p50, p95, p99
   → Breakdown: BM25 time / embedding time / HNSW time / RRF time

4. Indexing throughput
   → Re-index current corpus
   → Report: chunks/sec (chunking), embeddings/sec, total wall time
```

---

## 17. Supported Languages

| Language | Tree-sitter Grammar | Chunking | Call Graph |
|----------|---------------------|----------|------------|
| Rust | `tree-sitter-rust` | Functions, structs, impls, traits | ✓ |
| Python | `tree-sitter-python` | Functions, classes, methods | ✓ |
| TypeScript | `tree-sitter-typescript` | Functions, classes, arrow fns | ✓ |
| JavaScript | `tree-sitter-typescript` (JS grammar) | Functions, classes, arrow fns | ✓ |
| Go | `tree-sitter-go` | Functions, types, interfaces | ✓ |
| Java | `tree-sitter-java` | Classes, methods | ✓ |
| C / C++ | `tree-sitter-cpp` | Functions, structs | ✓ |
| Markdown | — (section-aware) | Heading + paragraph splits | — |
| PDF | `pdf-extract` | Text extraction + paragraph chunks | — |
| Other | — (sliding window) | Fixed-size overlapping windows | — |

---

## 18. Configuration

`~/.needle/needle.toml` (auto-created on first `needle init`):

```toml
[index]
directories = ["/home/user/code/my-project"]
chunk_size = 512          # max tokens per chunk
chunk_overlap = 64        # overlap between adjacent chunks (sliding window)

[server]
port = 7700
host = "127.0.0.1"        # 0.0.0.0 for Docker/cloud

[llm]
provider = "anthropic"    # anthropic | openai | groq | ollama
model = "claude-haiku-4-5-20251001"

[graph]
enable_call_graph = true
enable_import_graph = true
```

Environment variable overrides:

| Variable | Effect |
|----------|--------|
| `NEEDLE_PORT` | Override server port |
| `ANTHROPIC_API_KEY` | Enable Anthropic LLM |
| `OPENAI_API_KEY` | Enable OpenAI LLM |
| `GROQ_API_KEY` | Enable Groq LLM |
| `OLLAMA_URL` | Override Ollama endpoint |
| `GITHUB_CLIENT_ID` | Enable GitHub OAuth (cloud mode) |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth secret |
| `SESSION_SECRET` | Cookie signing key |
| `RAILWAY_ENVIRONMENT` | Auto-enable cloud mode |
