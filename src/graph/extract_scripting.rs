//! Python and TypeScript/JavaScript AST extractors.

use super::{detect_http_method, find_calls, NodeKind, RawDef};
use std::collections::HashSet;

// ── Python ────────────────────────────────────────────────────────────────────

pub(super) fn extract_python_defs(content: &str) -> Vec<RawDef> {
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
                let name = node_text!(n, src).to_string();
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
            return;
        }
        "class_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = node_text!(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Class,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                    for i in 0..node.child_count() {
                        if let Some(child) = node.child(i) {
                            walk_py(&child, src, out, true);
                        }
                    }
                    return;
                }
            }
        }
        "decorated_definition" => {
            let mut http: Option<String> = None;
            {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        if child.kind() == "decorator" {
                            http = detect_http_method(node_text!(child, src));
                        }
                    }
                }
            }
            {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        if child.kind() == "function_definition" {
                            if let Some(n) = child.child_by_field_name("name") {
                                let name = node_text!(n, src).to_string();
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
            }
            return;
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_py(&child, src, out, in_class);
        }
    }
}

pub(super) fn extract_python_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with("import ") {
            let rest = &l[7..];
            let module = rest.split_whitespace().next().unwrap_or("").split('.').next().unwrap_or("");
            if !module.is_empty() { out.push(module.to_string()); }
        } else if l.starts_with("from ") {
            if let Some(module_part) = l[5..].split(" import ").next() {
                let m = module_part.trim().trim_start_matches('.').split('.').next().unwrap_or("").trim();
                if !m.is_empty() { out.push(m.to_string()); }
            }
        }
    }
    out
}

pub(super) fn extract_python_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
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
        let fn_name = node.child_by_field_name("name")
            .map(|n| node_text!(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            if let Some(body) = node.child_by_field_name("body") {
                let mut calls = Vec::new();
                find_calls(&body, src, known, &mut calls);
                calls.sort_unstable();
                calls.dedup();
                if !calls.is_empty() { out.push((fn_name, calls)); }
            }
        }
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_fn_calls_py(&child, src, known, out);
        }
    }
}

// ── TypeScript / JavaScript ───────────────────────────────────────────────────

pub(super) fn extract_ts_defs(content: &str) -> Vec<RawDef> {
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
                let name = node_text!(n, src).to_string();
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
                let name = node_text!(n, src).to_string();
                if !name.is_empty() {
                    out.push(RawDef {
                        name,
                        kind: NodeKind::Class,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        detail: None,
                    });
                    for i in 0..node.child_count() {
                        if let Some(child) = node.child(i) {
                            walk_ts(&child, src, out, true);
                        }
                    }
                    return;
                }
            }
        }
        "method_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = node_text!(n, src).to_string();
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
            if let Some(func) = node.child_by_field_name("function") {
                if func.kind() == "member_expression" {
                    let prop = func.child_by_field_name("property")
                        .map(|n| node_text!(n, src))
                        .unwrap_or_default();
                    if matches!(prop, "get" | "post" | "put" | "delete" | "patch") {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let first = (0..args.child_count()).find_map(|i| {
                                args.child(i).filter(|n| n.is_named() && matches!(n.kind(), "string" | "template_string"))
                            });
                            if let Some(path_node) = first {
                                let route = node_text!(path_node, src)
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
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_ts(&child, src, out, in_class);
                }
            }
            return;
        }
        _ => {}
    }
    if !matches!(node.kind(), "function_declaration" | "arrow_function" | "function") {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                walk_ts(&child, src, out, in_class);
            }
        }
    }
}

pub(super) fn extract_ts_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with("import ") && l.contains(" from ") {
            if let Some(from_part) = l.split(" from ").last() {
                let m = from_part.trim().trim_end_matches(';')
                    .trim_matches(|c| matches!(c, '\'' | '"' | '`'));
                if m.starts_with('.') || m.starts_with('/') { out.push(m.to_string()); }
            }
        } else if l.contains("require(") {
            if let Some(start) = l.find("require(") {
                let rest = &l[start + 8..];
                if let Some(end) = rest.find(')') {
                    let m = rest[..end].trim().trim_matches(|c| matches!(c, '\'' | '"'));
                    if m.starts_with('.') { out.push(m.to_string()); }
                }
            }
        }
    }
    out
}

pub(super) fn extract_ts_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
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
        let fn_name = node.child_by_field_name("name")
            .map(|n| node_text!(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            if let Some(body) = node.child_by_field_name("body") {
                let mut calls = Vec::new();
                find_calls(&body, src, known, &mut calls);
                calls.sort_unstable();
                calls.dedup();
                if !calls.is_empty() { out.push((fn_name, calls)); }
            }
        }
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_fn_calls_ts(&child, src, known, out);
        }
    }
}
