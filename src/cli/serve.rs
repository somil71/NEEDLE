//! `needle serve` — local web UI backed by axum.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use tower_cookies::CookieManagerLayer;
use colored::Colorize;
use needle::{
    embedding::EmbeddingModel,
    graph::CodeGraph,
    query::QueryEngine,
    schema::Language,
    storage::Storage,
};
use serde::{Deserialize, Serialize};
use needle::server::{oauth, users};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tower_cookies::Cookies;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    engine:     Option<Arc<Mutex<QueryEngine>>>,
    storage:    Option<Storage>,
    graph:      Arc<CodeGraph>,
    has_ollama: bool,
}

macro_rules! require_index {
    ($state:expr) => {
        match $state.engine.as_ref() {
            Some(e) => e,
            None => return Json(serde_json::json!({"error": "no_index", "message": "No index found. Run needle init first."})).into_response(),
        }
    };
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    lang: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<ResultJson>,
    timing: TimingJson,
    total: usize,
}

#[derive(Serialize, Clone)]
struct ResultJson {
    chunk_id: u64,
    file_path: String,
    line_start: u32,
    line_end: u32,
    language: String,
    chunk_type: String,
    content: String,
    score: f32,
    signal: String,
}

#[derive(Serialize)]
struct TimingJson {
    total_ms: f64,
    bm25_ms: f64,
    hnsw_ms: f64,
    embed_ms: f64,
    fusion_ms: f64,
}

#[derive(Serialize)]
struct StatusResponse {
    total_chunks: u64,
    total_files: u64,
    watched_dirs: Vec<String>,
    last_update_ts: u64,
    embedding_model: String,
    embedding_dim: u32,
    vocabulary: u32,
    disk_bytes: u64,
    languages: Vec<String>,
    has_ollama: bool,
    is_cloud: bool,
    has_index: bool,
}

#[derive(Deserialize)]
struct OpenRequest {
    path: String,
    line: Option<u32>,
}

#[derive(Deserialize)]
struct AskRequest {
    question: String,
    model: Option<String>,
}

#[derive(Serialize)]
struct AskResponse {
    answer: String,
    sources: Vec<ResultJson>,
    model_used: String,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SimilarRequest {
    code: String,
    exclude_id: Option<u64>,
    limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(port: u16, no_open: bool) -> needle::Result<()> {
    // ── Determine bind address ──────────────────────────────────────────────
    // Railway injects $PORT; locally we use the CLI arg.
    // 0.0.0.0 is required for Railway (container networking).
    // Locally we still open the browser on localhost.
    let bind_port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(port);

    let is_cloud = std::env::var("RAILWAY_ENVIRONMENT").is_ok()
        || std::env::var("RENDER").is_ok();

    let bind_addr = if is_cloud {
        format!("0.0.0.0:{}", bind_port)
    } else {
        format!("127.0.0.1:{}", bind_port)
    };

    // ── Load search index (optional — marketing mode if missing) ───────────
    let has_index = Storage::index_exists();

    let state = if has_index {
        let storage = Storage::new(Storage::default_index_dir())?;
        let config   = Storage::load_config().unwrap_or_default();
        let meta     = storage.load_metadata().unwrap_or_default();
        let bm25     = storage.load_bm25()?;
        let hnsw     = storage.load_hnsw()?;
        let chunks   = storage.load_chunks()?;
        let embedding = EmbeddingModel::from_metadata(&meta.embedding_model, meta.embedding_dim as usize)?;
        let graph    = storage.load_graph().unwrap_or_default();
        let has_ollama = embedding.is_ollama();
        let mut engine = QueryEngine::new(bm25, hnsw, chunks, embedding);
        engine.ef_search = config.hnsw_ef_search as usize;
        engine.rrf_k     = config.rrf_k;
        AppState {
            engine:     Some(Arc::new(Mutex::new(engine))),
            storage:    Some(storage),
            graph:      Arc::new(graph),
            has_ollama,
        }
    } else {
        if !is_cloud {
            eprintln!("{} No index found — running in marketing mode.\n  Run: needle init <dirs...> to index your code.",
                "Note:".yellow().bold());
        }
        AppState {
            engine:    None,
            storage:   None,
            graph:     Arc::new(needle::graph::CodeGraph::default()),
            has_ollama: false,
        }
    };

    // ── OAuth config (optional — only if env vars are set) ─────────────────
    let oauth_cfg = oauth::OAuthConfig::from_env();
    let has_oauth = oauth_cfg.is_some();

    // ── Router ─────────────────────────────────────────────────────────────
    let mut app = Router::new()
        .route("/", get(serve_ui))
        // Search APIs
        .route("/api/search",  get(api_search))
        .route("/api/status",  get(api_status_handler))
        .route("/api/open",    post(api_open))
        .route("/api/ask",     post(api_ask))
        .route("/api/similar", post(api_similar))
        .route("/api/todos",   get(api_todos))
        .route("/api/files",   get(api_files))
        .route("/api/graph",   get(api_graph))
        // Auth & user APIs (always registered; return 503 if OAuth not configured)
        .route("/auth/github",          get(api_auth_github))
        .route("/auth/callback",        get(api_auth_callback))
        .route("/auth/logout",          get(api_auth_logout))
        .route("/api/me",                   get(api_me))
        .route("/api/me/regenerate-key",    post(api_regenerate_key))
        .route("/api/me/revoke-key",        post(api_revoke_key))
        .route("/api/validate-key",         post(api_validate_key))
        .route("/api/github/repos",         get(api_github_repos_handler))
        .route("/api/repos/connect",        post(api_repo_connect))
        .with_state(state)
        .layer(CookieManagerLayer::new());

    if let Some(tower_http_cors) = build_cors() {
        app = app.layer(tower_http_cors);
    }

    // ── Start ──────────────────────────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.map_err(|e| {
        needle::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    })?;

