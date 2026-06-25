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

use needle::{
    embedding::EmbeddingModel,
    graph::{self, CodeGraph, EdgeKind, NodeKind},
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

struct CloudConfig {
    api_key:  String,
    base_url: String,
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
#[derive(Deserialize)]
struct ApiSearchResult {
    file_path:  String,
    line_start: u32,
    line_end:   u32,
    language:   String,
    content:    String,
    score:      f32,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run() -> needle::Result<()> {
    let cloud = CloudConfig::from_env();
    let llm   = needle::llm::LlmClient::from_env();

    // Load local index (optional when cloud is configured)
    let local: Option<(QueryEngine, CodeGraph)> = if Storage::index_exists() {
        let storage  = Storage::new(Storage::default_index_dir())?;
        let config   = Storage::load_config().unwrap_or_default();
        let bm25     = storage.load_bm25()?;
        let hnsw     = storage.load_hnsw()?;
        let chunks   = storage.load_chunks()?;
        let graph    = storage.load_graph().unwrap_or_default();
        let embedding = EmbeddingModel::new(config.embedding_dim)?;
        Some((QueryEngine::new(bm25, hnsw, chunks, embedding), graph))
    } else {
        if cloud.is_none() {
            eprintln!(
                "[needle-mcp] No local index and no cloud config.\n\
                 • Local:  run `needle init <dirs...>` to index a codebase\n\
                 • Cloud:  set NEEDLE_API_KEY + NEEDLE_CLOUD_URL env vars"
            );
            std::process::exit(1);
        }
        None
    };

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
        "search_code"        => tool_search_code(args, local, cloud).await,
        "find_similar"       => tool_find_similar(args, local, cloud).await,
        "get_stats"          => tool_get_stats(local, cloud).await,
        "find_callers"       => tool_find_callers(args, graph_of(local)?),
        "find_callees"       => tool_find_callees(args, graph_of(local)?),
        "get_endpoints"      => tool_get_endpoints(graph_of(local)?),
        "get_file_structure" => tool_get_file_structure(args, graph_of(local)?),
        "get_god_nodes"      => tool_get_god_nodes(args, graph_of(local)?),
        "get_communities"    => tool_get_communities(graph_of(local)?),
        "get_surprises"      => tool_get_surprises(graph_of(local)?),
        "explain"            => tool_explain(args, local, llm).await,
        unknown => Err(format!("Unknown tool: {unknown}")),
    }
}

fn graph_of(local: Option<&(QueryEngine, CodeGraph)>) -> Result<&CodeGraph, String> {
    local
        .map(|(_, g)| g)
        .ok_or_else(|| "Graph tools require a local index. Run: needle init <dirs...>".to_string())
}

// ── Cloud helpers ─────────────────────────────────────────────────────────────

async fn cloud_search(cfg: &CloudConfig, query: &str, limit: usize) -> Vec<ApiSearchResult> {
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

async fn cloud_similar(cfg: &CloudConfig, code: &str, limit: usize) -> Vec<ApiSearchResult> {
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

/// Strip the internal index path prefix for readability.
/// `/data/indexes/usr_abc/owner_repo/src/lib.rs` → `owner_repo: lib.rs`
fn display_cloud_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(pos) = norm.find("/src/") {
        let before = &norm[..pos];
        let repo   = before.split('/').last().unwrap_or("cloud");
        let rel    = &norm[pos + 5..];
        return format!("{}: {}", repo, rel);
    }
    norm
}

// ── Tool implementations ──────────────────────────────────────────────────────

async fn tool_search_code(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or("");
    if query.is_empty() { return Err("query is required".into()); }
    let limit       = args["limit"].as_u64().unwrap_or(5).min(20) as usize;
    let lang_filter = args["lang"].as_str().and_then(lang_from_short);

    let mut out   = String::new();
    let mut total = 0usize;

    // Cloud results first
    if let Some(cfg) = cloud {
        for r in cloud_search(cfg, query, limit).await {
            total += 1;
            out.push_str(&format!(
                "### Result {} [cloud] — {}:{}-{}\n```{}\n{}\n```\n\n",
                total,
                display_cloud_path(&r.file_path),
                r.line_start, r.line_end,
                r.language,
                r.content.trim()
            ));
        }
    }

    // Local results
    if let Some((engine, _)) = local {
        if let Ok((results, timing)) = engine.search(query, limit, lang_filter) {
            for r in &results {
                total += 1;
                out.push_str(&format!(
                    "### Result {} [local] — {}:{}-{}\n```{}\n{}\n```\n\n",
                    total,
                    r.file_path.replace('\\', "/"),
                    r.line_start, r.line_end,
                    r.language.short_name(),
                    r.content.trim()
                ));
            }
            if total > 0 {
                let mut header = format!("Found {} result(s) in {:.1}ms", total, timing.total_ms);
                if cloud.is_some() { header.push_str(" (cloud + local)"); }
                return Ok(format!("{header}:\n\n{out}"));
            }
        }
    }

    if total == 0 {
        Ok(format!("No results found for: {query}"))
    } else {
        Ok(format!("Found {total} result(s) (cloud):\n\n{out}"))
    }
}

async fn tool_find_similar(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let code = args["code"].as_str().unwrap_or("");
    if code.is_empty() { return Err("code is required".into()); }
    let limit = args["limit"].as_u64().unwrap_or(5).min(20) as usize;

    let mut out   = String::new();
    let mut total = 0usize;

    if let Some(cfg) = cloud {
        for r in cloud_similar(cfg, code, limit).await {
            total += 1;
            out.push_str(&format!(
                "### Similar {} [cloud] — {}:{}-{} (score {:.2})\n```{}\n{}\n```\n\n",
                total,
                display_cloud_path(&r.file_path),
                r.line_start, r.line_end,
                r.score,
                r.language,
                r.content.trim()
            ));
        }
    }

    if let Some((engine, _)) = local {
        if let Ok(results) = engine.search_similar(code, limit, None) {
            for r in &results {
                total += 1;
                out.push_str(&format!(
                    "### Similar {} [local] — {}:{}-{} (score {:.2})\n```{}\n{}\n```\n\n",
                    total,
                    r.file_path.replace('\\', "/"),
                    r.line_start, r.line_end,
                    r.score,
                    r.language.short_name(),
                    r.content.trim()
                ));
            }
        }
    }

    if total == 0 { Ok("No similar code found.".into()) }
    else { Ok(format!("Found {total} similar chunk(s):\n\n{out}")) }
}

async fn tool_get_stats(
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let mut out = String::from("## Needle Index Overview\n\n");

    if let Some((engine, graph)) = local {
        let files = engine.file_list();
        let s = &graph.stats;
        out.push_str("### Local Index\n");
        out.push_str(&format!("- **Files**: {}\n", files.len()));
        out.push_str(&format!("- **Chunks**: {}\n", engine.chunks.len()));
        out.push_str(&format!("- **Graph nodes**: {}\n", s.total_nodes));
        out.push_str(&format!("- **Graph edges**: {}\n", s.total_edges));
        out.push_str(&format!("- **Functions**: {}\n", s.functions));
        out.push_str(&format!("- **Methods**: {}\n", s.methods));
        out.push_str(&format!("- **Structs/Classes/Traits**: {}\n", s.classes));

        out.push_str("\n**Languages:**\n");
        let mut lang_counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for f in &files { *lang_counts.entry(f.lang.as_str()).or_default() += 1; }
        let mut langs: Vec<_> = lang_counts.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1));
        for (lang, count) in langs { out.push_str(&format!("- {lang}: {count} file(s)\n")); }
    } else {
        out.push_str("### Local Index\nNo local index (run `needle init <dirs...>` to index a codebase)\n");
    }

    if let Some(cfg) = cloud {
        out.push_str(&format!("\n### Cloud ({}) — connected\n", cfg.base_url));
        out.push_str(&format!("- **API key**: {}…\n", &cfg.api_key[..cfg.api_key.len().min(12)]));
        out.push_str("- Search queries include your indexed GitHub repos\n");
        out.push_str("- Graph tools (callers, file structure) use the local index only\n");
    } else {
        out.push_str("\n### Cloud\nNot configured (set NEEDLE_API_KEY + NEEDLE_CLOUD_URL to enable)\n");
    }

    Ok(out)
}

fn tool_find_callers(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let name = args["name"].as_str().unwrap_or("");
    if name.is_empty() { return Err("name is required".into()); }

    let targets: Vec<u32> = graph
        .nodes.iter()
        .filter(|n| n.name == name || n.name.ends_with(&format!("::{name}")))
        .map(|n| n.id)
        .collect();

    if targets.is_empty() { return Ok(format!("No symbol named '{name}' found in the graph.")); }

    let callers: Vec<_> = graph
        .edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls) && targets.contains(&e.to))
        .filter_map(|e| graph.nodes.get(e.from as usize))
        .collect();

    if callers.is_empty() { return Ok(format!("'{name}' is not called by any indexed function.")); }

    let mut out = format!("Callers of '{name}' ({} found):\n\n", callers.len());
    for c in &callers {
        out.push_str(&format!("- **{}** ({}) — {}:{}\n",
            c.name, node_kind_label(&c.kind), c.file_path.replace('\\', "/"), c.line_start));
    }
    Ok(out)
}

