use myopc::config::Config;
use myopc::llm::{parse_anthropic_response, parse_openai_response, HttpLlm, LlmBackend, Msg};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

fn config(provider: &str, base_url: String) -> Config {
    Config {
        provider: provider.into(),
        api_key: "test-key".into(),
        base_url,
        default_model: "test-model".into(),
        team_root: ".myopc_agent".into(),
        code_root: Some(".".into()),
        max_tokens: 1024,
        temperature: 0.0,
        llm_connect_timeout_seconds: 1,
        llm_request_timeout_seconds: 5,
        llm_max_attempts: 3,
    }
}

#[test]
fn openai_response_parsing_is_strict() {
    let response = parse_openai_response(&json!({
        "choices": [{"message": {"content": "working", "tool_calls": [{
            "id": "call-1",
            "function": {"name": "get_project_status", "arguments": "{\"verbose\":true}"}
        }]}}]
    }))
    .unwrap();
    assert_eq!(response.text, "working");
    assert_eq!(response.tool_calls[0].arguments, json!({"verbose": true}));

    assert!(parse_openai_response(&json!({})).is_err());
    assert!(parse_openai_response(&json!({
        "choices": [{"message": {"content": null, "tool_calls": [{
            "id": "call-1", "function": {"name": "run_bash", "arguments": "{bad"}
        }]}}]
    }))
    .is_err());
    assert!(parse_openai_response(&json!({
        "choices": [{"message": {"content": null, "tool_calls": [{
            "id": "", "function": {"name": "run_bash", "arguments": "{}"}
        }]}}]
    }))
    .is_err());
    assert!(parse_openai_response(&json!({
        "choices": [{"message": {"content": null}}]
    }))
    .is_err());
}

#[test]
fn anthropic_response_parsing_is_strict() {
    let response = parse_anthropic_response(&json!({
        "content": [
            {"type": "text", "text": "working"},
            {"type": "tool_use", "id": "tool-1", "name": "get_project_status", "input": {}}
        ]
    }))
    .unwrap();
    assert_eq!(response.text, "working");
    assert_eq!(response.tool_calls.len(), 1);

    assert!(parse_anthropic_response(&json!({})).is_err());
    assert!(parse_anthropic_response(&json!({
        "content": [{"type": "tool_use", "id": "tool-1", "name": "run_bash", "input": []}]
    }))
    .is_err());
    assert!(parse_anthropic_response(&json!({"content": []})).is_err());
}

struct ServerResponse {
    status: u16,
    body: Value,
    retry_after: Option<u64>,
    delay: Duration,
}

impl ServerResponse {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body,
            retry_after: None,
            delay: Duration::ZERO,
        }
    }
}

async fn server(responses: Vec<ServerResponse>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            if !response.delay.is_zero() {
                tokio::time::sleep(response.delay).await;
            }
            let body = response.body.to_string();
            let reason = if response.status == 200 {
                "OK"
            } else {
                "ERROR"
            };
            let retry_after = response
                .retry_after
                .map(|value| format!("Retry-After: {value}\r\n"))
                .unwrap_or_default();
            let message = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                response.status,
                reason,
                body.len(),
                retry_after,
                body
            );
            let _ = stream.write_all(message.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        requests
    });
    (format!("http://{address}/v1"), task)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        if count == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..count]);
        if let Some(header_end) = data.windows(4).position(|value| value == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&data[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::to_string)
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn openai_body() -> Value {
    json!({"choices": [{"message": {"content": "done"}}]})
}

#[tokio::test]
async fn openai_retries_500_then_succeeds() {
    let (base_url, requests) = server(vec![
        ServerResponse {
            retry_after: Some(0),
            ..ServerResponse::json(500, json!({"error": "temporary"}))
        },
        ServerResponse::json(200, openai_body()),
    ])
    .await;
    let mut config = config("openai", base_url);
    config.llm_max_attempts = 2;
    let llm = HttpLlm::from_config(&config).unwrap();

    let response = llm
        .complete(&[Msg::User("hello".into())], &[], &config)
        .await
        .unwrap();
    assert_eq!(response.text, "done");
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.starts_with("POST /v1/chat/completions")));
}

#[tokio::test]
async fn client_error_is_not_retried() {
    let (base_url, requests) = server(vec![ServerResponse::json(
        400,
        json!({"error": "bad request"}),
    )])
    .await;
    let config = config("openai", base_url);
    let llm = HttpLlm::from_config(&config).unwrap();

    let error = llm
        .complete(&[Msg::User("hello".into())], &[], &config)
        .await
        .err()
        .unwrap();
    assert!(error.contains("400"));
    assert_eq!(requests.await.unwrap().len(), 1);
}

#[tokio::test]
async fn request_timeout_is_enforced() {
    let (base_url, requests) = server(vec![ServerResponse {
        delay: Duration::from_secs(2),
        ..ServerResponse::json(200, openai_body())
    }])
    .await;
    let mut config = config("openai", base_url);
    config.llm_request_timeout_seconds = 1;
    config.llm_max_attempts = 1;
    let llm = HttpLlm::from_config(&config).unwrap();

    let started = Instant::now();
    let error = llm
        .complete(&[Msg::User("hello".into())], &[], &config)
        .await
        .err()
        .unwrap();
    assert!(error.contains("request failed"), "{error}");
    assert!(started.elapsed() < Duration::from_millis(1800));
    requests.abort();
}

#[tokio::test]
async fn anthropic_uses_configured_endpoint() {
    let (base_url, requests) = server(vec![ServerResponse::json(
        200,
        json!({"content": [{"type": "text", "text": "done"}]}),
    )])
    .await;
    let config = config("anthropic", base_url);
    let llm = HttpLlm::from_config(&config).unwrap();

    let response = llm
        .complete(&[Msg::User("hello".into())], &[], &config)
        .await
        .unwrap();
    assert_eq!(response.text, "done");
    assert!(requests.await.unwrap()[0].starts_with("POST /v1/messages"));
}
