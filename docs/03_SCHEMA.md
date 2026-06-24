# Needle — Data Schema & Storage Layout

This project has no traditional database. All data lives in custom binary structures
on disk, memory-mapped for zero-copy access. This document defines every data structure,
its on-disk format, and how structures reference each other.

---

## Storage directory layout

```
~/.needle/
├── config.toml                  # user configuration
├── index/
│   ├── meta.bin                 # index metadata + snapshot sequence
│   ├── chunks.store             # chunk content + metadata (mmap'd)
│   ├── inverted.idx             # BM25 inverted index (mmap'd)
│   ├── hnsw.idx                 # HNSW graph structure (mmap'd)
│   ├── embeddings.bin           # raw float32 vectors (mmap'd)
│   ├── filemap.idx              # file_path → chunk_id[] mapping
│   └── wal/
│       ├── segment_000001.wal
│       ├── segment_000002.wal
│       └── ...
└── models/
    └── minilm-l6-v2.onnx
```

---

## Entity definitions

### 1. Chunk

The atomic unit of indexing. Every searchable piece of content is a chunk.

```
ChunkId:    u64           # monotonic, never reused (tombstoned IDs are skipped)

Chunk {
    id:             ChunkId,
    file_path:      String,         # relative to watched root, e.g. "src/http/retry.rs"
    root_dir:       u16,            # index into config.watched_dirs[]
    byte_offset:    u64,            # start byte in original file
    byte_length:    u32,            # length in bytes
    line_start:     u32,            # 1-indexed
    line_end:       u32,            # inclusive
    language:       Language,       # enum: Rust, Python, TypeScript, Markdown, PlainText, ...
    chunk_type:     ChunkType,      # enum: Function, Class, Method, Module, Paragraph,
                                    #        Section, ConfigBlock, Import, Comment
    content_hash:   u64,            # xxh3 of raw content bytes
    token_count:    u32,            # number of BM25 tokens (for length normalization)
    embedding_id:   u64,            # index into embeddings.bin (= id × dim × sizeof(f32))
    status:         ChunkStatus,    # enum: Active, Tombstoned
    created_at:     u64,            # unix timestamp (seconds)
    tombstoned_at:  Option<u64>,    # set when soft-deleted
}
```

**Storage**: `chunks.store` — a flat array of fixed-size records, indexed by `ChunkId`. Content text is stored inline (variable-length region after the fixed fields, length = `byte_length`). The file is memory-mapped; lookups are O(1) by ID via offset arithmetic.

**Record layout in chunks.store**:
```
┌──────────────────────────────────────────────────────────┐
│  Header (16 bytes)                                       │
│    magic:           [u8; 4]    = b"NCHK"                 │
│    version:         u16                                   │
│    record_count:    u64                                   │
│    next_id:         u64        (next ChunkId to assign)  │
├──────────────────────────────────────────────────────────┤
│  Record index (record_count × 16 bytes)                  │
│    For each chunk:                                       │
│      content_offset: u64       (byte offset to content)  │
│      content_length: u32                                 │
│      status:         u8        (0=active, 1=tombstoned)  │
│      _padding:       [u8; 3]                             │
├──────────────────────────────────────────────────────────┤
│  Fixed metadata (record_count × 64 bytes)                │
│    Each record: packed struct of Chunk fields above      │
│    (excluding content, which is in the variable region)  │
├──────────────────────────────────────────────────────────┤
│  Variable content region                                 │
│    Raw UTF-8 text of each chunk, concatenated            │
│    Addressed by (content_offset, content_length) pairs   │
└──────────────────────────────────────────────────────────┘
```

---

### 2. Inverted index

Maps terms to the chunks that contain them, with term frequencies for BM25 scoring.

```
Term:           String          # normalized, lowercased, optionally stemmed

PostingsEntry {
    chunk_id:       ChunkId,
    term_freq:      u16,        # how many times this term appears in this chunk
}

PostingsList {
    term:           Term,
    doc_freq:       u32,        # number of chunks containing this term
    entries:        Vec<PostingsEntry>,  # sorted by chunk_id (for merge joins + delta encoding)
}

InvertedIndex {
    total_chunks:       u64,    # for IDF calculation: N
    avg_chunk_length:   f32,    # average token_count across all active chunks (for BM25 b)
    vocabulary:         HashMap<Term, PostingsListOffset>,  # term → offset in postings file
    postings:           [PostingsList],                     # contiguous, sorted by term
}
```

