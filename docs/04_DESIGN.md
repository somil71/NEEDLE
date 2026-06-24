# Needle — Design Details

---

## Design philosophy

Needle's interface should feel like ripgrep meets Spotlight: invisible when you don't need it, instant when you do. The design serves one job — get the user from a half-formed thought to the right chunk of code or text in under a second, including the time they spend reading results. Every design decision optimizes for scan speed and signal clarity.

---

## Part 1: CLI interface design

The CLI is the primary and v1 interface. It must be fast to invoke, fast to scan, and information-dense without being noisy.

### Color system

Use ANSI 256 colors, with automatic detection and fallback to 16-color or no-color modes. Respect `NO_COLOR` env var and `--no-color` flag.

```
Semantic color assignments:

  File paths          →  Blue (ANSI 33)         — the anchor your eye finds first
  Line numbers        →  Dim gray (ANSI 245)    — present but not competing
  Match highlights    →  Bold yellow (ANSI 1;33) — the reason you searched
  Chunk type badges   →  Cyan (ANSI 36)         — categorical, not urgent
  Score / metadata    →  Dim (ANSI 2)           — available, never in the way
  Signal badges:
    [HYBRID]          →  Bold green (ANSI 1;32)  — the best kind of result
    [KW]              →  Yellow (ANSI 33)        — keyword match only
    [SEM]             →  Magenta (ANSI 35)       — semantic match only
  Errors              →  Red (ANSI 31)
  Warnings            →  Yellow (ANSI 33)
  Progress bars       →  Blue (ANSI 34)
  Dim separators      →  Dark gray (ANSI 238)
```

### Result card layout

Each search result is a self-contained card. The user scans the left edge for file paths and badges, then reads inward for content.

```
 ❶ src/http/retry.rs:42-67  fn  [HYBRID]  0.031
 │  pub async fn retry_with_backoff<F, T>(
 │      f: F, max_retries: u32
 │  ) -> Result<T>
 │  where F: Fn() -> Future<Output = Result<T>> {
 │      for attempt in 0..max_retries {
 │          match f().await {
 │              Ok(v) => return Ok(v),
 │              Err(e) if attempt < max_retries - 1 => {
 │                  sleep(<<backoff>>(attempt)).await;
 │
 ❷ docs/architecture/resilience.md:15-28  §  [SEM]  0.024
 │  ## Retry strategy
 │  All outbound HTTP calls use exponential <<backoff>> with
 │  jitter. The base delay is 100ms, capped at 30s. The
 │  <<retry>> budget is per-request, not per-circuit...
```

**Anatomy**:
- **Rank number** (❶❷❸...): left gutter, bold, gives positional reference.
- **File path**: blue, truncated from the left if too long (`…/deeply/nested/path.rs`).
- **Line range**: dim, colon-separated after path.
- **Chunk type badge**: short, fixed-width: `fn` (function), `cl` (class), `mt` (method), `§` (section), `¶` (paragraph), `{}` (config block), `im` (import).
- **Signal badge**: `[HYBRID]`, `[KW]`, or `[SEM]` in their assigned color.
- **Score**: dim, right-aligned. Shown for power users; hidden with `--compact`.
- **Content snippet**: 3–8 lines of the chunk with keyword matches highlighted in bold yellow. `<<matched term>>` notation above represents bold yellow in the actual terminal.
- **Vertical bar**: dim gray left border, visually groups the snippet under its header.

### Snippet rendering rules

- Show the most relevant contiguous window of the chunk (center on the densest cluster of keyword matches, or the first 6 lines if semantic-only).
- Highlight matched keywords inline with bold + yellow.
- If the snippet is truncated, show `...` at the cut boundary.
- Preserve original indentation.
- Syntax highlighting is a stretch goal — v1 uses plain text with match highlights only.
- Line numbers in the gutter (dim) are absolute (from the original file), not relative to the snippet.

### Command output specifications

**`needle search <query>`**

