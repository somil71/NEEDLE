# Needle — Full Architecture & Decision Record

_Detailed technical reference for the author. Covers every design decision, component, schema, and deployment option._

---

## 1. Problem Statement

Modern codebases are large, poorly documented, and hard to navigate — especially for new contributors or AI coding assistants. The alternatives:

| Tool | Problem |
|------|---------|
| GitHub code search | Keyword-only, requires internet, no graph |
| Sourcegraph | Cloud upload required, expensive |
| ctags / ripgrep | Regex only, no semantic understanding |
| Copilot / Cursor | No persistent memory of your specific codebase |
| Language servers (LSP) | Single-file scope, no cross-repo graph |

**Needle's solution**: a local-first binary that builds a hybrid search index + call graph from any codebase, serves a web UI, and exposes an MCP server — with zero cloud dependency.

---

## 2. Feature List (complete)

### Search
- [x] BM25 inverted index (keyword search, exact token matching)
- [x] HNSW vector index (semantic search, embedding similarity)
- [x] Reciprocal Rank Fusion (hybrid ranking)
- [x] Per-language filter (`--lang rust`, `--lang python`, etc.)
- [x] Limit parameter (top-N results)
- [x] Score explanation (BM25-only, semantic-only, hybrid badge)

### Indexing
- [x] Tree-sitter AST chunking (Rust, Python, TS/JS, Go, Java, C, C++)
- [x] Prose chunking (Markdown, plain text)
- [x] PDF text extraction + paragraph chunking
- [x] Sliding-window fallback for unknown file types
- [x] Content-hash based deduplication (skip unchanged files)
- [x] JSON persistence (no external DB for local mode)

### Graph
- [x] Definition extraction: function, method, class, struct, trait, module, enum
- [x] Endpoint detection: Axum `.route()`, Express `app.get/post/...`, FastAPI decorators
- [x] Call edge extraction with same-file disambiguation
- [x] Import edge extraction
- [x] Contains edge (module → member)
- [x] D3 force-directed interactive visualization (web UI)
- [x] Standalone D3 HTML export (`needle graph`)
- [x] Node detail panel (file path, line range, HTTP method, neighbors)
- [x] Filter by node kind (module/function/method/class/struct/endpoint)
- [x] Filter by edge type (calls/imports/contains)
- [x] Node search with opacity highlight
- [x] Zoom/pan, drag-to-pin, reset

### Analysis (needle report)
- [x] God nodes — degree centrality ranking
- [x] Community detection — label propagation on call/import edges
- [x] Surprise edges — cross-community call detection
- [x] Markdown output

### MCP Server (11 tools)
- [x] `search_code` — hybrid search
- [x] `find_callers` — reverse call lookup
- [x] `find_callees` — forward call lookup
- [x] `find_similar` — semantic similarity
- [x] `get_god_nodes` — top-N by degree
- [x] `get_endpoints` — all HTTP routes
- [x] `get_communities` — label propagation clusters
- [x] `get_surprises` — cross-community edges
- [x] `get_file_structure` — directory tree
- [x] `get_stats` — index summary
- [x] `explain` — LLM-based explanation (requires API key)

### Web UI
- [x] Single-file SPA (embedded at compile time via `include_str!`)
- [x] Search page (hybrid, with source code highlighting)
- [x] Ask / RAG page (LLM Q&A with source attribution)
- [x] Index dashboard (stats, language breakdown, file table, TODO tracker)
- [x] Graph page (full D3 visualization)
- [x] Setup page (platform-specific installation guides)
- [x] Download page
- [x] Settings page (model selection, API keys)
- [x] Dark / light mode
- [x] Live search demo widget on home page

### Cloud / Auth
- [x] GitHub OAuth login
- [x] Per-user API key generation + revocation
- [x] SQLite user database (rusqlite, bundled)
- [x] Session cookies (HMAC-SHA256 signed)
- [x] Multi-repo support (via GitHub API)
- [x] Docker image (two-stage, ~80MB runtime)
- [x] Railway deployment support

---

## 3. Tech Stack

### Core language
- **Rust 1.75+** — chosen for: zero-overhead memory-mapped indexes, no GC pauses during search, single binary distribution, safe concurrency

### Web framework
- **Axum 0.7** — async HTTP server, tower middleware, type-safe extractors
- **Tokio** — async runtime
- **Tower-http** — CORS, compression

### Parsing
- **tree-sitter 0.20** — language-agnostic AST parser
  - `tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-javascript`, `tree-sitter-typescript`, `tree-sitter-go`, `tree-sitter-java`, `tree-sitter-c`, `tree-sitter-cpp`
- **pdf-extract** — PDF text extraction (pdfium-based)

