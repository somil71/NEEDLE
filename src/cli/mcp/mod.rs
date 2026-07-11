//! `needle mcp` — Model Context Protocol server over stdio (JSON-RPC 2.0).
//!
//! Local-only mode (default):
//!   { "mcpServers": { "needle": { "command": "needle", "args": ["mcp"] } } }
//!
//! Cloud mode — searches your connected GitHub repos on Needle cloud:
//!   { "mcpServers": { "needle": {
//!       "command": "needle", "args": ["mcp"],
//!       "env": {
//!           "NEEDLE_API_KEY":   "ndk_your_key_here",
//!           "NEEDLE_CLOUD_URL": "https://needle-4kdk.onrender.com"
//!       }
//!   }}}
//!
//! Both modes can run together: local results + cloud results are merged.
//!
//! Exposed tools:
//!   search_code, find_callers, find_callees, get_endpoints, get_file_structure,
//!   find_similar, get_stats, get_god_nodes, get_communities, get_surprises, explain
//!
//! LLM for explain: set ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, or run Ollama.

mod tools_graph;
mod tools_search;

use needle::{
    embedding::EmbeddingModel,
    graph::{CodeGraph, NodeKind},
    query::QueryEngine,
    schema::Language,
    storage::Storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
        Self { jsonrpc: "2.0", id, result: None, error: Some(RpcError { code, message: msg.into() }) }
    }
}

// ── Cloud config ──────────────────────────────────────────────────────────────

pub(super) struct CloudConfig {
    pub(super) api_key:  String,
    pub(super) base_url: String,
}

impl CloudConfig {
    fn from_env() -> Option<Self> {
        Some(Self {
            api_key:  std::env::var("NEEDLE_API_KEY").ok()?,
            base_url: std::env::var("NEEDLE_CLOUD_URL")
                .ok()?
                .trim_end_matches('/')
                .to_string(),
        })
    }
}

// Deserialized result from /api/search
#[derive(serde::Deserialize)]
pub(super) struct ApiSearchResult {
    pub(super) file_path:  String,
    pub(super) line_start: u32,
    pub(super) line_end:   u32,
    pub(super) language:   String,
    pub(super) content:    String,
    pub(super) score:      f32,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn load_local() -> needle::Result<Option<(QueryEngine, CodeGraph)>> {
    if !Storage::index_exists() {
        return Ok(None);
    }
    let storage  = Storage::new(Storage::default_index_dir())?;
    let config   = Storage::load_config().unwrap_or_default();
    let bm25     = storage.load_bm25()?;
    let hnsw     = storage.load_hnsw()?;
    let chunks   = storage.load_chunks()?;
    let graph    = storage.load_graph().unwrap_or_default();
    let embedding = EmbeddingModel::new(config.embedding_dim)?;
    Ok(Some((QueryEngine::new(bm25, hnsw, chunks, embedding), graph)))
}

fn index_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(Storage::default_index_dir().join("meta.json"))
        .and_then(|m| m.modified())
        .ok()
}

pub async fn run() -> needle::Result<()> {
    let cloud = CloudConfig::from_env();
    let llm   = needle::llm::LlmClient::from_env();

    let mut local = load_local()?;
    let mut local_mtime = index_mtime();

    if local.is_none() && cloud.is_none() {
        eprintln!(
            "[needle-mcp] No local index and no cloud config.\n\
             • Local:  run `needle init <dirs...>` to index a codebase\n\
             • Cloud:  set NEEDLE_API_KEY + NEEDLE_CLOUD_URL env vars"
        );
        std::process::exit(1);
    }

    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line   = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        let req: RpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                write_rpc(&mut stdout, RpcResponse::err(None, -32700, format!("Parse error: {e}"))).await;
                continue;
            }
        };

        let fresh_mtime = index_mtime();
        if fresh_mtime != local_mtime {
            match load_local() {
                Ok(new_local) => {
                    local = new_local;
                    local_mtime = fresh_mtime;
                }
                Err(e) => eprintln!("[needle-mcp] index reload failed: {e}"),
            }
        }

        let is_notif = req.id.is_none();
        let id = req.id.clone();

        let resp = match handle_request(req, local.as_ref(), cloud.as_ref(), &llm).await {
            Ok(v)    => RpcResponse::ok(id, v),
            Err(msg) => RpcResponse::err(id, -32603, msg),
        };

        if !is_notif {
            write_rpc(&mut stdout, resp).await;
        }
    }

    Ok(())
}