fn tool_find_callees(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let name = args["name"].as_str().unwrap_or("");
    if name.is_empty() { return Err("name is required".into()); }

    let sources: Vec<u32> = graph
        .nodes.iter()
        .filter(|n| n.name == name || n.name.ends_with(&format!("::{name}")))
        .map(|n| n.id)
        .collect();

    if sources.is_empty() { return Ok(format!("No symbol named '{name}' found in the graph.")); }

    let callees: Vec<_> = graph
        .edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls) && sources.contains(&e.from))
        .filter_map(|e| graph.nodes.get(e.to as usize))
        .collect();

    if callees.is_empty() { return Ok(format!("'{name}' does not call any other indexed function.")); }

    let mut out = format!("'{name}' calls {} function(s):\n\n", callees.len());
    for c in &callees {
        out.push_str(&format!("- **{}** ({}) — {}:{}\n",
            c.name, node_kind_label(&c.kind), c.file_path.replace('\\', "/"), c.line_start));
    }
    Ok(out)
}

fn tool_get_endpoints(graph: &CodeGraph) -> Result<String, String> {
    let endpoints: Vec<_> = graph.nodes.iter().filter(|n| n.kind == NodeKind::Endpoint).collect();
    if endpoints.is_empty() { return Ok("No API endpoints found in the index.".into()); }

    let mut out = format!("{} API endpoint(s):\n\n", endpoints.len());
    for ep in &endpoints {
        let method = ep.detail.as_deref().unwrap_or("?");
        out.push_str(&format!("- **{}** [{}] — {}:{}\n",
            ep.name, method, ep.file_path.replace('\\', "/"), ep.line_start));
    }
    Ok(out)
}

