use serde::Serialize;
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;
use uuid::Uuid;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

static POOL: OnceCell<PgPool> = OnceCell::const_new();

/// Returns the shared Postgres pool, connecting lazily on first use.
/// Cloud features (OAuth, sessions, repo tracking) require `DATABASE_URL`;
/// pure local/desktop usage never calls this.
///
/// Tries `DATABASE_URL` (Neon) first; if that's unreachable — e.g. paused
/// after hitting a free-tier compute-hour quota — falls back to
/// `DATABASE_URL_FALLBACK` (e.g. Supabase). Both are plain Postgres, so the
/// same schema/queries work against either. Once a backend connects
/// successfully it's used for the rest of the process; restart to retry
/// the primary.
pub async fn pool() -> anyhow::Result<&'static PgPool> {
    POOL.get_or_try_init(|| async {
        let primary  = std::env::var("DATABASE_URL").ok();
        let fallback = std::env::var("DATABASE_URL_FALLBACK").ok();

        if primary.is_none() && fallback.is_none() {
            anyhow::bail!("DATABASE_URL not set — cloud features disabled");
        }

        for (label, url) in [("DATABASE_URL", primary), ("DATABASE_URL_FALLBACK", fallback)] {
            let Some(url) = url else { continue };
            match connect(&url).await {
                Ok(pool) => {
                    if label == "DATABASE_URL_FALLBACK" {
                        eprintln!("[db] DATABASE_URL unreachable — using DATABASE_URL_FALLBACK");
                    }
                    return Ok(pool);
                }
                Err(e) => eprintln!("[db] {label} connection failed: {e}"),
            }
        }

        anyhow::bail!("could not connect to any configured database")
    })
    .await
}

async fn connect(url: &str) -> anyhow::Result<PgPool> {
    // min_connections(0) + a short idle_timeout let the pool fully release
    // its connection between background-indexer polls, so a Neon/Supabase
    // free-tier compute can actually autosuspend instead of staying "active"
    // (and burning compute-hour quota) the whole time.
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .min_connections(0)
        .idle_timeout(Some(Duration::from_secs(10)))
        .acquire_timeout(Duration::from_secs(8))
        .connect(url)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &PgPool) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id               TEXT PRIMARY KEY,
            github_id        BIGINT UNIQUE,
            github_username  TEXT NOT NULL,
            github_avatar    TEXT,
            email            TEXT,
            api_key          TEXT UNIQUE NOT NULL,
            gh_token         TEXT,
            created_at       BIGINT NOT NULL,
            last_seen        BIGINT NOT NULL,
            is_active        BOOLEAN NOT NULL DEFAULT TRUE
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            token       TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL REFERENCES users(id),
            created_at  BIGINT NOT NULL,
            expires_at  BIGINT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS user_repos (
            id            TEXT PRIMARY KEY,
            user_id       TEXT NOT NULL REFERENCES users(id),
            repo_name     TEXT NOT NULL,
            repo_full     TEXT NOT NULL,
            repo_url      TEXT NOT NULL,
            private       BOOLEAN NOT NULL DEFAULT FALSE,
            status        TEXT NOT NULL DEFAULT 'pending',
            indexed_at    BIGINT,
            seq           BIGSERIAL,
            UNIQUE(user_id, repo_full)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_repos_user ON user_repos(user_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_key ON users(api_key)")
        .execute(pool)
        .await?;

    Ok(())
}

// ── Key generation ──────────────────────────────────────────────────────────

pub fn generate_api_key() -> String {
    let raw = Uuid::new_v4().as_simple().to_string();
    format!("ndk_{}", &raw)
}

pub fn generate_session_token() -> String {
    Uuid::new_v4().as_simple().to_string()
}

// ── User ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct User {
    pub id: String,
    pub github_id: i64,
    pub github_username: String,
    pub github_avatar: String,
    pub email: Option<String>,
    pub api_key: String,
    pub created_at: u64,
    pub last_seen: u64,
}

#[derive(FromRow)]
struct UserRow {
    id: String,
    github_id: i64,
    github_username: String,
    github_avatar: Option<String>,
    email: Option<String>,
    api_key: String,
    created_at: i64,
    last_seen: i64,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            github_id: r.github_id,
            github_username: r.github_username,
            github_avatar: r.github_avatar.unwrap_or_default(),
            email: r.email,
            api_key: r.api_key,
            created_at: r.created_at as u64,
            last_seen: r.last_seen as u64,
        }
    }
}