```
$ needle search "retry backoff"

  Found 7 results in 2.1ms (BM25: 1.3ms, HNSW: 0.8ms, fusion: 0.1ms)

  ❶ src/http/retry.rs:42-67  fn  [HYBRID]  0.031
  │  pub async fn retry_with_backoff<F, T>(
  │  ...
  │
  ❷ docs/architecture/resilience.md:15-28  §  [SEM]  0.024
  │  ...

  ──────────────────────────────────────────────
  7 results  ·  index: 94,217 chunks across 3,841 files
```

- Timing breakdown in the header: how long each stage took.
- Footer: total results + index summary (subtle context, not noise).
- Default: 10 results. `--limit N` to change. `--all` for everything above threshold.

**`needle init <dirs...>`**

```
$ needle init ~/code ~/notes

  Needle v0.1.0 — initializing index

  Scanning directories...
  ├── ~/code     12,847 files
  └── ~/notes     1,203 files

  Chunking  ████████████████████████████████  14,050 files  [42s]
  Embedding ████████████████████████████████  94,217 chunks [3m 12s]
  Building inverted index...  done [1.2s]
  Building HNSW graph...      done [8.4s]

  ✓ Index ready
    94,217 chunks · 14,050 files · 327 MB on disk
    Model: all-MiniLM-L6-v2 (384-dim)
    Stored at: ~/.needle/index/
```

- Progress bars for the two slow steps (chunking and embedding).
- Final summary: what was built, how big, where it lives.

**`needle status`**

```
$ needle status

  Needle v0.1.0 — index status

  Watched directories:
    ~/code     12,847 files  (watching ✓)
    ~/notes     1,203 files  (watching ✓)

  Index health:
    Chunks:     94,217 active  ·  342 tombstoned
    Files:      14,050
    Disk:       327 MB (chunks: 56MB, vectors: 150MB, postings: 101MB, graph: 20MB)
    Last update: 3 seconds ago
    Uptime:     2h 14m (watcher PID 48291)

  HNSW:
    Recall@10:  97.2% (last bench)
    Layers:     4 (entry point: chunk #8829, layer 3)
    M=16, efConstruction=200

  BM25:
    Vocabulary: 487,293 terms
    k1=1.2, b=0.75
```

**`needle bench`**

```
$ needle bench

  Running benchmarks on 94,217 chunks...

  HNSW recall (1000 random queries):
    recall@10:  97.2%  (min: 90%, p5: 94%)
    recall@50:  99.1%

  Query latency (100 queries, hybrid):
    p50:   1.8ms
    p95:   4.2ms
    p99:   6.1ms
    breakdown:  BM25 0.9ms · HNSW 0.7ms · embed 3.1ms · fuse 0.1ms

  Indexing throughput:
    Chunking:    2,140 files/sec
    Embedding:   489 chunks/sec (batch=32)

  Index size:
    Total: 327 MB
    ├── chunks.store     56 MB
    ├── embeddings.bin  150 MB
    ├── inverted.idx    101 MB
    ├── hnsw.idx         20 MB
    └── filemap.idx       2 MB
```

### Interactive mode (stretch)

`needle` with no arguments opens an interactive REPL with live-as-you-type results (like fzf). Renders results incrementally as the user types, with debounced search (150ms after last keystroke).

```
┌─ needle ──────────────────────────────────────┐
│ > retry back█                                 │
│                                               │
│ ❶ src/http/retry.rs:42  fn  [HYBRID]         │
│   retry_with_backoff                          │
│                                               │
│ ❷ docs/resilience.md:15  §  [SEM]            │
│   Retry strategy — exponential backoff...     │
│                                               │
│ ❸ tests/http_test.rs:89  fn  [KW]            │
│   test_retry_gives_up_after_max               │
│                                               │
│ ↑↓ navigate  ⏎ open in $EDITOR  ⌃C quit      │
└───────────────────────────────────────────────┘
```

Enter opens the selected result at the exact line in `$EDITOR`.

---

## Part 2: Web UI design (stretch goal)

