//! Knowledge graph extraction from source code ASTs.
//!
//! Parses each indexed file with tree-sitter and produces:
//!   Nodes — functions, methods, classes, structs, traits, API endpoints, modules
//!   Edges — Contains (file→def), Imports (file→file), Calls (fn→fn)

/// Short/common names that appear in every language's stdlib.
/// When a name is unambiguous (only one project node has it) but cross-module,
/// skip the edge — the call is almost certainly to a stdlib function, not the
/// one user-defined function that happens to share the name.
pub(super) const SKIP_CROSS_MODULE: &[&str] = &[
    "now", "new", "clone", "default", "from", "into", "as_ref", "as_mut",
    "len", "is_empty", "to_string", "as_str", "parse",
    "unwrap", "expect", "ok", "err", "map", "filter", "collect",
    "iter", "iter_mut", "into_iter", "next", "peek",
    "push", "pop", "get", "set", "insert", "remove", "contains",
    "fmt", "write", "read", "send", "recv", "spawn",
    "lock", "unlock", "drop", "close", "open", "init",
];

use crate::schema::Language;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// Inline macro — extracts UTF-8 text for a tree-sitter node without adding
// a call-graph edge (macros expand in place rather than generating call sites).
macro_rules! node_text {
    ($node:expr, $src:expr) => {
        std::str::from_utf8(&$src[$node.start_byte()..$node.end_byte()]).unwrap_or("")
    };
}

mod extract_scripting;
mod extract_rs_go;
mod extract_java_cpp;

// ── Public data structures ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Module,
    Function,
    Method,
    Class,
    Struct,
    Trait,
    Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Contains,
    Imports,
    Calls,
    Inherits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: u32,
    pub name: String,
    pub kind: NodeKind,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub language: String,
    /// Extra info: HTTP method for endpoints, parent class for methods, "enum"/"trait" for types
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: u32,
    pub to: u32,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphStats {
    pub total_nodes: u32,
    pub total_edges: u32,
    pub functions: u32,
    pub methods: u32,
    pub classes: u32,
    pub endpoints: u32,
    pub modules: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub stats: GraphStats,
}

// ── Internal extraction type ──────────────────────────────────────────────────

pub(super) struct RawDef {
    pub(super) name: String,
    pub(super) kind: NodeKind,
    pub(super) line_start: u32,
    pub(super) line_end: u32,
    pub(super) detail: Option<String>,
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Build a knowledge graph from a set of (path, language, content) triples.
/// Node IDs are stable and equal to the node's index in `graph.nodes`.
pub fn extract(file_entries: &[(PathBuf, Language, String)]) -> CodeGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();

    // file_path → module node id
    let mut file_module: HashMap<String, u32> = HashMap::new();
    // definition name → list of node ids (multiple files may define the same name)
    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();

    // ── Pass 1: Extract all nodes ─────────────────────────────────────────────
    for (path, lang, content) in file_entries {
        let fp = path.to_string_lossy().to_string();
        let fname = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| fp.clone());

        let module_id = nodes.len() as u32;
        nodes.push(GraphNode {
            id: module_id,
            name: fname,
            kind: NodeKind::Module,
            file_path: fp.clone(),
            line_start: 1,
            line_end: content.lines().count() as u32,
            language: lang.short_name().to_string(),
            detail: None,
        });
        file_module.insert(fp.clone(), module_id);

