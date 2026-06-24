use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub watched_dirs: Vec<String>,
    pub ignore_patterns: Vec<String>,
    pub bm25_k1: f32,
    pub bm25_b: f32,
    pub hnsw_m: u16,
    pub hnsw_ef_construction: u32,
    pub hnsw_ef_search: u32,
    pub embedding_dim: usize,
    pub default_limit: usize,
    pub rrf_k: usize,
    pub snippet_lines: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watched_dirs: vec![],
            ignore_patterns: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "__pycache__".to_string(),
                ".env".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                ".DS_Store".to_string(),
                "*.lock".to_string(),
            ],
            bm25_k1: 1.2,
            bm25_b: 0.75,
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            hnsw_ef_search: 50,
            embedding_dim: 384,
            default_limit: 10,
            rrf_k: 60,
            snippet_lines: 6,
        }
    }
}

impl Config {
    pub fn watched_dir_paths(&self) -> Vec<PathBuf> {
        self.watched_dirs.iter().map(PathBuf::from).collect()
    }

    pub fn should_ignore(&self, path: &str) -> bool {
        for pattern in &self.ignore_patterns {
            if pattern.starts_with('*') {
                let ext = &pattern[1..];
                if path.ends_with(ext) {
                    return true;
                }
            } else if path.contains(pattern.as_str()) {
                return true;
            }
        }
        false
    }

    pub fn from_file(path: &std::path::Path) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self, path: &std::path::Path) -> crate::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::error::Error::ConfigError(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