A single-page local web UI served on `localhost:7890`. One search bar, zero chrome.

### Design direction

The aesthetic is **terminal-native meets typographic clarity** — think a beautifully typeset man page, not a SaaS dashboard. The page is mostly empty space and a search bar. Results appear below with the same information density as the CLI but with the benefits of proportional type, syntax highlighting, and clickable file paths.

### Typography

```
Display / search input:    JetBrains Mono     — 20px, weight 400
Result file paths:         JetBrains Mono     — 14px, weight 500
Result content:            JetBrains Mono     — 13px, weight 400
Body / metadata:           Inter              — 13px, weight 400
Badges:                    Inter              — 11px, weight 600, uppercase tracking +0.05em
```

Monospace for everything the user might copy or that represents code. Proportional only for metadata and labels.

### Color palette

```
Background:         #0C0C0E   (near-black, warm)
Surface (cards):    #141416
Border:             #1E1E22
Text primary:       #E8E6E0   (warm white)
Text secondary:     #8A8880
Text dim:           #555550

Accent blue:        #5B9EF5   (file paths, links)
Accent yellow:      #E5C07B   (match highlights)
Accent green:       #7EC97A   (HYBRID badge)
Accent magenta:     #C678DD   (SEM badge)
Accent amber:       #D19A66   (KW badge)
Accent red:         #E06C75   (errors)
```

Dark-only. No light mode. This is a developer tool that runs locally — it should look like it belongs next to a terminal.

