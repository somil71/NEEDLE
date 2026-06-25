//! Knowledge graph extraction from source code ASTs.
//!
//! Parses each indexed file with tree-sitter and produces:
//!   Nodes — functions, methods, classes, structs, traits, API endpoints, modules
//!   Edges — Contains (file→def), Imports (file→file), Calls (fn→fn)

/// Short/common names that appear in every language's stdlib.
/// When a name is unambiguous (only one project node has it) but cross-module,
/// skip the edge — the call is almost certainly to a stdlib function, not the
/// one user-defined function that happens to share the name.
const SKIP_CROSS_MODULE: &[&str] = &[
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

struct RawDef {
    name: String,
    kind: NodeKind,
    line_start: u32,
    line_end: u32,
    detail: Option<String>,
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
        let fname = path
            .file_name()
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

    // ── Pass 1.5: Mark Axum / JAX-RS / Express route handlers as Endpoint ───────
    // Scans for .route("/path", get(handler)) patterns without requiring a new dep.
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
            // Find the caller node that lives in this exact file
            let caller_id = name_index
                .get(&caller_name)
                .and_then(|ids| ids.iter().find(|&&id| nodes[id as usize].file_path == fp))
                .copied();
            let Some(caller_id) = caller_id else { continue };

            for callee_name in callees {
                if callee_name == caller_name { continue; }
                if let Some(callee_ids) = name_index.get(&callee_name) {
                    // When a name is shared across multiple files (e.g. `run`),
                    // prefer the same-file definition to avoid false cross-file edges.
                    // For unambiguous names (one match), add the edge — UNLESS the
                    // name is in the stdlib-collision blocklist AND the callee lives
                    // in a different file (e.g. every `now()` call shouldn't link to
                    // the one user-defined `now` in server/users.rs).
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
                        edges.push(GraphEdge { from: caller_id, to: callee_id, kind: EdgeKind::Calls });
                    }
                }
            }
        }
    }

    // ── Deduplicate edges ─────────────────────────────────────────────────────
    {
        let mut seen: HashSet<(u32, u32, String)> = HashSet::new();
        edges.retain(|e| seen.insert((e.from, e.to, format!("{:?}", e.kind))));
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
            // Matches: get(fn_name) or post(fn_name)
            let pat = format!("{}(", method);
            if let Some(pos) = l.find(&pat) {
                // Make sure it's not inside a string (crude but effective)
                let before = &l[..pos];
                if before.contains('"') && before.matches('"').count() % 2 == 1 { continue; }
                let after = &l[pos + pat.len()..];
                let end = after.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(after.len());
                let handler = after[..end].trim();
                if handler.len() > 2 {
                    out.push((method.to_uppercase(), handler.to_string()));
                }
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
            // Matches: router.get('/path', handler) or app.post('/path', handler)
            let pat = format!(".{}('", method);
            let pat2 = format!(".{}(\"/", method);
            let base = if l.contains(&pat) { &pat } else if l.contains(&pat2) { &pat2 } else { continue };
            // Find closing quote then comma then handler
            if let Some(close_quote) = l.find(|c| c == '\'' || c == '"').and_then(|p| {
                l[p+1..].find(|c| c == '\'' || c == '"').map(|q| p + 1 + q)
            }) {
                let rest = &l[close_quote+1..];
                if let Some(comma) = rest.find(',') {
                    let handler_part = rest[comma+1..].trim();
                    let end = handler_part.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(handler_part.len());
                    let handler = handler_part[..end].trim();
                    if handler.len() > 2 {
                        out.push((method.to_uppercase(), handler.to_string()));
                    }
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
        Language::Python => extract_python_defs(content),
        Language::TypeScript | Language::JavaScript => extract_ts_defs(content),
        Language::Rust => extract_rust_defs(content),
        Language::Go => extract_go_defs(content),
        Language::Java => extract_java_defs(content),
        Language::C | Language::Cpp => extract_cpp_defs(content),
        _ => vec![],
    }
}

fn extract_imports(content: &str, lang: Language) -> Vec<String> {
    match lang {
        Language::Python => extract_python_imports(content),
        Language::TypeScript | Language::JavaScript => extract_ts_imports(content),
        Language::Rust => extract_rust_mod_decls(content),
        _ => vec![],
    }
}

fn extract_calls(content: &str, lang: Language, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    match lang {
        Language::Python => extract_python_calls(content, known),
        Language::TypeScript | Language::JavaScript => extract_ts_calls(content, known),
        Language::Rust => extract_rust_calls(content, known),
        Language::Java => extract_java_calls(content, known),
        Language::C | Language::Cpp => extract_cpp_calls(content, known),
        _ => vec![],
    }
}

// ── Python ────────────────────────────────────────────────────────────────────

fn extract_python_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_python::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let mut defs = Vec::new();
    walk_py(&tree.root_node(), content.as_bytes(), &mut defs, false);
    defs
}

fn walk_py(node: &tree_sitter::Node, src: &[u8], out: &mut Vec<RawDef>, in_class: bool) {
    match node.kind() {
        "function_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: if in_class { NodeKind::Method } else { NodeKind::Function },
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
            return; // don't descend into nested fns at this level
        }
        "class_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Class,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                    let mut c = node.walk();
                    for child in node.children(&mut c) {
                        walk_py(&child, src, out, true);
                    }
                    return;
                }
            }
        }
        "decorated_definition" => {
            let mut http: Option<String> = None;
            {
                let mut c = node.walk();
                for child in node.children(&mut c) {
                    if child.kind() == "decorator" {
                        http = detect_http_method(txt(child, src));
                    }
                }
            }
            {
                let mut c = node.walk();
                for child in node.children(&mut c) {
                    if child.kind() == "function_definition" {
                        if let Some(n) = child.child_by_field_name("name") {
                            let name = txt(n, src).to_string();
                            if !name.is_empty() {
                                let kind = if http.is_some() {
                                    NodeKind::Endpoint
                                } else if in_class {
                                    NodeKind::Method
                                } else {
                                    NodeKind::Function
                                };
                                out.push(RawDef {
                                    name,
                                    kind,
                                    line_start: node.start_position().row as u32 + 1,
                                    line_end: child.end_position().row as u32 + 1,
                                    detail: http.clone(),
                                });
                            }
                        }
                    } else if child.kind() == "class_definition" {
                        walk_py(&child, src, out, in_class);
                    }
                }
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk_py(&child, src, out, in_class);
    }
}

