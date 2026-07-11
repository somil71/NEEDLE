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
                // VCS / OS cruft
                ".git".to_string(),
                ".DS_Store".to_string(),
                // Dependency directories
                "node_modules".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                // Build output
                "target".to_string(),
                "dist".to_string(),
                "build".to_string(),
                "__pycache__".to_string(),
                // Config secrets
                ".env".to_string(),
                // Lock files and package manifests (verbose, low signal)
                "*.lock".to_string(),
                "package-lock.json".to_string(),
                "yarn.lock".to_string(),
                // Generated files
                "gen".to_string(),
                // Documentation and design artefacts — not source code
                "docs".to_string(),
                "design".to_string(),
                "*.md".to_string(),
                // Benchmarks and scripts — not part of the production call graph
                "benches".to_string(),
                "*.py".to_string(),
                // IDE extensions — compiled output, not the core library
                "needle-vscode".to_string(),
                // Minified / bundled assets — these dominate graph analysis with noise
                "*.min.js".to_string(),
                "*.min.css".to_string(),
                "*.bundle.js".to_string(),
                // Source maps
                "*.map".to_string(),
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
