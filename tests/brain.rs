use async_trait::async_trait;
use myopc::agent_runner::ToolExecutor;
use myopc::brain::Brain;
use myopc::config::Config;
use myopc::llm::{LlmBackend, LlmResponse, Msg, ToolCallMsg};
use myopc::tools::ToolDef;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct LoopLlm {
    valid: bool,
}

#[async_trait]
impl LlmBackend for LoopLlm {
    async fn complete(
        &self,
        _messages: &[Msg],
        _tools: &[ToolDef],
        _config: &Config,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            text: String::new(),
            tool_calls: vec![ToolCallMsg {
                id: if self.valid { "call-1" } else { "" }.into(),
                name: "get_project_status".into(),
                arguments: json!({}),
            }],
        })
    }
}

struct CountExecutor {
    calls: AtomicUsize,
}

#[async_trait]
impl ToolExecutor for CountExecutor {
    async fn execute(&self, _name: &str, _args: &Value) -> String {
        self.calls.fetch_add(1, Ordering::SeqCst);
        "ok".into()
    }
}

fn config() -> Config {
    Config {
        provider: "openai".into(),
        api_key: "test-key".into(),
        base_url: "http://localhost:0".into(),
        default_model: "test-model".into(),
        team_root: ".myopc_agent".into(),
        code_root: Some(".".into()),
        max_tokens: 1024,
        temperature: 0.0,
        llm_connect_timeout_seconds: 1,
        llm_request_timeout_seconds: 1,
        llm_max_attempts: 1,
    }
}

#[tokio::test]
async fn brain_stops_after_thirty_tool_rounds() {
    let executor = Arc::new(CountExecutor {
        calls: AtomicUsize::new(0),
    });
    let mut brain = Brain::new(
        config(),
        Arc::new(LoopLlm { valid: true }),
        None,
        Vec::new(),
    );

    let error = brain.chat("hello", executor.clone()).await.err().unwrap();
    assert!(error.contains("30"), "{error}");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 30);
}

#[tokio::test]
async fn brain_rejects_empty_tool_identifiers_before_execution() {
    let executor = Arc::new(CountExecutor {
        calls: AtomicUsize::new(0),
    });
    let mut brain = Brain::new(
        config(),
        Arc::new(LoopLlm { valid: false }),
        None,
        Vec::new(),
    );

    let error = brain.chat("hello", executor.clone()).await.err().unwrap();
    assert!(error.contains("id"), "{error}");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}
