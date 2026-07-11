//! Java and C++ AST extractors.

use super::{NodeKind, RawDef};
use std::collections::HashSet;

// tree-sitter-cpp 0.22 depends on tree-sitter 0.22 while the project uses 0.20.
// Both Language types are newtype wrappers over `*const TSLanguage` (stable C ABI),
// so transmuting between them is safe: same pointer, same underlying grammar struct.
fn cpp_language() -> tree_sitter::Language {
    #[allow(unsafe_code)]
    unsafe { std::mem::transmute(tree_sitter_cpp::language()) }
}

// ── Java ──────────────────────────────────────────────────────────────────────

pub(super) fn extract_java_defs(content: &str) -> Vec<RawDef> {
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
                    name: node_text!(n, src).to_string(),
                    kind: NodeKind::Class,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) { walk_java(&child, src, out, true); }
            }
            return;
        }
        "interface_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: node_text!(n, src).to_string(),
                    kind: NodeKind::Trait,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) { walk_java(&child, src, out, true); }
            }
            return;
        }
        "enum_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: node_text!(n, src).to_string(),
                    kind: NodeKind::Struct,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
        }
        "method_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = node_text!(n, src).to_string();
                if !name.is_empty() {
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
                let name = node_text!(n, src).to_string();
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
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { walk_java(&child, src, out, in_class); }
    }
}

fn detect_java_http(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    let http_annotations = ["GetMapping", "PostMapping", "PutMapping", "DeleteMapping",
        "PatchMapping", "RequestMapping", "GET", "POST", "PUT", "DELETE"];
    let parent = node.parent()?;
    for i in 0..parent.child_count() {
        if let Some(sibling) = parent.child(i) {
            if sibling.kind() == "modifiers" || sibling.kind() == "annotation" {
                let s = node_text!(sibling, src);
                for ann in &http_annotations {
                    if s.contains(ann) { return Some(ann.to_string()); }
                }
            }
        }
    }
    None
}

pub(super) fn extract_java_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
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
        let fn_name = node.child_by_field_name("name")
            .map(|n| node_text!(n, src).to_string())
            .unwrap_or_default();
        if !fn_name.is_empty() {
            let mut calls = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                find_java_calls(&body, src, known, &mut calls);
            }
            calls.sort_unstable();
            calls.dedup();
            if !calls.is_empty() { out.push((fn_name, calls)); }
        }
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { collect_fn_calls_java(&child, src, known, out); }
    }
}

fn find_java_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    if node.kind() == "method_invocation" {
        if let Some(name_node) = node.child_by_field_name("name") {
            let name = node_text!(name_node, src).to_string();
            if !name.is_empty() && known.contains(&name) { out.push(name); }
        }
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { find_java_calls(&child, src, known, out); }
    }
}

// ── C++ ───────────────────────────────────────────────────────────────────────

pub(super) fn extract_cpp_defs(content: &str) -> Vec<RawDef> {
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
                    name: node_text!(n, src).to_string(),
                    kind: NodeKind::Class,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) { walk_cpp(&child, src, out, true); }
            }
            return;
        }
        "struct_specifier" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.push(RawDef {
                    name: node_text!(n, src).to_string(),
                    kind: NodeKind::Struct,
                    line_start: node.start_position().row as u32 + 1,
                    line_end:   node.end_position().row as u32 + 1,
                    detail: None,
                });
            }
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) { walk_cpp(&child, src, out, true); }
            }
            return;
        }
        "function_definition" => {
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
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { walk_cpp(&child, src, out, in_class); }
    }
}

fn cpp_fn_name(node: &tree_sitter::Node, src: &[u8]) -> String {
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
        "identifier" | "field_identifier" => return node_text!(node, src).to_string(),
        "qualified_identifier" => {
            if let Some(last) = (0..node.child_count()).find_map(|i| node.child(node.child_count() - 1 - i)) {
                return node_text!(last, src).to_string();
            }
        }
        "destructor_name" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.kind() == "identifier" {
                        return format!("~{}", node_text!(child, src));
                    }
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

pub(super) fn extract_cpp_calls(content: &str, known: &HashSet<String>) -> Vec<(String, Vec<String>)> {
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
            if !calls.is_empty() { out.push((fn_name, calls)); }
        }
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) { collect_fn_calls_cpp(&child, src, known, out); }
    }
}

fn find_cpp_calls(node: &tree_sitter::Node, src: &[u8], known: &HashSet<String>, out: &mut Vec<String>) {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            let name = match func.kind() {
                "identifier" => node_text!(func, src).to_string(),
                "field_expression" => func.child_by_field_name("field")
                    .map(|n| node_text!(n, src).to_string())
                    .unwrap_or_default(),
                "qualified_identifier" => {
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
        if let Some(child) = node.child(i) { find_cpp_calls(&child, src, known, out); }
    }
}
