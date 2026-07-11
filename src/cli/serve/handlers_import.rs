//! Auth, import, and repository management handlers.

use super::{AppState, ImportedIndex, ImportStatus, MAX_REPOS_PER_USER, resolve_user};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use needle::server::{index_pipeline, oauth, users};
use std::sync::{Arc, Mutex};
use tower_cookies::Cookies;

pub(super) async fn api_auth_github() -> axum::response::Response {
    let cfg = match oauth::OAuthConfig::from_env() {
        Some(c) => std::sync::Arc::new(c),
        None    => return (StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"GitHub OAuth not configured"}))).into_response(),
    };
    oauth::auth_github(axum::extract::State(cfg)).await.into_response()
}

pub(super) async fn api_auth_callback(
    Query(params): Query<oauth::OAuthCallbackParams>,
    cookies: Cookies,
) -> axum::response::Response {
    let cfg = match oauth::OAuthConfig::from_env() {
        Some(c) => std::sync::Arc::new(c),
        None    => return (StatusCode::SERVICE_UNAVAILABLE,
            axum::response::Html("<p>OAuth not configured</p>".to_string())).into_response(),
    };
    oauth::auth_callback(axum::extract::State(cfg), Query(params), cookies).await.into_response()
}

pub(super) async fn api_auth_logout(cookies: Cookies) -> axum::response::Response {
    oauth::auth_logout(cookies).await.into_response()
}

