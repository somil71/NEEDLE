//! `needle mcp` — Model Context Protocol server over stdio (JSON-RPC 2.0).
//!
//! Add to your agent config and AI agents call Needle directly instead of
//! loading raw source files into context, dramatically reducing token usage:
//!
//!   { "mcpServers": { "needle": { "command": "needle", "args": ["mcp"] } } }
//!
//! Exposed tools:
//!   search_code, find_callers, find_callees, get_endpoints,
//!   get_file_structure, find_similar, get_stats

use needle::{
    embedding::EmbeddingModel,
    graph::{CodeGraph, EdgeKind, NodeKind},
    query::QueryEngine,
    schema::Language,
    storage::Storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

// ── JSON-RPC types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    fn err(id: Option<Value>, code: i32, msg: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError { code, message: msg.into() }),
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run() -> needle::Result<()> {
    if !Storage::index_exists() {
        eprintln!("[needle-mcp] No index found. Run: needle init <dirs...>");
        std::process::exit(1);
    }

    let storage = Storage::new(Storage::default_index_dir())?;
    let config = Storage::load_config()?;
    let bm25 = storage.load_bm25()?;
    let hnsw = storage.load_hnsw()?;
    let chunks = storage.load_chunks()?;
    let graph = storage.load_graph().unwrap_or_default();
    let embedding = EmbeddingModel::new(config.embedding_dim)?;

    let engine = QueryEngine::new(bm25, hnsw, chunks, embedding);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => l,
            Err(_) => break,
        };

        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = RpcResponse::err(None, -32700, format!("Parse error: {e}"));
                let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
                let _ = out.flush();
                continue;
            }
        };

        // JSON-RPC notifications have no id and expect no response
        let is_notification = req.id.is_none();
        let id = req.id.clone();

        let response = handle_request(req, &engine, &graph);

        if is_notification {
            continue;
        }

        let resp = match response {
            Ok(result) => RpcResponse::ok(id, result),
            Err(msg) => RpcResponse::err(id, -32603, msg),
        };

        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
        let _ = out.flush();
    }

    Ok(())
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

fn handle_request(
    req: RpcRequest,
    engine: &QueryEngine,
    graph: &CodeGraph,
) -> Result<Value, String> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "needle",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),

        "initialized" => Ok(json!({})),

        "tools/list" => Ok(json!({ "tools": tool_definitions() })),

        "tools/call" => {
            let name = req.params["name"].as_str().unwrap_or("").to_string();
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            let text = dispatch_tool(&name, &args, engine, graph)?;
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }))
        }

        "ping" => Ok(json!({})),

        unknown => Err(format!("Method not found: {unknown}")),
    }
}

// ── Tool definitions ──────────────────────────────────────────────────────────

fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_code",
            "description": "Hybrid BM25 + semantic search across the indexed codebase. Returns relevant code chunks with file paths and line numbers. Use to find implementations, usages, or related patterns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language or code identifier to search for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results to return (default 5, max 20)",
                        "default": 5
                    },
                    "lang": {
                        "type": "string",
                        "description": "Filter by language: rust, python, typescript, javascript, go"
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "find_callers",
            "description": "Find all functions/methods that call the specified function. Returns caller names with file paths. Use before refactoring or to understand how a function is used across the codebase.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Function or method name to find callers of"
                    }
                },
                "required": ["name"]
            }
        },
        {
            "name": "find_callees",
            "description": "Find all functions/methods called by the specified function. Returns callee names with file paths. Use to understand what a function depends on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Function or method name to inspect"
                    }
                },
                "required": ["name"]
            }
        },
        {
            "name": "get_endpoints",
            "description": "List all API endpoints in the codebase (Flask routes, FastAPI routes, Express routes) with HTTP method, path, and file location.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "get_file_structure",
            "description": "Get the structure of a specific file — all functions, classes, methods, and endpoints with line numbers. More efficient than reading the whole file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "File path or partial filename to look up (e.g. 'app.py' or 'src/auth.ts')"
                    }
                },
                "required": ["file"]
            }
        },
        {
            "name": "find_similar",
            "description": "Find code chunks semantically similar to a given snippet using vector search. Useful for finding duplicate logic, alternative implementations, or related patterns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Code snippet to find similar chunks for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 5)",
                        "default": 5
                    }
                },
                "required": ["code"]
            }
        },
        {
            "name": "get_stats",
            "description": "Get an overview of the indexed codebase: total files, functions, classes, API endpoints, and graph size. Call this first to understand the scope of a project.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }
    ])
}