### Embeddings
- **Hash-projection (custom)** — 384-dimensional embeddings without ONNX/GPU
  - Input: token string
  - Method: FNV-1a hash per token → bucketized into 384 float dimensions, L2-normalized
  - Tradeoff: lower recall vs. trained sentence transformers, but no model download, no ONNX runtime, instant startup, deterministic
  - Future: pluggable embedding backends (ONNX, llamacpp)

### Indexing
- **Custom BM25** — hand-written inverted index, no external crate
  - Unicode tokenization (`unicode_segmentation`)
  - Term frequency + document frequency
  - BM25 formula with k1=1.2, b=0.75
- **Custom HNSW** — hand-written hierarchical navigable small world graph
  - M=16 (max neighbors per layer), ef_construction=200
  - Diversity heuristic for neighbor selection (avoids clustering)
  - Soft-delete with periodic compaction

### Storage
- **serde + serde_json** — index serialization
- **rusqlite (bundled)** — user/session DB in cloud mode
- Index files: plain JSON for debuggability (can be inspected with `jq`)

### Frontend (web UI)
- Single HTML file, no build step, no bundler
- **D3.js v7** (CDN) — force simulation, zoom, drag
- **marked.js** (CDN) — Markdown rendering for Ask output
- **highlight.js** (CDN) — syntax highlighting for code results

### CLI
- **clap 4** — argument parsing
- **colored** — terminal output formatting
- **indicatif** — progress bars during indexing

### Async / concurrency
- **rayon** — parallel indexing (file scanning, chunking, embedding)
- **tokio** — async web server
- **crossbeam** — channels for index pipeline

---

## 4. Data Schema

### Chunk

```rust
pub struct Chunk {
    pub id: u32,
    pub file_path: String,
    pub language: Language,
    pub content: String,
    pub line_start: u32,
    pub line_end: u32,
    pub chunk_type: ChunkType,   // Function, Class, Method, Section, Paragraph, ...
    pub name: Option<String>,    // extracted function/class name
    pub content_hash: u64,       // FNV hash for dedup
    pub embedding: Vec<f32>,     // 384-dim (stored separately in HNSW)
}
```

### GraphNode

```rust
pub struct GraphNode {
    pub id: u32,
    pub name: String,
    pub kind: NodeKind,    // Function | Method | Class | Struct | Trait | Module | Endpoint | Enum
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub detail: Option<String>,  // HTTP method for endpoints ("GET", "POST", ...)
}
```

### GraphEdge

```rust
pub struct GraphEdge {
    pub from: u32,   // source node ID
    pub to: u32,     // target node ID
    pub kind: EdgeKind,  // Calls | Imports | Contains
}
```

### Index files (disk layout)

```
~/.needle/index/
├── meta.json          # { version, embedding_model, indexed_at, source_dirs }
├── chunks.json        # { "0": {Chunk}, "1": {Chunk}, ... }
├── filemap.json       # { "src/main.rs": [0, 1, 5, 12, ...], ... }
├── bm25.json          # { term_freqs, doc_freqs, doc_lengths, avg_doc_len }
├── hnsw.json          # { layers: [[neighbors]], entry_point, vectors }
└── graph.json         # { nodes: [...], edges: [...], stats: {...} }
```

### User DB (cloud mode, SQLite)

```sql
CREATE TABLE users (
    id          INTEGER PRIMARY KEY,
    github_id   INTEGER UNIQUE NOT NULL,
    username    TEXT NOT NULL,
    email       TEXT,
    avatar_url  TEXT,
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE api_keys (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER REFERENCES users(id),
    key_hash    TEXT UNIQUE NOT NULL,  -- SHA-256 of raw key
    prefix      TEXT NOT NULL,         -- first 8 chars of raw key (for display)
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_used   DATETIME,
    revoked     BOOLEAN DEFAULT FALSE
);

CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,      -- SHA-256 of session token
    user_id     INTEGER REFERENCES users(id),
    expires_at  DATETIME NOT NULL
);
```

---

## 5. API Endpoints (served by `needle serve`)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/` | `serve_ui` | Serves embedded `ui.html` |
| GET | `/api/search` | `api_search` | Hybrid search |
| GET | `/api/graph` | `api_graph` | Full graph JSON |
| POST | `/api/open` | `api_open` | Open file in editor |
| POST | `/api/ask` | `api_ask` | RAG + LLM Q&A |
| POST | `/api/similar` | `api_similar` | Similar chunk lookup |
| GET | `/api/status` | `api_status_handler` | Index stats |
| GET | `/api/files` | `api_files` | File list |
| GET | `/api/todos` | `api_todos` | TODO/FIXME scan |
| GET | `/auth/github` | `api_auth_github` | OAuth redirect |
| GET | `/auth/callback` | `api_auth_callback` | OAuth callback |
| GET | `/auth/logout` | `api_auth_logout` | Session clear |
| GET | `/api/me` | `api_me` | Current user |
| GET | `/api/repos` | `api_repos` | User's repos |
| GET | `/api/github/repos` | `api_github_repos_handler` | GitHub repo list |
| POST | `/api/repos/connect` | `api_repo_connect` | Connect a repo |
| POST | `/api/keys/validate` | `api_validate_key` | Validate API key |
| POST | `/api/keys/regenerate` | `api_regenerate_key` | Generate new key |
| POST | `/api/keys/revoke` | `api_revoke_key` | Revoke API key |