async fn write_rpc(stdout: &mut tokio::io::Stdout, resp: RpcResponse) {
    let line = format!("{}\n", serde_json::to_string(&resp).unwrap());
    let _ = stdout.write_all(line.as_bytes()).await;
    let _ = stdout.flush().await;
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

async fn handle_request(
    req:   RpcRequest,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
    llm:   &needle::llm::LlmClient,
) -> Result<Value, String> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "needle", "version": env!("CARGO_PKG_VERSION") }
        })),
        "initialized" => Ok(json!({})),
        "tools/list"  => Ok(json!({ "tools": tool_definitions() })),
        "tools/call"  => {
            let name = req.params["name"].as_str().unwrap_or("").to_string();
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            let text = dispatch_tool(&name, &args, local, cloud, llm).await?;
            Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
        }
        "ping" => Ok(json!({})),
        unknown => Err(format!("Method not found: {unknown}")),
    }
}

async fn dispatch_tool(
    name:  &str,
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
    llm:   &needle::llm::LlmClient,
) -> Result<String, String> {
    match name {
        "search_code" | "find_similar" | "get_stats" | "explain" | "get_security_scan" =>
            dispatch_search_tools(name, args, local, cloud, llm).await,
        "find_callers" | "find_callees" | "get_endpoints" | "get_file_structure"
        | "get_god_nodes" | "get_communities" | "get_surprises" | "get_health_score"
        | "blast_radius" =>
            dispatch_graph_tools(name, args, local),
        unknown => Err(format!("Unknown tool: {unknown}")),
    }
}

async fn dispatch_search_tools(
    name:  &str,
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
    llm:   &needle::llm::LlmClient,
) -> Result<String, String> {
    match name {
        "search_code"       => tools_search::search_code(args, local, cloud).await,
        "find_similar"      => tools_search::find_similar(args, local, cloud).await,
        "get_stats"         => tools_search::get_stats(local, cloud).await,
        "explain"           => tools_search::explain(args, local, llm).await,
        "get_security_scan" => tools_search::get_security_scan(local),
        unknown => Err(format!("Unknown search tool: {unknown}")),
    }
}

fn dispatch_graph_tools(
    name:  &str,
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
) -> Result<String, String> {
    let graph = local
        .map(|(_, g)| g)
        .ok_or_else(|| "Graph tools require a local index. Run: needle init <dirs...>".to_string())?;
    match name {
        "find_callers"       => tools_graph::find_callers(args, graph),
        "find_callees"       => tools_graph::find_callees(args, graph),
        "get_endpoints"      => tools_graph::get_endpoints(graph),
        "get_file_structure" => tools_graph::get_file_structure(args, graph),
        "get_god_nodes"      => tools_graph::get_god_nodes(args, graph),
        "get_communities"    => tools_graph::get_communities(graph),
        "get_surprises"      => tools_graph::get_surprises(graph),
        "get_health_score"   => tools_graph::get_health_score(graph),
        "blast_radius"       => tools_graph::blast_radius(args, graph),
        unknown => Err(format!("Unknown graph tool: {unknown}")),
    }
}

// ── Cloud helpers ─────────────────────────────────────────────────────────────

pub(super) async fn cloud_search(cfg: &CloudConfig, query: &str, limit: usize) -> Vec<ApiSearchResult> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let url = format!(
        "{}/api/search?q={}&limit={}",
        cfg.base_url,
        urlencoding::encode(query),
        limit
    );

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .send()
        .await
    {
        Ok(r)  => r,
        Err(e) => { eprintln!("[needle-mcp] cloud search error: {e}"); return vec![]; }
    };

    let data: Value = match resp.json().await {
        Ok(d)  => d,
        Err(_) => return vec![],
    };

    data["results"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect()
}

pub(super) async fn cloud_similar(cfg: &CloudConfig, code: &str, limit: usize) -> Vec<ApiSearchResult> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let url = format!("{}/api/similar", cfg.base_url);
    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .json(&json!({ "code": code, "limit": limit }))
        .send()
        .await
    {
        Ok(r)  => r,
        Err(e) => { eprintln!("[needle-mcp] cloud similar error: {e}"); return vec![]; }
    };

    let data: Value = match resp.json().await {
        Ok(d)  => d,
        Err(_) => return vec![],
    };

    data["results"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect()
}

