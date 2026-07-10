//! Static analysis passes on the CodeGraph and chunk store.
//!
//! Each function is pure / read-only — no disk writes.
//! Used by `needle serve` API handlers.

use crate::graph::{CodeGraph, EdgeKind, NodeKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

// ─── Blast Radius ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct BlastResult {
    pub source_file: String,
    pub affected: Vec<AffectedFile>,
    pub total_files: usize,
    pub risk_score: u32,
}

#[derive(Serialize)]
pub struct AffectedFile {
    pub path: String,
    pub depth: u32,
}

/// BFS on the reverse dependency graph.
/// "If I change `file_path`, which other files could break?"
pub fn blast_radius(graph: &CodeGraph, file_path: &str) -> BlastResult {
    let source_ids: HashSet<u32> = graph.nodes.iter()
        .filter(|n| n.file_path == file_path)
        .map(|n| n.id)
        .collect();

    if source_ids.is_empty() {
        return BlastResult { source_file: file_path.to_string(), affected: vec![], total_files: 0, risk_score: 0 };
    }

    // Build reverse adjacency: who depends on this node?
    let mut reverse: HashMap<u32, Vec<u32>> = HashMap::new();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Imports | EdgeKind::Calls) {
            reverse.entry(edge.to).or_default().push(edge.from);
        }
    }

    let node_file: HashMap<u32, &str> = graph.nodes.iter().map(|n| (n.id, n.file_path.as_str())).collect();

    let mut visited: HashSet<u32> = source_ids.clone();
    let mut queue: VecDeque<(u32, u32)> = source_ids.iter().map(|&id| (id, 0)).collect();
    let mut affected_files: HashMap<String, u32> = HashMap::new();

    while let Some((node_id, depth)) = queue.pop_front() {
        if let Some(deps) = reverse.get(&node_id) {
            for &dep_id in deps {
                if visited.insert(dep_id) {
                    if let Some(&fp) = node_file.get(&dep_id) {
                        if fp != file_path {
                            let e = affected_files.entry(fp.to_string()).or_insert(depth + 1);
                            *e = (*e).min(depth + 1);
                        }
                    }
                    queue.push_back((dep_id, depth + 1));
                }
            }
        }
    }

    let total_files = affected_files.len();
    let max_depth = affected_files.values().copied().max().unwrap_or(0);
    let risk_score = ((total_files as f32 * 8.0).min(70.0) as u32 + max_depth.min(30)).min(100);

    let mut affected: Vec<AffectedFile> = affected_files.into_iter()
        .map(|(path, depth)| AffectedFile { path, depth })
        .collect();
    affected.sort_by_key(|a| a.depth);

    BlastResult { source_file: file_path.to_string(), affected, total_files, risk_score }
}

// ─── Health Score ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct HealthReport {
    pub grade: String,
    pub score: u32,
    pub circular_deps: Vec<Vec<String>>,
    pub god_objects: Vec<GodObject>,
    pub orphaned_functions: Vec<String>,
    pub long_files: Vec<LongFile>,
    pub avg_coupling: f32,
    pub details: HealthDetails,
}

#[derive(Serialize)]
pub struct GodObject {
    pub name: String,
    pub file: String,
    pub caller_count: u32,
    pub callee_count: u32,
}

#[derive(Serialize)]
pub struct LongFile {
    pub path: String,
    pub lines: u32,
}

#[derive(Serialize)]
pub struct HealthDetails {
    pub circular_dep_penalty: u32,
    pub god_object_penalty: u32,
    pub orphan_penalty: u32,
    pub long_file_penalty: u32,
    pub coupling_penalty: u32,
}

