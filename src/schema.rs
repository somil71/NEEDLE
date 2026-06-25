use serde::{Deserialize, Serialize};

/// The atomic unit of indexing — every searchable piece of content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: u64,
    pub file_path: String,
    pub root_dir: u16,
    pub byte_offset: u64,
    pub byte_length: u32,
    pub line_start: u32,
    pub line_end: u32,
    pub language: Language,
    pub chunk_type: ChunkType,
    pub content_hash: u64,
    pub token_count: u32,
    pub embedding_id: u64,
    pub status: ChunkStatus,
    pub created_at: u64,
    pub tombstoned_at: Option<u64>,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    C,
    Cpp,
    Markdown,
    PlainText,
    Toml,
    Yaml,
    Json,
    Dockerfile,
    Shell,
    Pdf,
}

impl Language {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "rs" => Some(Language::Rust),
            "py" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "go" => Some(Language::Go),
            "java" => Some(Language::Java),
            "c" | "h" => Some(Language::C),
            "cpp" | "cc" | "cxx" | "hpp" => Some(Language::Cpp),
            "md" | "markdown" => Some(Language::Markdown),
            "txt" => Some(Language::PlainText),
            "toml" => Some(Language::Toml),
            "yaml" | "yml" => Some(Language::Yaml),
            "json" => Some(Language::Json),
            "dockerfile" => Some(Language::Dockerfile),
            "sh" | "bash" | "zsh" => Some(Language::Shell),
            "pdf" => Some(Language::Pdf),
            _ => None,
        }
    }

    pub fn from_short(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rs" | "rust" => Some(Language::Rust),
            "py" | "python" => Some(Language::Python),
            "ts" | "tsx" | "typescript" => Some(Language::TypeScript),
            "js" | "jsx" | "javascript" => Some(Language::JavaScript),
            "go" | "golang" => Some(Language::Go),
            "java" => Some(Language::Java),
            "c" => Some(Language::C),
            "cpp" | "cc" | "c++" => Some(Language::Cpp),
            "md" | "markdown" => Some(Language::Markdown),
            "txt" | "text" => Some(Language::PlainText),
            "toml" => Some(Language::Toml),
            "yaml" | "yml" => Some(Language::Yaml),
            "json" => Some(Language::Json),
            "sh" | "bash" | "shell" => Some(Language::Shell),
            "pdf" => Some(Language::Pdf),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::TypeScript => "TypeScript",
            Language::JavaScript => "JavaScript",
            Language::Go => "Go",
            Language::Java => "Java",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Markdown => "Markdown",
            Language::PlainText => "Text",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Json => "JSON",
            Language::Dockerfile => "Dockerfile",
            Language::Shell => "Shell",
            Language::Pdf => "PDF",
        }
    }

    pub fn highlight_class(&self) -> &'static str {
        match self {
            Language::Rust => "language-rust",
            Language::Python => "language-python",
            Language::TypeScript => "language-typescript",
            Language::JavaScript => "language-javascript",
            Language::Go => "language-go",
            Language::Java => "language-java",
            Language::C | Language::Cpp => "language-cpp",
            Language::Markdown => "language-markdown",
            Language::Toml => "language-toml",
            Language::Yaml => "language-yaml",
            Language::Json => "language-json",
            Language::Shell => "language-bash",
            Language::PlainText | Language::Dockerfile | Language::Pdf => "language-plaintext",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Language::Rust => "rs",
            Language::Python => "py",
            Language::TypeScript => "ts",
            Language::JavaScript => "js",
            Language::Go => "go",
            Language::Java => "java",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Markdown => "md",
            Language::PlainText => "txt",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Json => "json",
            Language::Dockerfile => "dockerfile",
            Language::Shell => "sh",
            Language::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkType {
    Function,
    Method,
    Class,
    Module,
    Import,
    Comment,
    Paragraph,
    Section,
    ConfigBlock,
}

impl ChunkType {
    /// Short badge for CLI display
    pub fn badge(&self) -> &'static str {
        match self {
            ChunkType::Function => "fn",
            ChunkType::Method => "mt",
            ChunkType::Class => "cl",
            ChunkType::Module => "md",
            ChunkType::Import => "im",
            ChunkType::Comment => "//",
            ChunkType::Paragraph => "¶",
            ChunkType::Section => "§",
            ChunkType::ConfigBlock => "{}",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkStatus {
    Active,
    Tombstoned,
}

/// Inverted index posting entry (chunk_id, tf) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostingsEntry {
    pub chunk_id: u64,
    pub term_freq: u16,
}

/// A single search result with all metadata needed to render the result card.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk_id: u64,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub language: Language,
    pub chunk_type: ChunkType,
    pub content: String,
    pub score: f32,
    pub signals: SearchSignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSignal {
    Keyword,
    Semantic,
    Hybrid,
}

impl SearchSignal {
    pub fn label(&self) -> &'static str {
        match self {
            SearchSignal::Hybrid => "HYBRID",
            SearchSignal::Keyword => "KW",
            SearchSignal::Semantic => "SEM",
        }
    }
}

/// Write-ahead log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntry {
    ChunkAdded(Chunk),
    ChunkDeleted(u64),
    FileModified { path: String, chunk_ids: Vec<u64> },
    Checkpoint(u64),
}

/// Persisted index metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub version: u16,
    pub total_chunks: u64,
    pub total_files: u64,
    pub tombstoned_chunks: u64,
    pub avg_chunk_length: f32,
    pub last_update_ts: u64,
    pub snapshot_sequence: u64,
    pub embedding_model: String,
    pub embedding_dim: u32,
    pub hnsw_m: u16,
    pub hnsw_ef_construction: u32,
    pub bm25_k1: f32,
    pub bm25_b: f32,
    pub watched_dirs: Vec<String>,
}

impl Default for IndexMetadata {
    fn default() -> Self {
        Self {
            version: 1,
            total_chunks: 0,
            total_files: 0,
            tombstoned_chunks: 0,
            avg_chunk_length: 0.0,
            last_update_ts: 0,
            snapshot_sequence: 0,
            embedding_model: "hash-projection-384".to_string(),
            embedding_dim: 384,
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            bm25_k1: 1.2,
            bm25_b: 0.75,
            watched_dirs: Vec::new(),
        }
    }
}
