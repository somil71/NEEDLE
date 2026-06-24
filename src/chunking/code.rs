//! Tree-sitter based AST chunking for code files.
//!
//! For each supported language, we parse the file with tree-sitter and extract
//! top-level semantic units (functions, classes, structs, impls, etc.) as
//! individual chunks. Each chunk is a complete, self-contained declaration —
//! a function definition includes its body.
//!
//! Falls back to a simple line-window chunker for unsupported languages or
//! if tree-sitter fails to parse (e.g., file is not valid syntax).

use crate::schema::{Chunk, ChunkStatus, ChunkType, Language};
use chrono::Utc;
use std::path::Path;
use xxhash_rust::xxh3::xxh3_64;

pub struct CodeChunker;

impl super::Chunker for CodeChunker {
    fn chunk(
        &self,
        content: &str,
        path: &Path,
        language: Language,
    ) -> crate::Result<Vec<Chunk>> {
        let blocks = extract_blocks(content, language)
            .unwrap_or_else(|| fallback_blocks(content));

        let path_str = path.to_string_lossy().to_string();
        let now = Utc::now().timestamp() as u64;

        let mut chunks = Vec::new();
        for block in blocks {
            let text = block.text.trim();
            if text.is_empty() || text.split_whitespace().count() < 3 {
                continue;
            }
            let content_hash = xxh3_64(text.as_bytes());
            chunks.push(Chunk {
                id: 0,
                file_path: path_str.clone(),
                root_dir: 0,
                byte_offset: block.start_byte as u64,
                byte_length: (block.end_byte - block.start_byte) as u32,
                line_start: block.start_line,
                line_end: block.end_line,
                language,
                chunk_type: block.chunk_type,
                content_hash,
                token_count: text.split_whitespace().count() as u32,
                embedding_id: 0,
                status: ChunkStatus::Active,
                created_at: now,
                tombstoned_at: None,
                content: text.to_string(),
            });
        }

        // If nothing was extracted, wrap the whole file as a single chunk
        if chunks.is_empty() && !content.trim().is_empty() {
            let text = content.trim();
            chunks.push(Chunk {
                id: 0,
                file_path: path_str,
                root_dir: 0,
                byte_offset: 0,
                byte_length: text.len() as u32,
                line_start: 1,
                line_end: text.lines().count() as u32,
                language,
                chunk_type: ChunkType::Module,
                content_hash: xxh3_64(text.as_bytes()),
                token_count: text.split_whitespace().count() as u32,
                embedding_id: 0,
                status: ChunkStatus::Active,
                created_at: Utc::now().timestamp() as u64,
                tombstoned_at: None,
                content: text.to_string(),
            });
        }

        Ok(chunks)
    }
}

// ---------------------------------------------------------------------------
// Internal block type
// ---------------------------------------------------------------------------

struct CodeBlock {
    text: String,
    start_byte: usize,
    end_byte: usize,
    start_line: u32,
    end_line: u32,
    chunk_type: ChunkType,
}

// ---------------------------------------------------------------------------
// Tree-sitter extraction
// ---------------------------------------------------------------------------

fn extract_blocks(content: &str, language: Language) -> Option<Vec<CodeBlock>> {
    let ts_language = match language {
        Language::Rust => tree_sitter_rust::language(),
        Language::Python => tree_sitter_python::language(),
        Language::TypeScript => tree_sitter_typescript::language_typescript(),
        Language::JavaScript => tree_sitter_typescript::language_typescript(),
        Language::Go => tree_sitter_go::language(),
        _ => return None,
    };

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(ts_language).ok()?;

    let tree = parser.parse(content, None)?;
    let root = tree.root_node();

    if root.has_error() {
        // Fall through to fallback if parse has errors
        // (still extract what we can from non-error subtrees)
    }

    let interesting_kinds = interesting_node_kinds(language);
    let mut blocks = Vec::new();
    collect_nodes(root, content.as_bytes(), &interesting_kinds, language, &mut blocks);

    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

fn interesting_node_kinds(language: Language) -> Vec<(&'static str, ChunkType)> {
    match language {
        Language::Rust => vec![
            ("function_item", ChunkType::Function),
            ("struct_item", ChunkType::Class),
            ("enum_item", ChunkType::Class),
            ("trait_item", ChunkType::Class),
            ("impl_item", ChunkType::Class),
            ("macro_definition", ChunkType::Function),
        ],
        Language::Python => vec![
            ("function_definition", ChunkType::Function),
            ("class_definition", ChunkType::Class),
        ],
        Language::TypeScript | Language::JavaScript => vec![
            ("function_declaration", ChunkType::Function),
            ("function_expression", ChunkType::Function),
            ("arrow_function", ChunkType::Function),
            ("method_definition", ChunkType::Method),
            ("class_declaration", ChunkType::Class),
            ("export_statement", ChunkType::Module),
        ],
        Language::Go => vec![
            ("function_declaration", ChunkType::Function),
            ("method_declaration", ChunkType::Method),
            ("type_declaration", ChunkType::Class),
        ],
        _ => vec![],
    }
}

fn collect_nodes(
    node: tree_sitter::Node,
    source: &[u8],
    kinds: &[(&'static str, ChunkType)],
    language: Language,
    blocks: &mut Vec<CodeBlock>,
) {
    let node_kind = node.kind();

    if let Some((_, chunk_type)) = kinds.iter().find(|(k, _)| *k == node_kind) {
        if let Ok(text) = std::str::from_utf8(&source[node.start_byte()..node.end_byte()]) {
            blocks.push(CodeBlock {
                text: text.to_string(),
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                chunk_type: *chunk_type,
            });
            // For impl blocks and classes, also recurse into methods
            // (so each method is also a separate chunk)
            if matches!(chunk_type, ChunkType::Class) {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    collect_nodes(child, source, kinds, language, blocks);
                }
            }
            return;
        }
    }

    // Recurse into children for everything not already captured
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes(child, source, kinds, language, blocks);
    }
}

// ---------------------------------------------------------------------------
// Fallback: sliding line window
// ---------------------------------------------------------------------------

const FALLBACK_WINDOW: usize = 40;
const FALLBACK_STEP: usize = 30; // 10-line overlap

fn fallback_blocks(content: &str) -> Vec<CodeBlock> {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let mut blocks = Vec::new();

    let mut start = 0usize;
    while start < total {
        let end = (start + FALLBACK_WINDOW).min(total);
        let text = lines[start..end].join("\n");
        let byte_start = lines[..start].iter().map(|l| l.len() + 1).sum::<usize>();
        let byte_end = byte_start + text.len();

        blocks.push(CodeBlock {
            text,
            start_byte: byte_start,
            end_byte: byte_end,
            start_line: start as u32 + 1,
            end_line: end as u32,
            chunk_type: ChunkType::Module,
        });

        if end == total {
            break;
        }
        start += FALLBACK_STEP;
    }

    blocks
}