pub fn health_score(graph: &CodeGraph) -> HealthReport {
    let circular_deps = find_circular_deps(graph);

    let mut call_in: HashMap<u32, u32> = HashMap::new();
    let mut call_out: HashMap<u32, u32> = HashMap::new();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Calls) {
            *call_out.entry(edge.from).or_default() += 1;
            *call_in.entry(edge.to).or_default() += 1;
        }
    }

    let god_objects: Vec<GodObject> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .filter_map(|n| {
            let inc = call_in.get(&n.id).copied().unwrap_or(0);
            let out = call_out.get(&n.id).copied().unwrap_or(0);
            if inc + out >= 10 {
                Some(GodObject { name: n.name.clone(), file: n.file_path.clone(), caller_count: inc, callee_count: out })
            } else { None }
        })
        .collect();

    let has_incoming: HashSet<u32> = graph.edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls))
        .map(|e| e.to)
        .collect();

    let orphaned: Vec<String> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .filter(|n| !has_incoming.contains(&n.id))
        .filter(|n| !n.name.starts_with('_') && n.name != "main" && n.name != "new")
        .take(20)
        .map(|n| format!("{}:{}", strip_unc(&n.file_path), n.name))
        .collect();

    let long_files: Vec<LongFile> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Module) && n.line_end.saturating_sub(n.line_start) > 400)
        .map(|n| LongFile { path: strip_unc(&n.file_path), lines: n.line_end.saturating_sub(n.line_start) })
        .collect();

    let import_counts: HashMap<u32, u32> = graph.edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Imports))
        .fold(HashMap::new(), |mut m, e| { *m.entry(e.from).or_default() += 1; m });

    let module_count = graph.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Module)).count() as f32;
    let total_imports: u32 = import_counts.values().sum();
    let avg_coupling = if module_count > 0.0 { total_imports / module_count as u32 } else { 0 } as f32;

    let circular_penalty  = (circular_deps.len() as u32 * 15).min(40);
    let god_penalty       = (god_objects.len() as u32 * 5).min(25);
    let orphan_penalty    = (orphaned.len() as u32 / 5).min(10);
    let long_penalty      = (long_files.len() as u32 * 3).min(15);
    let coupling_penalty  = if avg_coupling > 10.0 { 15 } else if avg_coupling > 5.0 { 7 } else { 0 };

    let score = 100u32.saturating_sub(circular_penalty + god_penalty + orphan_penalty + long_penalty + coupling_penalty);
    let grade = match score { 90..=100 => "A", 80..=89 => "B", 70..=79 => "C", 60..=69 => "D", _ => "F" }.to_string();

    HealthReport {
        grade, score, circular_deps, god_objects, orphaned_functions: orphaned,
        long_files, avg_coupling,
        details: HealthDetails { circular_dep_penalty: circular_penalty, god_object_penalty: god_penalty, orphan_penalty, long_file_penalty: long_penalty, coupling_penalty },
    }
}

fn find_circular_deps(graph: &CodeGraph) -> Vec<Vec<String>> {
    let modules: HashMap<u32, &str> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Module))
        .map(|n| (n.id, n.file_path.as_str()))
        .collect();

    let mut adj: HashMap<u32, Vec<u32>> = modules.keys().map(|&k| (k, vec![])).collect();
    for edge in &graph.edges {
        if matches!(edge.kind, EdgeKind::Imports) {
            if modules.contains_key(&edge.from) && modules.contains_key(&edge.to) {
                adj.entry(edge.from).or_default().push(edge.to);
            }
        }
    }

    let mut color: HashMap<u32, u8> = HashMap::new();
    let mut cycles: Vec<Vec<String>> = Vec::new();
    let mut path: Vec<u32> = Vec::new();

    for &start in modules.keys() {
        if !color.contains_key(&start) {
            dfs_cycles(start, &adj, &mut color, &mut path, &modules, &mut cycles);
        }
    }
    cycles.truncate(10);
    cycles
}

fn dfs_cycles(
    node: u32, adj: &HashMap<u32, Vec<u32>>, color: &mut HashMap<u32, u8>,
    path: &mut Vec<u32>, modules: &HashMap<u32, &str>, cycles: &mut Vec<Vec<String>>,
) {
    color.insert(node, 1);
    path.push(node);
    if let Some(neighbors) = adj.get(&node) {
        for &next in neighbors {
            match color.get(&next).copied().unwrap_or(0) {
                1 => {
                    if let Some(pos) = path.iter().position(|&n| n == next) {
                        let cycle: Vec<String> = path[pos..].iter()
                            .map(|&id| modules.get(&id).map(|s| strip_unc(s)).unwrap_or_default())
                            .collect();
                        if cycles.len() < 10 { cycles.push(cycle); }
                    }
                }
                0 => dfs_cycles(next, adj, color, path, modules, cycles),
                _ => {}
            }
        }
    }
    path.pop();
    color.insert(node, 2);
}

// ─── Security Scanning ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SecurityIssue {
    pub severity: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub snippet: String,
}