pub(super) fn display_cloud_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(pos) = norm.find("/src/") {
        let before = &norm[..pos];
        let repo   = before.split('/').last().unwrap_or("cloud");
        let rel    = &norm[pos + 5..];
        return format!("{}: {}", repo, rel);
    }
    norm
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub(super) fn node_kind_label(kind: &NodeKind) -> &'static str {
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

pub(super) fn node_kind_order(kind: &NodeKind) -> u8 {
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

pub(super) fn lang_from_short(s: &str) -> Option<Language> {
    match s {
        "rust" | "rs"         => Some(Language::Rust),
        "python" | "py"       => Some(Language::Python),
        "typescript" | "ts"   => Some(Language::TypeScript),
        "javascript" | "js"   => Some(Language::JavaScript),
        "go"                  => Some(Language::Go),
        _                     => None,
    }
}

// ── Tool definitions ──────────────────────────────────────────────────────────

fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_code",
            "description": "Hybrid BM25 + semantic search across the indexed codebase. Searches local index and/or cloud repos (if NEEDLE_API_KEY is set). Returns relevant code chunks with file paths and line numbers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language or code identifier to search for" },
                    "limit": { "type": "integer", "description": "Max results to return (default 5, max 20)", "default": 5 },
                    "lang":  { "type": "string", "description": "Filter by language: rust, python, typescript, javascript, go" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "find_callers",
            "description": "Find all functions/methods that call the specified function (local index only). Returns caller names with file paths.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "Function or method name to find callers of" } },
                "required": ["name"]
            }
        },
        {
            "name": "find_callees",
            "description": "Find all functions/methods called by the specified function (local index only). Returns callee names with file paths.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "Function or method name to inspect" } },
                "required": ["name"]
            }
        },
        {
            "name": "get_endpoints",
            "description": "List all API endpoints in the local codebase with HTTP method, path, and file location.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_file_structure",
            "description": "Get the structure of a specific file — all functions, classes, methods with line numbers (local index only).",
            "inputSchema": {
                "type": "object",
                "properties": { "file": { "type": "string", "description": "File path or partial filename (e.g. 'app.py' or 'src/auth.ts')" } },
                "required": ["file"]
            }
        },
        {
            "name": "find_similar",
            "description": "Find code chunks semantically similar to a given snippet. Searches local and cloud repos.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code":  { "type": "string", "description": "Code snippet to find similar chunks for" },
                    "limit": { "type": "integer", "description": "Max results (default 5)", "default": 5 }
                },
                "required": ["code"]
            }
        },
        {
            "name": "get_stats",
            "description": "Get an overview of the indexed codebase: file count, chunk count, graph size, and cloud connection status.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_god_nodes",
            "description": "Find the highest-degree nodes in the call graph — the architectural load-bearers that everything else depends on. Useful for understanding which symbols are the core of the codebase.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Number of top nodes to return (default 10, max 20)", "default": 10 }
                }
            }
        },
        {
            "name": "get_communities",
            "description": "Detect semantic clusters in the codebase using label propagation on call+import edges. Returns groups of symbols that work closely together.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_surprises",
            "description": "Find unexpected cross-cluster dependencies — function calls that bridge otherwise-disconnected parts of the codebase. These are the hidden couplings worth reviewing for architecture improvements.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "explain",
            "description": "Answer a natural language question about the codebase using hybrid search + call graph + community detection + an LLM. Requires ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, or Ollama.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question":       { "type": "string", "description": "What you want to understand about the codebase" },
                    "context_chunks": { "type": "integer", "description": "Number of code chunks to include as context (default 6, max 12)", "default": 6 }
                },
                "required": ["question"]
            }
        },
        {
            "name": "get_health_score",
            "description": "Compute a codebase health score (0–100) based on circular dependencies, god objects, long files, orphaned functions, and coupling. Returns a detailed breakdown.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_security_scan",
            "description": "Scan the indexed codebase for common security patterns: hardcoded secrets, XSS risks, SQL injection, shell injection, and security suppressions.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "blast_radius",
            "description": "Given a file path, compute which other files would be affected if that file changed. Returns dependency depth and risk score.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File path to analyze (partial matches work, e.g. 'auth.rs')" }
                },
                "required": ["file"]
            }
        }
    ])
}