        for def in extract_defs(content, *lang) {
            let node_id = nodes.len() as u32;
            name_index.entry(def.name.clone()).or_default().push(node_id);
            // Rust methods are stored qualified (`Type::method`), but call sites emit
            // the bare segment (`self.method()`, `x.method()`, `Type::method()` all
            // resolve to `method` in find_rust_calls). Index the bare name too so
            // method calls actually link. Ambiguity (same bare name in many types) is
            // handled downstream by same-file preference + SKIP_CROSS_MODULE.
            if let Some(bare) = def.name.rsplit("::").next() {
                if bare != def.name.as_str() {
                    name_index.entry(bare.to_string()).or_default().push(node_id);
                }
            }
            edges.push(GraphEdge { from: module_id, to: node_id, kind: EdgeKind::Contains });
            nodes.push(GraphNode {
                id: node_id,
                name: def.name,
                kind: def.kind,
                file_path: fp.clone(),
                line_start: def.line_start,
                line_end: def.line_end,
                language: lang.short_name().to_string(),
                detail: def.detail,
            });
        }
    }

    // ── Pass 1.5: Mark Axum / JAX-RS / Express route handlers as Endpoint ─────
    for (path, lang, content) in file_entries {
        let fp = path.to_string_lossy().to_string();
        let handlers = match lang {
            Language::Rust => extract_axum_routes(content),
            Language::TypeScript | Language::JavaScript => extract_express_routes(content),
            _ => vec![],
        };
        for (http_method, handler_name) in handlers {
            if let Some(node_ids) = name_index.get(&handler_name) {
                for &nid in node_ids {
                    if nodes[nid as usize].file_path == fp || node_ids.len() == 1 {
                        nodes[nid as usize].kind = NodeKind::Endpoint;
                        nodes[nid as usize].detail = Some(http_method.clone());
                    }
                }
            }
        }
    }

    // ── Pass 2: Import edges ──────────────────────────────────────────────────
    for (path, lang, content) in file_entries {
        let fp = path.to_string_lossy().to_string();
        let from_id = *file_module.get(&fp).unwrap();
        let dir = path.parent().unwrap_or(Path::new("."));

        for imp in extract_imports(content, *lang) {
            if let Some(to_id) = resolve_import(&imp, dir, *lang, &file_module) {
                if from_id != to_id {
                    edges.push(GraphEdge { from: from_id, to: to_id, kind: EdgeKind::Imports });
                }
            }
        }
    }

    // ── Pass 3: Call edges (best-effort by name matching) ─────────────────────
    let all_names: HashSet<String> = name_index.keys().cloned().collect();
    for (path, lang, content) in file_entries {
        let fp = path.to_string_lossy().to_string();
        for (caller_name, callees) in extract_calls(content, *lang, &all_names) {
            let caller_id = name_index
                .get(&caller_name)
                .and_then(|ids| ids.iter().find(|&&id| nodes[id as usize].file_path == fp))
                .copied();
            let Some(caller_id) = caller_id else { continue };

            for callee_name in callees {
                if callee_name == caller_name { continue; }
                if let Some(callee_ids) = name_index.get(&callee_name) {
                    let callee_id = if callee_ids.len() == 1 {
                        let candidate = callee_ids[0];
                        let cross_file = nodes[candidate as usize].file_path != fp;
                        if cross_file && SKIP_CROSS_MODULE.contains(&callee_name.as_str()) {
                            None
                        } else {
                            Some(candidate)
                        }
                    } else {
                        callee_ids.iter()
                            .find(|&&id| nodes[id as usize].file_path == fp)
                            .copied()
                    };
                    if let Some(callee_id) = callee_id {
                        if callee_id != caller_id {
                            edges.push(GraphEdge { from: caller_id, to: callee_id, kind: EdgeKind::Calls });
                        }
                    }
                }
            }
        }
    }

    // ── Deduplicate edges ─────────────────────────────────────────────────────
    {
        let mut seen: HashSet<(u32, u32, EdgeKind)> = HashSet::new();
        edges.retain(|e| seen.insert((e.from, e.to, e.kind.clone())));
    }

    let stats = GraphStats {
        total_nodes: nodes.len() as u32,
        total_edges: edges.len() as u32,
        functions: nodes.iter().filter(|n| n.kind == NodeKind::Function).count() as u32,
        methods: nodes.iter().filter(|n| n.kind == NodeKind::Method).count() as u32,
        classes: nodes.iter().filter(|n| matches!(n.kind, NodeKind::Class | NodeKind::Struct | NodeKind::Trait)).count() as u32,
        endpoints: nodes.iter().filter(|n| n.kind == NodeKind::Endpoint).count() as u32,
        modules: nodes.iter().filter(|n| n.kind == NodeKind::Module).count() as u32,
    };

    CodeGraph { nodes, edges, stats }
}

// ── Route handler detection ───────────────────────────────────────────────────

/// Detect Axum `.route("/path", get(handler))` patterns in Rust source.
fn extract_axum_routes(content: &str) -> Vec<(String, String)> {
    let http_methods = ["get", "post", "put", "delete", "patch", "head", "options"];
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if !l.contains(".route(") && !l.contains("route!(") { continue; }
        for method in &http_methods {
            let pat = format!("{}(", method);
            if let Some(pos) = l.find(&pat) {
                let before = &l[..pos];
                if before.contains('"') && before.matches('"').count() % 2 == 1 { continue; }
                let after = &l[pos + pat.len()..];
                let end = after.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(after.len());
                let handler = after[..end].trim();
                if handler.len() > 2 { out.push((method.to_uppercase(), handler.to_string())); }
            }
        }
    }
    out
}