fn extract_python_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with("import ") {
            let rest = &l[7..];
            let module = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("");
            if !module.is_empty() {
                out.push(module.to_string());
            }
        } else if l.starts_with("from ") {
            if let Some(module_part) = l[5..].split(" import ").next() {
                let m = module_part.trim().trim_start_matches('.').split('.').next().unwrap_or("").trim();
                if !m.is_empty() {
                    out.push(m.to_string());
                }
            }
        }
    }
    out
}

fn extract_python_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_python::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    collect_fn_calls_py(&tree.root_node(), src, known, &mut out);
    out
}

fn collect_fn_calls_py(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<(String, Vec<String>)>) {
    if node.kind() == "function_definition" {
        let fn_name = node
            .child_by_field_name("name")
            .map(|n| txt(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            if let Some(body) = node.child_by_field_name("body") {
                let mut calls = Vec::new();
                find_calls(&body, src, known, &mut calls);
                calls.sort_unstable();
                calls.dedup();
                if !calls.is_empty() {
                    out.push((fn_name, calls));
                }
            }
        }
        return;
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_fn_calls_py(&child, src, known, out);
    }
}

// ── TypeScript / JavaScript ───────────────────────────────────────────────────

fn extract_ts_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_typescript::language_typescript()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let mut defs = Vec::new();
    walk_ts(&tree.root_node(), content.as_bytes(), &mut defs, false);
    defs
}

