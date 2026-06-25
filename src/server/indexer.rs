//! Background indexer — polls the DB for pending repos, clones + indexes them.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use super::users::{open_db, set_repo_status};

pub struct IndexerConfig {
    pub data_dir: PathBuf,
}

/// Spawns a background loop that picks up pending repos and indexes them.
pub async fn run(cfg: Arc<IndexerConfig>) {
    loop {
        if let Err(e) = tick(&cfg).await {
            eprintln!("[indexer] error: {e}");
        }
        sleep(Duration::from_secs(30)).await;
    }
}

async fn tick(cfg: &IndexerConfig) -> anyhow::Result<()> {
    let conn = open_db()?;

    // Pick one pending repo that has a stored gh_token
    let row: Option<(String, String, String, String)> = conn.query_row(
        "SELECT ur.id, ur.user_id, ur.repo_full, u.gh_token
         FROM user_repos ur
         JOIN users u ON ur.user_id = u.id
         WHERE ur.status = 'pending' AND u.gh_token IS NOT NULL
         LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).ok();

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
    set_repo_status(&conn, &repo_id, "cloning")?;

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
        set_repo_status(&conn, &repo_id, &format!("error: {msg}"))?;
        return Ok(());
    }

    // ── Index ────────────────────────────────────────────────────────────────
    set_repo_status(&conn, &repo_id, "indexing")?;

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
            set_repo_status(&conn, &repo_id, "indexed")?;
        }
        Err(msg) => {
            eprintln!("[indexer] index failed: {msg}");
            let short = &msg[..msg.len().min(180)];
            set_repo_status(&conn, &repo_id, &format!("error: {short}"))?;
        }
    }

    Ok(())
}
