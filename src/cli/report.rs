//! `needle report` — generate GRAPH_REPORT.md with god nodes, communities, and surprise edges.

use colored::Colorize;
use needle::{
    graph::{self, CodeGraph, NodeKind},
    storage::Storage,
};
use std::collections::HashMap;

pub async fn run(output: Option<String>) -> needle::Result<()> {
    if !Storage::index_exists() {
        eprintln!(
            "{}: No index found. Run `needle init <dirs>` first.",
            "Error".red().bold()
        );
        return Ok(());
    }

    let storage = Storage::new(Storage::default_index_dir())?;
    let graph = storage.load_graph()?;

    if graph.nodes.is_empty() {
        eprintln!(
            "{}: Index graph is empty — try `needle reindex`.",
            "Warning".yellow().bold()
        );
        return Ok(());
    }

    print!("  Computing graph analytics... ");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let god_nodes = graph::compute_god_nodes(&graph, 10);
    let communities = graph::compute_communities(&graph);
    let surprises = graph::find_surprise_edges(&graph, &communities);

    println!("done");

    let n_communities = communities
        .values()
        .collect::<std::collections::HashSet<_>>()
        .len();

    let report = build_report(&graph, &god_nodes, &communities, &surprises);

    let out_path = output.unwrap_or_else(|| "GRAPH_REPORT.md".to_string());
    std::fs::write(&out_path, &report)?;

    println!("{}", format!("✓ Report written to {out_path}").green().bold());
    println!(
        "  {} nodes · {} edges · {} communities · {} surprise edges",
        graph.stats.total_nodes,
        graph.stats.total_edges,
        n_communities,
        surprises.len(),
    );

    Ok(())
}

fn build_report(
    graph: &CodeGraph,
    god_nodes: &[(u32, u32)],
    communities: &HashMap<u32, u32>,
    surprises: &[(u32, u32)],
) -> String {
    let n_communities = communities
        .values()
        .collect::<std::collections::HashSet<_>>()
        .len();

    let mut md = String::new();
    md.push_str("# NEEDLE Graph Report\n\n");
    md.push_str(&format!(
        "> **{}** nodes · **{}** edges · **{}** communities\n\n",
        graph.stats.total_nodes, graph.stats.total_edges, n_communities
    ));

    // ── God Nodes ──────────────────────────────────────────────────────────────
    md.push_str("## God Nodes — Architectural Load-Bearers\n\n");
    md.push_str("> Functions with the highest total call-graph degree. \
        Changes here have the widest blast radius in the codebase.\n\n");

    if god_nodes.is_empty() {
        md.push_str("_No call edges detected — run `needle init` on a code directory._\n\n");
    } else {
        md.push_str("| Rank | Symbol | Degree | Kind | File |\n");
        md.push_str("|------|--------|--------|------|------|\n");
        for (rank, &(node_id, degree)) in god_nodes.iter().enumerate() {
            let node = &graph.nodes[node_id as usize];
            let kind_str = match node.kind {
                NodeKind::Function => "fn",
                NodeKind::Method   => "method",
                NodeKind::Class    => "class",
                NodeKind::Struct   => "struct",
                NodeKind::Trait    => "trait",
                NodeKind::Endpoint => "endpoint",
                NodeKind::Module   => "module",
            };
            md.push_str(&format!(
                "| {} | `{}` | {} | {} | `{}` |\n",
                rank + 1,
                node.name,
                degree,
                kind_str,
                last_two_components(&node.file_path),
            ));
        }
        md.push('\n');
    }

    // ── Communities ────────────────────────────────────────────────────────────
    md.push_str("## Module Communities\n\n");
    md.push_str("> Label-propagation clusters on Calls + Imports edges. \
        Files/functions in the same cluster are tightly coupled and should evolve together.\n\n");

    // Invert: community_id → Vec<node_id>
    let mut comm_to_nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&node_id, &comm_id) in communities {
        comm_to_nodes.entry(comm_id).or_default().push(node_id);
    }
    let mut comm_list: Vec<(u32, Vec<u32>)> = comm_to_nodes.into_iter().collect();
    comm_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    if comm_list.is_empty() {
        md.push_str("_No communities detected._\n\n");
    } else {
        md.push_str(&format!("Found **{}** communities.\n\n", comm_list.len()));
        for (comm_id, members) in comm_list.iter().take(12) {
            let sample: Vec<String> = members
                .iter()
                .filter_map(|&nid| {
                    let n = &graph.nodes[nid as usize];
                    if n.kind != NodeKind::Module {
                        Some(format!("`{}`", n.name))
                    } else {
                        None
                    }
                })
                .take(6)
                .collect();
            md.push_str(&format!(
                "### Community #{} — {} nodes\n\n",
                comm_id,
                members.len()
            ));
            if !sample.is_empty() {
                md.push_str(&format!("{}\n\n", sample.join(" · ")));
            }
        }
        if comm_list.len() > 12 {
            md.push_str(&format!(
                "_…and {} more communities_\n\n",
                comm_list.len() - 12
            ));
        }
    }

    // ── Surprise Edges ─────────────────────────────────────────────────────────
    md.push_str("## Surprise Edges — Unexpected Dependencies\n\n");
    md.push_str("> Cross-community calls where the community pair has very few bridging edges. \
        These are the most architecturally unusual connections — worth reviewing for coupling violations.\n\n");

    if surprises.is_empty() {
        md.push_str("_No surprise edges detected._\n\n");
    } else {
        md.push_str("| From | To | File (caller) | Bridge |\n");
        md.push_str("|------|----|---------------|--------|\n");
        for &(from_id, to_id) in surprises {
            let from = &graph.nodes[from_id as usize];
            let to = &graph.nodes[to_id as usize];
            let from_comm = communities.get(&from_id).copied().unwrap_or(0);
            let to_comm = communities.get(&to_id).copied().unwrap_or(0);
            md.push_str(&format!(
                "| `{}` | `{}` | `{}` | #{} → #{} |\n",
                from.name,
                to.name,
                last_two_components(&from.file_path),
                from_comm,
                to_comm,
            ));
        }
        md.push('\n');
    }

    md
}

fn last_two_components(path: &str) -> String {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        path.to_string()
    }
}
