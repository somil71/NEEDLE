//! Chunking strategies for different file types.

pub mod code;
pub mod prose;

use crate::schema::{Chunk, Language};
use std::path::Path;

pub trait Chunker: Send + Sync {
    fn chunk(&self, content: &str, path: &Path, language: Language) -> crate::Result<Vec<Chunk>>;
}

/// Route a file to the right chunker based on its language.
pub fn detect_chunker(language: Language) -> Box<dyn Chunker> {
    match language {
        Language::Rust
        | Language::Python
        | Language::TypeScript
        | Language::JavaScript
        | Language::Go
        | Language::Java
        | Language::C
        | Language::Cpp => Box::new(code::CodeChunker),

        Language::Markdown | Language::PlainText | Language::Pdf => Box::new(prose::ProseChunker),

        // Config/data files: treat as prose (top-level key blocks)
        Language::Toml | Language::Yaml | Language::Json | Language::Shell
        | Language::Dockerfile => Box::new(prose::ProseChunker),
    }
}
