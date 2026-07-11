use super::{
    cloud_search, cloud_similar, display_cloud_path, lang_from_short, node_kind_label,
    CloudConfig,
};
use needle::{
    analysis,
    graph::{self, CodeGraph, EdgeKind, NodeKind},
    query::QueryEngine,
};
use serde_json::Value;

pub(super) async fn search_code(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or("");
    if query.is_empty() { return Err("query is required".into()); }
    let limit       = args["limit"].as_u64().unwrap_or(5).min(20) as usize;
    let lang_filter = args["lang"].as_str().and_then(lang_from_short);

    let mut out   = String::new();
    let mut total = 0usize;

    if let Some(cfg) = cloud {
        for r in cloud_search(cfg, query, limit).await {
            total += 1;
            out.push_str(&format!(
                "### Result {} [cloud] — {}:{}-{}\n```{}\n{}\n```\n\n",
                total,
                display_cloud_path(&r.file_path),
                r.line_start, r.line_end,
                r.language,
                r.content.trim()
            ));
        }
    }

    if let Some((engine, _)) = local {
        if let Ok((results, timing)) = engine.search(query, limit, lang_filter) {
            for r in &results {
                total += 1;
                out.push_str(&format!(
                    "### Result {} [local] — {}:{}-{}\n```{}\n{}\n```\n\n",
                    total,
                    r.file_path.replace('\\', "/"),
                    r.line_start, r.line_end,
                    r.language.short_name(),
                    r.content.trim()
                ));
            }
            if total > 0 {
                let mut header = format!("Found {} result(s) in {:.1}ms", total, timing.total_ms);
                if cloud.is_some() { header.push_str(" (cloud + local)"); }
                return Ok(format!("{header}:\n\n{out}"));
            }
        }
    }

    if total == 0 {
        Ok(format!("No results found for: {query}"))
    } else {
        Ok(format!("Found {total} result(s) (cloud):\n\n{out}"))
    }
}

pub(super) async fn find_similar(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let code = args["code"].as_str().unwrap_or("");
    if code.is_empty() { return Err("code is required".into()); }
    let limit = args["limit"].as_u64().unwrap_or(5).min(20) as usize;

    let mut out   = String::new();
    let mut total = 0usize;

    if let Some(cfg) = cloud {
        for r in cloud_similar(cfg, code, limit).await {
            total += 1;
            out.push_str(&format!(
                "### Similar {} [cloud] — {}:{}-{} (score {:.2})\n```{}\n{}\n```\n\n",
                total,
                display_cloud_path(&r.file_path),
                r.line_start, r.line_end,
                r.score,
                r.language,
                r.content.trim()
            ));
        }
    }

    if let Some((engine, _)) = local {
        if let Ok(results) = engine.search_similar(code, limit, None) {
            for r in &results {
                total += 1;
                out.push_str(&format!(
                    "### Similar {} [local] — {}:{}-{} (score {:.2})\n```{}\n{}\n```\n\n",
                    total,
                    r.file_path.replace('\\', "/"),
                    r.line_start, r.line_end,
                    r.score,
                    r.language.short_name(),
                    r.content.trim()
                ));
            }
        }
    }

    if total == 0 { Ok("No similar code found.".into()) }
    else { Ok(format!("Found {total} similar chunk(s):\n\n{out}")) }
}