pub(super) async fn api_me(cookies: Cookies) -> axum::response::Response {
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

pub(super) async fn api_validate_key(
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
            Json(serde_json::json!({ "valid": true, "username": u.github_username, "user_id": u.id })).into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"valid":false,"error":"invalid or revoked key"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// GitHub import
// ---------------------------------------------------------------------------

pub(super) async fn api_import_github(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let url = match body.get("url").and_then(|v| v.as_str()) {
        Some(u) if u.contains("github.com") => u.to_string(),
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"provide a valid github.com URL"}))).into_response(),
    };

    let repo_name = url.trim_end_matches('/')
        .trim_end_matches(".git")
        .split('/').last().unwrap_or("repo").to_string();

    {
        let st = state.import_status.lock().unwrap();
        if st.phase == "cloning" || st.phase == "indexing" {
            return (StatusCode::CONFLICT, Json(serde_json::json!({"error":"import already in progress"}))).into_response();
        }
    }

    *state.import_status.lock().unwrap() = ImportStatus {
        phase: "cloning".to_string(), progress: 0.05,
        message: format!("Cloning {}…", repo_name),
        repo_url: url.clone(), repo_name: repo_name.clone(),
        ..ImportStatus::default()
    };

    let imported      = Arc::clone(&state.imported);
    let import_status = Arc::clone(&state.import_status);

    tokio::spawn(async move {
        let tmp = std::env::temp_dir().join(format!("needle_import_{}", uuid::Uuid::new_v4()));

        let clone_result = tokio::process::Command::new("git")
            .args(["clone", "--depth=1", "--single-branch", &url, tmp.to_str().unwrap_or(".tmp")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().await;

        match clone_result {
            Ok(status) if status.success() => {}
            Ok(s) => {
                *import_status.lock().unwrap() = ImportStatus {
                    phase: "error".to_string(),
                    error: Some(format!("git clone failed (exit {})", s.code().unwrap_or(-1))),
                    ..ImportStatus::default()
                };
                return;
            }
            Err(e) => {
                *import_status.lock().unwrap() = ImportStatus {
                    phase: "error".to_string(),
                    error: Some(format!("git not found or failed: {}", e)),
                    ..ImportStatus::default()
                };
                return;
            }
        }

        *import_status.lock().unwrap() = ImportStatus {
            phase: "indexing".to_string(), progress: 0.35,
            message: format!("Indexing {}…", repo_name),
            repo_url: url.clone(), repo_name: repo_name.clone(),
            ..ImportStatus::default()
        };

        let tmp2     = tmp.clone();
        let idx_dir  = tmp.join("_needle_index");
        let idx_dir2 = idx_dir.clone();

        let result = tokio::task::spawn_blocking(move || {
            index_pipeline::run(&tmp2, &idx_dir2).map_err(|e| e.to_string())
        }).await;

        match result {
            Ok(Ok(stats)) => {
                let loaded = (|| -> Result<ImportedIndex, String> {
                    let s  = needle::storage::Storage::new(idx_dir).map_err(|e| e.to_string())?;
                    let m  = s.load_metadata().unwrap_or_default();
                    let b  = s.load_bm25().map_err(|e| e.to_string())?;
                    let h  = s.load_hnsw().map_err(|e| e.to_string())?;
                    let ch = s.load_chunks().map_err(|e| e.to_string())?;
                    let g  = s.load_graph().unwrap_or_default();
                    let em = needle::embedding::EmbeddingModel::from_metadata(&m.embedding_model, m.embedding_dim as usize)
                        .map_err(|e| e.to_string())?;
                    let engine = needle::query::QueryEngine::new(b, h, ch, em);
                    Ok(ImportedIndex { engine: Arc::new(Mutex::new(engine)), graph: Arc::new(g) })
                })();
                match loaded {
                    Ok(idx) => {
                        *imported.write().await = Some(idx);
                        *import_status.lock().unwrap() = ImportStatus {
                            phase: "done".to_string(), progress: 1.0,
                            message: format!("Ready — {} files, {} chunks", stats.total_files, stats.total_chunks),
                            repo_url: url, repo_name,
                            files: stats.total_files, chunks: stats.total_chunks, error: None,
                        };
                    }
                    Err(e) => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Failed to load index: {}", e)), ..ImportStatus::default() }; }
                }
            }
            Ok(Err(e)) => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Indexing failed: {}", e)), ..ImportStatus::default() }; }
            Err(e)     => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Task panicked: {}", e)), ..ImportStatus::default() }; }
        }

        let _ = std::fs::remove_dir_all(&tmp.join("..").join(tmp.file_name().unwrap_or_default()));
    });

    Json(serde_json::json!({"ok": true, "status": "started"})).into_response()
}

// ---------------------------------------------------------------------------
// Local import
// ---------------------------------------------------------------------------

pub(super) async fn api_import_local(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let raw_path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"provide a local folder path"}))).into_response(),
    };

    let source_dir = std::path::PathBuf::from(&raw_path);
    if !source_dir.exists() || !source_dir.is_dir() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"path does not exist or is not a directory"}))).into_response();
    }

    {
        let st = state.import_status.lock().unwrap();
        if st.phase == "cloning" || st.phase == "indexing" {
            return (StatusCode::CONFLICT, Json(serde_json::json!({"error":"import already in progress"}))).into_response();
        }
    }

    let folder_name = source_dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| raw_path.clone());

    *state.import_status.lock().unwrap() = ImportStatus {
        phase: "indexing".to_string(), progress: 0.05,
        message: format!("Indexing {}…", folder_name),
        repo_url: raw_path.clone(), repo_name: folder_name.clone(),
        ..ImportStatus::default()
    };

    let imported      = Arc::clone(&state.imported);
    let import_status = Arc::clone(&state.import_status);

    tokio::spawn(async move {
        let idx_dir = std::env::temp_dir().join(format!("needle_local_{}", uuid::Uuid::new_v4()));
        let source  = source_dir.clone();
        let idx     = idx_dir.clone();
        let result  = tokio::task::spawn_blocking(move || {
            index_pipeline::run(&source, &idx).map_err(|e| e.to_string())
        }).await;

        match result {
            Ok(Ok(stats)) => {
                let loaded = (|| -> Result<ImportedIndex, String> {
                    let s  = needle::storage::Storage::new(idx_dir.clone()).map_err(|e| e.to_string())?;
                    let m  = s.load_metadata().unwrap_or_default();
                    let b  = s.load_bm25().map_err(|e| e.to_string())?;
                    let h  = s.load_hnsw().map_err(|e| e.to_string())?;
                    let ch = s.load_chunks().map_err(|e| e.to_string())?;
                    let g  = s.load_graph().unwrap_or_default();
                    let em = needle::embedding::EmbeddingModel::from_metadata(&m.embedding_model, m.embedding_dim as usize)
                        .map_err(|e| e.to_string())?;
                    let engine = needle::query::QueryEngine::new(b, h, ch, em);
                    Ok(ImportedIndex { engine: Arc::new(Mutex::new(engine)), graph: Arc::new(g) })
                })();
                match loaded {
                    Ok(idx) => {
                        *imported.write().await = Some(idx);
                        *import_status.lock().unwrap() = ImportStatus {
                            phase: "done".to_string(), progress: 1.0,
                            message: format!("Ready — {} files, {} chunks", stats.total_files, stats.total_chunks),
                            repo_url: raw_path, repo_name: folder_name,
                            files: stats.total_files, chunks: stats.total_chunks, error: None,
                        };
                    }
                    Err(e) => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Failed to load index: {}", e)), ..ImportStatus::default() }; }
                }
            }
            Ok(Err(e)) => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Indexing failed: {}", e)), ..ImportStatus::default() }; }
            Err(e)     => { *import_status.lock().unwrap() = ImportStatus { phase: "error".to_string(), error: Some(format!("Task panicked: {}", e)), ..ImportStatus::default() }; }
        }
    });

    Json(serde_json::json!({"ok": true, "status": "started"})).into_response()
}