fn tool_get_file_structure(args: &Value, graph: &CodeGraph) -> Result<String, String> {
    let file_query = args["file"].as_str().unwrap_or("");
    if file_query.is_empty() { return Err("file is required".into()); }

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
        .edges.iter()
        .filter(|e| e.from == module.id && matches!(e.kind, EdgeKind::Contains))
        .filter_map(|e| graph.nodes.get(e.to as usize))
        .collect();

    let file = module.file_path.replace('\\', "/");
    let mut out = format!("**{}** ({} definition(s)):\n\n", file, children.len());

    let mut sorted = children.clone();
    sorted.sort_by_key(|n| (node_kind_order(&n.kind), n.line_start));

    for n in sorted {
        let kind   = node_kind_label(&n.kind);
        let detail = n.detail.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default();
        out.push_str(&format!("- L{}-{}: **{}** ({kind}{detail})\n", n.line_start, n.line_end, n.name));
    }
    Ok(out)
}

async fn tool_explain(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    llm:   &needle::llm::LlmClient,
) -> Result<String, String> {
    let question = args["question"].as_str().unwrap_or("").trim().to_string();
    if question.is_empty() { return Err("question is required".into()); }
    let chunks = args["context_chunks"].as_u64().unwrap_or(6).min(12) as usize;

    let (engine, graph) = local.ok_or_else(||
        "explain requires a local index. Run: needle init <dirs...>".to_string()
    )?;

    // ── 1. Retrieve relevant code via hybrid search ───────────────────────────
    let (results, _) = engine.search(&question, chunks, None)
        .map_err(|e| e.to_string())?;

    if results.is_empty() {
        return Ok(format!("No relevant code found for: {question}"));
    }

    let mut ctx = String::new();

    // ── 2. Relevant code chunks ───────────────────────────────────────────────
    ctx.push_str("## Relevant Code\n\n");
    for (i, r) in results.iter().enumerate() {
        ctx.push_str(&format!(
            "### [{i}] {}:{}-{}\n```{}\n{}\n```\n\n",
            r.file_path.replace('\\', "/"), r.line_start, r.line_end,
            r.language.short_name(), r.content.trim()
        ));
    }

    // ── 3. Call-graph context for the top result's primary symbol ─────────────
    // Find the function node closest to the top search result's line
    let top_file = &results[0].file_path;
    let top_line = results[0].line_start;
    let top_node = graph.nodes.iter()
        .filter(|n| &n.file_path == top_file
            && matches!(n.kind, NodeKind::Function | NodeKind::Method)
            && n.line_start <= top_line && top_line <= n.line_end)
        .min_by_key(|n| top_line - n.line_start);

    if let Some(node) = top_node {
        // Callers
        let callers: Vec<&needle::graph::GraphNode> = graph.edges.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Calls) && e.to == node.id)
            .filter_map(|e| graph.nodes.get(e.from as usize))
            .collect();
        // Callees
        let callees: Vec<&needle::graph::GraphNode> = graph.edges.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Calls) && e.from == node.id)
            .filter_map(|e| graph.nodes.get(e.to as usize))
            .collect();

        if !callers.is_empty() || !callees.is_empty() {
            ctx.push_str(&format!("## Call Graph — `{}`\n", node.name));
            if !callers.is_empty() {
                ctx.push_str("**Called by:** ");
                ctx.push_str(&callers.iter().map(|n| format!("`{}`", n.name)).collect::<Vec<_>>().join(", "));
                ctx.push('\n');
            }
            if !callees.is_empty() {
                ctx.push_str("**Calls:** ");
                ctx.push_str(&callees.iter().map(|n| format!("`{}`", n.name)).collect::<Vec<_>>().join(", "));
                ctx.push('\n');
            }
            ctx.push('\n');
        }
    }

    // ── 4. Community context ──────────────────────────────────────────────────
    let communities = graph::compute_communities(graph);
    let top_community = graph.nodes.iter()
        .find(|n| &n.file_path == top_file && matches!(n.kind, NodeKind::Module))
        .and_then(|n| communities.get(&n.id))
        .copied();

    if let Some(cid) = top_community {
        let cluster: Vec<&needle::graph::GraphNode> = graph.nodes.iter()
            .filter(|n| communities.get(&n.id) == Some(&cid)
                && !matches!(n.kind, NodeKind::Module))
            .collect();
        if !cluster.is_empty() {
            ctx.push_str(&format!("## Cluster (community {cid}) — {} symbols\n", cluster.len()));
            for m in cluster.iter().take(12) {
                ctx.push_str(&format!("- `{}` ({})\n", m.name, node_kind_label(&m.kind)));
            }
            if cluster.len() > 12 {
                ctx.push_str(&format!("- … and {} more\n", cluster.len() - 12));
            }
            ctx.push('\n');
        }
    }

    // ── 5. God nodes (architectural load-bearers) ─────────────────────────────
    let gods = graph::compute_god_nodes(graph, 5);
    if !gods.is_empty() {
        ctx.push_str("## Architectural Core (highest-degree symbols)\n");
        for (nid, deg) in &gods {
            if let Some(n) = graph.nodes.get(*nid as usize) {
                ctx.push_str(&format!("- `{}` — degree {}\n", n.name, deg));
            }
        }
        ctx.push('\n');
    }

    // ── 6. Call LLM ───────────────────────────────────────────────────────────
    let system = "\
You are an expert software architect helping a developer understand a codebase. \
You have been given: (1) the most relevant code chunks retrieved by hybrid BM25+vector search, \
(2) the call graph around the top result, (3) its semantic community cluster, \
and (4) the highest-degree architectural symbols. \
Answer WHY the code is structured this way — focus on design intent, trade-offs, \
and architectural patterns. Cite specific function names, file paths, and line numbers. \
If the provided context doesn't contain enough information to explain the 'why', say so clearly.";

    let user_msg = format!("## Codebase Context\n\n{ctx}## Question\n\n{question}");

    let answer = llm.complete(system, &user_msg).await
        .map_err(|e| format!("LLM error: {e}\n\nTip: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GROQ_API_KEY"))?;

    Ok(format!(
        "**Model:** {}\n**Context:** {} chunks · call graph · community · god nodes\n\n---\n\n{}",
        llm.display_name(), results.len(), answer
    ))
}

