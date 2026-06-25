//! `needle graph [output]` — export D3 force-directed visualization to graph.html.

use colored::Colorize;
use needle::storage::Storage;
use serde_json::{json, Value};

const TEMPLATE: &str = include_str!("../assets/graph_template.html");

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

    // Convert to D3-friendly format
    let nodes: Vec<Value> = graph
        .nodes
        .iter()
        .map(|n| {
            json!({
                "id":        n.id,
                "name":      n.name,
                "kind":      format!("{:?}", n.kind).to_lowercase(),
                "file_path": n.file_path,
                "line_start": n.line_start,
                "language":  n.language,
            })
        })
        .collect();

    let links: Vec<Value> = graph
        .edges
        .iter()
        .map(|e| {
            json!({
                "source": e.from,
                "target": e.to,
                "kind":   format!("{:?}", e.kind).to_lowercase(),
            })
        })
        .collect();

    let data = json!({
        "nodes": nodes,
        "links": links,
        "stats": {
            "total_nodes": graph.stats.total_nodes,
            "total_edges": graph.stats.total_edges,
            "functions":   graph.stats.functions,
            "methods":     graph.stats.methods,
            "classes":     graph.stats.classes,
            "endpoints":   graph.stats.endpoints,
            "modules":     graph.stats.modules,
        }
    });

    let html = TEMPLATE.replace("__GRAPH_DATA__", &data.to_string());

    let out_path = output.unwrap_or_else(|| "graph.html".to_string());
    std::fs::write(&out_path, &html)?;

    println!("{}", format!("✓ Graph written to {out_path}").green().bold());
    println!(
        "  {} nodes · {} edges",
        graph.stats.total_nodes,
        graph.stats.total_edges,
    );

    // Open in default browser
    let abs = std::fs::canonicalize(&out_path)
        .unwrap_or_else(|_| std::path::PathBuf::from(&out_path));
    if let Err(e) = open::that(&abs) {
        println!("  (Could not open browser: {e})");
    }

    Ok(())
}