pub async fn upsert_user(
    pool: &PgPool,
    github_id: i64,
    username: &str,
    avatar: &str,
    email: Option<&str>,
) -> sqlx::Result<User> {
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE github_id = $1")
        .bind(github_id)
        .fetch_optional(pool)
        .await?;

    let ts = now();

    if let Some(id) = existing {
        sqlx::query(
            "UPDATE users SET github_username=$1, github_avatar=$2, email=$3, last_seen=$4 WHERE id=$5",
        )
        .bind(username)
        .bind(avatar)
        .bind(email)
        .bind(ts)
        .bind(&id)
        .execute(pool)
        .await?;
        get_user_by_id(pool, &id).await
    } else {
        let id = format!("usr_{}", &Uuid::new_v4().as_simple().to_string()[..12]);
        let api_key = generate_api_key();
        sqlx::query(
            "INSERT INTO users (id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(&id)
        .bind(github_id)
        .bind(username)
        .bind(avatar)
        .bind(email)
        .bind(&api_key)
        .bind(ts)
        .bind(ts)
        .execute(pool)
        .await?;
        get_user_by_id(pool, &id).await
    }
}

pub async fn get_user_by_id(pool: &PgPool, id: &str) -> sqlx::Result<User> {
    let row: UserRow = sqlx::query_as(
        "SELECT id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen FROM users WHERE id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

pub async fn get_user_by_api_key(pool: &PgPool, key: &str) -> sqlx::Result<Option<User>> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen FROM users WHERE api_key=$1 AND is_active=true",
    )
    .bind(key)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

// ── Sessions ────────────────────────────────────────────────────────────────

pub async fn create_session(pool: &PgPool, user_id: &str) -> sqlx::Result<String> {
    let token = generate_session_token();
    let ts = now();
    let expires = ts + 30 * 24 * 3600; // 30 days
    sqlx::query("INSERT INTO sessions (token,user_id,created_at,expires_at) VALUES ($1,$2,$3,$4)")
        .bind(&token)
        .bind(user_id)
        .bind(ts)
        .bind(expires)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn get_session_user(pool: &PgPool, token: &str) -> sqlx::Result<Option<User>> {
    let ts = now();
    let user_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token=$1 AND expires_at>$2",
    )
    .bind(token)
    .bind(ts)
    .fetch_optional(pool)
    .await?;

    match user_id {
        None => Ok(None),
        Some(uid) => get_user_by_id(pool, &uid).await.map(Some),
    }
}

pub async fn delete_session(pool: &PgPool, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token=$1")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Repos ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct UserRepo {
    pub id: String,
    pub user_id: String,
    pub repo_name: String,
    pub repo_full: String,
    pub repo_url: String,
    pub private: bool,
    pub status: String,
    pub indexed_at: Option<i64>,
}

pub async fn list_user_repos(pool: &PgPool, user_id: &str) -> sqlx::Result<Vec<UserRepo>> {
    sqlx::query_as(
        "SELECT id,user_id,repo_name,repo_full,repo_url,private,status,indexed_at
         FROM user_repos WHERE user_id=$1 ORDER BY seq DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn upsert_repo(
    pool: &PgPool,
    user_id: &str,
    name: &str,
    full: &str,
    url: &str,
    private: bool,
) -> sqlx::Result<String> {
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT id FROM user_repos WHERE user_id=$1 AND repo_full=$2",
    )
    .bind(user_id)
    .bind(full)
    .fetch_optional(pool)
    .await?;

    if let Some(id) = existing {
        return Ok(id);
    }

    let id = format!("repo_{}", &Uuid::new_v4().as_simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO user_repos (id,user_id,repo_name,repo_full,repo_url,private,status)
         VALUES ($1,$2,$3,$4,$5,$6,'pending')",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(full)
    .bind(url)
    .bind(private)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn store_gh_token(pool: &PgPool, user_id: &str, token: &str) {
    let _ = sqlx::query("UPDATE users SET gh_token=$1 WHERE id=$2")
        .bind(token)
        .bind(user_id)
        .execute(pool)
        .await;
}

pub async fn touch_last_seen(pool: &PgPool, user_id: &str) {
    let ts = now();
    let _ = sqlx::query("UPDATE users SET last_seen=$1 WHERE id=$2")
        .bind(ts)
        .bind(user_id)
        .execute(pool)
        .await;
}

pub async fn set_repo_status(pool: &PgPool, repo_id: &str, status: &str) -> sqlx::Result<()> {
    let ts = now();
    if status == "indexed" {
        sqlx::query("UPDATE user_repos SET status=$1, indexed_at=$2 WHERE id=$3")
            .bind(status)
            .bind(ts)
            .bind(repo_id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE user_repos SET status=$1 WHERE id=$2")
            .bind(status)
            .bind(repo_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}