// ── Tool execution ────────────────────────────────────────────────────────────

fn dispatch_tool(
    name: &str,
    args: &Value,
    engine: &QueryEngine,
    graph: &CodeGraph,
) -> Result<String, String> {
    match name {
        "search_code"        => tool_search_code(args, engine),
        "find_callers"       => tool_find_callers(args, graph),
        "find_callees"       => tool_find_callees(args, graph),
        "get_endpoints"      => tool_get_endpoints(graph),
        "get_file_structure" => tool_get_file_structure(args, graph),
        "find_similar"       => tool_find_similar(args, engine),
        "get_stats"          => tool_get_stats(engine, graph),
        unknown => Err(format!("Unknown tool: {unknown}")),
    }
}

fn tool_search_code(args: &Value, engine: &QueryEngine) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return Err("query is required".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(5).min(20) as usize;
    let lang_filter = args["lang"].as_str().and_then(lang_from_short);

    let (results, timing) = engine
        .search(query, limit, lang_filter)
        .map_err(|e| e.to_string())?;

    if results.is_empty() {
        return Ok(format!("No results found for: {query}"));
    }

    let mut out = format!("Found {} result(s) in {:.1}ms:\n\n", results.len(), timing.total_ms);
    for (i, r) in results.iter().enumerate() {
        let file = r.file_path.replace('\\', "/");
        let lang = r.language.short_name();
        out.push_str(&format!(
            "### Result {} — {}:{}-{}\n```{lang}\n{}\n```\n\n",
            i + 1,
            file,
            r.line_start,
            r.line_end,
            r.content.trim()
        ));
    }
    Ok(out)
}

fn tool_find_callers(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let name = args["name"].as_str().unwrap_or("");
    if name.is_empty() {
        return Err("name is required".into());
    }

    let targets: Vec<u32> = graph
        .nodes
        .iter()
        .filter(|n| n.name == name || n.name.ends_with(&format!("::{name}")))
        .map(|n| n.id)
        .collect();

    if targets.is_empty() {
        return Ok(format!("No symbol named '{name}' found in the graph."));
    }

    let callers: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls) && targets.contains(&e.to))
        .filter_map(|e| graph.nodes.get(e.from as usize))
        .collect();

    if callers.is_empty() {
        return Ok(format!("'{name}' is not called by any indexed function."));
    }

    let mut out = format!("Callers of '{name}' ({} found):\n\n", callers.len());
    for c in &callers {
        let file = c.file_path.replace('\\', "/");
        out.push_str(&format!(
            "- **{}** ({}) — {}:{}\n",
            c.name,
            node_kind_label(&c.kind),
            file,
            c.line_start
        ));
    }
    Ok(out)
}

fn tool_find_callees(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let name = args["name"].as_str().unwrap_or("");
    if name.is_empty() {
        return Err("name is required".into());
    }

    let sources: Vec<u32> = graph
        .nodes
        .iter()
        .filter(|n| n.name == name || n.name.ends_with(&format!("::{name}")))
        .map(|n| n.id)
        .collect();

    if sources.is_empty() {
        return Ok(format!("No symbol named '{name}' found in the graph."));
    }

    let callees: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls) && sources.contains(&e.from))
        .filter_map(|e| graph.nodes.get(e.to as usize))
        .collect();

    if callees.is_empty() {
        return Ok(format!("'{name}' does not call any other indexed function."));
    }

    let mut out = format!("'{name}' calls {} function(s):\n\n", callees.len());
    for c in &callees {
        let file = c.file_path.replace('\\', "/");
        out.push_str(&format!(
            "- **{}** ({}) — {}:{}\n",
            c.name,
            node_kind_label(&c.kind),
            file,
            c.line_start
        ));
    }
    Ok(out)
}

fn tool_get_endpoints(graph: &CodeGraph) -> Result<String, String> {
    let endpoints: Vec<_> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint)
        .collect();

    if endpoints.is_empty() {
        return Ok("No API endpoints found in the index.".into());
    }

    let mut out = format!("{} API endpoint(s):\n\n", endpoints.len());
    for ep in &endpoints {
        let file = ep.file_path.replace('\\', "/");
        let method = ep.detail.as_deref().unwrap_or("?");
        out.push_str(&format!(
            "- **{}** [{}] — {}:{}\n",
            ep.name, method, file, ep.line_start
        ));
    }
    Ok(out)
}

