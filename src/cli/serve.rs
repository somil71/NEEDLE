//! `needle serve` — local web UI backed by axum.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
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
use needle::server::{indexer, oauth, users};
use std::collections::HashMap;
use std::collections::HashSet;
use tokio::sync::RwLock;
use std::sync::{Arc, Mutex};
use tower_cookies::Cookies;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Per-user loaded engine (cached after first load)
struct UserEngine {
    engine: Arc<Mutex<QueryEngine>>,
}

#[derive(Clone)]
struct AppState {
    engine:      Option<Arc<Mutex<QueryEngine>>>,
    storage:     Option<Storage>,
    graph:       Arc<CodeGraph>,
    has_ollama:  bool,
    /// user_id → their loaded engine (cloud repos)
    user_engines: Arc<RwLock<HashMap<String, UserEngine>>>,
    /// Root dir for per-user indexes (/data/indexes on cloud)
    indexes_dir:  Arc<std::path::PathBuf>,
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

    // ── Data directory (persistent volume on cloud, ~/.needle locally) ───────
    let data_dir = std::path::PathBuf::from(
        std::env::var("DATA_DIR").unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".needle")
                .to_string_lossy()
                .to_string()
        })
    );

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
            engine:       Some(Arc::new(Mutex::new(engine))),
            storage:      Some(storage),
            graph:        Arc::new(graph),
            has_ollama,
            user_engines: Arc::new(RwLock::new(HashMap::new())),
            indexes_dir:  Arc::new(data_dir.join("indexes")),
        }
    } else {
        if !is_cloud {
            eprintln!("{} No index found — running in marketing mode.\n  Run: needle init <dirs...> to index your code.",
                "Note:".yellow().bold());
        }
        AppState {
            engine:       None,
            storage:      None,
            graph:        Arc::new(needle::graph::CodeGraph::default()),
            has_ollama:   false,
            user_engines: Arc::new(RwLock::new(HashMap::new())),
            indexes_dir:  Arc::new(data_dir.join("indexes")),
        }
    };

    // ── Background indexer (picks up pending repos every 30s) ─────────────
    {
        let cfg = Arc::new(indexer::IndexerConfig { data_dir: data_dir.clone() });
        tokio::spawn(indexer::run(cfg));
    }

    // ── OAuth config (optional — only if env vars are set) ─────────────────
    let has_oauth = oauth::OAuthConfig::from_env().is_some();

    // ── Mode info (passed to frontend at startup) ──────────────────────────
    let mode_info = serde_json::json!({
        "is_cloud":     is_cloud,
        "has_oauth":    has_oauth,
        "has_index":    has_index,
        "max_repos":    MAX_REPOS_PER_USER,
    });

    // ── Router ─────────────────────────────────────────────────────────────
    let mut app = Router::new()
        .route("/", get(serve_ui))
        // Mode discovery — lets the UI adapt to cloud vs local vs desktop
        .route("/api/mode", get({
            let info = mode_info.clone();
            move || async move { Json(info) }
        }))
        // Search APIs
        .route("/api/search",  get(api_search))
        .route("/api/status",  get(api_status_handler))
        .route("/api/open",    post(api_open))
        .route("/api/ask",     post(api_ask))
        .route("/api/similar", post(api_similar))
        .route("/api/todos",         get(api_todos))
        .route("/api/files",         get(api_files))
        .route("/api/graph",         get(api_graph))
        .route("/api/blast-radius",  get(api_blast_radius))
        .route("/api/health",        get(api_health))
        .route("/api/security",      get(api_security))
        .route("/api/patterns",      get(api_patterns))
        .route("/api/git/churn",     get(api_git_churn))
        // Auth & user APIs (always registered; return 503 if OAuth not configured)
        .route("/auth/github",          get(api_auth_github))
        .route("/auth/callback",        get(api_auth_callback))
        .route("/auth/logout",          get(api_auth_logout))
        .route("/api/me",                   get(api_me))
        .route("/api/me/regenerate-key",    post(api_regenerate_key))
        .route("/api/me/revoke-key",        post(api_revoke_key))
        .route("/api/validate-key",         post(api_validate_key))
        .route("/api/github/repos",         get(api_github_repos_handler))
        .route("/api/repos",                get(api_repos))
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

/// Resolve user from either a session cookie or a Bearer API key header.
async fn resolve_user(cookies: &Cookies, headers: &HeaderMap) -> Option<users::User> {
    if let Some(u) = oauth::current_user_from_cookies(cookies).await {
        return Some(u);
    }
    let auth = headers.get("authorization")?.to_str().ok()?;
    let key  = auth.strip_prefix("Bearer ")?;
    let pool = users::pool().await.ok()?;
    users::get_user_by_api_key(pool, key).await.ok().flatten()
}

