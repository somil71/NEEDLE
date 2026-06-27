# Needle — Deployment & Usage Guide

> **Audience:** Developers, DevOps, AI tool users. This is the authoritative internal reference for every way Needle can be installed, run, and integrated. **Do not publish this file to GitHub.**

---

## Table of Contents

1. [Tech Stack Reference](#1-tech-stack-reference)
2. [Binary Download (Pre-built)](#2-binary-download-pre-built)
3. [Build from Source](#3-build-from-source)
4. [Local CLI Usage](#4-local-cli-usage)
5. [Web UI (Browser)](#5-web-ui-browser)
6. [VS Code Extension](#6-vs-code-extension)
7. [MCP Server (AI Tools)](#7-mcp-server-ai-tools)
8. [Docker (Self-hosted)](#8-docker-self-hosted)
9. [Cloud Deployment — Railway](#9-cloud-deployment--railway)
10. [Cloud Deployment — Render](#10-cloud-deployment--render)
11. [Cloud Deployment — Fly.io](#11-cloud-deployment--flyio)
12. [Environment Variables Reference](#12-environment-variables-reference)
13. [File & Directory Paths](#13-file--directory-paths)
14. [Ports Reference](#14-ports-reference)
15. [Quick Reference Card](#15-quick-reference-card)

---

## 1. Tech Stack Reference

### Core Dependencies

| Layer | Technology | Version | Notes |
|-------|-----------|---------|-------|
| **Language** | Rust | 1.75+ | Zero-overhead binary, no GC |
| **Web server** | Axum | 0.7 | Async, tower middleware |
| **Async runtime** | Tokio | 1.35 | Full feature set |
| **HTTP middleware** | Tower-http | 0.5 | CORS, compression |
| **CLI parsing** | clap | 4.4 | derive feature flag |
| **AST parsing** | tree-sitter | 0.20 | Language-agnostic grammar system |
| **Parallelism** | rayon | 1.7 | Parallel indexing pipeline |
| **Channels** | crossbeam | 0.8 | Index pipeline coordination |
| **Serialization** | serde + serde_json | 1.0 | Index persistence (JSON) |
| **User DB (cloud)** | rusqlite (bundled) | 0.31 | SQLite, no external DB needed |
| **Sessions** | tower-cookies | 0.10 | HMAC-SHA256 signed cookies |
| **HTTP client** | reqwest | 0.12 | LLM API + GitHub API calls |
| **PDF parsing** | pdf-extract | 0.7 | Pure Rust, pdfium-based |
| **Directory walk** | walkdir | 2.4 | Recursive file scanning |
| **File watching** | notify | 6.1 | inotify / FSEvents / ReadDirectoryChanges |
| **Memory map** | memmap2 | 0.9 | O(1) index file access |
| **Hashing** | xxhash-rust | 0.8 | Content-hash dedup (xxh3, xxh64) |
| **Progress bars** | indicatif | 0.17 | Terminal UX during indexing |
| **Colored output** | colored | 2.0 | Terminal output formatting |
| **Unicode** | unicode-segmentation + unicode-normalization | 1.10 / 0.1 | Tokenization |
| **Logging** | tracing + tracing-subscriber | 0.1 / 0.3 | Structured logging |
| **Error handling** | anyhow + thiserror | 1.0 | Flexible + typed errors |
| **Time** | chrono | 0.4 | Timestamps |
| **Paths** | dirs | 5.0 | Platform home/config dirs |
| **RNG** | rand | 0.8 | HNSW layer sampling |

### Tree-sitter Language Grammars

| Grammar | Version |
|---------|---------|
| tree-sitter-rust | 0.20 |
| tree-sitter-python | 0.20 |
| tree-sitter-typescript | 0.20 |
| tree-sitter-go | 0.20 |
| tree-sitter-java | 0.20 |
| tree-sitter-cpp | 0.22 |

### Frontend Stack

| Technology | Source | Purpose |
|-----------|--------|---------|
| HTML/CSS/JS | Compiled into binary | Single-file SPA (`src/assets/ui.html`) |
| D3.js v7 | CDN | Force-directed graph visualization |
| marked.js | CDN | Markdown rendering (Ask/RAG output) |
| highlight.js | CDN | Syntax highlighting for code results |

### VS Code Extension Stack

| Technology | Version | Purpose |
|-----------|---------|---------|
| TypeScript | 5.3 | Extension source language |
| @types/vscode | ^1.85.0 | VS Code API types |
| @vscode/vsce | ^2.22.0 | VSIX packaging |

### Benchmarks

| Tool | Version | Purpose |
|------|---------|---------|
| criterion | 0.5 | Benchmark harness with HTML reports |
| tempfile | 3.8 | Temporary directories in tests |

---

### Supported Languages for Indexing

| Language | Extensions | Chunking Strategy | Call Graph |
|----------|-----------|------------------|-----------|
| Rust | `.rs` | AST: functions, structs, impls, traits | Full |
| Python | `.py` | AST: functions, classes, methods | Full |
| TypeScript | `.ts`, `.tsx` | AST: functions, classes, arrow fns | Full |
| JavaScript | `.js`, `.jsx` | AST: functions, classes, arrow fns | Full |
| Go | `.go` | AST: functions, types, interfaces | Full |
| Java | `.java` | AST: classes, methods | Full |
| C | `.c`, `.h` | AST: functions, structs | Full |
| C++ | `.cpp`, `.hpp` | AST: functions, structs | Full |
| Markdown | `.md` | Section-by-section prose chunks | — |
| PDF | `.pdf` | Text extraction + paragraph chunks | — |
| Unknown | any | Sliding-window fallback (256 tokens, 64 overlap) | — |

---

## 2. Binary Download (Pre-built)

The fastest way to get started. **No Rust toolchain required.**

### Windows x64

A pre-built Windows binary ships in the repository root:

```
d:\NEEDLE\needle-windows-x64.exe
```

**Installation steps:**

```powershell
# Option A: Add to system PATH permanently (run as admin)
Copy-Item .\needle-windows-x64.exe C:\Windows\System32\needle.exe

# Option B: Add the project folder to PATH (user scope)
$env:PATH += ";d:\NEEDLE"

# Option C: Use directly by full path
d:\NEEDLE\needle-windows-x64.exe init C:\code\my-project

# Verify
needle --version
```

**First use:**

```powershell
# Index a project
needle init C:\code\my-project

# Start the web UI
needle serve
# Open: http://localhost:7700
```

### Linux / macOS

Build from source (see §3) or use Docker (see §8).

> **Planned:** GitHub Releases with CI-built binaries for Linux x64, macOS arm64/x64 via `cargo-dist`.

---

## 3. Build from Source

### Prerequisites

| Tool | Min Version | Install |
|------|-------------|---------|
| Rust | 1.75 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Git | any | OS package manager |
| C compiler | any | gcc / clang / MSVC — required for rusqlite bundled |

### Clone & Build

```bash
# Clone the repository
git clone https://github.com/somil71/needle
cd needle

# Debug build (fast compile, unoptimized — for development)
cargo build
# → target/debug/needle(.exe)

# Release build (fully optimized — use in production)
cargo build --release
# → target/release/needle(.exe)
```

### Install system-wide

```bash
# Linux / macOS
sudo cp target/release/needle /usr/local/bin/

# Windows (PowerShell, administrator)
Copy-Item target\release\needle.exe C:\Windows\System32\needle.exe

# Via cargo (installs to ~/.cargo/bin/needle)
cargo install --path .
```

### Build flags & commands

```bash
# Release binary only (skip benches)
cargo build --release --bin needle

# Run with debug logging
RUST_LOG=needle=debug cargo run -- search "query"

# Run all tests
cargo test

# Run benchmarks (generates HTML reports in target/criterion/)
cargo bench

# Watch and auto-rebuild (requires cargo-watch)
cargo watch -x "build"
```

### Cargo release profile

```toml
[profile.release]
opt-level = 3        # Maximum CPU optimization
lto = true           # Link-time optimization (cross-crate inlining)
codegen-units = 1    # Single codegen unit for best optimization
```

---

## 4. Local CLI Usage

```
needle <COMMAND> [OPTIONS]
```

### Command Reference

| Command | Description |
|---------|-------------|
| `needle init <dirs...>` | Build search index for one or more directories |
| `needle serve` | Start web UI + full REST API server |
| `needle search <query>` | Search from the terminal (no browser) |
| `needle reindex` | Rebuild index after code changes |
| `needle graph` | Export standalone D3 graph as HTML |
| `needle report` | Print architectural analysis in Markdown |
| `needle mcp` | Start MCP stdio server for AI tools |
| `needle bench` | Run performance benchmarks |
| `needle status` | Show index health and stats |

### `needle init`

```bash
# Single directory
needle init ~/code/my-app

# Multiple directories (merged into one index)
needle init ~/code/backend ~/code/frontend ~/code/shared

# Windows paths
needle init C:\Users\you\code\my-project
```

### `needle serve`

```bash
# Default (port 7700)
needle serve

# Custom port
needle serve --port 8888

# Access the web UI
# → http://localhost:7700
```

### `needle search`

```bash
# Basic search
needle search "authentication middleware"

# Filter by language
needle search "error handling" --lang rust

# Return top 20 results
needle search "database query" --limit 20

# Exact keyword only (no semantic)
needle search "fn main" --mode bm25

# Semantic only
needle search "user login flow" --mode semantic
```

### `needle graph`

```bash
# Export to file
needle graph > my-project-graph.html

# Open directly in browser (Windows)
needle graph > %TEMP%\graph.html && start %TEMP%\graph.html
```

### `needle report`

```bash
# Print to terminal
needle report

# Save to file
needle report > ARCHITECTURE_REPORT.md
```

---

## 5. Web UI (Browser)

When `needle serve` is running, the full web application is at:

```
http://localhost:7700          (local default)
http://localhost:8080          (Docker / cloud)
```

### Pages

| Page | URL Fragment | Description |
|------|-------------|-------------|
| Home | `/#home` | Landing page with live search demo widget |
| Search | `/#search` | Hybrid search with syntax-highlighted results |
| Ask / RAG | `/#ask` | LLM Q&A with source file attribution |
| Graph | `/#graph` | Interactive D3 force-directed call graph |
| Index | `/#index` | Stats, language breakdown, file table, TODO tracker |
| Setup | `/#setup` | Platform-specific installation guide (live rendered) |
| Download | `/#download` | Binary download links |
| Settings | `/#settings` | LLM model selection, API key management |

### REST API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Serves embedded `ui.html` |
| `GET` | `/api/search` | Hybrid search (query, limit, lang params) |
| `GET` | `/api/graph` | Full graph JSON |
| `POST` | `/api/open` | Open file in local editor |
| `POST` | `/api/ask` | RAG + LLM Q&A |
| `POST` | `/api/similar` | Similar chunk lookup |
| `GET` | `/api/status` | Index stats + health check |
| `GET` | `/api/files` | File list |
| `GET` | `/api/todos` | TODO/FIXME scan |
| `GET` | `/auth/github` | GitHub OAuth redirect |
| `GET` | `/auth/callback` | GitHub OAuth callback |
| `GET` | `/auth/logout` | Session clear |
| `GET` | `/api/me` | Current user info |
| `GET` | `/api/repos` | Connected repositories |
| `GET` | `/api/github/repos` | GitHub repo list (cloud mode) |
| `POST` | `/api/repos/connect` | Connect a repo (cloud mode) |
| `POST` | `/api/keys/validate` | Validate API key |
| `POST` | `/api/keys/regenerate` | Generate new API key |
| `POST` | `/api/keys/revoke` | Revoke API key |

### UI Features

- **Dark / light mode** — toggle in top bar
- **Syntax highlighting** — via highlight.js
- **D3 graph controls:** zoom, pan, drag-to-pin, reset view
- **Node detail panel:** file path, line range, HTTP method, neighbor list
- **Graph filters:** by node kind (function/class/struct/endpoint/module/enum), by edge type (calls/imports/contains)
- **Node search:** highlights matching nodes with opacity
- **LLM routing:** Anthropic → OpenAI → Groq → Ollama (cascade fallback)

### UI implementation note

The entire UI is a **single `ui.html` file** embedded at compile time via `include_str!()`. The binary serves everything — no static file serving, no CDN for the app shell. Only D3.js, highlight.js, and marked.js are CDN-loaded at runtime.

**Source file:** `src/assets/ui.html`

---

## 6. VS Code Extension

The VS Code extension (`needle-search`) lives in `needle-vscode/` and adds a Needle sidebar panel inside the editor.

### Install from VSIX (no marketplace required)

**Method 1 — VS Code UI:**
1. `Ctrl+Shift+P` → `Extensions: Install from VSIX...`
2. Navigate to `needle-vscode/` and select the latest `.vsix`

**Method 2 — CLI:**
```bash
code --install-extension needle-vscode/needle-search-0.5.0.vsix
```

**Available VSIX versions:**

| File | Notes |
|------|-------|
| `needle-vscode/needle-search-0.1.0.vsix` | Initial release |
| `needle-vscode/needle-search-0.2.0.vsix` | |
| `needle-vscode/needle-search-0.3.0.vsix` | |
| `needle-vscode/needle-search-0.4.0.vsix` | |
| `needle-vscode/needle-search-0.5.0.vsix` | **Current — install this one** |

### Build the extension yourself

```bash
cd needle-vscode
npm install
npm run compile     # TypeScript → JavaScript (output: out/)
npm run package     # Creates needle-search-X.Y.Z.vsix
```

### Configure the extension

In VS Code Settings (`Ctrl+,`) search for **Needle**, or add to `settings.json`:

```json
{
  "needle.serverUrl": "http://localhost:7700"
}
```

> The extension connects to a running `needle serve` instance. Start `needle serve` before using the sidebar.

### Extension capabilities

| Feature | How to trigger |
|---------|---------------|
| **Search sidebar** | Activity Bar → Needle icon (needle icon in left bar) |
| **Show Knowledge Graph** | `Ctrl+Shift+P` → `Needle: Show Knowledge Graph` |
| **Open full UI in browser** | `Ctrl+Shift+P` → `Needle: Open in Browser` |

**Engine requirement:** VS Code `^1.85.0`  
**Extension source:** `needle-vscode/src/`  
**Extension manifest:** `needle-vscode/package.json`

---

## 7. MCP Server (AI Tools)

Needle exposes an **11-tool MCP server** over stdio, giving AI agents structured access to your indexed codebase.

### Start the MCP server

```bash
needle mcp
# Runs on stdio — launched and managed by the AI client
```

### Available MCP Tools

| Tool | Input | Description |
|------|-------|-------------|
| `search_code` | `query`, `limit`, `lang` | Hybrid BM25 + semantic search |
| `find_callers` | `function_name` | Who calls a given function? |
| `find_callees` | `function_name` | What does a function call? |
| `find_similar` | `chunk_id` or `text` | Semantically similar code chunks |
| `get_god_nodes` | `limit` | Top-N highest-degree symbols |
| `get_endpoints` | — | All detected HTTP routes (Axum/Express/FastAPI) |
| `get_communities` | — | Label-propagation community clusters |
| `get_surprises` | — | Cross-community call edges |
| `get_file_structure` | `path` | Directory/module tree |
| `get_stats` | — | Index summary (chunks, nodes, edges, languages) |
| `explain` | `name` | LLM-based explanation of a symbol (requires API key) |

---

### Connect to Claude Code

**One-line setup:**
```bash
claude mcp add needle needle mcp
```

**Manual config** — `~/.claude/claude_desktop_config.json`:
```json
{
  "mcpServers": {
    "needle": {
      "command": "needle",
      "args": ["mcp"]
    }
  }
}
```

---

### Connect to Cursor

Create `.cursor/mcp.json` at project root:

```json
{
  "mcpServers": {
    "needle": {
      "command": "needle",
      "args": ["mcp"]
    }
  }
}
```

---

### Connect to Windsurf

Create `.windsurf/mcp.json` at project root:

```json
{
  "mcpServers": {
    "needle": {
      "command": "needle",
      "args": ["mcp"]
    }
  }
}
```

---

### Connect to VS Code Copilot

Add to `.vscode/settings.json`:

```json
{
  "github.copilot.chat.mcp.servers": {
    "needle": {
      "command": "needle",
      "args": ["mcp"]
    }
  }
}
```

---

### Connect to any MCP-compatible client

The generic config (MCP spec stdio transport):

```json
{
  "mcpServers": {
    "needle": {
      "command": "needle",
      "args": ["mcp"]
    }
  }
}
```

> **Note:** `needle` must be on `PATH`, or use the full binary path in `"command"`:  
> Windows: `"command": "d:\\\\NEEDLE\\\\needle-windows-x64.exe"`  
> Linux/macOS: `"command": "/usr/local/bin/needle"`

---

## 8. Docker (Self-hosted)

Docker mode enables GitHub OAuth, per-user API keys, multi-repo support, and session management.

### Quick start

```bash
# Build the image
docker build -t needle .

# Run with all required environment variables
docker run -p 8080:8080 \
  -e GITHUB_CLIENT_ID=your_client_id \
  -e GITHUB_CLIENT_SECRET=your_client_secret \
  -e SESSION_SECRET=at_least_32_random_characters_here \
  -e BASE_URL=http://localhost:8080 \
  -v needle_data:/data \
  needle
```

Then open: `http://localhost:8080`

### Dockerfile — Two-stage build

```
Stage 1 — Builder (rust:1.88-bookworm):
  WORKDIR /app
  apt-get: pkg-config libssl-dev build-essential
  → Dependency cache layer (manifests only, dummy src)
  → cargo build --release (with real src)
  Output: /app/target/release/needle

Stage 2 — Runtime (debian:bookworm-slim):
  WORKDIR /app
  apt-get: ca-certificates libssl3 git
  COPY needle binary → /usr/local/bin/needle
  RUN mkdir -p /data
  ENV DATA_DIR=/data
  EXPOSE 8080
  CMD ["needle", "serve"]

Final image size: ~80MB
```

### Persistent volume

The `/data` volume stores:
- `index/` — Server-side indexes (if remote repos are connected)

User/session/repo state lives in **Postgres (Neon)**, not on this volume — see
`DATABASE_URL` below. This matters on hosts without persistent disks (e.g.
Render's free tier): without a database that survives restarts, every
spin-down used to wipe out signed-in users and their `gh_token`, silently
stranding connected repos in "queued" forever. Moving that state to Neon
fixes this regardless of whether `/data` is persistent.

If `/data` is also not persistent, server-side indexes for connected repos
will simply be re-cloned and re-indexed by the background indexer on next
boot — slower, but not silently broken.

### Docker environment variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes (cloud) | — | Primary Postgres connection string — Neon (users, sessions, repos) |
| `DATABASE_URL_FALLBACK` | No | — | Backup Postgres connection string — Supabase. Used automatically if `DATABASE_URL` is unreachable (e.g. paused on a free-tier quota) |
| `GITHUB_CLIENT_ID` | Yes (cloud) | — | GitHub OAuth App client ID |
| `GITHUB_CLIENT_SECRET` | Yes (cloud) | — | GitHub OAuth App client secret |
| `SESSION_SECRET` | Yes (cloud) | — | HMAC key for session cookies (32+ chars) |
| `BASE_URL` | Yes (cloud) | — | Full public URL (e.g. `https://needle.yourdomain.com`) |
| `PORT` | No | `8080` | HTTP listen port |
| `DATA_DIR` | No | `/data` | Persistent storage directory (search indexes only) |

### Neon Postgres setup

1. Create a free project at [neon.tech](https://neon.tech).
2. Copy the connection string from the dashboard (it includes `?sslmode=require`).
3. Set it as `DATABASE_URL` on whichever host you deploy to (Render, Railway, Fly.io, Docker, …).
4. On first connection, Needle auto-creates the `users`, `sessions`, and `user_repos` tables — no manual migration needed.

This replaces SQLite for all user/session/repo state. It also fixes hosts
without persistent disks (e.g. Render's free tier): previously, every
restart wiped `gh_token`s and stranded connected repos in "queued" forever.

### Supabase fallback (optional but recommended)

Both Neon and Supabase are plain Postgres, so the same schema and queries
work against either with zero code changes. Needle tries `DATABASE_URL`
first; if that connection fails — most commonly because a free-tier project
got paused after exceeding its **monthly compute-hour quota** — it
automatically retries `DATABASE_URL_FALLBACK` instead of going down.

1. Create a free project at [supabase.com](https://supabase.com).
2. Project Settings → Database → Connection string → copy the URI (use the
   "Transaction" pooler string if offered; append `?sslmode=require` if it's
   not already there).
3. Set it as `DATABASE_URL_FALLBACK` alongside `DATABASE_URL`.

Once a backend connects successfully it's used for the rest of that
process's lifetime — it does not switch back to the primary mid-run even if
the primary recovers. Restart the service to retry `DATABASE_URL` first.

Note this is a fallback for *outages*, not a guard against quota exhaustion
itself: if traffic regularly burns through Neon's free-tier hours, it will
likely burn through Supabase's free tier too on the same timeline once
failed over. The background indexer's poll interval (6 minutes, see below)
and the connection pool's short idle timeout are tuned specifically to let
compute autosuspend between polls and minimize how much quota gets used in
the first place.

---

## 9. Cloud Deployment — Railway

### One-command deploy

```bash
railway login
railway init       # Links to Railway project
railway up         # Builds and deploys
```

### `railway.toml` (already in repo root)

```toml
[build]
builder = "dockerfile"
dockerfilePath = "Dockerfile"

[deploy]
startCommand = "needle serve"
healthcheckPath = "/api/status"
healthcheckTimeout = 30
restartPolicyType = "on_failure"
restartPolicyMaxRetries = 3
```

### Required environment variables (Railway dashboard)

| Variable | Value |
|----------|-------|
| `DATABASE_URL` | Neon Postgres connection string (see "Neon Postgres setup" in §8) |
| `DATABASE_URL_FALLBACK` | (Optional) Supabase connection string — used if Neon is unreachable |
| `GITHUB_CLIENT_ID` | From GitHub OAuth App settings |
| `GITHUB_CLIENT_SECRET` | From GitHub OAuth App secrets |
| `SESSION_SECRET` | Random 32+ character string |
| `BASE_URL` | Railway domain, e.g. `https://needle-production.up.railway.app` |
| `PORT` | `8080` |

### Persistent Volume (Railway)

In Railway dashboard → your service → Add Volume:
- **Mount path:** `/data`
- **Size:** 1GB minimum

### GitHub OAuth App setup

1. GitHub → Settings → Developer settings → OAuth Apps → **New OAuth App**
2. **Application name:** Needle
3. **Homepage URL:** `https://your-railway-domain.up.railway.app`
4. **Authorization callback URL:** `https://your-railway-domain.up.railway.app/auth/callback`

---

## 10. Cloud Deployment — Render

### Setup in Render dashboard

1. Create new **Web Service** on [render.com](https://render.com)
2. Connect your GitHub repository
3. Configure:

| Setting | Value |
|---------|-------|
| **Runtime** | Docker |
| **Root directory** | `/` (default) |
| **Dockerfile path** | `Dockerfile` |
| **Start command** | `needle serve` |
| **Port** | `8080` |
| **Health check path** | `/api/status` |

### Persistent Disk (Render)

**Render's free tier does not support persistent disks** — the "Disks" tab
will prompt you to upgrade to the Starter plan. On free tier, `/data`
(and anything written to it) is wiped on every deploy and on every
spin-down after ~15 minutes of inactivity.

This is exactly why user/session/repo state now lives in Neon Postgres
(`DATABASE_URL`, set up in §8) instead of on `/data` — that state survives
restarts even without a paid disk. Without `DATABASE_URL` set, repos can
still get cloned into `/data/indexes`, but that index itself will be lost
on the next spin-down and re-indexed from scratch on the next request that
needs it (slower, not broken).

If you upgrade to Starter, add a real Disk:
- **Mount path:** `/data`
- **Size:** 1GB minimum

### Environment variables (Render)

Set in Render dashboard → Environment:

```
DATABASE_URL          = <Neon Postgres connection string, see §8>
DATABASE_URL_FALLBACK = <Supabase connection string, optional, see §8>
GITHUB_CLIENT_ID      = <from GitHub OAuth App>
GITHUB_CLIENT_SECRET = <from GitHub OAuth App>
SESSION_SECRET       = <32+ char random string>
BASE_URL             = https://your-service.onrender.com
PORT                 = 8080
```

---

## 11. Cloud Deployment — Fly.io

### `fly.toml`

```toml
app = "needle"

[build]
  dockerfile = "Dockerfile"

[env]
  PORT = "8080"

[[services]]
  http_checks = []
  internal_port = 8080
  protocol = "tcp"

  [[services.ports]]
    port = 443
    handlers = ["tls", "http"]

  [[services.ports]]
    port = 80
    handlers = ["http"]

  [services.concurrency]
    type = "connections"
    hard_limit = 25
    soft_limit = 20

[mounts]
  source = "needle_data"
  destination = "/data"
```

### Deploy steps

```bash
# Authenticate
fly auth login

# Create app
fly launch --no-deploy

# Create persistent volume (1GB)
fly volumes create needle_data --size 1

# Set secrets
fly secrets set \
  DATABASE_URL=... \
  DATABASE_URL_FALLBACK=... \
  GITHUB_CLIENT_ID=... \
  GITHUB_CLIENT_SECRET=... \
  SESSION_SECRET=... \
  BASE_URL=https://needle.fly.dev

# Deploy
fly deploy

# Tail logs
fly logs
```

---

## 12. Environment Variables Reference

| Variable | Mode | Default | Description |
|----------|------|---------|-------------|
| `DATABASE_URL` | Cloud | — | Primary Postgres connection string (Neon) — users, sessions, repos |
| `DATABASE_URL_FALLBACK` | Cloud | — | Backup Postgres connection string (Supabase), used if the primary is unreachable |
| `GITHUB_CLIENT_ID` | Cloud | — | GitHub OAuth App client ID |
| `GITHUB_CLIENT_SECRET` | Cloud | — | GitHub OAuth App client secret |
| `SESSION_SECRET` | Cloud | — | HMAC-SHA256 signing key for cookies (32+ chars) |
| `BASE_URL` | Cloud | — | Full public URL of the deployment |
| `PORT` | Both | `8080` | HTTP listen port (local default is `7700` via CLI flag) |
| `DATA_DIR` | Cloud | `/data` | Server-side search-index root (no longer stores user data) |
| `RUST_LOG` | Dev | `warn` | Log filter, e.g. `needle=debug`, `needle=trace` |

Local/desktop usage never needs `DATABASE_URL` — without it, cloud-only
routes (`/api/me`, repo connect, etc.) simply report "not configured" and
the UI hides the sign-in flow (see `/api/mode`).

---

## 13. File & Directory Paths

### Local index (dev machine)

```
~/.needle/
└── index/
    ├── meta.json       # { version, embedding_model, indexed_at, source_dirs }
    ├── chunks.json     # { "0": {Chunk}, "1": {Chunk}, ... }  ← main search data
    ├── filemap.json    # { "src/main.rs": [0, 1, 5, 12], ... }
    ├── bm25.json       # { term_freqs, doc_freqs, doc_lengths, avg_doc_len }
    ├── hnsw.json       # { layers, entry_point, vectors }
    └── graph.json      # { nodes: [...], edges: [...], stats: {...} }
```

All index files are plain JSON — debuggable with `jq`.

### Cloud / Docker volume

```
/data/
├── users.db            # SQLite: users, sessions, api_keys tables
└── index/              # (server-side indexing, if connected repos)
    └── <same layout as local>
```

### Source code layout

```
d:\NEEDLE\
├── src/
│   ├── main.rs                    # CLI entry point (clap subcommands)
│   ├── schema.rs                  # Chunk, Language, NodeKind, GraphNode, GraphEdge
│   ├── config.rs                  # Config struct (TOML + env)
│   ├── error.rs                   # NeedleError (thiserror)
│   ├── lib.rs                     # Library crate root + module exports
│   ├── llm.rs                     # LLM API routing (Anthropic/OpenAI/Groq/Ollama)
│   ├── assets/
│   │   ├── ui.html                # Single-file SPA (embedded via include_str!)
│   │   └── graph_template.html    # Standalone D3 export template
│   ├── chunking/
│   │   ├── code.rs                # Tree-sitter AST chunking (8 languages)
│   │   └── prose.rs               # Paragraph + PDF chunking
│   ├── indexing/
│   │   ├── bm25.rs                # Inverted index + BM25 scoring (k1=1.2, b=0.75)
│   │   └── hnsw.rs                # HNSW graph (M=16, ef_construction=200)
│   ├── query/
│   │   ├── mod.rs                 # QueryEngine struct
│   │   └── fusion.rs              # Reciprocal Rank Fusion (k=60)
│   ├── embedding/
│   │   └── mod.rs                 # Hash-projection 384-dim embeddings (FNV-1a)
│   ├── graph/
│   │   └── mod.rs                 # CodeGraph: extraction passes + community detection
│   ├── storage/
│   │   └── mod.rs                 # JSON index read/write + path management
│   ├── watcher/
│   │   └── mod.rs                 # inotify file watcher (incremental indexing)
│   ├── cli/
│   │   ├── init.rs                # `needle init` — index build
│   │   ├── serve.rs               # `needle serve` — Axum server + all route handlers
│   │   ├── mcp.rs                 # `needle mcp` — stdio MCP server (11 tools)
│   │   ├── report.rs              # `needle report` — architectural Markdown report
│   │   ├── bench.rs               # `needle bench` — latency benchmarks
│   │   └── search.rs              # `needle search` — CLI terminal search
│   └── server/
│       ├── index_pipeline.rs      # Full scan → chunk → embed → index pipeline
│       └── users.rs               # Auth: GitHub OAuth, sessions, API keys (SQLite)
├── needle-vscode/
│   ├── src/                       # TypeScript extension source
│   │   └── extension.ts           # Main extension entry point
│   ├── media/
│   │   └── needle.svg             # Activity bar icon
│   ├── out/                       # Compiled JavaScript (extension.js)
│   ├── package.json               # Extension manifest (contributes, commands, config)
│   ├── tsconfig.json              # TypeScript config
│   ├── needle-search-0.5.0.vsix   # Latest packaged extension ← install this
│   └── needle-search-0.*.0.vsix   # Previous versions (0.1–0.4)
├── benches/
│   ├── hnsw_bench.rs              # HNSW insertion + search benchmarks (criterion)
│   ├── bm25_bench.rs              # BM25 scoring benchmarks
│   └── embedding_bench.rs         # Embedding throughput benchmark
├── tests/                         # Integration tests
├── docs/                          # Additional documentation (PRD, schema, design)
├── design/                        # UI design files + prototypes
├── Cargo.toml                     # Rust package manifest + all dependencies
├── Cargo.lock                     # Pinned dependency versions
├── Dockerfile                     # Two-stage rust:1.88-bookworm → debian:bookworm-slim
├── railway.toml                   # Railway deployment config
├── needle-windows-x64.exe         # Pre-built Windows x64 binary
├── graph.html                     # Sample exported call graph
├── check.py                       # Index health check script
├── demo.py                        # Demo / walkthrough script
├── generate_report.py             # Architectural report generator (Python)
├── README.md                      # Public project overview
├── ARCHITECTURE.md                # Full technical architecture + decisions
├── DEPLOYMENT.md                  # This file (internal, do not publish)
└── .env                           # Local secrets (not committed to git)
```

---

## 14. Ports Reference

| Port | Context | Notes |
|------|---------|-------|
| `7700` | Local `needle serve` | Default when running locally |
| `8080` | Docker / cloud | Set via `PORT` env var |
| `443` | Fly.io (TLS) | External HTTPS, handled by fly proxy |
| `80` | Fly.io (HTTP) | Auto-redirected to 443 |

---

## 15. Quick Reference Card

```
┌──────────────────────────────────────────────────────────────────┐
│                      NEEDLE — QUICK REFERENCE                    │
├────────────────────┬─────────────────────────────────────────────┤
│  INSTALL           │                                             │
│  Pre-built EXE     │ needle-windows-x64.exe  (repo root)        │
│  Build from source │ cargo build --release                       │
│  Install globally  │ cargo install --path .                      │
├────────────────────┼─────────────────────────────────────────────┤
│  CLI COMMANDS      │                                             │
│  Index a project   │ needle init <path>                          │
│  Web UI            │ needle serve  →  localhost:7700             │
│  Terminal search   │ needle search "query" [--lang rust]         │
│  Call graph HTML   │ needle graph > graph.html                   │
│  Arch report       │ needle report > report.md                   │
│  MCP server        │ needle mcp                                  │
│  Rebuild index     │ needle reindex                              │
│  Index stats       │ needle status                               │
├────────────────────┼─────────────────────────────────────────────┤
│  EDITOR / AI       │                                             │
│  VS Code           │ Install needle-search-0.5.0.vsix            │
│  Claude Code       │ claude mcp add needle needle mcp            │
│  Cursor            │ .cursor/mcp.json                            │
│  Windsurf          │ .windsurf/mcp.json                          │
│  Copilot           │ .vscode/settings.json                       │
├────────────────────┼─────────────────────────────────────────────┤
│  CLOUD             │                                             │
│  Docker            │ docker build -t needle . && docker run ...  │
│  Railway           │ railway login && railway up                 │
│  Render            │ Docker runtime, port 8080, /data disk       │
│  Fly.io            │ fly launch && fly deploy                    │
├────────────────────┼─────────────────────────────────────────────┤
│  PATHS             │                                             │
│  Local index       │ ~/.needle/index/                            │
│  Cloud index       │ /data/  (Docker volume)                     │
│  Source            │ d:\NEEDLE\src\                              │
│  VS Code ext       │ d:\NEEDLE\needle-vscode\                    │
└────────────────────┴─────────────────────────────────────────────┘
```

---

_Last updated: 2026-06-26_  
_Internal document — do not publish to GitHub._