/// Detect Express.js `router.get('/path', handler)` patterns.
fn extract_express_routes(content: &str) -> Vec<(String, String)> {
    let http_methods = ["get", "post", "put", "delete", "patch", "use"];
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        for method in &http_methods {
            let pat = format!(".{}('", method);
            let pat2 = format!(".{}(\"/", method);
            let base = if l.contains(&pat) { &pat } else if l.contains(&pat2) { &pat2 } else { continue };
            if let Some(close_quote) = l.find(|c| c == '\'' || c == '"').and_then(|p| {
                l[p+1..].find(|c| c == '\'' || c == '"').map(|q| p + 1 + q)
            }) {
                let rest = &l[close_quote+1..];
                if let Some(comma) = rest.find(',') {
                    let handler_part = rest[comma+1..].trim();
                    let end = handler_part.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(handler_part.len());
                    let handler = handler_part[..end].trim();
                    if handler.len() > 2 { out.push((method.to_uppercase(), handler.to_string())); }
                }
            }
            let _ = base;
        }
    }
    out
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

fn extract_defs(content: &str, lang: Language) -> Vec<RawDef> {
    match lang {
        Language::Python => extract_scripting::extract_python_defs(content),
        Language::TypeScript | Language::JavaScript => extract_scripting::extract_ts_defs(content),
        Language::Rust => extract_rs_go::extract_rust_defs(content),
        Language::Go => extract_rs_go::extract_go_defs(content),
        Language::Java => extract_java_cpp::extract_java_defs(content),
        Language::C | Language::Cpp => extract_java_cpp::extract_cpp_defs(content),
        _ => vec![],
    }
}

fn extract_imports(content: &str, lang: Language) -> Vec<String> {
    match lang {
        Language::Python => extract_scripting::extract_python_imports(content),
        Language::TypeScript | Language::JavaScript => extract_scripting::extract_ts_imports(content),
        Language::Rust => extract_rs_go::extract_rust_mod_decls(content),
        _ => vec![],
    }
}

fn extract_calls(content: &str, lang: Language, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    match lang {
        Language::Python => extract_scripting::extract_python_calls(content, known),
        Language::TypeScript | Language::JavaScript => extract_scripting::extract_ts_calls(content, known),
        Language::Rust => extract_rs_go::extract_rust_calls(content, known),
        Language::Java => extract_java_cpp::extract_java_calls(content, known),
        Language::C | Language::Cpp => extract_java_cpp::extract_cpp_calls(content, known),
        _ => vec![],
    }
}

// ── Shared call-extraction ────────────────────────────────────────────────────

pub(super) fn find_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    // "call" = Python,  "call_expression" = TypeScript/JavaScript
    if matches!(node.kind(), "call" | "call_expression") {
        if let Some(func) = node.child_by_field_name("function") {
            let name = match func.kind() {
                "identifier" => node_text!(func, src).to_string(),
                "attribute" | "member_expression" => {
                    (0..func.child_count())
                        .find_map(|i| func.child(func.child_count() - 1 - i))
                        .map(|n| node_text!(n, src).to_string())
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            if !name.is_empty() && known.contains(&name) { out.push(name); }
        }
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { find_calls(&child, src, known, out); }
    }
}

// ── Import resolution ─────────────────────────────────────────────────────────

fn resolve_import(
    module: &str,
    current_dir: &Path,
    lang: Language,
    file_module: &HashMap<String, u32>,
) -> Option<u32> {
    if matches!(lang, Language::TypeScript | Language::JavaScript) && module.starts_with('.') {
        let base = current_dir.join(module.trim_start_matches("./"));
        let exts = ["ts", "tsx", "js", "jsx"];
        let suffixes: Vec<PathBuf> = exts.iter().map(|e| PathBuf::from(format!("{}.{}", base.to_string_lossy(), e)))
            .chain(exts.iter().map(|e| base.join(format!("index.{}", e))))
            .collect();
        for candidate in suffixes {
            let c = candidate.to_string_lossy().replace('/', "\\");
            if let Some(&id) = file_module.get(&c) { return Some(id); }
            let c2 = candidate.to_string_lossy().replace('\\', "/");
            if let Some(&id) = file_module.get(&c2) { return Some(id); }
        }
    }

    let stem = module.split('.').last().unwrap_or(module).to_lowercase();
    if stem.is_empty() { return None; }

    for (path, &id) in file_module {
        let file_stem = Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if file_stem == stem { return Some(id); }
    }

    None
}

// ── Analytics ─────────────────────────────────────────────────────────────────

/// Label-propagation community detection on Calls + Imports edges (undirected).
/// Returns map from node_id → community_id (renumbered 0..N).
pub fn compute_communities(graph: &CodeGraph) -> HashMap<u32, u32> {
    let n = graph.nodes.len();
    if n == 0 { return HashMap::new(); }

    let mut neighbors: HashMap<u32, Vec<u32>> = graph.nodes.iter().map(|n| (n.id, vec![])).collect();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Calls | EdgeKind::Imports) {
            neighbors.entry(edge.from).or_default().push(edge.to);
            neighbors.entry(edge.to).or_default().push(edge.from);
        }
    }

    let mut community: HashMap<u32, u32> = graph.nodes.iter().map(|n| (n.id, n.id)).collect();

    for _ in 0..50 {
        let mut changed = false;
        let node_ids: Vec<u32> = graph.nodes.iter().map(|n| n.id).collect();
        for &node_id in &node_ids {
            let Some(nbrs) = neighbors.get(&node_id) else { continue };
            if nbrs.is_empty() { continue; }

            let mut freq: HashMap<u32, u32> = HashMap::new();
            for &nbr in nbrs {
                if let Some(&c) = community.get(&nbr) { *freq.entry(c).or_default() += 1; }
            }
            if let Some((&best_c, _)) = freq.iter().max_by_key(|&(_, &cnt)| cnt) {
                if community.get(&node_id) != Some(&best_c) {
                    community.insert(node_id, best_c);
                    changed = true;
                }
            }
        }
        if !changed { break; }
    }

    let mut id_map: HashMap<u32, u32> = HashMap::new();
    let mut next = 0u32;
    graph.nodes.iter().map(|node| {
        let old = community[&node.id];
        let new = *id_map.entry(old).or_insert_with(|| { let v = next; next += 1; v });
        (node.id, new)
    }).collect()
}

