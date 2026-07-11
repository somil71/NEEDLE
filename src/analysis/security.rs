use serde::Serialize;
use super::strip_unc;

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
                if lower.contains(pat) {
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
