//! Background indexer — polls the DB for pending repos, clones + indexes them.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use super::users::{pool, set_repo_status};

pub struct IndexerConfig {
    pub data_dir: PathBuf,
}

/// Spawns a background loop that picks up pending repos and indexes them.
///
/// Polls every 6 minutes rather than every few seconds — Neon/Supabase
/// free-tier compute autosuspends after ~5 minutes with zero active
/// connections, so a tighter loop would keep it "active" (and burning
/// compute-hour quota) around the clock. A connected repo may sit a few
/// minutes longer in "pending" as a result; `api_repo_connect` could also
/// trigger an immediate tick in the future if that latency matters.
pub async fn run(cfg: Arc<IndexerConfig>) {
    loop {
        if let Err(e) = tick(&cfg).await {
            eprintln!("[indexer] error: {e}");
        }
        sleep(Duration::from_secs(360)).await;
    }
}

async fn tick(cfg: &IndexerConfig) -> anyhow::Result<()> {
    // No DATABASE_URL configured (pure local/desktop usage) — nothing to do.
    let db = match pool().await {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    // Mark any pending repos whose owner has no gh_token so they don't
    // sit silently in "queued" forever — the user needs to re-authenticate.
    sqlx::query(
        "UPDATE user_repos SET status = 'error: GitHub token missing — sign out and sign in again'
         WHERE status = 'pending'
           AND user_id IN (SELECT id FROM users WHERE COALESCE(gh_token,'') = '')",
    )
    .execute(db)
    .await
    .ok();

    // Pick one pending repo that has a stored gh_token
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT ur.id, ur.user_id, ur.repo_full, u.gh_token
         FROM user_repos ur
         JOIN users u ON ur.user_id = u.id
         WHERE ur.status = 'pending' AND COALESCE(u.gh_token,'') != ''
         LIMIT 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    let (repo_id, user_id, repo_full, gh_token) = match row {
        Some(r) => r,
        None    => return Ok(()),   // nothing pending
    };

    eprintln!("[indexer] starting {repo_full} for {user_id}");

    // Directories
    let safe_name = repo_full.replace('/', "_");
    let base      = cfg.data_dir.join("indexes").join(&user_id).join(&safe_name);
    let src_dir   = base.join("src");
    let idx_dir   = base.join("index");

    // ── Clone ────────────────────────────────────────────────────────────────
    set_repo_status(db, &repo_id, "cloning").await?;

    if src_dir.exists() { std::fs::remove_dir_all(&src_dir)?; }
    std::fs::create_dir_all(&src_dir)?;

    let clone_url = format!("https://x-access-token:{gh_token}@github.com/{repo_full}.git");

    let clone_out = tokio::process::Command::new("git")
        .args(["clone", "--depth=1", "--single-branch", &clone_url])
        .arg(&src_dir)
        .output()
        .await?;

    if !clone_out.status.success() {
        let msg = format!("clone failed: {}", String::from_utf8_lossy(&clone_out.stderr));
        eprintln!("[indexer] {msg}");
        set_repo_status(db, &repo_id, &format!("error: {msg}")).await?;
        return Ok(());
    }

    // ── Index ────────────────────────────────────────────────────────────────
    set_repo_status(db, &repo_id, "indexing").await?;

    std::fs::create_dir_all(&idx_dir)?;

    let src_dir2 = src_dir.clone();
    let idx_dir2 = idx_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        super::index_pipeline::run(&src_dir2, &idx_dir2).map_err(|e| e.to_string())
    }).await?;

    match result {
        Ok(stats) => {
            eprintln!("[indexer] ✓ {repo_full} — {} chunks, {} files",
                stats.total_chunks, stats.total_files);
            set_repo_status(db, &repo_id, "indexed").await?;
        }
        Err(msg) => {
            eprintln!("[indexer] index failed for {repo_full}: {msg}");
            // Give empty repos a friendlier label than a generic error.
            let status = if msg.contains("No supported files") {
                "empty: no indexable source files found".to_string()
            } else {
                let short = &msg[..msg.len().min(200)];
                format!("error: {short}")
            };
            set_repo_status(db, &repo_id, &status).await?;
        }
    }

    Ok(())
}
