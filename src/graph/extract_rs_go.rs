//! Rust and Go AST extractors.

use super::{NodeKind, RawDef};
use std::collections::HashSet;

// ── Rust ──────────────────────────────────────────────────────────────────────

pub(super) fn extract_rust_defs(content: &str) -> Vec<RawDef> {
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
                let name = node_text!(n, src).to_string();
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
                let name = node_text!(n, src).to_string();
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
                let name = node_text!(n, src).to_string();
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
                let name = node_text!(n, src).to_string();
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
            let type_name = node.child_by_field_name("type")
                .map(|n| node_text!(n, src).to_string())
                .unwrap_or_default();
            for ci in 0..node.child_count() {
                if let Some(child) = node.child(ci) {
                    if child.kind() == "declaration_list" {
                        for ji in 0..child.child_count() {
                            if let Some(inner) = child.child(ji) {
                                if inner.kind() == "function_item" {
                                    if let Some(n) = inner.child_by_field_name("name") {
                                        let fn_name = node_text!(n, src).to_string();
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
                }
            }
            return;
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_rust(&child, src, out);
        }
    }
}

pub(super) fn extract_rust_mod_decls(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if (l.starts_with("mod ") || l.starts_with("pub mod ")) && l.ends_with(';') {
            let module = l.trim_start_matches("pub ")
                .trim_start_matches("mod ")
                .trim_end_matches(';')
                .trim();
            if !module.is_empty() { out.push(module.to_string()); }
        }
    }
    out
}

pub(super) fn extract_rust_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
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
            let type_name = node.child_by_field_name("type")
                .map(|n| node_text!(n, src).to_string())
                .unwrap_or_default();
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    collect_fn_calls_rust(&child, src, known, out, Some(&type_name));
                }
            }
            return;
        }
        "function_item" => {
            let fn_name = node.child_by_field_name("name")
                .map(|n| node_text!(n, src).to_string())
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
                    if !calls.is_empty() { out.push((qualified, calls)); }
                }
            }
            return;
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_fn_calls_rust(&child, src, known, out, impl_type);
        }
    }
}

fn find_rust_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = match func.kind() {
                    "identifier" => node_text!(func, src).to_string(),
                    "scoped_identifier" => {
                        (0..func.child_count()).rev()
                            .find_map(|i| func.child(i))
                            .map(|n| node_text!(n, src).to_string())
                            .unwrap_or_default()
                    }
                    "field_expression" => func.child_by_field_name("field")
                        .map(|n| node_text!(n, src).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() && known.contains(&name) { out.push(name); }
            }
        }
        "method_call_expression" => {
            if let Some(method) = node.child_by_field_name("method") {
                let name = node_text!(method, src).to_string();
                if !name.is_empty() && known.contains(&name) { out.push(name); }
            }
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            find_rust_calls(&child, src, known, out);
        }
    }
}

// ── Go ────────────────────────────────────────────────────────────────────────

pub(super) fn extract_go_defs(content: &str) -> Vec<RawDef> {
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

    let root = tree.root_node();
    for ci in 0..root.child_count() {
        let Some(child) = root.child(ci) else { continue };
        match child.kind() {
            "function_declaration" => {
                if let Some(n) = child.child_by_field_name("name") {
                    let name = node_text!(n, src).to_string();
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
                    let name = node_text!(n, src).to_string();
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
                for si in 0..child.child_count() {
                    if let Some(spec) = child.child(si) {
                        if spec.kind() == "type_spec" {
                            if let Some(n) = spec.child_by_field_name("name") {
                                let name = node_text!(n, src).to_string();
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
            }
            _ => {}
        }
    }
    out
}