**On-disk layout (inverted.idx)**:
```
┌──────────────────────────────────────────────────────────┐
│  Header (32 bytes)                                       │
│    magic:             [u8; 4]  = b"NINV"                 │
│    version:           u16                                 │
│    total_chunks:      u64                                 │
│    avg_chunk_length:  f32                                 │
│    vocab_size:        u64      (distinct terms)          │
│    postings_offset:   u64      (byte offset to postings) │
├──────────────────────────────────────────────────────────┤
│  Vocabulary table (vocab_size entries)                    │
│    For each term:                                        │
│      term_length:     u16                                 │
│      term_bytes:      [u8; term_length]                  │
│      doc_freq:        u32                                 │
│      postings_offset: u64     (into postings region)     │
│      postings_length: u32     (number of entries)        │
├──────────────────────────────────────────────────────────┤
│  Postings region                                         │
│    For each PostingsList (sorted by term):               │
│      entries: packed array of (chunk_id: u64, tf: u16)   │
│      Sorted by chunk_id ascending.                       │
│                                                          │
│      Stretch: delta-encoded chunk_ids + varint encoding  │
│        first_id: u64                                     │
│        deltas:   [varint]  (each = current - previous)   │
│        tfs:      [u16]                                   │
└──────────────────────────────────────────────────────────┘
```

---

### 3. HNSW graph

Multi-layer navigable small-world graph over embedding vectors.

```
HnswNode {
    id:             ChunkId,        # same as the chunk it represents
    layer:          u8,             # max layer this node exists on (0 = bottom only)
    neighbors:      Vec<Vec<ChunkId>>,  # neighbors[l] = neighbor IDs at layer l
                                        # len(neighbors) = layer + 1
}

HnswGraph {
    entry_point:        ChunkId,    # node with the highest layer
    max_layer:          u8,         # current highest layer in the graph
    M:                  u16,        # max neighbors per node per layer (default 16)
    M_max0:             u16,        # max neighbors at layer 0 (default 2*M = 32)
    ef_construction:    u32,        # candidate pool during build (default 200)
    mL:                 f64,        # level generation factor: 1/ln(M)
    node_count:         u64,        # active (non-tombstoned) nodes
    nodes:              Vec<HnswNode>,  # indexed by internal node ID
}
```

**On-disk layout (hnsw.idx)**:
```
┌──────────────────────────────────────────────────────────┐
│  Header (64 bytes)                                       │
│    magic:             [u8; 4]  = b"NHNS"                 │
│    version:           u16                                 │
│    entry_point:       u64                                 │
│    max_layer:         u8                                  │
│    M:                 u16                                 │
│    M_max0:            u16                                 │
│    ef_construction:   u32                                 │
│    node_count:        u64                                 │
│    total_slots:       u64      (including tombstoned)    │
├──────────────────────────────────────────────────────────┤
│  Node table (total_slots entries)                        │
│    For each slot:                                        │
│      chunk_id:        u64                                 │
│      layer:           u8                                  │
│      status:          u8       (0=active, 1=tombstoned)  │
│      neighbor_offset: u64      (into adjacency region)   │
├──────────────────────────────────────────────────────────┤
│  Adjacency region                                        │
│    For each node (by neighbor_offset):                   │
│      For layer l in 0..=node.layer:                      │
│        count:         u16                                 │
│        neighbor_ids:  [u64; count]                       │
│      Padded to max capacity (M_max0 for l=0, M for l>0) │
│      so in-place updates don't require reallocation.     │
└──────────────────────────────────────────────────────────┘
```

---

### 4. Embeddings store

Raw float32 vectors, one per chunk, laid out for direct mmap access.

```
EmbeddingStore {
    dim:        u32,            # 384 for MiniLM
    count:      u64,            # total vectors stored (including tombstoned slots)
    vectors:    [f32; count × dim],  # flat contiguous array
}
```

**On-disk layout (embeddings.bin)**:
```
┌──────────────────────────────────────────────────────────┐
│  Header (16 bytes)                                       │
│    magic:     [u8; 4]  = b"NEMB"                         │
│    version:   u16                                         │
│    dim:       u32       (384)                             │
│    count:     u64                                         │
├──────────────────────────────────────────────────────────┤
│  Vector data                                             │
│    vector[0]:  [f32; 384]    → 1536 bytes                │
│    vector[1]:  [f32; 384]    → 1536 bytes                │
│    ...                                                    │
│    vector[n]:  [f32; 384]    → 1536 bytes                │
│                                                          │
│    Access: offset = 16 + (chunk_id × 384 × 4)           │
│    Tombstoned slots: first f32 = NaN sentinel            │
└──────────────────────────────────────────────────────────┘
```

---

### 5. File map

Reverse index from file paths to their chunks, used for incremental updates.

```
FileEntry {
    file_path:      String,         # relative path
    root_dir:       u16,
    chunk_ids:      Vec<ChunkId>,   # all chunks belonging to this file
    last_modified:  u64,            # filesystem mtime (unix seconds)
    content_hash:   u64,            # xxh3 of entire file content
}

FileMap {
    entries:    HashMap<String, FileEntry>,  # keyed by relative file_path
}
```

**On-disk layout (filemap.idx)**:
```
┌──────────────────────────────────────────────────────────┐
│  Header (16 bytes)                                       │
│    magic:         [u8; 4]  = b"NFMP"                     │
│    version:       u16                                     │
│    entry_count:   u64                                     │
├──────────────────────────────────────────────────────────┤
│  Entries (variable length, sequential)                   │
│    For each file:                                        │
│      path_length:     u16                                 │
│      path_bytes:      [u8; path_length]                  │
│      root_dir:        u16                                 │
│      last_modified:   u64                                 │
│      content_hash:    u64                                 │
│      chunk_count:     u32                                 │
│      chunk_ids:       [u64; chunk_count]                 │
├──────────────────────────────────────────────────────────┤
│  Path hash table (for O(1) lookup by path)               │
│    Bucket count:      u64                                 │
│    Buckets:           [u64; bucket_count]                 │
│      Each bucket → byte offset into entries region       │
│      Collision: linear probing                           │
└──────────────────────────────────────────────────────────┘
```