fn walk_ts(node: &tree_sitter::Node, src: &[u8], out: &mut Vec<RawDef>, in_class: bool) {
    match node.kind() {
        "function_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: if in_class { NodeKind::Method } else { NodeKind::Function },
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
        }
        "class_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Class,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                    let mut c = node.walk();
                    for child in node.children(&mut c) {
                        walk_ts(&child, src, out, true);
                    }
                    return;
                }
            }
        }
        "method_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() && name != "constructor" {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Method,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
        }
        "call_expression" => {
            // Detect Express routes: app.get('/path', handler) / router.post('/path', ...)
            if let Some(func) = node.child_by_field_name("function") {
                if func.kind() == "member_expression" {
                    let prop = func.child_by_field_name("property")
                        .map(|n| txt(n, src))
                        .unwrap_or_default();
                    if matches!(prop, "get" | "post" | "put" | "delete" | "patch") {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let mut ac = args.walk();
                            let first = args.children(&mut ac).find(|n| {
                                n.is_named() && matches!(n.kind(), "string" | "template_string")
                            });
                            if let Some(path_node) = first {
                                let route = txt(path_node, src)
                                    .trim_matches(|c| matches!(c, '\'' | '"' | '`'))
                                    .to_string();
                                out.push(RawDef {
                                    name: format!("{} {}", prop.to_uppercase(), route),
                                    kind: NodeKind::Endpoint,
                                    line_start: node.start_position().row as u32 + 1,
                                    line_end: node.end_position().row as u32 + 1,
                                    detail: Some(prop.to_uppercase()),
                                });
                            }
                        }
                    }
                }
            }
        }
        "export_statement" => {
            // export function X / export class X / export const X = ...
            let mut c = node.walk();
            for child in node.children(&mut c) {
                walk_ts(&child, src, out, in_class);
            }
            return;
        }
        _ => {}
    }
    // Don't recurse into function bodies for top-level scanning
    if !matches!(node.kind(), "function_declaration" | "arrow_function" | "function") {
        let mut c = node.walk();
        for child in node.children(&mut c) {
            walk_ts(&child, src, out, in_class);
        }
    }
}

fn extract_ts_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with("import ") && l.contains(" from ") {
            if let Some(from_part) = l.split(" from ").last() {
                let m = from_part
                    .trim()
                    .trim_end_matches(';')
                    .trim_matches(|c| matches!(c, '\'' | '"' | '`'));
                if m.starts_with('.') || m.starts_with('/') {
                    out.push(m.to_string());
                }
            }
        } else if l.contains("require(") {
            if let Some(start) = l.find("require(") {
                let rest = &l[start + 8..];
                if let Some(end) = rest.find(')') {
                    let m = rest[..end].trim().trim_matches(|c| matches!(c, '\'' | '"'));
                    if m.starts_with('.') {
                        out.push(m.to_string());
                    }
                }
            }
        }
    }
    out
}

fn extract_ts_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_typescript::language_typescript()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    collect_fn_calls_ts(&tree.root_node(), src, known, &mut out);
    out
}

