mod response;
mod retry;

pub use response::{parse_anthropic_response, parse_openai_response, validate_tool_call};

use crate::config::Config;
use crate::tools::{to_anthropic_tools, to_openai_tools, ToolDef};
use async_trait::async_trait;
use retry::{is_network_error, is_retriable_status, retry_delay};
use serde_json::{json, Value};
use std::time::Duration;

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
        Self::with_timeouts(Duration::from_secs(10), Duration::from_secs(180))
            .expect("default HTTP timeouts are valid")
    }

    pub fn from_config(config: &Config) -> Result<Self, String> {
        Self::with_timeouts(
            Duration::from_secs(config.llm_connect_timeout_seconds),
            Duration::from_secs(config.llm_request_timeout_seconds),
        )
    }

    pub fn with_timeouts(connect: Duration, request: Duration) -> Result<Self, String> {
        if connect.is_zero() || request.is_zero() {
            return Err("LLM timeouts must be greater than zero".into());
        }
        let client = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(request)
            .build()
            .map_err(|error| format!("Cannot create LLM HTTP client: {error}"))?;
        Ok(Self { client })
    }
}

impl Default for HttpLlm {
    fn default() -> Self {
        Self::new()
    }
}

pub fn openai_messages(messages: &[Msg]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        match message {
            Msg::System(text) => out.push(json!({ "role": "system", "content": text })),
            Msg::User(text) => out.push(json!({ "role": "user", "content": text })),
            Msg::Assistant(text) => out.push(json!({ "role": "assistant", "content": text })),
            Msg::AssistantToolCalls(calls) => {
                let calls: Vec<Value> = calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into()),
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

pub fn anthropic_split(messages: &[Msg]) -> (String, Vec<Value>) {
    let mut system = String::new();
    let mut out = Vec::new();
    for message in messages {
        match message {
            Msg::System(text) => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(text);
            }
            Msg::User(text) => out.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": text }]
            })),
            Msg::Assistant(text) => out.push(json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": text }]
            })),
            Msg::AssistantToolCalls(calls) => {
                let content: Vec<Value> = calls
                    .iter()
                    .map(|call| {
                        json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.arguments,
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

#[async_trait]
impl LlmBackend for HttpLlm {
    async fn complete(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        config: &Config,
    ) -> Result<LlmResponse, String> {
        if config.provider == "anthropic" {
            self.complete_anthropic(messages, tools, config).await
        } else {
            self.complete_openai(messages, tools, config).await
        }
    }
}

impl HttpLlm {
    async fn complete_openai(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        config: &Config,
    ) -> Result<LlmResponse, String> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let body = json!({
            "model": config.default_model,
            "messages": openai_messages(messages),
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "tools": to_openai_tools(tools),
        });
        let attempts = config.llm_max_attempts.max(1);
        for attempt in 1..=attempts {
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", config.api_key))
                .timeout(Duration::from_secs(config.llm_request_timeout_seconds))
                .json(&body)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    if is_network_error(&error) && attempt < attempts {
                        tokio::time::sleep(retry_delay(attempt, &Default::default())).await;
                        continue;
                    }
                    return Err(config.redact(&format!("OpenAI request failed: {error}")));
                }
            };
            let status = response.status();
            let headers = response.headers().clone();
            let text = match response.text().await {
                Ok(text) => text,
                Err(error) => {
                    if is_network_error(&error) && attempt < attempts {
                        tokio::time::sleep(retry_delay(attempt, &headers)).await;
                        continue;
                    }
                    return Err(config.redact(&format!("OpenAI read body failed: {error}")));
                }
            };
            if status.is_success() {
                let parsed: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("OpenAI parse failed: {error}"))?;
                return parse_openai_response(&parsed)
                    .map_err(|error| format!("OpenAI invalid response: {error}"));
            }
            if is_retriable_status(status) && attempt < attempts {
                tokio::time::sleep(retry_delay(attempt, &headers)).await;
                continue;
            }
            return Err(config.redact(&format!("OpenAI error {status}: {text}")));
        }
        Err("OpenAI request exhausted its retry policy".into())
    }

    async fn complete_anthropic(
        &self,
        messages: &[Msg],
        tools: &[ToolDef],
        config: &Config,
    ) -> Result<LlmResponse, String> {
        let (system, messages) = anthropic_split(messages);
        let url = format!("{}/messages", config.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": config.default_model,
            "messages": messages,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "tools": to_anthropic_tools(tools),
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        let attempts = config.llm_max_attempts.max(1);
        for attempt in 1..=attempts {
            let response = self
                .client
                .post(&url)
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .timeout(Duration::from_secs(config.llm_request_timeout_seconds))
                .json(&body)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    if is_network_error(&error) && attempt < attempts {
                        tokio::time::sleep(retry_delay(attempt, &Default::default())).await;
                        continue;
                    }
                    return Err(config.redact(&format!("Anthropic request failed: {error}")));
                }
            };
            let status = response.status();
            let headers = response.headers().clone();
            let text = match response.text().await {
                Ok(text) => text,
                Err(error) => {
                    if is_network_error(&error) && attempt < attempts {
                        tokio::time::sleep(retry_delay(attempt, &headers)).await;
                        continue;
                    }
                    return Err(config.redact(&format!("Anthropic read body failed: {error}")));
                }
            };
            if status.is_success() {
                let parsed: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("Anthropic parse failed: {error}"))?;
                return parse_anthropic_response(&parsed)
                    .map_err(|error| format!("Anthropic invalid response: {error}"));
            }
            if is_retriable_status(status) && attempt < attempts {
                tokio::time::sleep(retry_delay(attempt, &headers)).await;
                continue;
            }
            return Err(config.redact(&format!("Anthropic error {status}: {text}")));
        }
        Err("Anthropic request exhausted its retry policy".into())
    }
}