### Layout

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│                                                             │
│                          ◇ needle                           │
│                                                             │
│              ┌────────────────────────────────┐             │
│              │  Search your code and notes... │             │
│              └────────────────────────────────┘             │
│                                                             │
│         3 results in 1.8ms  ·  94,217 chunks indexed        │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  src/http/retry.rs:42-67                    fn       │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  pub async fn retry_with_backoff<F, T>(        │  │   │
│  │  │      f: F, max_retries: u32                    │  │   │
│  │  │  ) -> Result<T> {                              │  │   │
│  │  │      for attempt in 0..max_retries {           │  │   │
│  │  │          ...                                   │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  │  HYBRID  0.031                                       │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  docs/architecture/resilience.md:15-28          §    │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  ## Retry strategy                             │  │   │
│  │  │  All outbound HTTP calls use exponential       │  │   │
│  │  │  backoff with jitter...                        │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  │  SEM  0.024                                          │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Component specifications

**Search bar**:
- Centered, 560px max-width, 48px height.
- 1px border, `#1E1E22`, 8px radius.
- Background: `#141416`.
- Placeholder: "Search your code and notes..." in `#555550`.
- No search button — results stream as the user types (150ms debounce).
- Focus state: border shifts to `#5B9EF5` at 50% opacity.
- Keyboard: `Esc` clears, `↑↓` navigates results, `Enter` opens file.

**Result card**:
- Full-width (max 720px, centered), 1px border `#1E1E22`, 8px radius.
- 16px padding.
- Stacked vertically with 8px gap between cards.
- Header row: file path (blue, monospace, clickable → opens in editor via `vscode://` or configurable scheme) + chunk type badge (right-aligned).
- Code block: inner container with `#0C0C0E` background, 4px radius, 12px padding. Syntax-highlighted (stretch) or plain monospace with keyword highlights (v1).
- Footer row: signal badge (pill shape, 4px radius, colored background at 15% opacity with text in full color) + score (dim, right).

**Signal badges**:
```
  ┌──────────┐
  │  HYBRID  │   bg: #7EC97A at 15%   text: #7EC97A   border: none
  └──────────┘

  ┌──────┐
  │  KW  │       bg: #D19A66 at 15%   text: #D19A66
  └──────┘

  ┌──────┐
  │  SEM │       bg: #C678DD at 15%   text: #C678DD
  └──────┘
```

**Timing header**:
- Appears between search bar and results.
- "3 results in 1.8ms · 94,217 chunks indexed" — secondary text color, Inter 13px.
- Fades in with 150ms ease after results arrive.

**Empty state** (before first search):
- Just the logo mark and search bar. Nothing else. No tips, no onboarding, no feature list.

**No results state**:
- "No results for 'xyz'" in secondary text, centered below the search bar.
- No suggestions, no "did you mean?" (v1). Just honest emptiness.

### Interaction patterns

**Live search**: results update as the user types. The last query result always wins (cancel in-flight requests if a new keystroke arrives). Show a subtle 2px loading bar below the search input during the query (visible only if query takes > 50ms).

**Keyboard navigation**: `↑↓` moves a subtle highlight (left border accent, 2px blue) between result cards. `Enter` on a highlighted card opens the file at the line number. `Escape` returns focus to the search bar.

**File opening**: clicking a file path constructs a URI: `vscode://file/{absolute_path}:{line_start}` by default. Configurable in `config.toml` to support other editors (vim, emacs, IntelliJ, Sublime). Falls back to copying the path to clipboard if no editor scheme is configured.

### Animation and motion

- Results: fade in + translate up 8px, staggered 30ms per card, 200ms ease-out.
- Search bar focus: border color transition 150ms ease.
- Loading bar: indeterminate left-to-right sweep, 1.5s duration.
- Respect `prefers-reduced-motion`: disable all animation, show results instantly.

### Performance budget

- HTML + CSS + JS total: < 50KB (no framework, vanilla JS).
- First paint: < 100ms (it's localhost, serving static files).
- Search-to-results: < 20ms perceived (query + render).
- No external CDN dependencies. Everything inlined or served locally.

---

## Part 3: Visual identity

### Logo mark

A simple geometric mark: a **needle** (thin diagonal line, 45°) piercing through a **dot grid** (representing the haystack of files). Minimal, monochrome, works at 16px (favicon) and 128px.

```
Concept (ASCII approximation):

    · · · ·
    · · /·
    · /· ·
    /· · ·
```

### Name treatment

"needle" in lowercase JetBrains Mono, weight 300 (light), with generous letter-spacing (+0.08em). The word itself is the brand — no wordmark decoration needed.

### README hero

The README should open with a single animated GIF (or asciinema recording) showing:

1. `needle init ~/code` — indexing 10k files in real time.
2. `needle search "retry backoff"` — results in 2ms.
3. A second search with a vague natural-language query showing the semantic result that keyword search would miss.

Under 15 seconds. No narration. The speed is the pitch.

---

## Part 4: Information architecture

### Config file (`~/.needle/config.toml`)

```toml
# Needle configuration

[directories]
watch = ["~/code", "~/notes", "~/docs"]
ignore = [".git", "node_modules", "target", "__pycache__", ".env", "*.lock"]

[index]
embedding_model = "all-MiniLM-L6-v2"
embedding_dim = 384

[hnsw]
M = 16
M_max0 = 32
ef_construction = 200
ef_search = 50

[bm25]
k1 = 1.2
b = 0.75
stemming = true
stopwords = true

[search]
default_limit = 10
rrf_k = 60
snippet_lines = 6
snippet_context = 2  # lines of context around matches

[ui]
editor_scheme = "vscode"  # vscode | vim | emacs | intellij | sublime | copy
color = "auto"            # auto | always | never
web_port = 7890

[watcher]
debounce_ms = 300
```

### Error messages

Errors are direct, actionable, and never apologetic.

```
Error: ~/code does not exist
  The directory you specified for indexing was not found.
  Check the path and try again: needle init ~/code

Error: embedding model not found
  Expected: ~/.needle/models/minilm-l6-v2.onnx
  Run: needle init --download-model

Error: index corrupted (WAL checksum mismatch at sequence 4821)
  The index may have been damaged by an unclean shutdown.
  Run: needle reindex
  This will rebuild the full index from your files (~3 minutes for 14k files).
```

No "oops!", no "something went wrong", no "please try again later." State what happened, why, and what to do.