fn collect_fn_calls_ts(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<(String, Vec<String>)>) {
    if matches!(node.kind(), "function_declaration" | "method_definition") {
        let fn_name = node
            .child_by_field_name("name")
            .map(|n| txt(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            if let Some(body) = node.child_by_field_name("body") {
                let mut calls = Vec::new();
                find_calls(&body, src, known, &mut calls);
                calls.sort_unstable();
                calls.dedup();
                if !calls.is_empty() {
                    out.push((fn_name, calls));
                }
            }
        }
        return;
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_fn_calls_ts(&child, src, known, out);
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn extract_rust_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_rust::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    walk_rust(&tree.root_node(), src, &mut out);
    out
}

fn walk_rust(node: &tree_sitter::Node, src: &[u8], out: &mut Vec<RawDef>) {
    match node.kind() {
        "function_item" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Function,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
        }
        "struct_item" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Struct,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
        }
        "enum_item" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Class,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: Some("enum".to_string()),
                    });
                }
            }
        }
        "trait_item" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Trait,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
        }
        "impl_item" => {
            // impl Foo or impl Bar for Foo — extract methods
            let type_name = node
                .child_by_field_name("type")
                .map(|n| txt(n, src).to_string())
                .unwrap_or_default();
            // Recurse into the declaration_list to find function_items
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.kind() == "declaration_list" {
                    let mut c2 = child.walk();
                    for inner in child.children(&mut c2) {
                        if inner.kind() == "function_item" {
                            if let Some(n) = inner.child_by_field_name("name") {
                                let fn_name = txt(n, src).to_string();
                                if !fn_name.is_empty() {
                                    out.push(RawDef {
                                        name: if type_name.is_empty() {
                                            fn_name
                                        } else {
                                            format!("{}::{}", type_name, fn_name)
                                        },
                                        kind: NodeKind::Method,
                                        line_start: inner.start_position().row as u32 + 1,
                                        line_end: inner.end_position().row as u32 + 1,
                                        detail: if type_name.is_empty() { None } else { Some(type_name.clone()) },
                                    });
                                }
                            }
                        }
                    }
                }
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk_rust(&child, src, out);
    }
}

fn extract_rust_mod_decls(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if (l.starts_with("mod ") || l.starts_with("pub mod ")) && l.ends_with(';') {
            let module = l
                .trim_start_matches("pub ")
                .trim_start_matches("mod ")
                .trim_end_matches(';')
                .trim();
            if !module.is_empty() {
                out.push(module.to_string());
            }
        }
    }
    out
}

fn extract_rust_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_rust::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    collect_fn_calls_rust(&tree.root_node(), src, known, &mut out, None);
    out
}