fn tool_get_file_structure(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let file_query = args["file"].as_str().unwrap_or("");
    if file_query.is_empty() {
        return Err("file is required".into());
    }

    let q = file_query.replace('\\', "/").to_lowercase();

    let module = graph.nodes.iter().find(|n| {
        matches!(n.kind, NodeKind::Module) && {
            let fp = n.file_path.replace('\\', "/").to_lowercase();
            fp == q || fp.ends_with(&format!("/{q}")) || fp.contains(&q)
        }
    });

    let Some(module) = module else {
        return Ok(format!("File '{file_query}' not found in the index."));
    };

    let children: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.from == module.id && matches!(e.kind, EdgeKind::Contains))
        .filter_map(|e| graph.nodes.get(e.to as usize))
        .collect();

    let file = module.file_path.replace('\\', "/");
    let mut out = format!("**{}** ({} definition(s)):\n\n", file, children.len());

    let mut sorted = children.clone();
    sorted.sort_by_key(|n| (node_kind_order(&n.kind), n.line_start));

    for n in sorted {
        let kind = node_kind_label(&n.kind);
        let detail = n.detail.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default();
        out.push_str(&format!(
            "- L{}-{}: **{}** ({kind}{detail})\n",
            n.line_start, n.line_end, n.name
        ));
    }
    Ok(out)
}

fn tool_find_similar(args: &Value, engine: &QueryEngine) -> Result<String, String> {
    let code = args["code"].as_str().unwrap_or("");
    if code.is_empty() {
        return Err("code is required".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(5).min(20) as usize;

    let results = engine
        .search_similar(code, limit, None)
        .map_err(|e| e.to_string())?;

    if results.is_empty() {
        return Ok("No similar code found.".into());
    }

    let mut out = format!("Found {} similar chunk(s):\n\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let file = r.file_path.replace('\\', "/");
        let lang = r.language.short_name();
        out.push_str(&format!(
            "### Similar {} — {}:{}-{} (score {:.2})\n```{lang}\n{}\n```\n\n",
            i + 1,
            file,
            r.line_start,
            r.line_end,
            r.score,
            r.content.trim()
        ));
    }
    Ok(out)
}

fn tool_get_stats(engine: &QueryEngine, graph: &CodeGraph) -> Result<String, String> {
    let files = engine.file_list();
    let s = &graph.stats;

    let mut out = String::from("## Needle Index Overview\n\n");
    out.push_str(&format!("- **Files indexed**: {}\n", files.len()));
    out.push_str(&format!("- **Code chunks**: {}\n", engine.chunks.len()));
    out.push_str(&format!("- **Graph nodes**: {}\n", s.total_nodes));
    out.push_str(&format!("- **Graph edges**: {}\n", s.total_edges));
    out.push_str(&format!("- **Functions**: {}\n", s.functions));
    out.push_str(&format!("- **Methods**: {}\n", s.methods));
    out.push_str(&format!("- **Classes / Structs / Traits**: {}\n", s.classes));
    out.push_str(&format!("- **API Endpoints**: {}\n", s.endpoints));
    out.push_str(&format!("- **Modules (files)**: {}\n", s.modules));

    out.push_str("\n### Languages\n");
    let mut lang_counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for f in &files {
        *lang_counts.entry(f.lang.as_str()).or_default() += 1;
    }
    let mut langs: Vec<_> = lang_counts.into_iter().collect();
    langs.sort_by(|a, b| b.1.cmp(&a.1));
    for (lang, count) in langs {
        out.push_str(&format!("- {lang}: {count} file(s)\n"));
    }

    Ok(out)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn lang_from_short(s: &str) -> Option<Language> {
    match s {
        "rust" | "rs"         => Some(Language::Rust),
        "python" | "py"       => Some(Language::Python),
        "typescript" | "ts"   => Some(Language::TypeScript),
        "javascript" | "js"   => Some(Language::JavaScript),
        "go"                  => Some(Language::Go),
        _                     => None,
    }
}

fn node_kind_label(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Module   => "module",
        NodeKind::Function => "function",
        NodeKind::Method   => "method",
        NodeKind::Class    => "class",
        NodeKind::Struct   => "struct",
        NodeKind::Trait    => "trait",
        NodeKind::Endpoint => "endpoint",
    }
}

fn node_kind_order(kind: &NodeKind) -> u8 {
    match kind {
        NodeKind::Endpoint => 0,
        NodeKind::Class    => 1,
        NodeKind::Struct   => 2,
        NodeKind::Trait    => 3,
        NodeKind::Function => 4,
        NodeKind::Method   => 5,
        NodeKind::Module   => 6,
    }
}
