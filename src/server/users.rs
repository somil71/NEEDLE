use rusqlite::{Connection, Result, params};
use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn db_path() -> PathBuf {
    // On Railway: /data/needle-users.db  (persistent volume)
    // Locally:    ~/.needle/users.db
    if let Ok(p) = std::env::var("DATA_DIR") {
        return PathBuf::from(p).join("needle-users.db");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".needle")
        .join("users.db")
}

pub fn open_db() -> Result<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS users (
            id               TEXT PRIMARY KEY,
            github_id        INTEGER UNIQUE,
            github_username  TEXT NOT NULL,
            github_avatar    TEXT,
            email            TEXT,
            api_key          TEXT UNIQUE NOT NULL,
            created_at       INTEGER NOT NULL,
            last_seen        INTEGER NOT NULL,
            is_active        INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token       TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL REFERENCES users(id),
            created_at  INTEGER NOT NULL,
            expires_at  INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_repos (
            id            TEXT PRIMARY KEY,
            user_id       TEXT NOT NULL REFERENCES users(id),
            repo_name     TEXT NOT NULL,
            repo_full     TEXT NOT NULL,
            repo_url      TEXT NOT NULL,
            private       INTEGER NOT NULL DEFAULT 0,
            status        TEXT NOT NULL DEFAULT 'pending',
            indexed_at    INTEGER,
            UNIQUE(user_id, repo_full)
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_repos_user    ON user_repos(user_id);
        CREATE INDEX IF NOT EXISTS idx_api_key       ON users(api_key);
    "#)
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

pub fn upsert_user(conn: &Connection, github_id: i64, username: &str, avatar: &str, email: Option<&str>) -> Result<User> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM users WHERE github_id = ?1",
            params![github_id],
            |r| r.get(0),
        )
        .ok();

    let ts = now();

    if let Some(id) = existing {
        conn.execute(
            "UPDATE users SET github_username=?1, github_avatar=?2, email=?3, last_seen=?4 WHERE id=?5",
            params![username, avatar, email, ts as i64, id],
        )?;
        get_user_by_id(conn, &id)
    } else {
        let id = format!("usr_{}", &Uuid::new_v4().as_simple().to_string()[..12]);
        let api_key = generate_api_key();
        conn.execute(
            "INSERT INTO users (id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, github_id, username, avatar, email, api_key, ts as i64, ts as i64],
        )?;
        get_user_by_id(conn, &id)
    }
}

pub fn get_user_by_id(conn: &Connection, id: &str) -> Result<User> {
    conn.query_row(
        "SELECT id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen FROM users WHERE id=?1",
        params![id],
        row_to_user,
    )
}

pub fn get_user_by_api_key(conn: &Connection, key: &str) -> Result<Option<User>> {
    conn.query_row(
        "SELECT id,github_id,github_username,github_avatar,email,api_key,created_at,last_seen FROM users WHERE api_key=?1 AND is_active=1",
        params![key],
        row_to_user,
    ).map(Some).or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e),
    })
}

fn row_to_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id:              r.get(0)?,
        github_id:       r.get(1)?,
        github_username: r.get(2)?,
        github_avatar:   r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        email:           r.get(4)?,
        api_key:         r.get(5)?,
        created_at:      r.get::<_, i64>(6)? as u64,
        last_seen:       r.get::<_, i64>(7)? as u64,
    })
}

// ── Sessions ────────────────────────────────────────────────────────────────

pub fn create_session(conn: &Connection, user_id: &str) -> Result<String> {
    let token = generate_session_token();
    let ts = now() as i64;
    let expires = ts + 30 * 24 * 3600; // 30 days
    conn.execute(
        "INSERT INTO sessions (token,user_id,created_at,expires_at) VALUES (?1,?2,?3,?4)",
        params![token, user_id, ts, expires],
    )?;
    Ok(token)
}

pub fn get_session_user(conn: &Connection, token: &str) -> Result<Option<User>> {
    let ts = now() as i64;
    let user_id: Option<String> = conn.query_row(
        "SELECT user_id FROM sessions WHERE token=?1 AND expires_at>?2",
        params![token, ts],
        |r| r.get(0),
    ).ok();

    match user_id {
        None => Ok(None),
        Some(uid) => get_user_by_id(conn, &uid).map(Some),
    }
}

pub fn delete_session(conn: &Connection, token: &str) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE token=?1", params![token])?;
    Ok(())
}

// ── Repos ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct UserRepo {
    pub id: String,
    pub user_id: String,
    pub repo_name: String,
    pub repo_full: String,
    pub repo_url: String,
    pub private: bool,
    pub status: String,
    pub indexed_at: Option<u64>,
}

pub fn list_user_repos(conn: &Connection, user_id: &str) -> Result<Vec<UserRepo>> {
    let mut stmt = conn.prepare(
        "SELECT id,user_id,repo_name,repo_full,repo_url,private,status,indexed_at FROM user_repos WHERE user_id=?1 ORDER BY rowid DESC"
    )?;
    let rows = stmt.query_map(params![user_id], |r| Ok(UserRepo {
        id:         r.get(0)?,
        user_id:    r.get(1)?,
        repo_name:  r.get(2)?,
        repo_full:  r.get(3)?,
        repo_url:   r.get(4)?,
        private:    r.get::<_, i64>(5)? != 0,
        status:     r.get(6)?,
        indexed_at: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
    }))?;
    rows.collect()
}

pub fn upsert_repo(conn: &Connection, user_id: &str, name: &str, full: &str, url: &str, private: bool) -> Result<String> {
    let existing: Option<String> = conn.query_row(
        "SELECT id FROM user_repos WHERE user_id=?1 AND repo_full=?2",
        params![user_id, full],
        |r| r.get(0),
    ).ok();

    if let Some(id) = existing {
        return Ok(id);
    }

    let id = format!("repo_{}", &Uuid::new_v4().as_simple().to_string()[..12]);
    conn.execute(
        "INSERT INTO user_repos (id,user_id,repo_name,repo_full,repo_url,private,status)
         VALUES (?1,?2,?3,?4,?5,?6,'pending')",
        params![id, user_id, name, full, url, private as i64],
    )?;
    Ok(id)
}

pub fn touch_last_seen(conn: &Connection, user_id: &str) {
    let ts = now() as i64;
    let _ = conn.execute("UPDATE users SET last_seen=?1 WHERE id=?2", params![ts, user_id]);
}

pub fn set_repo_status(conn: &Connection, repo_id: &str, status: &str) -> Result<()> {
    let ts = now() as i64;
    conn.execute(
        "UPDATE user_repos SET status=?1, indexed_at=?2 WHERE id=?3",
        params![status, ts, repo_id],
    )?;
    Ok(())
}
