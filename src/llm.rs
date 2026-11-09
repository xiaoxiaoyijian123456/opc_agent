use crate::config::Config;
use crate::tools::{to_anthropic_tools, to_openai_tools, ToolDef};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// LLM 请求最大重试次数（网络错误 / 429 / 5xx 时重试，指数退避 1s/2s/4s）
const LLM_MAX_ATTEMPTS: u32 = 3;

fn is_retriable_status(status: reqwest::StatusCode) -> bool {
    let code = status.as_u16();
    code == 429 || (500..=599).contains(&code)
}

async fn retry_delay(attempt: u32) {
    let secs = 1u64 << (attempt - 1);
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn complete(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        cfg: &Config,
    ) -> Result<LlmResponse, String>;
}

#[derive(Clone, Debug)]
pub struct ToolCallMsg {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub enum Msg {
    System(String),
    User(String),
    Assistant(String),
    AssistantToolCalls(Vec<ToolCallMsg>),
    ToolResult { id: String, content: String },
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallMsg>,
}

#[derive(Clone)]
pub struct HttpLlm {
    pub client: reqwest::Client,
}

impl HttpLlm {
    pub fn new() -> Self {
        HttpLlm {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for HttpLlm {
    fn default() -> Self {
        Self::new()
    }
}

pub fn openai_messages(messages: &[Msg]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m {
            Msg::System(s) => out.push(json!({ "role": "system", "content": s })),
            Msg::User(s) => out.push(json!({ "role": "user", "content": s })),
            Msg::Assistant(s) => out.push(json!({ "role": "assistant", "content": s })),
            Msg::AssistantToolCalls(calls) => {
                let calls: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".into()),
                            }
                        })
                    })
                    .collect();
                out.push(json!({ "role": "assistant", "content": null, "tool_calls": calls }));
            }
            Msg::ToolResult { id, content } => {
                out.push(json!({ "role": "tool", "tool_call_id": id, "content": content }))
            }
        }
    }
    out
}

/// Splits out System messages (used as the Anthropic `system` param).
pub fn anthropic_split(messages: &[Msg]) -> (String, Vec<Value>) {
    let mut system = String::new();
    let mut out = Vec::new();
    for m in messages {
        match m {
            Msg::System(s) => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(s);
            }
            Msg::User(s) => out.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": s }]
            })),
            Msg::Assistant(s) => out.push(json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": s }]
            })),
            Msg::AssistantToolCalls(calls) => {
                let content: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": c.arguments,
                        })
                    })
                    .collect();
                out.push(json!({ "role": "assistant", "content": content }));
            }
            Msg::ToolResult { id, content } => out.push(json!({
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": id, "content": content }]
            })),
        }
    }
    (system, out)
}

pub fn parse_openai_response(body: &Value) -> LlmResponse {
    let msg = &body["choices"][0]["message"];
    let text = msg["content"].as_str().unwrap_or("").to_string();
    let mut tool_calls = Vec::new();
    if let Some(arr) = msg["tool_calls"].as_array() {
        for tc in arr {
            let arguments: Value =
                serde_json::from_str(tc["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(Value::Null);
            tool_calls.push(ToolCallMsg {
                id: tc["id"].as_str().unwrap_or("").to_string(),
                name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                arguments,
            });
        }
    }
    LlmResponse { text, tool_calls }
}

pub fn parse_anthropic_response(body: &Value) -> LlmResponse {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    if let Some(content) = body["content"].as_array() {
        for block in content {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
                Some("tool_use") => tool_calls.push(ToolCallMsg {
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                    arguments: block["input"].clone(),
                }),
                _ => {}
            }
        }
    }
    LlmResponse { text, tool_calls }
}

#[async_trait]
impl LlmBackend for HttpLlm {
    async fn complete(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        cfg: &Config,
    ) -> Result<LlmResponse, String> {
        if cfg.provider == "anthropic" {
            self.complete_anthropic(messages, tools, cfg).await
        } else {
            self.complete_openai(messages, tools, cfg).await
        }
    }
}

impl HttpLlm {
    async fn complete_openai(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        cfg: &Config,
    ) -> Result<LlmResponse, String> {
        let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": cfg.default_model,
            "messages": openai_messages(messages),
            "max_tokens": cfg.max_tokens,
            "temperature": cfg.temperature,
        });
        body["tools"] = Value::Array(to_openai_tools(tools));

        let mut attempt: u32 = 1;
        loop {
            let req = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", cfg.api_key))
                .json(&body);
            match req.try_clone().unwrap_or(req).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp
                        .text()
                        .await
                        .map_err(|e| format!("OpenAI read body failed: {e}"))?;
                    if status.is_success() {
                        let parsed: Value = serde_json::from_str(&text)
                            .map_err(|e| format!("OpenAI parse failed: {e}"))?;
                        return Ok(parse_openai_response(&parsed));
                    }
                    if is_retriable_status(status) && attempt < LLM_MAX_ATTEMPTS {
                        eprintln!(
                            "  ⚠ OpenAI 返回 {status}，{} 秒后重试 (第 {} 次)",
                            1u64 << (attempt - 1),
                            attempt
                        );
                        retry_delay(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("OpenAI error {status}: {text}"));
                }
                Err(e) => {
                    if attempt < LLM_MAX_ATTEMPTS {
                        eprintln!(
                            "  ⚠ OpenAI 请求失败：{e}，{} 秒后重试 (第 {} 次)",
                            1u64 << (attempt - 1),
                            attempt
                        );
                        retry_delay(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("OpenAI request failed: {e}"));
                }
            }
        }
    }

    async fn complete_anthropic(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        cfg: &Config,
    ) -> Result<LlmResponse, String> {
        let (system, msgs) = anthropic_split(messages);
        let mut body = json!({
            "model": cfg.default_model,
            "messages": msgs,
            "max_tokens": cfg.max_tokens,
            "temperature": cfg.temperature,
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        body["tools"] = Value::Array(to_anthropic_tools(tools));

        let mut attempt: u32 = 1;
        loop {
            let req = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &cfg.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body);
            match req.try_clone().unwrap_or(req).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp
                        .text()
                        .await
                        .map_err(|e| format!("Anthropic read body failed: {e}"))?;
                    if status.is_success() {
                        let parsed: Value = serde_json::from_str(&text)
                            .map_err(|e| format!("Anthropic parse failed: {e}"))?;
                        return Ok(parse_anthropic_response(&parsed));
                    }
                    if is_retriable_status(status) && attempt < LLM_MAX_ATTEMPTS {
                        eprintln!(
                            "  ⚠ Anthropic 返回 {status}，{} 秒后重试 (第 {} 次)",
                            1u64 << (attempt - 1),
                            attempt
                        );
                        retry_delay(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("Anthropic error {status}: {text}"));
                }
                Err(e) => {
                    if attempt < LLM_MAX_ATTEMPTS {
                        eprintln!(
                            "  ⚠ Anthropic 请求失败：{e}，{} 秒后重试 (第 {} 次)",
                            1u64 << (attempt - 1),
                            attempt
                        );
                        retry_delay(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("Anthropic request failed: {e}"));
                }
            }
        }
    }
}