fn tool_get_god_nodes(args: &Value, g: &CodeGraph) -> Result<String, String> {
    let limit = args["limit"].as_u64().unwrap_or(10).min(20) as usize;
    let ranked = graph::compute_god_nodes(g, limit);

    if ranked.is_empty() {
        return Ok("No call-graph edges found. Run `needle init` on a codebase with functions.".into());
    }

    let mut out = format!("## God Nodes — Top {} by call-graph degree\n\n", ranked.len());
    out.push_str("These are the most connected symbols: highest-degree nodes are architectural load-bearers.\n\n");

    for (rank, (node_id, degree)) in ranked.iter().enumerate() {
        if let Some(node) = g.nodes.get(*node_id as usize) {
            out.push_str(&format!(
                "{}. **{}** ({}) — degree **{}** — {}:{}\n",
                rank + 1,
                node.name,
                node_kind_label(&node.kind),
                degree,
                node.file_path.replace('\\', "/"),
                node.line_start,
            ));
        }
    }
    Ok(out)
}

fn tool_get_communities(g: &CodeGraph) -> Result<String, String> {
    let communities = graph::compute_communities(g);

    // Group nodes by community
    let mut by_community: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for (&node_id, &community_id) in &communities {
        by_community.entry(community_id).or_default().push(node_id);
    }

    let mut sorted: Vec<(u32, Vec<u32>)> = by_community.into_iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len())); // largest first

    let total = sorted.len();
    let mut out = format!("## Semantic Communities ({total} clusters)\n\n");
    out.push_str("Detected via label propagation on call + import edges.\n\n");

    for (community_id, node_ids) in &sorted {
        // Collect non-module nodes, sorted by name
        let mut members: Vec<&needle::graph::GraphNode> = node_ids.iter()
            .filter_map(|&id| g.nodes.get(id as usize))
            .filter(|n| !matches!(n.kind, NodeKind::Module))
            .collect();
        if members.is_empty() { continue; }
        members.sort_by(|a, b| a.name.cmp(&b.name));

        // Representative file = most common file among members
        let mut file_freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for m in &members { *file_freq.entry(m.file_path.as_str()).or_default() += 1; }
        let rep_file = file_freq.iter().max_by_key(|(_, &c)| c)
            .map(|(&f, _)| f.replace('\\', "/").split('/').last().unwrap_or(f).to_string())
            .unwrap_or_default();

        out.push_str(&format!("### Cluster {} — {} symbols · `{}`\n", community_id, members.len(), rep_file));
        for m in members.iter().take(8) {
            out.push_str(&format!("  - **{}** ({})\n", m.name, node_kind_label(&m.kind)));
        }
        if members.len() > 8 {
            out.push_str(&format!("  - … and {} more\n", members.len() - 8));
        }
        out.push('\n');
    }
    Ok(out)
}

