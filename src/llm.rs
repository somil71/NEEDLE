//! Unified LLM client — Anthropic, OpenAI, Groq, Ollama.
//!
//! Provider priority (first matching env var wins):
//!   ANTHROPIC_API_KEY  → Claude Haiku 4.5   (fast, smart, default model)
//!   OPENAI_API_KEY     → GPT-4o-mini
//!   GROQ_API_KEY       → Llama-3.3-70b      (fast free tier)
//!   (none)             → Ollama localhost    (fully offline)
//!
//! Override model via ANTHROPIC_MODEL / OPENAI_MODEL / OLLAMA_MODEL env vars.

use serde_json::{json, Value};

// ── Provider ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum Provider {
    Anthropic { api_key: String, model: String },
    OpenAI    { api_key: String, model: String },
    Groq      { api_key: String, model: String },
    Ollama    { model: String },
}

// ── Client ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LlmClient {
    pub provider: Provider,
}

impl LlmClient {
    /// Detect provider from env vars in priority order. Always returns Some —
    /// falls back to Ollama even if it might not be running.
    pub fn from_env() -> Self {
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            let model = std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".into());
            return Self { provider: Provider::Anthropic { api_key: key, model } };
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            let model = std::env::var("OPENAI_MODEL")
                .unwrap_or_else(|_| "gpt-4o-mini".into());
            return Self { provider: Provider::OpenAI { api_key: key, model } };
        }
        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            let model = std::env::var("GROQ_MODEL")
                .unwrap_or_else(|_| "llama-3.3-70b-versatile".into());
            return Self { provider: Provider::Groq { api_key: key, model } };
        }
        let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".into());
        Self { provider: Provider::Ollama { model } }
    }

    /// True if a real API key is configured (not just Ollama fallback).
    pub fn has_api_key() -> bool {
        std::env::var("ANTHROPIC_API_KEY").is_ok()
            || std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("GROQ_API_KEY").is_ok()
    }

    pub fn display_name(&self) -> String {
        match &self.provider {
            Provider::Anthropic { model, .. } => format!("Anthropic/{model}"),
            Provider::OpenAI    { model, .. } => format!("OpenAI/{model}"),
            Provider::Groq      { model, .. } => format!("Groq/{model}"),
            Provider::Ollama    { model }      => format!("Ollama/{model}"),
        }
    }

    /// Send a system + user message and return the assistant reply.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        match &self.provider {
            Provider::Anthropic { api_key, model } =>
                anthropic_complete(api_key, model, system, user).await,
            Provider::OpenAI { api_key, model } =>
                openai_complete(api_key, "https://api.openai.com", model, system, user).await,
            Provider::Groq { api_key, model } =>
                openai_complete(api_key, "https://api.groq.com/openai", model, system, user).await,
            Provider::Ollama { model } =>
                ollama_complete(model, system, user).await,
        }
    }
}

// ── Anthropic ─────────────────────────────────────────────────────────────────

async fn anthropic_complete(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let client = http_client(60)?;
    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "system": system,
        "messages": [{"role": "user", "content": user}]
    });
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Anthropic request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Anthropic HTTP {status}: {body}"));
    }

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    data["content"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Unexpected Anthropic response: {data}"))
}

// ── OpenAI-compatible (OpenAI + Groq share the same wire format) ──────────────

async fn openai_complete(
    api_key: &str,
    base_url: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let client = http_client(60)?;
    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user",   "content": user}
        ]
    });
    let resp = client
        .post(format!("{base_url}/v1/chat/completions"))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    data["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Unexpected response: {data}"))
}

// ── Ollama ────────────────────────────────────────────────────────────────────

async fn ollama_complete(model: &str, system: &str, user: &str) -> Result<String, String> {
    let client = http_client(120)?;
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user",   "content": user}
        ]
    });
    let resp = client
        .post("http://127.0.0.1:11434/api/chat")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama not running at localhost:11434 — {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("Model '{model}' not found — run: ollama pull {model}"));
    }
    if !resp.status().is_success() {
        return Err(format!("Ollama HTTP {}", resp.status()));
    }

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(data["message"]["content"].as_str().unwrap_or("").to_string())
}

// ── Shared ────────────────────────────────────────────────────────────────────

fn http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| e.to_string())
}