---

### 6. Write-ahead log (WAL)

Ensures crash consistency. Every index mutation is logged before being applied.

```
WalEntryType:   enum { AddChunks, DeleteChunks, UpdateFilePath, Checkpoint }

WalEntry {
    sequence:       u64,            # monotonic, global
    entry_type:     WalEntryType,
    timestamp:      u64,
    file_path:      Option<String>,

    # For AddChunks:
    added_chunks:   Vec<ChunkId>,

    # For DeleteChunks:
    deleted_chunks: Vec<ChunkId>,

    # For UpdateFilePath:
    old_path:       Option<String>,
    new_path:       Option<String>,

    checksum:       u32,            # CRC32 of the entry (excluding this field)
    committed:      bool,           # commit marker written AFTER successful apply
}
```

**On-disk layout (wal/segment_NNNNNN.wal)**:
```
┌──────────────────────────────────────────────────────────┐
│  Segment header (16 bytes)                               │
│    magic:             [u8; 4]  = b"NWAL"                 │
│    version:           u16                                 │
│    first_sequence:    u64                                 │
├──────────────────────────────────────────────────────────┤
│  Entries (variable length, append-only)                  │
│    For each entry:                                       │
│      entry_length:    u32      (of the entire entry)     │
│      sequence:        u64                                 │
│      entry_type:      u8                                  │
│      timestamp:       u64                                 │
│      payload:         [u8; ...]  (type-specific data)    │
│      checksum:        u32      (CRC32 of above)          │
│      commit_marker:   u8       (0x00=uncommitted,        │
│                                 0xFF=committed)          │
│                                                          │
│  Write protocol:                                         │
│    1. Append entry with commit_marker = 0x00             │
│    2. fsync                                              │
│    3. Apply mutations to in-memory indexes               │
│    4. Overwrite commit_marker → 0xFF                     │
│    5. fsync                                              │
│                                                          │
│  Recovery: entries with commit=0xFF → replay.            │
│            entries with commit=0x00 → discard.           │
└──────────────────────────────────────────────────────────┘
```

---

### 7. Index metadata

Global metadata about the index state.

```
IndexMeta {
    magic:              [u8; 4]  = b"NMTA",
    version:            u16,
    created_at:         u64,
    last_snapshot_seq:  u64,        # WAL sequence of last full snapshot
    last_compaction:    u64,        # unix timestamp
    total_files:        u64,
    total_chunks:       u64,
    total_tombstoned:   u64,
    embedding_model:    String,     # "all-MiniLM-L6-v2"
    embedding_dim:      u32,        # 384
    hnsw_M:             u16,
    hnsw_ef_construction: u32,
    bm25_k1:            f32,
    bm25_b:             f32,
    watched_dirs:       Vec<String>,
}
```

---

## Entity relationship diagram

```
┌─────────────┐         ┌──────────────┐
│  FileMap     │ 1───*  │    Chunk      │
│  (per file)  │────────│  (per unit)   │
└─────────────┘         └──────┬───────┘
                               │
               ┌───────────────┼───────────────┐
               │               │               │
               ▼               ▼               ▼
      ┌────────────┐  ┌──────────────┐  ┌────────────┐
      │  Inverted   │  │  Embeddings  │  │   HNSW     │
      │  Index      │  │  Store       │  │   Graph    │
      │  (postings) │  │  (vectors)   │  │  (edges)   │
      └────────────┘  └──────────────┘  └────────────┘

  FileMap     →  Chunk:         1:N  (one file produces many chunks)
  Chunk       →  Embeddings:    1:1  (one vector per chunk)
  Chunk       →  HNSW Node:     1:1  (one graph node per chunk)
  Chunk       →  Postings:      1:N  (one chunk appears in many terms' lists)
  Term        →  Postings:      1:N  (one term's list references many chunks)

  All references use ChunkId (u64) as the join key.
  No foreign keys — all indexes are co-indexed by ChunkId.
```

---

## Size estimates (100k chunks)

| Structure | Calculation | Size |
|---|---|---|
| chunks.store (metadata) | 100k × 64 bytes | ~6 MB |
| chunks.store (content) | 100k × avg 500 bytes | ~50 MB |
| embeddings.bin | 100k × 384 × 4 bytes | ~150 MB |
| inverted.idx (postings) | ~500k terms × avg 20 entries × 10 bytes | ~100 MB |
| hnsw.idx | 100k × avg 20 neighbors × 8 bytes + overhead | ~20 MB |
| filemap.idx | ~10k files × avg 200 bytes | ~2 MB |
| **Total index** | | **~330 MB** |