fn collect_fn_calls_rust(
    node: &tree_sitter::Node,
    src: &[u8],
    known: &HashSet<String>,
    out: &mut Vec<(String, Vec<String>)>,
    impl_type: Option<&str>,
) {
    match node.kind() {
        "impl_item" => {
            let type_name = node
                .child_by_field_name("type")
                .map(|n| txt(n, src).to_string())
                .unwrap_or_default();
            let mut c = node.walk();
            for child in node.children(&mut c) {
                collect_fn_calls_rust(&child, src, known, out, Some(&type_name));
            }
            return;
        }
        "function_item" => {
            let fn_name = node
                .child_by_field_name("name")
                .map(|n| txt(n, src).to_string())
                .unwrap_or_default();
            if !fn_name.is_empty() {
                let qualified = match impl_type {
                    Some(t) if !t.is_empty() => format!("{t}::{fn_name}"),
                    _ => fn_name,
                };
                if let Some(body) = node.child_by_field_name("body") {
                    let mut calls = Vec::new();
                    find_rust_calls(&body, src, known, &mut calls);
                    calls.sort_unstable();
                    calls.dedup();
                    if !calls.is_empty() {
                        out.push((qualified, calls));
                    }
                }
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_fn_calls_rust(&child, src, known, out, impl_type);
    }
}

fn find_rust_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = match func.kind() {
                    "identifier" => txt(func, src).to_string(),
                    "scoped_identifier" => {
                        // last segment of foo::bar::baz
                        let mut wk = func.walk();
                        func.children(&mut wk)
                            .last()
                            .map(|n| txt(n, src).to_string())
                            .unwrap_or_default()
                    }
                    "field_expression" => func
                        .child_by_field_name("field")
                        .map(|n| txt(n, src).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() && known.contains(&name) {
                    out.push(name);
                }
            }
        }
        "method_call_expression" => {
            if let Some(method) = node.child_by_field_name("method") {
                let name = txt(method, src).to_string();
                if !name.is_empty() && known.contains(&name) {
                    out.push(name);
                }
            }
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        find_rust_calls(&child, src, known, out);
    }
}

// ── Go ────────────────────────────────────────────────────────────────────────

fn extract_go_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_go::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();

    let mut c = tree.root_node().walk();
    for child in tree.root_node().children(&mut c) {
        match child.kind() {
            "function_declaration" => {
                if let Some(n) = child.child_by_field_name("name") {
                    let name = txt(n, src).to_string();
                    if !name.is_empty() {
                        out.push(RawDef {
                            name,
                            kind: NodeKind::Function,
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            detail: None,
                        });
                    }
                }
            }
            "method_declaration" => {
                if let Some(n) = child.child_by_field_name("name") {
                    let name = txt(n, src).to_string();
                    if !name.is_empty() {
                        out.push(RawDef {
                            name,
                            kind: NodeKind::Method,
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            detail: None,
                        });
                    }
                }
            }
            "type_declaration" => {
                let mut c2 = child.walk();
                for spec in child.children(&mut c2) {
                    if spec.kind() == "type_spec" {
                        if let Some(n) = spec.child_by_field_name("name") {
                            let name = txt(n, src).to_string();
                            if !name.is_empty() {
                                out.push(RawDef {
                                    name,
                                    kind: NodeKind::Struct,
                                    line_start: child.start_position().row as u32 + 1,
                                    line_end: child.end_position().row as u32 + 1,
                                    detail: None,
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ── Version bridge ───────────────────────────────────────────────────────────
// tree-sitter-cpp 0.22 depends on tree-sitter 0.22 while the project uses 0.20.
// Both Language types are newtype wrappers over `*const TSLanguage` (stable C ABI),
// so transmuting between them is safe: same pointer, same underlying grammar struct.

fn cpp_language() -> tree_sitter::Language {
    #[allow(unsafe_code)]
    unsafe { std::mem::transmute(tree_sitter_cpp::language()) }
}

// ── Java ──────────────────────────────────────────────────────────────────────

fn extract_java_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_java::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    walk_java(&tree.root_node(), src, &mut out, false);
    out
}

fn walk_java(node: &tree_sitter::Node, src: &[u8], out: &mut Vec<RawDef>, in_class: bool) {
    match node.kind() {
        "class_declaration" | "record_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: txt(n, src).to_string(),
                    kind: NodeKind::Class,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            let mut c = node.walk();
            for child in node.children(&mut c) {
                walk_java(&child, src, out, true);
            }
            return;
        }
        "interface_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: txt(n, src).to_string(),
                    kind: NodeKind::Trait,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            let mut c = node.walk();
            for child in node.children(&mut c) {
                walk_java(&child, src, out, true);
            }
            return;
        }
        "enum_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: txt(n, src).to_string(),
                    kind: NodeKind::Struct,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
        }
        "method_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    // Detect Spring/JAX-RS HTTP annotations from annotations before method
                    let http = detect_java_http(node, src);
                    out.push(RawDef {
                        name,
                        kind: if http.is_some() { NodeKind::Endpoint } else if in_class { NodeKind::Method } else { NodeKind::Function },
                        line_start: node.start_position().row as u32 + 1,
                        line_end:   node.end_position().row as u32 + 1,
                        detail: http,
                    });
                }
            }
            return;
        }
        "constructor_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = txt(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Function,
                        line_start: node.start_position().row as u32 + 1,
                        line_end:   node.end_position().row as u32 + 1,
                        detail: None,
                    });
                }
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk_java(&child, src, out, in_class);
    }
}

fn detect_java_http(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    let http_annotations = ["GetMapping", "PostMapping", "PutMapping", "DeleteMapping",
        "PatchMapping", "RequestMapping", "GET", "POST", "PUT", "DELETE"];
    let parent = node.parent()?;
    let mut c = parent.walk();
    for sibling in parent.children(&mut c) {
        if sibling.kind() == "modifiers" || sibling.kind() == "annotation" {
            let s = txt(sibling, src);
            for ann in &http_annotations {
                if s.contains(ann) {
                    return Some(ann.to_string());
                }
            }
        }
    }
    None
}

fn extract_java_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_java::language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    collect_fn_calls_java(&tree.root_node(), src, known, &mut out);
    out
}