    let url = format!("http://localhost:{}", bind_port);
    println!();
    println!("  {}  {}", "Needle".bold(), url.cyan().bold());
    if has_index {
        println!("  {} Index loaded", "✓".green());
    } else {
        println!("  {} No index — marketing mode (run 'needle init' to index code)", "·".dimmed());
    }
    if has_oauth {
        println!("  {} GitHub OAuth configured", "✓".green());
    }
    println!("  {} Ctrl+C to stop\n", "·".dimmed());

    if !no_open && !is_cloud {
        let _ = open::that(&url);
    }

    axum::serve(listener, app).await.map_err(|e| {
        needle::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    })?;

    Ok(())
}

fn build_cors() -> Option<tower_http::cors::CorsLayer> {
    Some(tower_http::cors::CorsLayer::permissive())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn serve_ui() -> Html<&'static str> {
    Html(include_str!("../assets/ui.html"))
}

async fn api_search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Json<SearchResponse> {
    let empty = Json(SearchResponse {
        results: vec![],
        timing: TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 },
        total: 0,
    });

    let engine = match state.engine.as_ref() { Some(e) => e.clone(), None => return empty };
    let q = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(12).min(50);
    let lang_filter = params.lang.as_deref().and_then(Language::from_short);
    if q.trim().is_empty() { return empty; }

    let q_clone = q.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        guard.search(&q_clone, limit, lang_filter).map_err(|e| e.to_string())
    }).await;

    let (results, timing) = match result { Ok(Ok(r)) => r, _ => return empty };
    let total = results.len();
    Json(SearchResponse {
        total,
        results: results.into_iter().map(to_result_json).collect(),
        timing: TimingJson {
            total_ms: timing.total_ms, bm25_ms: timing.bm25_ms,
            hnsw_ms: timing.hnsw_ms, embed_ms: timing.embed_ms, fusion_ms: timing.fusion_ms,
        },
    })
}

