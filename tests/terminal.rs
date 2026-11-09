use myopc::input::{command_input_guard, input_is_busy, prompt_guard};
use myopc::notification::NotificationQueue;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn prompts_exclude_command_input_until_released() {
    let prompt = prompt_guard();
    assert!(input_is_busy());
    assert!(command_input_guard().is_none());
    drop(prompt);

    assert!(!input_is_busy());
    let command = command_input_guard().unwrap();
    assert!(input_is_busy());
    assert!(command_input_guard().is_none());
    drop(command);
    assert!(!input_is_busy());
}

#[test]
fn notifications_are_fifo_and_drained_atomically() {
    let queue = NotificationQueue::default();
    queue.push("first");
    queue.push("second");

    assert_eq!(queue.drain(), vec!["first", "second"]);
    assert!(queue.drain().is_empty());
}

#[tokio::test]
async fn cli_accepts_queries_which_finish_during_a_background_chat() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = tokio::io::AsyncReadExt::read(&mut stream, &mut chunk)
                .await
                .unwrap();
            if count == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..count]);
            if data.windows(4).any(|value| value == b"\r\n\r\n") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let body = r#"{"choices":[{"message":{"content":"background-response"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
            .await
            .unwrap();
    });

    let dir = std::env::temp_dir().join(format!(
        "myopc-terminal-background-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    myopc::init::init_project(&dir);
    std::fs::write(
        dir.join("myopc.json"),
        format!(
            "{{\"provider\":\"openai\",\"base_url\":\"http://{address}/v1\",\"default_model\":\"test-model\",\"team_root\":\".myopc_agent\",\"llm_request_timeout_seconds\":5,\"llm_max_attempts\":1}}"
        ),
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_myopc"));
    command
        .current_dir(&dir)
        .env("HOME", &dir)
        .env("MYOPC_API_KEY", "test-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in [
        "MYOPC_BASE_URL",
        "MYOPC_MODEL",
        "MYOPC_PROVIDER",
        "MYOPC_TEAM_ROOT",
        "MYOPC_CODE_ROOT",
        "MYOPC_MAX_TOKENS",
        "MYOPC_TEMPERATURE",
        "MYOPC_LLM_CONNECT_TIMEOUT_SECONDS",
        "MYOPC_LLM_REQUEST_TIMEOUT_SECONDS",
        "MYOPC_LLM_MAX_ATTEMPTS",
    ] {
        command.env_remove(name);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"hello\n").unwrap();
    tokio::time::sleep(Duration::from_millis(75)).await;
    stdin.write_all(b"/status\n/tasks\n/help\n").unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    stdin.write_all(b"\n/exit\n").unwrap();
    drop(stdin);

    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("background-response"), "{stdout}");
    assert!(stdout.contains("/help"), "{stdout}");
    std::fs::remove_dir_all(dir).ok();
}
