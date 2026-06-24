//! Needle — Local-first hybrid search engine
//!
//! A hybrid keyword + semantic search engine that indexes files, code, and notes
//! entirely offline with sub-5ms query latency.

pub mod chunking;
pub mod config;
pub mod embedding;
pub mod error;
pub mod graph;
pub mod indexing;
pub mod query;
pub mod schema;
pub mod storage;
pub mod server;
pub mod watcher;

pub use error::{Error, Result};