pub(super) async fn get_stats(
    local: Option<&(QueryEngine, CodeGraph)>,
    cloud: Option<&CloudConfig>,
) -> Result<String, String> {
    let mut out = String::from("## Needle Index Overview\n\n");

    if let Some((engine, graph)) = local {
        let files = engine.file_list();
        let s = &graph.stats;
        out.push_str("### Local Index\n");
        out.push_str(&format!("- **Files**: {}\n", files.len()));
        out.push_str(&format!("- **Chunks**: {}\n", engine.chunks.len()));
        out.push_str(&format!("- **Graph nodes**: {}\n", s.total_nodes));
        out.push_str(&format!("- **Graph edges**: {}\n", s.total_edges));
        out.push_str(&format!("- **Functions**: {}\n", s.functions));
        out.push_str(&format!("- **Methods**: {}\n", s.methods));
        out.push_str(&format!("- **Structs/Classes/Traits**: {}\n", s.classes));

        out.push_str("\n**Languages:**\n");
        let mut lang_counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for f in &files { *lang_counts.entry(f.lang.as_str()).or_default() += 1; }
        let mut langs: Vec<_> = lang_counts.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1));
        for (lang, count) in langs { out.push_str(&format!("- {lang}: {count} file(s)\n")); }
    } else {
        out.push_str("### Local Index\nNo local index (run `needle init <dirs...>` to index a codebase)\n");
    }

    if let Some(cfg) = cloud {
        out.push_str(&format!("\n### Cloud ({}) — connected\n", cfg.base_url));
        out.push_str(&format!("- **API key**: {}…\n", &cfg.api_key[..cfg.api_key.len().min(12)]));
        out.push_str("- Search queries include your indexed GitHub repos\n");
        out.push_str("- Graph tools (callers, file structure) use the local index only\n");
    } else {
        out.push_str("\n### Cloud\nNot configured (set NEEDLE_API_KEY + NEEDLE_CLOUD_URL to enable)\n");
    }

    Ok(out)
}

pub(super) async fn explain(
    args:  &Value,
    local: Option<&(QueryEngine, CodeGraph)>,
    llm:   &needle::llm::LlmClient,
) -> Result<String, String> {
    let question = args["question"].as_str().unwrap_or("").trim().to_string();
    if question.is_empty() { return Err("question is required".into()); }
    let chunks = args["context_chunks"].as_u64().unwrap_or(6).min(12) as usize;

    let (engine, graph) = local.ok_or_else(||
        "explain requires a local index. Run: needle init <dirs...>".to_string()
    )?;

    let (results, _) = engine.search(&question, chunks, None)
        .map_err(|e| e.to_string())?;

    if results.is_empty() {
        return Ok(format!("No relevant code found for: {question}"));
    }

    let mut ctx = String::new();

    ctx.push_str("## Relevant Code\n\n");
    for (i, r) in results.iter().enumerate() {
        ctx.push_str(&format!(
            "### [{i}] {}:{}-{}\n```{}\n{}\n```\n\n",
            r.file_path.replace('\\', "/"), r.line_start, r.line_end,
            r.language.short_name(), r.content.trim()
        ));
    }

    let top_file = &results[0].file_path;
    let top_line = results[0].line_start;
    let top_node = graph.nodes.iter()
        .filter(|n| &n.file_path == top_file
            && matches!(n.kind, NodeKind::Function | NodeKind::Method)
            && n.line_start <= top_line && top_line <= n.line_end)
        .min_by_key(|n| top_line - n.line_start);

    if let Some(node) = top_node {
        let callers: Vec<&needle::graph::GraphNode> = graph.edges.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Calls) && e.to == node.id)
            .filter_map(|e| graph.nodes.get(e.from as usize))
            .collect();
        let callees: Vec<&needle::graph::GraphNode> = graph.edges.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Calls) && e.from == node.id)
            .filter_map(|e| graph.nodes.get(e.to as usize))
            .collect();

        if !callers.is_empty() || !callees.is_empty() {
            ctx.push_str(&format!("## Call Graph — `{}`\n", node.name));
            if !callers.is_empty() {
                ctx.push_str("**Called by:** ");
                ctx.push_str(&callers.iter().map(|n| format!("`{}`", n.name)).collect::<Vec<_>>().join(", "));
                ctx.push('\n');
            }
            if !callees.is_empty() {
                ctx.push_str("**Calls:** ");
                ctx.push_str(&callees.iter().map(|n| format!("`{}`", n.name)).collect::<Vec<_>>().join(", "));
                ctx.push('\n');
            }
            ctx.push('\n');
        }
    }

    let communities = graph::compute_communities(graph);
    let top_community = graph.nodes.iter()
        .find(|n| &n.file_path == top_file && matches!(n.kind, NodeKind::Module))
        .and_then(|n| communities.get(&n.id))
        .copied();

    if let Some(cid) = top_community {
        let cluster: Vec<&needle::graph::GraphNode> = graph.nodes.iter()
            .filter(|n| communities.get(&n.id) == Some(&cid)
                && !matches!(n.kind, NodeKind::Module))
            .collect();
        if !cluster.is_empty() {
            ctx.push_str(&format!("## Cluster (community {cid}) — {} symbols\n", cluster.len()));
            for m in cluster.iter().take(12) {
                ctx.push_str(&format!("- `{}` ({})\n", m.name, node_kind_label(&m.kind)));
            }
            if cluster.len() > 12 {
                ctx.push_str(&format!("- … and {} more\n", cluster.len() - 12));
            }
            ctx.push('\n');
        }
    }

    let gods = graph::compute_god_nodes(graph, 5);
    if !gods.is_empty() {
        ctx.push_str("## Architectural Core (highest-degree symbols)\n");
        for (nid, deg) in &gods {
            if let Some(n) = graph.nodes.get(*nid as usize) {
                ctx.push_str(&format!("- `{}` — degree {}\n", n.name, deg));
            }
        }
        ctx.push('\n');
    }

    let system = "\
You are an expert software architect helping a developer understand a codebase. \
You have been given: (1) the most relevant code chunks retrieved by hybrid BM25+vector search, \
(2) the call graph around the top result, (3) its semantic community cluster, \
and (4) the highest-degree architectural symbols. \
Answer WHY the code is structured this way — focus on design intent, trade-offs, \
and architectural patterns. Cite specific function names, file paths, and line numbers. \
If the provided context doesn't contain enough information to explain the 'why', say so clearly.";

    let user_msg = format!("## Codebase Context\n\n{ctx}## Question\n\n{question}");

    let answer = llm.complete(system, &user_msg).await
        .map_err(|e| format!("LLM error: {e}\n\nTip: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GROQ_API_KEY"))?;

    Ok(format!(
        "**Model:** {}\n**Context:** {} chunks · call graph · community · god nodes\n\n---\n\n{}",
        llm.display_name(), results.len(), answer
    ))
}