fn tool_get_surprises(g: &CodeGraph) -> Result<String, String> {
    let communities = graph::compute_communities(g);
    let surprises   = graph::find_surprise_edges(g, &communities);

    if surprises.is_empty() {
        return Ok("No surprise edges found — the codebase is well-clustered with no unexpected cross-module calls.".into());
    }

    let mut out = format!("## Surprise Edges ({} found)\n\n", surprises.len());
    out.push_str("These are calls that bridge otherwise-disconnected clusters — unexpected dependencies worth reviewing.\n\n");

    for (from_id, to_id) in &surprises {
        let from = g.nodes.get(*from_id as usize);
        let to   = g.nodes.get(*to_id as usize);
        if let (Some(f), Some(t)) = (from, to) {
            let from_file = f.file_path.replace('\\', "/").split('/').last().unwrap_or("").to_string();
            let to_file   = t.file_path.replace('\\', "/").split('/').last().unwrap_or("").to_string();
            out.push_str(&format!(
                "- **{}** (`{}`) → **{}** (`{}`)\n",
                f.name, from_file, t.name, to_file
            ));
        }
    }
    Ok(out)
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
            "description": "Cluster the codebase into semantic communities using label propagation on the call graph. Each cluster groups symbols that frequently call each other — revealing the architectural layers of the system.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_surprises",
            "description": "Find unexpected cross-cluster dependencies — function calls that bridge otherwise-disconnected parts of the codebase. These are the hidden couplings worth reviewing for architecture improvements.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "explain",
            "description": "Ask WHY the code is structured a certain way. Uses hybrid search to find relevant code, enriches it with call-graph context + community + god nodes, then asks an LLM to explain the design intent and trade-offs. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, or run Ollama locally.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question":       { "type": "string",  "description": "The 'why' question about the codebase (e.g. 'Why does resolve_user check cookies before Bearer tokens?')" },
                    "context_chunks": { "type": "integer", "description": "Number of code chunks to retrieve as context (default 6, max 12)", "default": 6 }
                },
                "required": ["question"]
            }
        }
    ])
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