---

## 6. Graph Extraction — Detailed

Graph extraction runs in multiple passes over every source file:

### Pass 1 — Definition extraction
Using tree-sitter queries, extract all named symbols:

```rust
// Rust example queries
(function_item name: (identifier) @name) -> NodeKind::Function
(impl_item trait: _ type: (type_identifier) @name) -> NodeKind::Trait  
(struct_item name: (type_identifier) @name) -> NodeKind::Struct
```

Each extracted symbol becomes a `GraphNode` with `id`, `name`, `kind`, `file_path`, `line_start`, `line_end`.

### Pass 1.5 — Endpoint detection (Axum/Express/FastAPI)
Regex scan for route registration patterns:

```rust
// Axum: .route("/path", get(handler))
// Express: app.get('/path', handler)  
// FastAPI: @app.get('/path')
```

When a matching route is found, the handler function name is looked up in the node map and its `kind` is promoted to `NodeKind::Endpoint` with `detail = "GET"` (or POST, PUT, DELETE, etc.).

### Pass 2 — Call/import extraction
For each file, extract all function calls and imports. Then resolve them:

1. Look up the call name in the node map
2. If multiple matches exist, prefer the node in the **same file** (disambiguation to avoid false edges for common names like `run`, `new`, `clone`)
3. Add a `Calls` edge if resolved, `Imports` edge for use/import statements

### Community detection (label propagation)
```
label[node] = node.id  // start: each node is its own community
repeat until stable:
  for each (src, dst) in call_edges:
    label[dst] = label[src]  // propagate label downstream
```

Converges in ~10 iterations for typical codebases. Output: `Map<NodeId, CommunityId>`.

### God nodes
```
degree[node] = count(edges where from==node or to==node)
top_god_nodes = degree.most_common(N)
```

### Surprise edges
```
for each calls edge (src, dst):
  if community[src] != community[dst]:
    surprises.append((src, dst))
```

---

## 7. BM25 Index — Implementation Details

### Tokenization
```
input -> unicode normalization (NFC) -> lowercase -> split on non-alphanumeric -> filter stop words -> tokens
```

### Index structure
```rust
struct BM25Index {
    // term -> list of (doc_id, term_frequency)
    postings: HashMap<String, Vec<(u32, u32)>>,
    // doc_id -> document length (token count)
    doc_lengths: HashMap<u32, u32>,
    // number of docs containing each term
    doc_freqs: HashMap<String, u32>,
    // total number of docs
    num_docs: u32,
    // average document length
    avg_doc_len: f32,
}
```

### Scoring
```
score(q, d) = Σ_t idf(t) * (tf(t,d) * (k1+1)) / (tf(t,d) + k1*(1 - b + b*|d|/avgdl))

idf(t) = ln((N - df(t) + 0.5) / (df(t) + 0.5) + 1)

k1 = 1.2, b = 0.75 (BM25 standard defaults)
```

---

## 8. HNSW Index — Implementation Details

### Graph structure
- L layers, each layer is a `HashMap<NodeId, Vec<NodeId>>`
- Layer 0 (bottom) has all nodes; each higher layer has `1/ln(M)` fraction of nodes
- M=16 neighbors per node in layer 0, M/2=8 in upper layers

### Insertion
```
1. Assign layer l = floor(-ln(random) * 1/ln(M))
2. If l > max_layer: new entry point
3. For each layer from max_layer down to l+1: greedy walk to closest node (ef=1)
4. For each layer from l down to 0: collect ef_construction candidates, apply diversity heuristic, add M neighbors
```

### Search
```
1. Enter at entry_point on top layer
2. Greedy descent to layer 1: move to closest neighbor at each step  
3. At layer 0: beam search with ef=max(ef_search, k) candidates
4. Return top-k results
```

### Diversity heuristic
When selecting M neighbors, use "simple heuristic":
```
for candidate in sorted(candidates, by_distance):
  if distance(candidate, result_set) > distance(candidate, query):
    add to result set
    if |result_set| == M: stop
```

Avoids clustering all neighbors around the same dense region.

---

## 9. Embedding Model

**Current**: hash-projection-384 (custom, no external model)

```
embedding(text) = normalize(project(tokenize(text)))

tokenize: split on whitespace + punctuation, lowercase
project: for each token t: hash = fnv1a(t); for each dim d: add sign(hash >> d & 1) / sqrt(num_tokens)
normalize: L2 normalization to unit vector
```