async fn api_status_handler(State(state): State<AppState>) -> Json<StatusResponse> {
    let has_ollama = state.has_ollama;
    let (meta, disk) = match state.storage.as_ref() {
        Some(s) => (s.load_metadata().unwrap_or_default(), s.index_size_bytes()),
        None    => (Default::default(), 0),
    };
    let (languages, vocabulary) = match state.engine.as_ref() {
        Some(e) => {
            let engine = e.lock().unwrap();
            let mut set = HashSet::new();
            for chunk in engine.chunks.values() { set.insert(chunk.language.short_name().to_string()); }
            let vocab = engine.bm25.vocabulary_size() as u32;
            let mut v: Vec<String> = set.into_iter().collect(); v.sort();
            (v, vocab)
        }
        None => (vec![], 0),
    };
    let is_cloud = std::env::var("RAILWAY_ENVIRONMENT").is_ok() || std::env::var("RENDER").is_ok();
    let has_index = state.engine.is_some();
    Json(StatusResponse {
        total_chunks: meta.total_chunks, total_files: meta.total_files,
        watched_dirs: meta.watched_dirs, last_update_ts: meta.last_update_ts,
        embedding_model: meta.embedding_model, embedding_dim: meta.embedding_dim,
        vocabulary, disk_bytes: disk, languages, has_ollama, is_cloud, has_index,
    })
}

async fn api_graph(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*state.graph).unwrap_or(serde_json::json!({"nodes":[],"edges":[],"stats":{}})))
}

async fn api_open(Json(req): Json<OpenRequest>) -> Json<serde_json::Value> {
    let clean = strip_unc(&req.path);
    let goto = req.line.map(|l| format!("{}:{}", clean, l)).unwrap_or(clean.clone());

    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("cmd")
        .args(["/c", "code", "--goto", &goto])
        .creation_flags(0x08000000)
        .spawn()
        .is_ok();

    #[cfg(not(target_os = "windows"))]
    let opened = std::process::Command::new("code")
        .args(["--goto", &goto])
        .spawn()
        .is_ok();

    if !opened {
        if let Some(parent) = std::path::Path::new(&clean).parent() {
            let _ = open::that(parent);
        }
    }

    Json(serde_json::json!({ "ok": true }))
}

async fn api_ask(
    State(state): State<AppState>,
    Json(req): Json<AskRequest>,
) -> Json<AskResponse> {
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Json(AskResponse {
            answer: String::new(),
            sources: vec![],
            model_used: String::new(),
            error: Some("Empty question".into()),
        });
    }

    // Step 1: Retrieve relevant chunks via hybrid search
    let engine = match state.engine.as_ref() {
        Some(e) => e.clone(),
        None => return Json(AskResponse { answer: String::new(), sources: vec![], model_used: String::new(), error: Some("No index".into()) }),
    };
    let q = question.clone();
    let search_result = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        guard.search(&q, 8, None).map_err(|e| e.to_string())
    }).await;

    let (results, _timing) = match search_result {
        Ok(Ok(r)) => r,
        _ => return Json(AskResponse {
            answer: String::new(),
            sources: vec![],
            model_used: String::new(),
            error: Some("Search failed".into()),
        }),
    };

    // Step 2: Build context from top chunks
    let context: String = results.iter().enumerate().map(|(i, r)| {
        format!(
            "### Source {} — {}:{}-{}\n```{}\n{}\n```",
            i + 1,
            r.file_path.replace('\\', "/"),
            r.line_start, r.line_end,
            r.language.short_name(),
            r.content
        )
    }).collect::<Vec<_>>().join("\n\n");

    let prompt = format!(
        "You are an expert code assistant. The user has a question about their codebase.\n\
        Below are the most relevant code chunks retrieved from the index.\n\n\
        {context}\n\n\
        ---\n\
        Question: {question}\n\n\
        Answer concisely and accurately. Reference specific file names and functions where relevant. \
        Use markdown formatting. If the context doesn't fully answer the question, say so clearly."
    );

    let sources: Vec<ResultJson> = results.into_iter().map(to_result_json).collect();

    // Step 3: Try Ollama, then Groq, then return error
    let preferred_model = req.model.as_deref().unwrap_or("llama3.2");
    match call_ollama_chat(&prompt, preferred_model).await {
        Ok(answer) => Json(AskResponse {
            answer,
            sources,
            model_used: format!("Ollama/{}", preferred_model),
            error: None,
        }),
        Err(ollama_err) => {
            // Try Groq fallback
            match call_groq_chat(&prompt).await {
                Ok((answer, model)) => Json(AskResponse {
                    answer,
                    sources,
                    model_used: format!("Groq/{}", model),
                    error: None,
                }),
                Err(_) => Json(AskResponse {
                    answer: String::new(),
                    sources,
                    model_used: String::new(),
                    error: Some(format!(
                        "No LLM available.\n\
                        • Ollama: {}\n  Fix: run `ollama serve` and `ollama pull {}`\n\
                        • Groq: GROQ_API_KEY env var not set\n  Fix: get a free key at console.groq.com",
                        ollama_err, preferred_model
                    )),
                }),
            }
        }
    }
}