pub fn scan_security(chunks: &std::collections::HashMap<u64, crate::schema::Chunk>) -> Vec<SecurityIssue> {
    const HIGH: &[(&str, &str)] = &[
        ("password = \"",        "hardcoded_secret"),
        ("password = '",         "hardcoded_secret"),
        ("secret_key = \"",      "hardcoded_secret"),
        ("secret = \"",          "hardcoded_secret"),
        ("api_key = \"",         "hardcoded_secret"),
        ("api_key = '",          "hardcoded_secret"),
        ("aws_secret",           "hardcoded_secret"),
        ("BEGIN RSA PRIVATE KEY","exposed_private_key"),
        ("BEGIN PRIVATE KEY",    "exposed_private_key"),
    ];
    const MEDIUM: &[(&str, &str)] = &[
        ("eval(",                  "unsafe_eval"),
        (".innerHTML =",           "xss_risk"),
        ("innerHTML+=",            "xss_risk"),
        ("dangerouslySetInnerHTML","xss_risk"),
        ("document.write(",        "xss_risk"),
        ("os.system(",             "shell_injection"),
        ("subprocess.call(",       "shell_injection"),
        ("subprocess.Popen(",      "shell_injection"),
        ("shell=True",             "shell_injection"),
        ("cursor.execute(f\"",     "sql_injection"),
        ("cursor.execute(f'",      "sql_injection"),
        ("execute(\"SELECT",       "sql_injection"),
    ];
    const LOW: &[(&str, &str)] = &[
        ("TODO: security",  "todo_security"),
        ("FIXME: security", "todo_security"),
        ("nosec",           "security_suppression"),
        ("// eslint-disable","lint_suppression"),
        ("#nosec",          "security_suppression"),
    ];

    let mut issues: Vec<SecurityIssue> = Vec::new();

    for chunk in chunks.values() {
        for (i, line) in chunk.content.lines().enumerate() {
            let line_no = chunk.line_start + i as u32;
            let lower = line.to_lowercase();
            let snippet: String = line.trim().chars().take(150).collect();

            for (pat, kind) in HIGH {
                if lower.contains(&pat.to_lowercase()) {
                    issues.push(SecurityIssue { severity: "high".into(), kind: kind.to_string(), file: strip_unc(&chunk.file_path), line: line_no, snippet: snippet.clone() });
                }
            }
            for (pat, kind) in MEDIUM {
                if line.contains(pat) {
                    issues.push(SecurityIssue { severity: "medium".into(), kind: kind.to_string(), file: strip_unc(&chunk.file_path), line: line_no, snippet: snippet.clone() });
                }
            }
            for (pat, kind) in LOW {
                if line.contains(pat) {
                    issues.push(SecurityIssue { severity: "low".into(), kind: kind.to_string(), file: strip_unc(&chunk.file_path), line: line_no, snippet: snippet.clone() });
                }
            }
        }
    }

    issues.sort_by_key(|i| match i.severity.as_str() { "high" => 0u8, "medium" => 1, _ => 2 });
    issues.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.kind == b.kind);
    issues.truncate(300);
    issues
}

// ─── Pattern Detection ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PatternReport {
    pub god_objects: Vec<GodObjectRef>,
    pub long_files: Vec<LongFile>,
    pub high_coupling: Vec<CouplingEntry>,
    pub layer_violations: Vec<LayerViolation>,
    pub singleton_suspects: Vec<String>,
}

#[derive(Serialize)]
pub struct GodObjectRef {
    pub name: String,
    pub file: String,
    pub out_calls: u32,
}

#[derive(Serialize)]
pub struct CouplingEntry {
    pub file: String,
    pub import_count: u32,
}

#[derive(Serialize)]
pub struct LayerViolation {
    pub from_file: String,
    pub to_file: String,
    pub from_layer: String,
    pub to_layer: String,
}