**Tradeoffs:**
- Pro: zero-dependency, instant startup, deterministic, sub-1ms per chunk
- Con: no semantic generalization (synonyms won't match), lower recall than sentence-transformers

**Future**: pluggable backend interface for:
- ONNX Runtime (all-MiniLM-L6-v2, nomic-embed-text)
- llama.cpp (local embedding models)
- API-based (OpenAI text-embedding-3-small)

---

## 10. Deployment Options

### Option A: Local binary (offline)

```bash
cargo build --release
./target/release/needle init ~/code
./target/release/needle serve
```

Index stored at `~/.needle/index/`. No network traffic.

### Option B: Docker (self-hosted)

```bash
docker build -t needle .
docker run -p 8080:8080 -v needle_data:/data needle
```

Dockerfile uses a 2-stage build: Rust 1.88 bookworm builder → Debian bookworm-slim runtime (~80MB). SQLite user DB is persisted in `/data` mount.

### Option C: Railway

```bash
railway login && railway init && railway up
```

Set env vars: `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `SESSION_SECRET`, `BASE_URL`, `PORT=8080`. Mount a 1GB volume at `/data`.

### Option D: Render

- Runtime: Docker
- Root directory: `/`
- Start command: `needle serve`
- Persistent disk: mount at `/data`

### Option E: Fly.io

```toml
# fly.toml
app = "needle"
[build]
  dockerfile = "Dockerfile"
[[services]]
  http_checks = []
  internal_port = 8080
  [[services.ports]]
    port = 443
    handlers = ["tls", "http"]
[mounts]
  source = "needle_data"
  destination = "/data"
```

---

## 11. Key Engineering Decisions

### Decision 1: Single-file SPA embedded in binary

The entire web UI is a single `ui.html` file embedded via `include_str!()` at compile time. This means:
- Zero-dependency distribution: one binary serves everything
- No CDN risk for the app itself (only D3/highlight.js/marked are CDN-loaded for file size)
- Downside: every UI change requires a rebuild

### Decision 2: Hash-projection embeddings over ONNX

Avoids a 23MB ONNX Runtime dependency and model download step. The hash-projection approach gives ~80% of the utility for zero overhead. Semantic search is good enough for "describe the intent" queries; exact name queries fall through to BM25.

### Decision 3: JSON storage over SQLite for the index

The search index (chunks, BM25, HNSW, graph) uses plain JSON files:
- Human-readable, debuggable with `jq`
- No migration complexity
- Acceptable for 100k-chunk codebases (JSON load time ~200ms)
- Future: binary serialization with `rkyv` or `bincode` for >500k chunks

SQLite is only used for the user/auth tables in cloud mode (structured relational data with concurrent writes).

### Decision 4: Label propagation over Louvain for communities

Label propagation is O(E) and converges in ~10 passes. Louvain is more accurate but O(E log V) and harder to implement correctly. For the purpose of "show developers which modules cluster together," label propagation is sufficient and understandable.

### Decision 5: Same-file disambiguation for call resolution

When a call target matches multiple nodes (e.g., `run` exists in 15 files), prefer the match in the same file as the caller. This eliminates >80% of false cross-file call edges for common short names, at the cost of missing some valid cross-file calls for identically-named functions.

### Decision 6: Axum as the web server (over actix-web)

Axum has cleaner tower/middleware integration, better type-safe routing, and more active ecosystem. actix-web was slightly faster in benchmarks but Needle is not CPU-bound on the serving side.

---

## 12. Known Limitations & Future Work

| Area | Current Limitation | Planned Fix |
|------|-------------------|-------------|
| Embeddings | Hash-projection (low recall) | ONNX/llama.cpp pluggable backend |
| Index format | JSON (slow for >500k chunks) | rkyv binary serialization |
| Call resolution | Same-file heuristic only | Type-inference-based resolution |
| Languages | 8 languages | Add Ruby, PHP, Swift, Kotlin |
| Incremental index | Full reindex on `reindex` | File-watcher based incremental |
| Multi-index | One index per machine | Per-project index switching |
| VS Code extension | Stub (empty) | Full sidebar + inline search |
| PDF | Text extraction only | Table extraction, image captions |

---

## 13. Performance Benchmarks (on Needle's own codebase)

_Index: 364 nodes, 548 edges, 361 chunks, 35 files_

| Operation | Time |
|-----------|------|
| Full reindex | ~1.2s |
| BM25 query (p50) | ~1ms |
| HNSW query (p50) | ~2ms |
| Hybrid query (p50) | ~3ms |
| Graph render (D3) | ~180ms first paint |
| Label propagation (220 clusters) | ~4ms |

---

_Last updated: 2026-06-25_