async fn api_similar(
    State(state): State<AppState>,
    Json(req): Json<SimilarRequest>,
) -> Json<SearchResponse> {
    let limit = req.limit.unwrap_or(10).min(30);
    let code = req.code.clone();
    let exclude_id = req.exclude_id;
    let engine = match state.engine.as_ref() { Some(e) => e.clone(), None => return Json(SearchResponse { results: vec![], timing: TimingJson { total_ms:0.0,bm25_ms:0.0,hnsw_ms:0.0,embed_ms:0.0,fusion_ms:0.0 }, total: 0 }) };

    let result = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        guard.search_similar(&code, limit, exclude_id).map_err(|e| e.to_string())
    }).await;

    let results = match result {
        Ok(Ok(r)) => r,
        _ => vec![],
    };

    let total = results.len();
    Json(SearchResponse {
        total,
        results: results.into_iter().map(to_result_json).collect(),
        timing: TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 },
    })
}

async fn api_todos(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = match state.engine.as_ref() { Some(e) => e.clone(), None => return Json(serde_json::json!({"todos":[],"total":0})) };
    let todos = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        guard.scan_todos()
    }).await.unwrap_or_default();

    let total = todos.len();
    let json_todos: Vec<serde_json::Value> = todos.into_iter().map(|t| serde_json::json!({
        "file_path": strip_unc(&t.file_path),
        "line": t.line,
        "kind": t.kind,
        "text": t.text,
        "language": t.language,
    })).collect();

    Json(serde_json::json!({ "todos": json_todos, "total": total }))
}

async fn api_files(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = match state.engine.as_ref() { Some(e) => e.clone(), None => return Json(serde_json::json!({"files":[],"total":0})) };
    let files = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        guard.file_list()
    }).await.unwrap_or_default();

    let total = files.len();
    let json_files: Vec<serde_json::Value> = files.into_iter().map(|f| serde_json::json!({
        "path": strip_unc(&f.path),
        "chunks": f.chunks,
        "lang": f.lang,
    })).collect();

    Json(serde_json::json!({ "files": json_files, "total": total }))
}

// ---------------------------------------------------------------------------
// LLM helpers
// ---------------------------------------------------------------------------

