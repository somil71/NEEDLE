# Needle Graph Report

_Generated: 2026-06-25 16:18_

## Index Statistics

| Metric | Value |
|--------|-------|
| Files indexed | 35 |
| Code chunks | 361 |
| Languages | Rust (361) |
| Graph nodes | 364 |
| Graph edges | 548 |
| Functions | 144 |
| Methods | 99 |
| Classes/Structs | 67 |
| Endpoints | 19 |
| Modules | 35 |
| Embedding model | `hash-projection-384` |

---

## God Nodes — Architectural Load-Bearers

The highest-degree nodes in the call graph; everything else depends on these.

| Rank | Name | Degree | Kind | File |
|------|------|--------|------|------|
| 1 | `mod.rs` | 51 | module | `graph/mod.rs` |
| 2 | `serve.rs` | 37 | module | `cli/serve.rs` |
| 3 | `mcp.rs` | 32 | module | `cli/mcp.rs` |
| 4 | `hnsw.rs` | 24 | module | `indexing/hnsw.rs` |
| 5 | `mod.rs` | 23 | module | `storage/mod.rs` |
| 6 | `users.rs` | 21 | module | `server/users.rs` |
| 7 | `schema.rs` | 18 | module | `src/schema.rs` |
| 8 | `txt` | 17 | function | `graph/mod.rs` |
| 9 | `mod.rs` | 15 | module | `embedding/mod.rs` |
| 10 | `dispatch_tool` | 14 | function | `cli/mcp.rs` |
| 11 | `bm25.rs` | 14 | module | `indexing/bm25.rs` |
| 12 | `now` | 14 | function | `server/users.rs` |
| 13 | `open_db` | 14 | function | `server/users.rs` |
| 14 | `oauth.rs` | 13 | module | `server/oauth.rs` |
| 15 | `llm.rs` | 11 | module | `src/llm.rs` |

---

## API Endpoints

All 19 HTTP routes detected via Axum `.route()` pattern analysis.

| Method | Handler | File | Line |
|--------|---------|------|------|
| `GET` | `api_auth_callback` | `cli/serve.rs` | 681 |
| `GET` | `api_auth_github` | `cli/serve.rs` | 672 |
| `GET` | `api_auth_logout` | `cli/serve.rs` | 693 |
| `GET` | `api_files` | `cli/serve.rs` | 629 |
| `GET` | `api_github_repos_handler` | `cli/serve.rs` | 738 |
| `GET` | `api_graph` | `cli/serve.rs` | 432 |
| `GET` | `api_me` | `cli/serve.rs` | 698 |
| `GET` | `api_repos` | `cli/serve.rs` | 770 |
| `GET` | `api_search` | `cli/serve.rs` | 340 |
| `GET` | `api_status_handler` | `cli/serve.rs` | 405 |
| `GET` | `api_todos` | `cli/serve.rs` | 610 |
| `GET` | `serve_ui` | `cli/serve.rs` | 283 |
| `POST` | `api_ask` | `cli/serve.rs` | 462 |
| `POST` | `api_open` | `cli/serve.rs` | 436 |
| `POST` | `api_regenerate_key` | `cli/serve.rs` | 802 |
| `POST` | `api_repo_connect` | `cli/serve.rs` | 744 |
| `POST` | `api_revoke_key` | `cli/serve.rs` | 786 |
| `POST` | `api_similar` | `cli/serve.rs` | 559 |
| `POST` | `api_validate_key` | `cli/serve.rs` | 712 |

---

## Semantic Communities

Label propagation on the call graph found **220 clusters**.
The top clusters by membership:

### Cluster 1 — graph/mod.rs (43 nodes)

Members: `run`, `collect_files`, `clean_path`, `extract_axum_routes`, `extract_express_routes`, `extract_defs`, `extract_imports`, `extract_calls`
_(+35 more)_

### Cluster 2 — cli/mcp.rs (20 nodes)

Members: `run`, `write_rpc`, `handle_request`, `dispatch_tool`, `graph_of`, `cloud_search`, `cloud_similar`, `tool_search_code`
_(+12 more)_

### Cluster 3 — cli/bench.rs (10 nodes)

Members: 

### Cluster 4 — server/users.rs (8 nodes)

Members: `api_auth_callback`, `auth_callback`, `error_page`, `generate_api_key`, `generate_session_token`, `upsert_user`, `create_session`, `store_gh_token`

### Cluster 5 — server/users.rs (7 nodes)

Members: `api_regenerate_key`, `current_user_from_cookies`, `db_path`, `open_db`, `migrate`, `get_user_by_id`, `get_session_user`

### Cluster 6 — cli/graph.rs (6 nodes)

Members: 

### Cluster 7 — chunking/code.rs (5 nodes)

Members: `CodeChunker::chunk`, `extract_blocks`, `interesting_node_kinds`, `collect_nodes`, `fallback_blocks`

### Cluster 8 — graph/mod.rs (5 nodes)

Members: `run`, `build_report`, `compute_communities`, `compute_god_nodes`, `find_surprise_edges`

### Cluster 9 — src/llm.rs (5 nodes)

Members: `LlmClient::complete`, `anthropic_complete`, `openai_complete`, `ollama_complete`, `http_client`

### Cluster 10 — server/indexer.rs (5 nodes)

Members: 

---

## Surprise Edges — Hidden Couplings

Cross-community calls that reveal unexpected architectural dependencies.

| Caller | Callee | File |
|--------|--------|------|
| `CodeChunker::chunk` | `now` | `chunking/code.rs` |
| `chunk_markdown` | `now` | `chunking/prose.rs` |
| `chunk_plain_text` | `now` | `chunking/prose.rs` |
| `run` | `detect_chunker` | `cli/init.rs` |
| `run` | `extract` | `cli/init.rs` |
| `run` | `now` | `cli/init.rs` |
| `tool_explain` | `compute_communities` | `cli/mcp.rs` |
| `tool_explain` | `compute_god_nodes` | `cli/mcp.rs` |
| `tool_get_god_nodes` | `compute_god_nodes` | `cli/mcp.rs` |
| `tool_get_communities` | `compute_communities` | `cli/mcp.rs` |
| `tool_get_surprises` | `compute_communities` | `cli/mcp.rs` |
| `tool_get_surprises` | `find_surprise_edges` | `cli/mcp.rs` |
| `run` | `tokenize` | `cli/search.rs` |
| `resolve_user` | `current_user_from_cookies` | `cli/serve.rs` |
| `resolve_user` | `get_user_by_api_key` | `cli/serve.rs` |

---

## Callers & Callees — Key Functions

### `dispatch_tool`

**Called by:** `handle_request`

**Calls:** `graph_of`, `tool_explain`, `tool_find_callees`, `tool_find_callers`, `tool_find_similar`, `tool_get_communities`, `tool_get_endpoints`, `tool_get_file_structure`, `tool_get_god_nodes`, `tool_get_stats`

### `api_search`

**Called by:** _(entry point / no callers in index)_

**Calls:** `load_cloud_engines`, `resolve_user`

### `extract`

**Called by:** `run`, `run`

**Calls:** `extract_axum_routes`, `extract_calls`, `extract_defs`, `extract_express_routes`, `extract_imports`, `resolve_import`

### `handle_request`

**Called by:** `run`

**Calls:** `dispatch_tool`

---

_Report generated by [Needle](https://github.com/your-org/needle) · index at `C:\Users\Somil\.needle\index`_