fn collect_fn_calls_java(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<(String, Vec<String>)>) {
    if matches!(node.kind(), "method_declaration" | "constructor_declaration") {
        let fn_name = node
            .child_by_field_name("name")
            .map(|n| txt(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            let mut calls = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                find_java_calls(&body, src, known, &mut calls);
            }
            calls.sort_unstable();
            calls.dedup();
            if !calls.is_empty() {
                out.push((fn_name, calls));
            }
        }
        return;
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_fn_calls_java(&child, src, known, out);
    }
}

fn find_java_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    if node.kind() == "method_invocation" {
        if let Some(name_node) = node.child_by_field_name("name") {
            let name = txt(name_node, src).to_string();
            if !name.is_empty() && known.contains(&name) {
                out.push(name);
            }
        }
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        find_java_calls(&child, src, known, out);
    }
}

// ── C++ ───────────────────────────────────────────────────────────────────────

fn extract_cpp_defs(content: &str) -> Vec<RawDef> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(cpp_language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    walk_cpp(&tree.root_node(), src, &mut out, false);
    out
}

fn walk_cpp(node: &tree_sitter::Node, src: &[u8], out: &mut Vec<RawDef>, in_class: bool) {
    match node.kind() {
        "class_specifier" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: txt(n, src).to_string(),
                    kind: NodeKind::Class,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            let mut c = node.walk();
            for child in node.children(&mut c) {
                walk_cpp(&child, src, out, true);
            }
            return;
        }
        "struct_specifier" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: txt(n, src).to_string(),
                    kind: NodeKind::Struct,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            let mut c = node.walk();
            for child in node.children(&mut c) {
                walk_cpp(&child, src, out, true);
            }
            return;
        }
        "function_definition" => {
            // The declarator chain: function_declarator → identifier or qualified_identifier
            let name = cpp_fn_name(node, src);
            if !name.is_empty() {
                out.push(RawDef {
                    name,
                    kind: if in_class { NodeKind::Method } else { NodeKind::Function },
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk_cpp(&child, src, out, in_class);
    }
}

fn cpp_fn_name(node: &tree_sitter::Node, src: &[u8]) -> String {
    // Walk declarator chain to find the function name
    if let Some(decl) = node.child_by_field_name("declarator") {
        return cpp_extract_declarator_name(decl, src);
    }
    String::new()
}

fn cpp_extract_declarator_name(node: tree_sitter::Node, src: &[u8]) -> String {
    match node.kind() {
        "function_declarator" => {
            if let Some(inner) = node.child_by_field_name("declarator") {
                return cpp_extract_declarator_name(inner, src);
            }
        }
        "identifier" | "field_identifier" => return txt(node, src).to_string(),
        "qualified_identifier" => {
            // last segment of ns::Class::method
            let mut c = node.walk();
            if let Some(last) = node.children(&mut c).last() {
                return txt(last, src).to_string();
            }
        }
        "destructor_name" => {
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.kind() == "identifier" {
                    return format!("~{}", txt(child, src));
                }
            }
        }
        "pointer_declarator" | "reference_declarator" => {
            if let Some(inner) = node.child_by_field_name("declarator") {
                return cpp_extract_declarator_name(inner, src);
            }
        }
        _ => {}
    }
    String::new()
}

fn extract_cpp_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(cpp_language()).is_err() {
        return vec![];
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    collect_fn_calls_cpp(&tree.root_node(), src, known, &mut out);
    out
}

fn collect_fn_calls_cpp(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<(String, Vec<String>)>) {
    if node.kind() == "function_definition" {
        let fn_name = cpp_fn_name(node, src);
        if !fn_name.is_empty() {
            let mut calls = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                find_cpp_calls(&body, src, known, &mut calls);
            }
            calls.sort_unstable();
            calls.dedup();
            if !calls.is_empty() {
                out.push((fn_name, calls));
            }
        }
        return;
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_fn_calls_cpp(&child, src, known, out);
    }
}

