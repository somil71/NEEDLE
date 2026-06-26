# Needle — Local-first Code Search

> Index any codebase. Search it semantically. Map its call graph. Plug it into any AI tool. Everything offline.

[![MIT License](https://img.shields.io/badge/license-MIT-7C3AED)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built_with-Rust-orange)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/somil71/NEEDLE?color=7C3AED)](https://github.com/somil71/NEEDLE/releases)

---

## What is Needle?

Needle is a local-first code search engine that runs entirely on your machine. No cloud, no API keys, no data leaving your system.

- **Hybrid search** — BM25 keyword + HNSW vector search fused via Reciprocal Rank Fusion, sub-50ms
- **Call graph** — live D3 force graph with endpoint detection and architectural analysis
- **MCP server** — 11 tools for Claude Code, Cursor, Windsurf, Copilot
- **Desktop app** — native window via Tauri, or run headless as a CLI / Docker container

---

## Install

### Windows (Desktop App)

Download **[Needle_0.1.0_x64-setup.exe](https://github.com/somil71/NEEDLE/releases/download/v0.1.0/Needle_0.1.0_x64-setup.exe)** and run the installer. Needle appears in your Start Menu.

### VS Code Extension

Download **[needle-search-0.5.0.vsix](https://github.com/somil71/NEEDLE/releases/download/v0.1.0/needle-search-0.5.0.vsix)** and install via:
```
Extensions panel → ⋯ → Install from VSIX
```

### Build from Source

```bash
git clone https://github.com/somil71/NEEDLE
cd NEEDLE
cargo build --release
# Binary at: target/release/needle
```

---

## Quick Start (CLI)

```bash
# Index a project
needle init ~/code/my-project

# Open the web UI
needle serve
# → http://localhost:7700

# Search from terminal
needle search "authentication middleware"

# Start MCP server for AI tools
needle mcp
```

---

## MCP Integration

Connect Needle to any MCP-compatible AI tool:

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

**Claude Code:** `claude mcp add needle needle mcp`

**Cursor / Windsurf:** add to `.cursor/mcp.json` or `.windsurf/mcp.json`

### Available MCP Tools

| Tool | Description |
|------|-------------|
| `search_code` | Hybrid keyword + semantic search |
| `find_callers` | Who calls a given function? |
| `find_callees` | What does a function call? |
| `find_similar` | Semantically similar code chunks |
| `get_god_nodes` | Highest-degree symbols |
| `get_endpoints` | All detected HTTP routes |
| `get_communities` | Label-propagation clusters |
| `get_surprises` | Cross-community edges |
| `get_file_structure` | Directory/module tree |
| `get_stats` | Index summary |
| `explain` | LLM explanation of a symbol |

---

## Supported Languages

| Language | Chunking | Call Graph |
|----------|----------|------------|
| Rust | AST (functions, structs, impls, traits) | ✓ |
| Python | AST (functions, classes, methods) | ✓ |
| TypeScript / JavaScript | AST (functions, classes, arrow fns) | ✓ |
| Go | AST (functions, types, interfaces) | ✓ |
| Java | AST (classes, methods) | ✓ |
| C / C++ | AST (functions, structs) | ✓ |
| Markdown | Section-by-section prose | — |
| PDF | Text extraction + paragraph chunks | — |

---

## Cloud / Docker

```bash
docker build -t needle .
docker run -p 8080:8080 \
  -e GITHUB_CLIENT_ID=... \
  -e GITHUB_CLIENT_SECRET=... \
  -e SESSION_SECRET=... \
  -v needle_data:/data \
  needle
```

Cloud mode adds GitHub OAuth and multi-repo support. Deploy to Railway, Render, or any Docker host.

---

## Architecture

```
src/
├── main.rs              # CLI entry (clap)
├── lib.rs               # Library crate root
├── schema.rs            # Chunk, Language, NodeKind types
├── chunking/            # Tree-sitter AST + prose chunking
├── indexing/            # BM25 inverted index + HNSW graph
├── query/               # QueryEngine + Reciprocal Rank Fusion
├── embedding/           # Hash-projection 384-dim embeddings
├── graph/               # CodeGraph, communities, god nodes
├── storage/             # JSON index persistence
├── server/              # Axum HTTP server + API routes
├── watcher/             # File watcher (live reindex)
└── assets/ui.html       # Web UI (single-file SPA, embedded at compile time)

src-tauri/               # Tauri desktop app wrapper
needle-vscode/           # VS Code extension
```

---

## License

MIT — see [LICENSE](LICENSE)