/// Load all `indexed` engines for the given user, caching them in AppState.
async fn load_cloud_engines(state: &AppState, user: Option<&users::User>) -> Vec<Arc<Mutex<QueryEngine>>> {
    let mut out = vec![];
    let Some(user) = user else { return out };
    let Ok(pool)  = users::pool().await else { return out };
    let Ok(repos) = users::list_user_repos(pool, &user.id).await else { return out };

    for repo in repos.into_iter().filter(|r| r.status == "indexed") {
        let safe_name  = repo.repo_full.replace('/', "_");
        let engine_key = format!("{}:{}", user.id, safe_name);

        let cached = state.user_engines.read().await.get(&engine_key).map(|ue| ue.engine.clone());

        let engine = if let Some(e) = cached {
            e
        } else {
            let idx_dir = state.indexes_dir.join(&user.id).join(&safe_name).join("index");
            if !idx_dir.exists() { continue; }
            match tokio::task::spawn_blocking(move || -> Result<QueryEngine, String> {
                let s  = Storage::new(idx_dir).map_err(|e| e.to_string())?;
                let m  = s.load_metadata().unwrap_or_default();
                let b  = s.load_bm25().map_err(|e| e.to_string())?;
                let h  = s.load_hnsw().map_err(|e| e.to_string())?;
                let ch = s.load_chunks().map_err(|e| e.to_string())?;
                let em = EmbeddingModel::from_metadata(&m.embedding_model, m.embedding_dim as usize)
                    .map_err(|e| e.to_string())?;
                Ok(QueryEngine::new(b, h, ch, em))
            }).await {
                Ok(Ok(qe)) => {
                    let arc = Arc::new(Mutex::new(qe));
                    state.user_engines.write().await.insert(engine_key, UserEngine { engine: arc.clone() });
                    arc
                }
                _ => continue,
            }
        };

        out.push(engine);
    }
    out
}

