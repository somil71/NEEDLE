use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_cookies::{Cookie, Cookies};

use super::users::{create_session, get_session_user, open_db, upsert_user};

// ── GitHub API types ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct OAuthCallbackParams {
    pub code: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct GhTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GhUser {
    id: i64,
    login: String,
    avatar_url: String,
    email: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct GhRepo {
    pub name: String,
    pub full_name: String,
    pub html_url: String,
    pub private: bool,
    pub description: Option<String>,
    pub language: Option<String>,
    pub updated_at: Option<String>,
}

// ── Shared GitHub client state ──────────────────────────────────────────────

#[derive(Clone)]
pub struct OAuthConfig {
    pub client_id:     String,
    pub client_secret: String,
    pub redirect_uri:  String,
}

impl OAuthConfig {
    pub fn from_env() -> Option<Self> {
        Some(Self {
            client_id:     std::env::var("GITHUB_CLIENT_ID").ok()?,
            client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok()?,
            redirect_uri:  std::env::var("GITHUB_REDIRECT_URI")
                .unwrap_or_else(|_| "http://localhost:7700/auth/callback".to_string()),
        })
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

pub async fn auth_github(State(cfg): State<Arc<OAuthConfig>>) -> impl IntoResponse {
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=user:email,repo&allow_signup=true",
        cfg.client_id,
        urlencoding::encode(&cfg.redirect_uri),
    );
    Redirect::to(&url)
}

pub async fn auth_callback(
    State(cfg): State<Arc<OAuthConfig>>,
    Query(params): Query<OAuthCallbackParams>,
    cookies: Cookies,
) -> impl IntoResponse {
    if let Some(err) = params.error {
        return Html(error_page(&format!("GitHub OAuth error: {err}"))).into_response();
    }

    let code = match params.code {
        Some(c) => c,
        None => return Html(error_page("Missing OAuth code")).into_response(),
    };

    // Exchange code for access token
    let client = reqwest::Client::new();
    let token_res = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": cfg.client_id,
            "client_secret": cfg.client_secret,
            "code": code,
            "redirect_uri": cfg.redirect_uri,
        }))
        .send()
        .await;

    let token_body = match token_res {
        Ok(r) => match r.json::<GhTokenResponse>().await {
            Ok(b) => b,
            Err(e) => return Html(error_page(&format!("Token parse error: {e}"))).into_response(),
        },
        Err(e) => return Html(error_page(&format!("Token request failed: {e}"))).into_response(),
    };

    if let Some(err) = token_body.error {
        let desc = token_body.error_description.unwrap_or_default();
        return Html(error_page(&format!("{err}: {desc}"))).into_response();
    }

    let access_token = match token_body.access_token {
        Some(t) => t,
        None => return Html(error_page("No access token in response")).into_response(),
    };

    // Fetch GitHub user
    let user_res = client
        .get("https://api.github.com/user")
        .bearer_auth(&access_token)
        .header("User-Agent", "needle-search/0.1")
        .send()
        .await;

    let gh_user = match user_res {
        Ok(r) => match r.json::<GhUser>().await {
            Ok(u) => u,
            Err(e) => return Html(error_page(&format!("User parse error: {e}"))).into_response(),
        },
        Err(e) => return Html(error_page(&format!("User request failed: {e}"))).into_response(),
    };

    // Upsert user in DB + create session
    let conn = match open_db() {
        Ok(c) => c,
        Err(e) => return Html(error_page(&format!("DB error: {e}"))).into_response(),
    };

    let user = match upsert_user(&conn, gh_user.id, &gh_user.login, &gh_user.avatar_url, gh_user.email.as_deref()) {
        Ok(u) => u,
        Err(e) => return Html(error_page(&format!("DB upsert error: {e}"))).into_response(),
    };

    let session_token = match create_session(&conn, &user.id) {
        Ok(t) => t,
        Err(e) => return Html(error_page(&format!("Session error: {e}"))).into_response(),
    };

    // Persist GitHub token in DB so the background indexer can clone private repos
    super::users::store_gh_token(&conn, &user.id, &access_token);

    // Store access token in a separate cookie (used client-side for repo listing)
    let mut gh_cookie = Cookie::new("gh_token", access_token);
    gh_cookie.set_http_only(true);
    gh_cookie.set_path("/");
    gh_cookie.set_max_age(time::Duration::days(30));

    let mut sess_cookie = Cookie::new("needle_session", session_token);
    sess_cookie.set_http_only(true);
    sess_cookie.set_path("/");
    sess_cookie.set_max_age(time::Duration::days(30));

    cookies.add(gh_cookie);
    cookies.add(sess_cookie);

    Redirect::to("/#/dashboard").into_response()
}

pub async fn auth_logout(cookies: Cookies) -> impl IntoResponse {
    if let Some(token) = cookies.get("needle_session").map(|c| c.value().to_string()) {
        if let Ok(conn) = open_db() {
            let _ = super::users::delete_session(&conn, &token);
        }
    }
    let mut c = Cookie::new("needle_session", "");
    c.set_path("/");
    c.set_max_age(time::Duration::seconds(0));
    cookies.add(c);
    Redirect::to("/")
}

// ── GitHub repos API (proxied — keeps gh_token server-side) ────────────────

pub async fn api_github_repos(cookies: Cookies) -> impl IntoResponse {
    let gh_token = match cookies.get("gh_token").map(|c| c.value().to_string()) {
        Some(t) => t,
        None => return axum::Json(serde_json::json!({"error": "not authenticated"})).into_response(),
    };

    let client = reqwest::Client::new();
    let res = client
        .get("https://api.github.com/user/repos?sort=updated&per_page=100&affiliation=owner")
        .bearer_auth(&gh_token)
        .header("User-Agent", "needle-search/0.1")
        .send()
        .await;

    match res {
        Ok(r) => match r.json::<Vec<GhRepo>>().await {
            Ok(repos) => axum::Json(serde_json::json!({"repos": repos})).into_response(),
            Err(e) => axum::Json(serde_json::json!({"error": e.to_string()})).into_response(),
        },
        Err(e) => axum::Json(serde_json::json!({"error": e.to_string()})).into_response(),
    }
}

// ── Session helper ──────────────────────────────────────────────────────────

pub fn current_user_from_cookies(cookies: &Cookies) -> Option<super::users::User> {
    let token = cookies.get("needle_session")?.value().to_string();
    let conn = open_db().ok()?;
    get_session_user(&conn, &token).ok().flatten()
}

// ── Error page ──────────────────────────────────────────────────────────────

fn error_page(msg: &str) -> String {
    format!(r#"<!DOCTYPE html><html><head><meta charset="UTF-8"><title>Error — Needle</title>
<style>body{{font-family:system-ui;background:#0c0c0d;color:#fafafa;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}}
.box{{background:#18181b;border:1px solid #27272a;border-radius:12px;padding:40px;max-width:480px;text-align:center}}
h2{{color:#ef4444;margin:0 0 12px}}p{{color:#a1a1aa}}a{{color:#7c3aed}}</style></head>
<body><div class="box"><h2>Something went wrong</h2><p>{msg}</p><br><a href="/">← Back to Needle</a></div></body></html>"#)
}