pub fn detect_patterns(graph: &CodeGraph) -> PatternReport {
    let mut out_degree: HashMap<u32, u32> = HashMap::new();
    for e in &graph.edges {
        if matches!(e.kind, EdgeKind::Calls) { *out_degree.entry(e.from).or_default() += 1; }
    }

    let god_objects: Vec<GodObjectRef> = graph.nodes.iter()
        .filter(|n| out_degree.get(&n.id).copied().unwrap_or(0) >= 15)
        .map(|n| GodObjectRef { name: n.name.clone(), file: strip_unc(&n.file_path), out_calls: out_degree[&n.id] })
        .collect();

    let long_files: Vec<LongFile> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Module) && n.line_end.saturating_sub(n.line_start) > 400)
        .map(|n| LongFile { path: strip_unc(&n.file_path), lines: n.line_end.saturating_sub(n.line_start) })
        .collect();

    let module_by_id: HashMap<u32, &str> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Module))
        .map(|n| (n.id, n.file_path.as_str()))
        .collect();

    let mut import_count: HashMap<String, u32> = HashMap::new();
    let mut layer_violations: Vec<LayerViolation> = Vec::new();

    for e in &graph.edges {
        if matches!(e.kind, EdgeKind::Imports) {
            if let Some(&from_fp) = module_by_id.get(&e.from) {
                *import_count.entry(strip_unc(from_fp)).or_default() += 1;

                if let Some(&to_fp) = module_by_id.get(&e.to) {
                    let fl = detect_layer(from_fp);
                    let tl = detect_layer(to_fp);
                    if fl != "unknown" && tl != "unknown" && layer_order(fl) < layer_order(tl) {
                        layer_violations.push(LayerViolation {
                            from_file: strip_unc(from_fp), to_file: strip_unc(to_fp),
                            from_layer: fl.to_string(), to_layer: tl.to_string(),
                        });
                    }
                }
            }
        }
    }

    let mut high_coupling: Vec<CouplingEntry> = import_count.into_iter()
        .filter(|(_, c)| *c > 8)
        .map(|(f, c)| CouplingEntry { file: f, import_count: c })
        .collect();
    high_coupling.sort_by(|a, b| b.import_count.cmp(&a.import_count));

    layer_violations.truncate(20);

    let singleton_suspects: Vec<String> = graph.nodes.iter()
        .filter(|n| matches!(n.kind, NodeKind::Struct | NodeKind::Class))
        .filter(|n| {
            let l = n.name.to_lowercase();
            l.contains("singleton") || l.contains("registry") || l == "config" || l == "settings"
        })
        .map(|n| n.name.clone())
        .collect();

    PatternReport { god_objects, long_files, high_coupling, layer_violations, singleton_suspects }
}

fn detect_layer(path: &str) -> &'static str {
    let p = path.replace('\\', "/").to_lowercase();
    if p.contains("/test") || p.contains("_test") || p.contains(".test.") || p.contains("spec") { return "test"; }
    if p.contains("/component") || p.contains("/ui/") || p.contains("/views/") || p.contains("/pages/") { return "ui"; }
    if p.contains("/service") || p.contains("/api/") || p.contains("/controller") || p.contains("/handler") { return "service"; }
    if p.contains("/util") || p.contains("/helper") || p.contains("/lib/") { return "util"; }
    if p.contains("/model") || p.contains("/store") || p.contains("/repository") || p.contains("/db/") { return "data"; }
    if p.contains("/config") || p.contains("/settings") { return "config"; }
    "unknown"
}

fn layer_order(layer: &str) -> i32 {
    match layer { "ui" => 3, "service" => 2, "data" => 1, "util" => 0, "config" => -1, _ => -2 }
}

// ─── Git Churn ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ChurnEntry {
    pub path: String,
    pub commits: u32,
    pub owner: String,
}

/// Run `git log` in `dir` and return per-file commit counts + primary author.
/// Safe to call even if dir is not a git repo — returns empty vec in that case.
pub fn git_churn(dir: &str) -> Vec<ChurnEntry> {
    let out = std::process::Command::new("git")
        .args(["-C", dir, "log", "--format=%ae", "--name-only", "--diff-filter=MA", "-n", "1000"])
        .output();

    let Ok(out) = out else { return vec![] };
    if !out.status.success() { return vec![]; }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut file_data: HashMap<String, (u32, HashMap<String, u32>)> = HashMap::new();
    let mut current_author = String::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() { continue; }

        // Author lines contain @
        if line.contains('@') && !line.contains('/') && !line.contains('\\') && !line.contains('.') {
            current_author = line.to_string();
            continue;
        }
        if line.contains('@') && !line.contains('/') && !line.contains('\\') {
            current_author = line.to_string();
            continue;
        }

        // File path lines: contain a dot or path separator, don't look like emails
        let looks_like_file = line.contains('.') || line.contains('/') || line.contains('\\');
        if looks_like_file && !line.starts_with('[') {
            let entry = file_data.entry(line.to_string()).or_insert_with(|| (0, HashMap::new()));
            entry.0 += 1;
            if !current_author.is_empty() {
                *entry.1.entry(current_author.clone()).or_default() += 1;
            }
        }
    }

    let mut result: Vec<ChurnEntry> = file_data.into_iter().map(|(path, (commits, authors))| {
        let owner = authors.into_iter().max_by_key(|(_, c)| *c).map(|(a, _)| a).unwrap_or_default();
        ChurnEntry { path, commits, owner }
    }).collect();

    result.sort_by(|a, b| b.commits.cmp(&a.commits));
    result.truncate(200);
    result
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn strip_unc(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}
