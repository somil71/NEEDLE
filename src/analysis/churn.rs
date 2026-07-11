use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct ChurnEntry {
    pub path: String,
    pub commits: u32,
    pub owner: String,
}

/// Run `git log` in `dir` and return per-file commit counts + primary author.
/// Safe to call even if dir is not a git repo — returns empty vec in that case.
pub fn git_churn(dir: &str) -> Vec<ChurnEntry> {
    let out = std::process::Command::new("git")
        .args(["-C", dir, "log", "--format=%ae", "--name-only", "--diff-filter=MA", "-n", "1000"])
        .output();

    let Ok(out) = out else { return vec![] };
    if !out.status.success() { return vec![]; }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut file_data: HashMap<String, (u32, HashMap<String, u32>)> = HashMap::new();
    let mut current_author = String::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() { continue; }

        if line.contains('@') && !line.contains('/') && !line.contains('\\') && !line.contains('.') {
            current_author = line.to_string();
            continue;
        }
        if line.contains('@') && !line.contains('/') && !line.contains('\\') {
            current_author = line.to_string();
            continue;
        }

        let looks_like_file = line.contains('.') || line.contains('/') || line.contains('\\');
        if looks_like_file && !line.starts_with('[') {
            let entry = file_data.entry(line.to_string()).or_insert_with(|| (0, HashMap::new()));
            entry.0 += 1;
            if !current_author.is_empty() {
                *entry.1.entry(current_author.clone()).or_default() += 1;
            }
        }
    }

    let mut result: Vec<ChurnEntry> = file_data.into_iter().map(|(path, (commits, authors))| {
        let owner = authors.into_iter().max_by_key(|(_, c)| *c).map(|(a, _)| a).unwrap_or_default();
        ChurnEntry { path, commits, owner }
    }).collect();

    result.sort_by(|a, b| b.commits.cmp(&a.commits));
    result.truncate(200);
    result
}