pub(super) async fn api_import_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.import_status.lock().unwrap().clone();
    Json(serde_json::to_value(st).unwrap_or(serde_json::json!({"phase":"idle"})))
}

pub(super) async fn api_import_clear(State(state): State<AppState>) -> Json<serde_json::Value> {
    *state.imported.write().await = None;
    *state.import_status.lock().unwrap() = ImportStatus::default();
    Json(serde_json::json!({"ok": true}))
}

// ---------------------------------------------------------------------------
// Repository management
// ---------------------------------------------------------------------------

pub(super) async fn api_github_repos_handler(cookies: Cookies) -> axum::response::Response {
    oauth::api_github_repos(cookies).await.into_response()
}

pub(super) async fn api_repo_connect(
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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_repos WHERE user_id=$1")
        .bind(&user.id).fetch_one(pool).await.unwrap_or(0);
    if count >= MAX_REPOS_PER_USER {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": format!("Repo limit reached ({MAX_REPOS_PER_USER} max on free tier). Remove an existing repo first."),
            "code":  "REPO_LIMIT"
        }))).into_response();
    }

    let has_token: bool = sqlx::query_scalar("SELECT COALESCE(gh_token, '') != '' FROM users WHERE id=$1")
        .bind(&user.id).fetch_one(pool).await.unwrap_or(false);
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

pub(super) async fn api_repos(cookies: Cookies, headers: axum::http::HeaderMap) -> axum::response::Response {
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

pub(super) async fn api_revoke_key(cookies: Cookies) -> axum::response::Response {
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

pub(super) async fn api_regenerate_key(cookies: Cookies) -> axum::response::Response {
    let user = match oauth::current_user_from_cookies(&cookies).await {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"not_authenticated"}))).into_response(),
    };
    let pool = match users::pool().await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    };
    let new_key = users::generate_api_key();
    match sqlx::query("UPDATE users SET api_key=$1, is_active=true WHERE id=$2")
        .bind(&new_key).bind(&user.id).execute(pool).await {
        Ok(_) => Json(serde_json::json!({"ok":true,"api_key":new_key})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e.to_string()}))).into_response(),
    }
}