fn find_cpp_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            let name = match func.kind() {
                "identifier" => txt(func, src).to_string(),
                "field_expression" => func
                    .child_by_field_name("field")
                    .map(|n| txt(n, src).to_string())
                    .unwrap_or_default(),
                "qualified_identifier" => {
                    let mut c = func.walk();
                    func.children(&mut c)
                        .last()
                        .map(|n| txt(n, src).to_string())
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            if !name.is_empty() && known.contains(&name) {
                out.push(name);
            }
        }
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        find_cpp_calls(&child, src, known, out);
    }
}

// ── Shared call-extraction ────────────────────────────────────────────────────

fn find_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    // "call" = Python,  "call_expression" = TypeScript/JavaScript
    if matches!(node.kind(), "call" | "call_expression") {
        if let Some(func) = node.child_by_field_name("function") {
            let name = match func.kind() {
                "identifier" => txt(func, src).to_string(),
                // obj.method or obj.attribute — take the last child (the property name)
                "attribute" | "member_expression" => {
                    let mut c = func.walk();
                    func.children(&mut c)
                        .last()
                        .map(|n| txt(n, src).to_string())
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            if !name.is_empty() && known.contains(&name) {
                out.push(name);
            }
        }
    }
    let mut c = node.walk();
    for child in node.children(&mut c) {
        find_calls(&child, src, known, out);
    }
}

// ── Import resolution ─────────────────────────────────────────────────────────

fn resolve_import(
    module: &str,
    current_dir: &Path,
    lang: Language,
    file_module: &HashMap<String, u32>,
) -> Option<u32> {
    // Strategy 1: For TypeScript/JS relative imports (start with . or /), resolve directly.
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

    // Strategy 2: Stem matching — find any file whose stem matches the module name.
    let stem = module.split('.').last().unwrap_or(module).to_lowercase();
    if stem.is_empty() { return None; }

    for (path, &id) in file_module {
        let file_stem = Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if file_stem == stem {
            return Some(id);
        }
    }

    None
}

// ── Analytics ─────────────────────────────────────────────────────────────────

/// Label-propagation community detection on Calls + Imports edges (undirected).
/// Returns map from node_id → community_id (renumbered 0..N).
pub fn compute_communities(graph: &CodeGraph) -> HashMap<u32, u32> {
    let n = graph.nodes.len();
    if n == 0 { return HashMap::new(); }

    // Build undirected adjacency list using Calls + Imports edges only
    let mut neighbors: HashMap<u32, Vec<u32>> = graph.nodes.iter().map(|n| (n.id, vec![])).collect();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Calls | EdgeKind::Imports) {
            neighbors.entry(edge.from).or_default().push(edge.to);
            neighbors.entry(edge.to).or_default().push(edge.from);
        }
    }

    // Initialize: every node = its own community
    let mut community: HashMap<u32, u32> = graph.nodes.iter().map(|n| (n.id, n.id)).collect();

    for _ in 0..50 {
        let mut changed = false;
        let node_ids: Vec<u32> = graph.nodes.iter().map(|n| n.id).collect();
        for &node_id in &node_ids {
            let Some(nbrs) = neighbors.get(&node_id) else { continue };
            if nbrs.is_empty() { continue; }

            let mut freq: HashMap<u32, u32> = HashMap::new();
            for &nbr in nbrs {
                if let Some(&c) = community.get(&nbr) {
                    *freq.entry(c).or_default() += 1;
                }
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

    // Renumber community IDs to 0..N
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
    // Count total cross-community links per (ca, cb) pair
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

    // Collect edges whose community pair has exactly 1 cross-edge (most surprising)
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

#[inline]
fn txt<'a>(node: tree_sitter::Node<'_>, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.start_byte()..node.end_byte()]).unwrap_or("")
}

fn detect_http_method(decorator: &str) -> Option<String> {
    let d = decorator.to_lowercase();
    for m in &["get", "post", "put", "delete", "patch", "head", "options"] {
        if d.contains(&format!(".{}(", m)) {
            return Some(m.to_uppercase());
        }
    }
    if d.contains(".route(") {
        return Some("ROUTE".to_string());
    }
    None
}