async fn api_search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
    cookies: Cookies,
    headers: HeaderMap,
) -> Json<SearchResponse> {
    let empty = Json(SearchResponse {
        results: vec![],
        timing: TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 },
        total: 0,
    });

    let q = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(12).min(50);
    let lang_filter = params.lang.as_deref().and_then(Language::from_short);
    if q.trim().is_empty() { return empty; }

    // ── Cloud path ────────────────────────────────────────────────────────
    let user = resolve_user(&cookies, &headers).await;
    let cloud = load_cloud_engines(&state, user.as_ref()).await;
    if !cloud.is_empty() {
        let mut all_results: Vec<needle::schema::SearchResult> = vec![];
        let mut last_timing = TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 };
        for engine in cloud {
            let q2 = q.clone();
            if let Ok(Ok((results, timing))) = tokio::task::spawn_blocking(move || {
                engine.lock().unwrap().search(&q2, limit, lang_filter).map_err(|e| e.to_string())
            }).await {
                all_results.extend(results);
                last_timing = TimingJson {
                    total_ms: timing.total_ms, bm25_ms: timing.bm25_ms,
                    hnsw_ms: timing.hnsw_ms, embed_ms: timing.embed_ms, fusion_ms: timing.fusion_ms,
                };
            }
        }
        if !all_results.is_empty() {
            all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            all_results.truncate(limit);
            let total = all_results.len();
            return Json(SearchResponse {
                total,
                results: all_results.into_iter().map(to_result_json).collect(),
                timing: last_timing,
            });
        }
    }

    // ── Local path ────────────────────────────────────────────────────────
    let engine = match state.engine.as_ref() { Some(e) => e.clone(), None => return empty };
    let q2 = q.clone();
    let result = tokio::task::spawn_blocking(move || {
        engine.lock().unwrap().search(&q2, limit, lang_filter).map_err(|e| e.to_string())
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

// ---------------------------------------------------------------------------
// Analysis handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct BlastParams {
    file: Option<String>,
}

async fn api_blast_radius(State(state): State<AppState>, Query(p): Query<BlastParams>) -> Json<serde_json::Value> {
    let file = match p.file {
        Some(f) => f,
        None => return Json(serde_json::json!({"error":"missing ?file= parameter"})),
    };
    let graph = Arc::clone(&state.graph);
    let result = tokio::task::spawn_blocking(move || needle::analysis::blast_radius(&graph, &file))
        .await
        .unwrap_or_else(|_| needle::analysis::BlastResult {
            source_file: String::new(), affected: vec![], total_files: 0, risk_score: 0,
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

async fn api_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let graph = Arc::clone(&state.graph);
    let result = tokio::task::spawn_blocking(move || needle::analysis::health_score(&graph))
        .await
        .unwrap_or_else(|_| needle::analysis::HealthReport {
            grade: "?".into(), score: 0, circular_deps: vec![], god_objects: vec![],
            orphaned_functions: vec![], long_files: vec![], avg_coupling: 0.0,
            details: needle::analysis::HealthDetails { circular_dep_penalty: 0, god_object_penalty: 0, orphan_penalty: 0, long_file_penalty: 0, coupling_penalty: 0 },
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

async fn api_security(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = match state.engine.as_ref() {
        Some(e) => e.clone(),
        None => return Json(serde_json::json!({"issues":[], "total":0})),
    };
    let issues = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        needle::analysis::scan_security(&guard.chunks)
    }).await.unwrap_or_default();
    let total = issues.len();
    Json(serde_json::json!({ "issues": issues, "total": total }))
}

async fn api_patterns(State(state): State<AppState>) -> Json<serde_json::Value> {
    let graph = Arc::clone(&state.graph);
    let result = tokio::task::spawn_blocking(move || needle::analysis::detect_patterns(&graph))
        .await
        .unwrap_or_else(|_| needle::analysis::PatternReport {
            god_objects: vec![], long_files: vec![], high_coupling: vec![],
            layer_violations: vec![], singleton_suspects: vec![],
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

async fn api_git_churn(State(state): State<AppState>) -> Json<serde_json::Value> {
    let dirs: Vec<String> = if state.storage.is_some() {
        Storage::load_config().map(|c| c.watched_dirs).unwrap_or_default()
    } else {
        vec![]
    };

    if dirs.is_empty() {
        return Json(serde_json::json!({"entries": [], "total": 0}));
    }

    let dir = dirs[0].clone();
    let entries = tokio::task::spawn_blocking(move || needle::analysis::git_churn(&dir))
        .await
        .unwrap_or_default();

    let total = entries.len();
    Json(serde_json::json!({ "entries": entries, "total": total }))
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
    cookies: Cookies,
    headers: HeaderMap,
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

    // Step 1: Retrieve relevant chunks — cloud repos first, local fallback
    let results: Vec<needle::schema::SearchResult> = {
        let user = resolve_user(&cookies, &headers).await;
        let cloud = load_cloud_engines(&state, user.as_ref()).await;
        if !cloud.is_empty() {
            let mut all: Vec<needle::schema::SearchResult> = vec![];
            for engine in cloud {
                let q2 = question.clone();
                if let Ok(Ok((res, _))) = tokio::task::spawn_blocking(move || {
                    engine.lock().unwrap().search(&q2, 8, None).map_err(|e| e.to_string())
                }).await { all.extend(res); }
            }
            all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            all.truncate(8);
            all
        } else {
            let engine = match state.engine.as_ref() {
                Some(e) => e.clone(),
                None => return Json(AskResponse { answer: String::new(), sources: vec![], model_used: String::new(), error: Some("No index".into()) }),
            };
            let q = question.clone();
            match tokio::task::spawn_blocking(move || {
                engine.lock().unwrap().search(&q, 8, None).map_err(|e| e.to_string())
            }).await {
                Ok(Ok((r, _))) => r,
                _ => return Json(AskResponse { answer: String::new(), sources: vec![], model_used: String::new(), error: Some("Search failed".into()) }),
            }
        }
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

    // Step 3: Try LLM providers in priority order
    let client = needle::llm::LlmClient::from_env();
    match client.complete(
        "You are an expert code assistant. Answer the user's question about their codebase \
         using only the provided code context. Be specific: cite function names, file names, \
         and line numbers. Use markdown. If the context is insufficient, say so clearly.",
        &prompt,
    ).await {
        Ok(answer) => Json(AskResponse {
            answer,
            sources,
            model_used: client.display_name(),
            error: None,
        }),
        Err(e) => Json(AskResponse {
            answer: String::new(),
            sources,
            model_used: String::new(),
            error: Some(format!(
                "No LLM available ({e}).\n\
                 Set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY\n\
                 Or run Ollama locally: ollama serve && ollama pull llama3.2"
            )),
        }),
    }
}

async fn api_similar(
    State(state): State<AppState>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(req): Json<SimilarRequest>,
) -> Json<SearchResponse> {
    let limit = req.limit.unwrap_or(10).min(30);
    let code = req.code.clone();
    let exclude_id = req.exclude_id;
    let empty_timing = TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 };

    // ── Cloud path ────────────────────────────────────────────────────────
    let user = resolve_user(&cookies, &headers).await;
    let cloud = load_cloud_engines(&state, user.as_ref()).await;
    if !cloud.is_empty() {
        let mut all: Vec<needle::schema::SearchResult> = vec![];
        for engine in cloud {
            let code2 = code.clone();
            if let Ok(Ok(res)) = tokio::task::spawn_blocking(move || {
                engine.lock().unwrap().search_similar(&code2, limit, exclude_id).map_err(|e| e.to_string())
            }).await { all.extend(res); }
        }
        all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(limit);
        let total = all.len();
        return Json(SearchResponse {
            total,
            results: all.into_iter().map(to_result_json).collect(),
            timing: empty_timing,
        });
    }

    // ── Local path ────────────────────────────────────────────────────────
    let engine = match state.engine.as_ref() {
        Some(e) => e.clone(),
        None => return Json(SearchResponse { results: vec![], timing: empty_timing, total: 0 }),
    };
    let results = match tokio::task::spawn_blocking(move || {
        engine.lock().unwrap().search_similar(&code, limit, exclude_id).map_err(|e| e.to_string())
    }).await {
        Ok(Ok(r)) => r,
        _ => vec![],
    };
    let total = results.len();
    Json(SearchResponse {
        total,
        results: results.into_iter().map(to_result_json).collect(),
        timing: empty_timing,
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
    match oauth::current_user_from_cookies(&cookies).await {
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
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"db error"}))).into_response(),
    };
    match users::get_user_by_api_key(pool, &key).await {
        Ok(Some(u)) => {
            users::touch_last_seen(pool, &u.id).await;
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

// Free-tier repo limit.  Raise this when paid plans exist.
const MAX_REPOS_PER_USER: i64 = 3;

/// POST /api/repos/connect — save a repo to user's account
/// Body: {"repo_full": "owner/name", "repo_url": "https://github.com/...", "private": false}
async fn api_repo_connect(
    cookies: Cookies,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies).await {
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
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };

    // ── Repo limit ────────────────────────────────────────────────────────────
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_repos WHERE user_id=$1")
        .bind(&user.id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    if count >= MAX_REPOS_PER_USER {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": format!("Repo limit reached ({MAX_REPOS_PER_USER} max on free tier). Remove an existing repo first."),
            "code":  "REPO_LIMIT"
        }))).into_response();
    }

    // ── GitHub token check ────────────────────────────────────────────────────
    // The background indexer clones repos using the stored gh_token.
    // If it is missing (e.g. the server restarted after OAuth), the repo
    // will sit in "pending" forever — tell the user to re-authenticate now.
    let has_token: bool = sqlx::query_scalar("SELECT COALESCE(gh_token, '') != '' FROM users WHERE id=$1")
        .bind(&user.id)
        .fetch_one(pool)
        .await
        .unwrap_or(false);
    if !has_token {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({
            "error": "GitHub access token missing — please sign out and sign in again before adding a repo.",
            "code":  "TOKEN_MISSING"
        }))).into_response();
    }

    match users::upsert_repo(pool, &user.id, &name, &full, &url, private).await {
        Ok(repo_id) => Json(serde_json::json!({"ok":true,"repo_id":repo_id,"status":"pending"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// GET /api/repos — list authenticated user's repos with current status (for polling)
async fn api_repos(cookies: Cookies, headers: HeaderMap) -> axum::response::Response {
    let user = match resolve_user(&cookies, &headers).await {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    match users::list_user_repos(pool, &user.id).await {
        Ok(repos) => Json(serde_json::json!({"repos": repos})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// POST /api/me/revoke-key — disable API key without issuing a new one
async fn api_revoke_key(cookies: Cookies) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies).await {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    match sqlx::query("UPDATE users SET is_active=false WHERE id=$1").bind(&user.id).execute(pool).await {
        Ok(_) => Json(serde_json::json!({"ok":true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

/// POST /api/me/regenerate-key — issue a new API key, invalidate the old one
async fn api_regenerate_key(cookies: Cookies) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies).await {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    let new_key = users::generate_api_key();
    match sqlx::query("UPDATE users SET api_key=$1, is_active=true WHERE id=$2").bind(&new_key).bind(&user.id).execute(pool).await {
        Ok(_) => Json(serde_json::json!({"ok":true,"api_key":new_key})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}
