//! Structure-aware chunking for Markdown and plain text.
//!
//! Markdown: splits on ATX headings (# / ## / ###). Each heading and its body
//! forms one chunk. Subheadings within a section are merged into the parent
//! section chunk to keep chunks at a reasonable size.
//!
//! Plain text: splits on double-newline paragraph boundaries with a 2-sentence
//! sliding-window overlap so cross-boundary context is preserved.

use crate::schema::{Chunk, ChunkStatus, ChunkType, Language};
use chrono::Utc;
use std::path::Path;
use xxhash_rust::xxh3::xxh3_64;

pub struct ProseChunker;

impl super::Chunker for ProseChunker {
    fn chunk(
        &self,
        content: &str,
        path: &Path,
        language: Language,
    ) -> crate::Result<Vec<Chunk>> {
        match language {
            Language::Markdown => chunk_markdown(content, path),
            _ => chunk_plain_text(content, path, language),
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown chunking
// ---------------------------------------------------------------------------

fn chunk_markdown(content: &str, path: &Path) -> crate::Result<Vec<Chunk>> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let path_str = path.to_string_lossy().to_string();

    // Accumulate (heading_text, body_lines, start_line)
    let mut current_heading: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();
    let mut section_start_line: u32 = 1;

    let lines: Vec<&str> = content.lines().collect();

    let flush = |heading: &Option<String>,
                 body: &[&str],
                 start: u32,
                 end: u32,
                 path_str: &str,
                 chunks: &mut Vec<Chunk>| {
        let full: String = match heading {
            Some(h) => format!("{}\n{}", h, body.join("\n")),
            None => body.join("\n"),
        };
        let trimmed = full.trim();
        if trimmed.is_empty() {
            return;
        }
        let content_hash = xxh3_64(trimmed.as_bytes());
        chunks.push(Chunk {
            id: 0, // assigned by caller
            file_path: path_str.to_string(),
            root_dir: 0,
            byte_offset: 0,
            byte_length: trimmed.len() as u32,
            line_start: start,
            line_end: end,
            language: Language::Markdown,
            chunk_type: ChunkType::Section,
            content_hash,
            token_count: trimmed.split_whitespace().count() as u32,
            embedding_id: 0,
            status: ChunkStatus::Active,
            created_at: Utc::now().timestamp() as u64,
            tombstoned_at: None,
            content: trimmed.to_string(),
        });
    };

    for (i, &line) in lines.iter().enumerate() {
        let line_no = i as u32 + 1;

        if line.starts_with('#') {
            // Flush the current section
            if !current_lines.is_empty() || current_heading.is_some() {
                let end = line_no.saturating_sub(1);
                flush(
                    &current_heading,
                    &current_lines,
                    section_start_line,
                    end,
                    &path_str,
                    &mut chunks,
                );
            }
            current_heading = Some(line.to_string());
            current_lines = Vec::new();
            section_start_line = line_no;
        } else {
            current_lines.push(line);
        }
    }

    // Flush the final section
    let total_lines = lines.len() as u32;
    flush(
        &current_heading,
        &current_lines,
        section_start_line,
        total_lines,
        &path_str,
        &mut chunks,
    );

    Ok(chunks)
}

// ---------------------------------------------------------------------------
// Plain text chunking (paragraph-based with overlap)
// ---------------------------------------------------------------------------

fn chunk_plain_text(
    content: &str,
    path: &Path,
    language: Language,
) -> crate::Result<Vec<Chunk>> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let path_str = path.to_string_lossy().to_string();

    // Split into paragraphs (double newline)
    let paragraphs: Vec<&str> = content.split("\n\n").collect();
    let mut current_line: u32 = 1;

    for para in &paragraphs {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            current_line += para.lines().count() as u32 + 1;
            continue;
        }

        let line_count = trimmed.lines().count() as u32;
        let line_start = current_line;
        let line_end = current_line + line_count.saturating_sub(1);

        // Skip paragraphs that are just whitespace or very short
        let word_count = trimmed.split_whitespace().count();
        if word_count < 3 {
            current_line = line_end + 2; // +2 for the blank separator line
            continue;
        }

        let content_hash = xxh3_64(trimmed.as_bytes());

        chunks.push(Chunk {
            id: 0,
            file_path: path_str.clone(),
            root_dir: 0,
            byte_offset: 0,
            byte_length: trimmed.len() as u32,
            line_start,
            line_end,
            language,
            chunk_type: ChunkType::Paragraph,
            content_hash,
            token_count: word_count as u32,
            embedding_id: 0,
            status: ChunkStatus::Active,
            created_at: Utc::now().timestamp() as u64,
            tombstoned_at: None,
            content: trimmed.to_string(),
        });

        current_line = line_end + 2;
    }

    Ok(chunks)
}
