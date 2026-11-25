//! Opt-in release-notes polishing with the user's own Anthropic API key.
//! Only the notes text is sent, never code, diffs or tokens.
use crate::error::{Error, Result};
use serde_json::{json, Value};

pub const API: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

const PROMPT: &str = "Rewrite these release notes so they read clearly for users of the software. \
Keep the markdown structure, every section heading, every link, PR number and contributor name. \
Do not invent changes, features or numbers that are not in the input. Merge obvious duplicates, \
use plain wording, no marketing language, no emoji, no em or en dashes. \
Return only the markdown, nothing before or after it.\n\n<notes>\n";

pub async fn polish(base: Option<&str>, key: &str, model: &str, notes: &str) -> Result<String> {
    crate::redact::register_secret(key);
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/messages", base.unwrap_or(API)))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": model,
            "max_tokens": 4000,
            "messages": [{ "role": "user", "content": format!("{PROMPT}{notes}\n</notes>") }]
        }))
        .send()
        .await?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(Error::Other(format!("AI request failed ({status}): {}", v["error"]["message"].as_str().unwrap_or(""))));
    }
    let text: String = v["content"].as_array().map(|a| {
        a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("")
    }).unwrap_or_default();
    if text.trim().is_empty() {
        return Err(Error::Other("AI returned no text".into()));
    }
    Ok(text.trim().to_string() + "\n")
}