async fn call_ollama_chat(prompt: &str, model: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": false
    });

    let resp = client
        .post("http://localhost:11434/api/chat")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("connection refused — is Ollama running? ({})", e))?;

    if resp.status() == 404 {
        return Err(format!("model '{}' not found — run `ollama pull {}`", model, model));
    }
    if !resp.status().is_success() {
        return Err(format!("Ollama HTTP {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(data["message"]["content"].as_str().unwrap_or("").to_string())
}

async fn call_groq_chat(prompt: &str) -> Result<(String, String), String> {
    let api_key = std::env::var("GROQ_API_KEY").map_err(|_| "GROQ_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    const MODEL: &str = "llama-3.1-8b-instant";
    let body = serde_json::json!({
        "model": MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 1024
    });

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Groq HTTP {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let answer = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok((answer, MODEL.to_string()))
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn to_result_json(r: needle::schema::SearchResult) -> ResultJson {
    ResultJson {
        chunk_id: r.chunk_id,
        file_path: strip_unc(&r.file_path),
        line_start: r.line_start,
        line_end: r.line_end,
        language: r.language.short_name().to_string(),
        chunk_type: r.chunk_type.badge().to_string(),
        content: r.content,
        score: r.score,
        signal: r.signals.label().to_string(),
    }
}

fn strip_unc(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

// ---------------------------------------------------------------------------
// Auth handlers
// ---------------------------------------------------------------------------

async fn api_auth_github() -> axum::response::Response {
    let cfg = match oauth::OAuthConfig::from_env() {
        Some(c) => std::sync::Arc::new(c),
        None    => return (StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"GitHub OAuth not configured"}))).into_response(),
    };
    oauth::auth_github(State(cfg)).await.into_response()
}

async fn api_auth_callback(
    Query(params): Query<oauth::OAuthCallbackParams>,
    cookies: Cookies,
) -> axum::response::Response {
    let cfg = match oauth::OAuthConfig::from_env() {
        Some(c) => std::sync::Arc::new(c),
        None    => return (StatusCode::SERVICE_UNAVAILABLE,
            axum::response::Html("<p>OAuth not configured</p>".to_string())).into_response(),
    };
    oauth::auth_callback(State(cfg), Query(params), cookies).await.into_response()
}

async fn api_auth_logout(cookies: Cookies) -> axum::response::Response {
    oauth::auth_logout(cookies).await.into_response()
}

/// GET /api/me — returns current user or 401
async fn api_me(cookies: Cookies) -> axum::response::Response {
    match oauth::current_user_from_cookies(&cookies) {
        Some(u) => Json(serde_json::json!({
            "id": u.id,
            "username": u.github_username,
            "avatar": u.github_avatar,
            "api_key": u.api_key,
        })).into_response(),
        None => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    }
}

/// POST /api/validate-key — local CLI calls this to verify its API key is valid
/// Body: {"key": "ndk_..."}
async fn api_validate_key(
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let key = match body.get("key").and_then(|v| v.as_str()) {
        Some(k) => k.to_string(),
        None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"missing key"}))).into_response(),
    };
    let conn = match users::open_db() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"db error"}))).into_response(),
    };
    match users::get_user_by_api_key(&conn, &key) {
        Ok(Some(u)) => {
            users::touch_last_seen(&conn, &u.id);
            Json(serde_json::json!({
                "valid": true,
                "username": u.github_username,
                "user_id": u.id,
            })).into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"valid":false,"error":"invalid or revoked key"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// GET /api/github/repos — list user's GitHub repos (proxied, keeps token server-side)
async fn api_github_repos_handler(cookies: Cookies) -> axum::response::Response {
    oauth::api_github_repos(cookies).await.into_response()
}

/// POST /api/repos/connect — save a repo to user's account
/// Body: {"repo_full": "owner/name", "repo_url": "https://github.com/...", "private": false}
async fn api_repo_connect(
    cookies: Cookies,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies) {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let full    = body.get("repo_full").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let url     = body.get("repo_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let private = body.get("private").and_then(|v| v.as_bool()).unwrap_or(false);
    let name    = full.split('/').last().unwrap_or(&full).to_string();
    if full.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"repo_full required"}))).into_response();
    }
    let conn = match users::open_db() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    match users::upsert_repo(&conn, &user.id, &name, &full, &url, private) {
        Ok(repo_id) => Json(serde_json::json!({"ok":true,"repo_id":repo_id,"status":"pending"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// POST /api/me/revoke-key — disable API key without issuing a new one
async fn api_revoke_key(cookies: Cookies) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies) {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let conn = match users::open_db() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    match conn.execute("UPDATE users SET is_active=0 WHERE id=?1", rusqlite::params![user.id]) {
        Ok(_) => Json(serde_json::json!({"ok":true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// POST /api/me/regenerate-key — issue a new API key, invalidate the old one
async fn api_regenerate_key(cookies: Cookies) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies) {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let conn = match users::open_db() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    let new_key = users::generate_api_key();
    match conn.execute("UPDATE users SET api_key=?1, is_active=1 WHERE id=?2", rusqlite::params![new_key, user.id]) {
        Ok(_) => Json(serde_json::json!({"ok":true,"api_key":new_key})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}
