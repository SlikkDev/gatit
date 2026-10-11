//! Meeting summaries from the Claude API. Only transcript text is sent, never audio.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_PROMPT: &str = include_str!("../prompts/summary.md");
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    /// "me" or "them".
    pub owner: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub title: String,
    pub overview: String,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub action_items: Vec<ActionItem>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub model: String,
}

impl Summary {
    pub fn to_markdown(&self) -> String {
        let mut md = format!("## Summary\n\n{}\n", self.overview);
        let mut list = |heading: &str, items: Vec<String>| {
            if !items.is_empty() {
                md.push_str(&format!("\n### {heading}\n\n"));
                for item in items {
                    md.push_str(&format!("- {item}\n"));
                }
            }
        };
        list("Key points", self.key_points.clone());
        list("Decisions", self.decisions.clone());
        list(
            "Action items",
            self.action_items
                .iter()
                .map(|a| {
                    format!(
                        "[ ] {} ({})",
                        a.text,
                        if a.owner == "me" { "me" } else { "them" }
                    )
                })
                .collect(),
        );
        list("Open questions", self.open_questions.clone());
        md
    }
}

fn schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "overview": { "type": "string" },
            "key_points": strings,
            "decisions": strings,
            "action_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string", "enum": ["me", "them"] },
                        "text": { "type": "string" }
                    },
                    "required": ["owner", "text"],
                    "additionalProperties": false
                }
            },
            "open_questions": strings
        },
        "required": ["title", "overview", "key_points", "decisions", "action_items", "open_questions"],
        "additionalProperties": false
    })
}

pub fn build_request(model: &str, prompt: &str, context: &str, transcript: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16000,
        "system": prompt,
        "messages": [{
            "role": "user",
            "content": format!("{context}\n\n<transcript>\n{transcript}\n</transcript>")
        }],
        "output_config": {
            "effort": "medium",
            "format": { "type": "json_schema", "schema": schema() }
        },
        // If a safety classifier declines, retry on Anthropic's recommended fallback model.
        "fallbacks": "default"
    })
}

/// Reads the summary out of a Messages API response.
pub fn parse_response(response: &Value) -> Result<Summary, String> {
    match response["stop_reason"].as_str() {
        Some("refusal") => return Err("Claude declined to summarize this call.".into()),
        Some("max_tokens") => return Err("The summary was cut off. Try summarizing again.".into()),
        _ => {}
    }
    let text = response["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .ok_or("Claude's response had no summary text.")?;
    let mut summary: Summary =
        serde_json::from_str(text).map_err(|e| format!("unexpected summary format: {e}"))?;
    summary.model = response["model"].as_str().unwrap_or_default().to_string();
    Ok(summary)
}

pub async fn summarize(
    api_key: &str,
    model: &str,
    prompt: &str,
    context: &str,
    transcript: &str,
) -> Result<Summary, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .json(&build_request(model, prompt, context, transcript))
            .send()
            .await;
        let response = match result {
            Ok(r) => r,
            Err(e) if attempt < 3 => {
                tokio::time::sleep(Duration::from_secs(5 * attempt)).await;
                let _ = e;
                continue;
            }
            Err(e) => return Err(format!("Couldn't reach Claude: {e}")),
        };
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return parse_response(&body);
        }
        let retryable =
            status.as_u16() == 429 || status.as_u16() == 529 || status.is_server_error();
        if retryable && attempt < 3 {
            tokio::time::sleep(Duration::from_secs(10 * attempt)).await;
            continue;
        }
        let message = body["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(match status.as_u16() {
            401 => "Claude rejected the API key. Check it in Settings.".into(),
            _ => format!("Claude API error {}: {message}", status.as_u16()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_structured_output_and_fallbacks() {
        let req = build_request(
            "claude-opus-5-5",
            "prompt",
            "Call on Sunday",
            "[00:01] Me: hi",
        );
        assert_eq!(req["model"], "claude-opus-5-5");
        assert_eq!(req["output_config"]["format"]["type"], "json_schema");
        assert_eq!(req["fallbacks"], "default");
        assert!(
            req.get("thinking").is_none(),
            "Opus 5.5 rejects disabled thinking; leave it adaptive"
        );
        let content = req["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("<transcript>\n[00:01] Me: hi\n</transcript>"));
    }

    #[test]
    fn parses_summary_after_thinking_block() {
        let response = json!({
            "model": "claude-opus-5-5",
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": r#"{"title":"Pricing call with Dana","overview":"We agreed on pricing.","key_points":["Price is 10k"],"decisions":["Go with plan B"],"action_items":[{"owner":"me","text":"Send the contract by Thursday"}],"open_questions":[]}"# }
            ]
        });
        let summary = parse_response(&response).unwrap();
        assert_eq!(summary.title, "Pricing call with Dana");
        assert_eq!(summary.action_items[0].owner, "me");
        assert_eq!(summary.model, "claude-opus-5-5");
        let md = summary.to_markdown();
        assert!(md.contains("- [ ] Send the contract by Thursday (me)"));
        assert!(!md.contains("Open questions"));
    }

    #[test]
    fn reports_refusals() {
        let response = json!({ "stop_reason": "refusal", "content": [] });
        assert!(parse_response(&response).unwrap_err().contains("declined"));
    }
}