pub(super) fn get_security_scan(local: Option<&(QueryEngine, CodeGraph)>) -> Result<String, String> {
    let (engine, _) = local.ok_or("Security scan requires a local index. Run: needle init <dirs...>")?;
    let issues = analysis::scan_security(&engine.chunks);

    if issues.is_empty() {
        return Ok("✓ No security issues found in the indexed codebase.".to_string());
    }

    let high: Vec<_>   = issues.iter().filter(|i| i.severity == "HIGH").collect();
    let medium: Vec<_> = issues.iter().filter(|i| i.severity == "MEDIUM").collect();
    let low: Vec<_>    = issues.iter().filter(|i| i.severity == "LOW").collect();

    let mut out = format!(
        "## Security Scan — {} issue(s) found\n\n**HIGH: {}  MEDIUM: {}  LOW: {}**\n\n",
        issues.len(), high.len(), medium.len(), low.len()
    );

    let render_group = |label: &str, items: &[&analysis::SecurityIssue]| -> String {
        if items.is_empty() { return String::new(); }
        let mut s = format!("### {} ({} issue{})\n", label, items.len(), if items.len()==1 {""} else {"s"});
        for i in items.iter().take(15) {
            let file = i.file.replace('\\', "/").split('/').last().unwrap_or("").to_string();
            s.push_str(&format!("- **{}** in `{}` line {}\n  `{}`\n",
                i.kind, file, i.line, i.snippet.trim().chars().take(80).collect::<String>()));
        }
        s.push('\n');
        s
    };

    out.push_str(&render_group("HIGH Severity", &high));
    out.push_str(&render_group("MEDIUM Severity", &medium));
    out.push_str(&render_group("LOW Severity", &low));
    Ok(out)
}
