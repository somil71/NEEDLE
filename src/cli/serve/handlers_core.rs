//! Core search, graph, and analysis handlers.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use super::{
    AppState, AskRequest, AskResponse, OpenRequest, ResultJson, SearchParams, SearchResponse,
    SimilarRequest, StatusResponse, TimingJson, load_cloud_engines, resolve_user,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use needle::schema::Language;
use std::collections::HashSet;
use std::sync::Arc;
use tower_cookies::Cookies;

pub(super) async fn api_search(
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

    if let Some(imp) = state.imported.read().await.as_ref() {
        let engine = imp.engine.clone();
        let q2 = q.clone();
        let result = tokio::task::spawn_blocking(move || {
            engine.lock().unwrap().search(&q2, limit, lang_filter).map_err(|e| e.to_string())
        }).await;
        if let Ok(Ok((results, timing))) = result {
            let total = results.len();
            return Json(SearchResponse {
                total,
                results: results.into_iter().map(to_result_json).collect(),
                timing: TimingJson {
                    total_ms: timing.total_ms, bm25_ms: timing.bm25_ms,
                    hnsw_ms: timing.hnsw_ms, embed_ms: timing.embed_ms, fusion_ms: timing.fusion_ms,
                },
            });
        }
    }

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

pub(super) async fn api_status_handler(State(state): State<AppState>) -> Json<StatusResponse> {
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

pub(super) async fn api_graph(State(state): State<AppState>) -> Json<serde_json::Value> {
    if let Some(imp) = state.imported.read().await.as_ref() {
        return Json(serde_json::to_value(&*imp.graph).unwrap_or(serde_json::json!({"nodes":[],"edges":[],"stats":{}})));
    }
    Json(serde_json::to_value(&*state.graph).unwrap_or(serde_json::json!({"nodes":[],"edges":[],"stats":{}})))
}

// ---------------------------------------------------------------------------
// Analysis handlers
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub(super) struct BlastParams {
    file: Option<String>,
}

pub(super) async fn api_blast_radius(State(state): State<AppState>, Query(p): Query<BlastParams>) -> Json<serde_json::Value> {
    let file = match p.file {
        Some(f) => f,
        None => return Json(serde_json::json!({"error":"missing ?file= parameter"})),
    };
    let graph = if let Some(imp) = state.imported.read().await.as_ref() {
        Arc::clone(&imp.graph)
    } else {
        Arc::clone(&state.graph)
    };
    let result = tokio::task::spawn_blocking(move || needle::analysis::blast_radius(&graph, &file))
        .await
        .unwrap_or_else(|_| needle::analysis::BlastResult {
            source_file: String::new(), affected: vec![], total_files: 0, risk_score: 0,
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

pub(super) async fn api_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let graph = if let Some(imp) = state.imported.read().await.as_ref() {
        Arc::clone(&imp.graph)
    } else {
        Arc::clone(&state.graph)
    };
    let result = tokio::task::spawn_blocking(move || needle::analysis::health_score(&graph))
        .await
        .unwrap_or_else(|_| needle::analysis::HealthReport {
            grade: "?".into(), score: 0, circular_deps: vec![], god_objects: vec![],
            orphaned_functions: vec![], long_files: vec![], avg_coupling: 0.0,
            details: needle::analysis::HealthDetails { circular_dep_penalty: 0, god_object_penalty: 0, orphan_penalty: 0, long_file_penalty: 0, coupling_penalty: 0 },
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

pub(super) async fn api_security(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = if let Some(imp) = state.imported.read().await.as_ref() {
        imp.engine.clone()
    } else {
        match state.engine.as_ref() {
            Some(e) => e.clone(),
            None => return Json(serde_json::json!({"issues":[], "total":0})),
        }
    };
    let issues = tokio::task::spawn_blocking(move || {
        let guard = engine.lock().unwrap();
        needle::analysis::scan_security(&guard.chunks)
    }).await.unwrap_or_default();
    let total = issues.len();
    Json(serde_json::json!({ "issues": issues, "total": total }))
}

pub(super) async fn api_patterns(State(state): State<AppState>) -> Json<serde_json::Value> {
    let graph = if let Some(imp) = state.imported.read().await.as_ref() {
        Arc::clone(&imp.graph)
    } else {
        Arc::clone(&state.graph)
    };
    let result = tokio::task::spawn_blocking(move || needle::analysis::detect_patterns(&graph))
        .await
        .unwrap_or_else(|_| needle::analysis::PatternReport {
            god_objects: vec![], long_files: vec![], high_coupling: vec![],
            layer_violations: vec![], singleton_suspects: vec![],
        });
    Json(serde_json::to_value(result).unwrap_or_default())
}

pub(super) async fn api_git_churn(State(state): State<AppState>) -> Json<serde_json::Value> {
    let dirs: Vec<String> = if state.storage.is_some() {
        needle::storage::Storage::load_config().map(|c| c.watched_dirs).unwrap_or_default()
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

pub(super) async fn api_open(axum::Json(req): axum::Json<OpenRequest>) -> Json<serde_json::Value> {
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

pub(super) async fn api_ask(
    State(state): State<AppState>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(req): Json<AskRequest>,
) -> Json<AskResponse> {
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Json(AskResponse {
            answer: String::new(), sources: vec![], model_used: String::new(),
            error: Some("Empty question".into()),
        });
    }

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
        } else if let Some(imp) = state.imported.read().await.as_ref() {
            let engine = imp.engine.clone();
            let q = question.clone();
            match tokio::task::spawn_blocking(move || {
                engine.lock().unwrap().search(&q, 8, None).map_err(|e| e.to_string())
            }).await {
                Ok(Ok((r, _))) => r,
                _ => vec![],
            }
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

    let context: String = results.iter().enumerate().map(|(i, r)| {
        format!(
            "### Source {} — {}:{}-{}\n```{}\n{}\n```",
            i + 1, r.file_path.replace('\\', "/"), r.line_start, r.line_end,
            r.language.short_name(), r.content
        )
    }).collect::<Vec<_>>().join("\n\n");

    let prompt = format!(
        "You are an expert code assistant. The user has a question about their codebase.\n\
        Below are the most relevant code chunks retrieved from the index.\n\n\
        {context}\n\n---\nQuestion: {question}\n\n\
        Answer concisely and accurately. Reference specific file names and functions where relevant. \
        Use markdown formatting. If the context doesn't fully answer the question, say so clearly."
    );

    let sources: Vec<ResultJson> = results.into_iter().map(to_result_json).collect();
    let client = needle::llm::LlmClient::from_env();
    match client.complete(
        "You are an expert code assistant. Answer the user's question about their codebase \
         using only the provided code context. Be specific: cite function names, file names, \
         and line numbers. Use markdown. If the context is insufficient, say so clearly.",
        &prompt,
    ).await {
        Ok(answer) => Json(AskResponse { answer, sources, model_used: client.display_name(), error: None }),
        Err(e) => Json(AskResponse {
            answer: String::new(), sources, model_used: String::new(),
            error: Some(format!(
                "No LLM available ({e}).\n\
                 Set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY\n\
                 Or run Ollama locally: ollama serve && ollama pull llama3.2"
            )),
        }),
    }
}

pub(super) async fn api_similar(
    State(state): State<AppState>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(req): Json<SimilarRequest>,
) -> Json<SearchResponse> {
    let limit = req.limit.unwrap_or(10).min(30);
    let code = req.code.clone();
    let exclude_id = req.exclude_id;
    let empty_timing = TimingJson { total_ms: 0.0, bm25_ms: 0.0, hnsw_ms: 0.0, embed_ms: 0.0, fusion_ms: 0.0 };

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
        return Json(SearchResponse { total, results: all.into_iter().map(to_result_json).collect(), timing: empty_timing });
    }

    if let Some(imp) = state.imported.read().await.as_ref() {
        let engine = imp.engine.clone();
        let code2 = code.clone();
        let results = match tokio::task::spawn_blocking(move || {
            engine.lock().unwrap().search_similar(&code2, limit, exclude_id).map_err(|e| e.to_string())
        }).await {
            Ok(Ok(r)) => r,
            _ => vec![],
        };
        let total = results.len();
        return Json(SearchResponse { total, results: results.into_iter().map(to_result_json).collect(), timing: empty_timing });
    }

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
    Json(SearchResponse { total, results: results.into_iter().map(to_result_json).collect(), timing: empty_timing })
}

pub(super) async fn api_todos(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = if let Some(imp) = state.imported.read().await.as_ref() {
        imp.engine.clone()
    } else {
        match state.engine.as_ref() { Some(e) => e.clone(), None => return Json(serde_json::json!({"todos":[],"total":0})) }
    };
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

pub(super) async fn api_files(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = if let Some(imp) = state.imported.read().await.as_ref() {
        imp.engine.clone()
    } else {
        match state.engine.as_ref() { Some(e) => e.clone(), None => return Json(serde_json::json!({"files":[],"total":0})) }
    };
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
    needle::analysis::strip_unc(path)
}