/// Returns the top `limit` nodes sorted by total call-graph degree (in + out).
/// Module nodes are excluded. Returns Vec<(node_id, degree)>.
pub fn compute_god_nodes(graph: &CodeGraph, limit: usize) -> Vec<(u32, u32)> {
    let mut degree: HashMap<u32, u32> = HashMap::new();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Calls) {
            *degree.entry(edge.from).or_default() += 1;
            *degree.entry(edge.to).or_default() += 1;
        }
    }

    let mut ranked: Vec<(u32, u32)> = graph.nodes.iter()
        .filter(|n| !matches!(n.kind, NodeKind::Module))
        .map(|n| (n.id, *degree.get(&n.id).unwrap_or(&0)))
        .filter(|&(_, d)| d > 0)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(limit);
    ranked
}

/// Returns the most surprising cross-community Calls edges — ones that bridge
/// otherwise disconnected clusters. Rarity (fewer parallel bridges) = higher surprise.
/// Returns Vec<(from_id, to_id)> up to 15 entries.
pub fn find_surprise_edges(graph: &CodeGraph, communities: &HashMap<u32, u32>) -> Vec<(u32, u32)> {
    let mut cross_count: HashMap<(u32, u32), u32> = HashMap::new();
    for edge in &graph.edges {
        if !matches!(edge.kind, EdgeKind::Calls) { continue; }
        let ca = communities.get(&edge.from).copied().unwrap_or(edge.from);
        let cb = communities.get(&edge.to).copied().unwrap_or(edge.to);
        if ca != cb {
            let key = if ca < cb { (ca, cb) } else { (cb, ca) };
            *cross_count.entry(key).or_default() += 1;
        }
    }

    let mut surprises: Vec<(u32, u32, u32)> = graph.edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls))
        .filter_map(|e| {
            let ca = communities.get(&e.from).copied().unwrap_or(e.from);
            let cb = communities.get(&e.to).copied().unwrap_or(e.to);
            if ca == cb { return None; }
            let key = if ca < cb { (ca, cb) } else { (cb, ca) };
            let count = *cross_count.get(&key).unwrap_or(&1);
            Some((e.from, e.to, count))
        })
        .collect();

    surprises.sort_by_key(|&(from, to, count)| (count, from, to));
    surprises.dedup_by_key(|s| (s.0, s.1));
    surprises.truncate(15);
    surprises.into_iter().map(|(f, t, _)| (f, t)).collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(super) fn detect_http_method(decorator: &str) -> Option<String> {
    let d = decorator.to_lowercase();
    for m in &["get", "post", "put", "delete", "patch", "head", "options"] {
        if d.contains(&format!(".{}(", m)) { return Some(m.to_uppercase()); }
    }
    if d.contains(".route(") { return Some("ROUTE".to_string()); }
    None
}
