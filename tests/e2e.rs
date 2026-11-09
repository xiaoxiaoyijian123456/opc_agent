use myopc::agent_runner::AgentRunner;
use myopc::bus::{Bus, DispatchOpts};
use myopc::config::Config;
use myopc::identity::IdentityEngine;
use myopc::init::init_project;
use myopc::llm::{LlmBackend, LlmResponse, Msg, ToolCallMsg};
use myopc::tools::ToolDef;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct MockLlm {
    responses: Arc<Mutex<VecDeque<LlmResponse>>>,
}

#[async_trait::async_trait]
impl LlmBackend for MockLlm {
    async fn complete(
        &self,
        _messages: &[Msg],
        _tools: &[ToolDef],
        _cfg: &Config,
    ) -> Result<LlmResponse, String> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "no more scripted responses".to_string())
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "myopc-rust-{}-{}",
        tag,
        uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_config(team_root: &std::path::Path) -> Config {
    Config {
        provider: "openai".into(),
        api_key: "mock-key".into(),
        base_url: "http://localhost:0".into(),
        default_model: "mock-model".into(),
        team_root: team_root.to_string_lossy().into_owned(),
        code_root: None,
        max_tokens: 8192,
        temperature: 0.7,
    }
}

#[tokio::test]
async fn bus_lifecycle() {
    let dir = temp_dir("bus");
    let bus = Bus::new(&dir);

    let task = bus.dispatch("pm", "分析需求", "做一个贪吃蛇", DispatchOpts {
        from_role: Some("boss".into()),
        msg_type: Some("requirement".into()),
        context: Some(json!({ "depends_on": ["x"] })),
    });
    assert_eq!(task.status, "pending");
    assert!(dir.join("communication/bus/pm/inbox").join(format!("{}.json", task.id)).exists());

    let claimed = bus.claim("pm", &task.id).unwrap();
    assert_eq!(claimed.status, "active");
    assert!(!dir.join("communication/bus/pm/inbox").join(format!("{}.json", task.id)).exists());
    assert!(dir.join("communication/bus/pm/active").join(format!("{}.json", task.id)).exists());

    let completed = bus.complete("pm", &task.id, "PRD 完成").unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.result.as_deref(), Some("PRD 完成"));
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(dir.join("memory_center/archive").join(&today).join(format!("{}.json", task.id)).exists());
    assert!(!dir.join("communication/bus/pm/active").join(format!("{}.json", task.id)).exists());

    let status = bus.get_all_status();
    assert_eq!(status["pm"].state, "idle");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn identity_system_prompt() {
    let dir = temp_dir("identity");
    init_project(&dir);

    let identity = IdentityEngine::new(&dir.join(".myopc_agent"));
    let prompt = identity.build_system_prompt("pm");
    assert!(prompt.contains("AGENTS.md") || prompt.contains("PM"));
    assert!(prompt.len() > 100);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn resolve_write_target_mappings() {
    let dir = temp_dir("wtarget");
    let team_root = dir.join(".myopc_agent");
    std::fs::create_dir_all(&team_root).ok();
    let config = test_config(&team_root);
    let bus = Bus::new(&team_root);
    let identity = IdentityEngine::new(&team_root);
    let runner = AgentRunner::new(
        config,
        bus,
        identity,
        team_root.join("workspace"),
        Arc::new(MockLlm { responses: Arc::new(Mutex::new(VecDeque::new())) }),
    );
    let code_root = dir.join("code");
    let runner = runner_set_code_root(runner, code_root.clone());

    let case = |raw: &str| runner.resolve_write_target(raw);

    let (full, display) = case("src/main.go").unwrap();
    assert_eq!(full, code_root.join("src/main.go"));
    assert_eq!(display, "src/main.go");

    let (full, display) = case("workspace/foo/bar.txt").unwrap();
    assert_eq!(full, code_root.join("foo/bar.txt"));
    assert_eq!(display, "foo/bar.txt");

    let (full, display) = case("code/a.py").unwrap();
    assert_eq!(full, code_root.join("a.py"));
    assert_eq!(display, "a.py");

    let (full, display) = case("artifacts/x.md").unwrap();
    assert_eq!(full, dir.join(".myopc_agent/.artifacts/x.md"));
    assert_eq!(display, "x.md");

    assert!(case("../escape").is_none());
    assert!(case("/abs/path").is_none());
    assert!(case("").is_none());

    std::fs::remove_dir_all(&dir).ok();
}

// Helper: AgentRunner field setters (pub fields)
fn runner_set_code_root(mut r: AgentRunner, cr: PathBuf) -> AgentRunner {
    r.code_root = Some(cr);
    r
}

#[test]
fn artifact_root_always_under_myopc_agent() {
    let dir = temp_dir("artifact_root");

    // 形态 1：team_root 已指向 .myopc_agent 目录（配置 team_root = ".myopc_agent"）
    let team_root = dir.join(".myopc_agent");
    std::fs::create_dir_all(&team_root).ok();
    assert_eq!(
        myopc::agent_runner::artifact_root(&team_root),
        dir.join(".myopc_agent/.artifacts")
    );

    // 形态 2：team_root 指向项目根目录（默认 "."），仍须产出到 .myopc_agent/.artifacts
    assert_eq!(
        myopc::agent_runner::artifact_root(&dir),
        dir.join(".myopc_agent/.artifacts")
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn agent_run_writes_code_and_artifact() {
    let dir = temp_dir("run");
    init_project(&dir);
    let team_root = dir.join(".myopc_agent");
    let code_root = dir.join("code");

    let responses: Arc<Mutex<VecDeque<LlmResponse>>> = Arc::new(Mutex::new(VecDeque::from([
        LlmResponse {
            text: String::new(),
            tool_calls: vec![ToolCallMsg {
                id: "call_1".into(),
                name: "write_file".into(),
                arguments: json!({ "path": "src/app.go", "content": "package main\n\nfunc main() {}\n" }),
            }],
        },
        LlmResponse { text: "开发完成".into(), tool_calls: vec![] },
    ])));

    let config = test_config(&team_root);
    let bus = Bus::new(&team_root);
    let identity = IdentityEngine::new(&team_root);
    let mut runner = AgentRunner::new(
        config,
        bus.clone(),
        identity,
        team_root.join("workspace"),
        Arc::new(MockLlm { responses: responses.clone() }),
    );
    runner.code_root = Some(code_root.clone());

    let task = bus.dispatch("dev", "实现贪吃蛇", "写一个 Go 程序", DispatchOpts {
        from_role: Some("boss".into()),
        msg_type: Some("implement".into()),
        context: None,
    });

    let summary = runner.run("dev", &task.id).await;
    assert!(summary.contains("开发完成"), "summary: {summary}");

    let file = code_root.join("src/app.go");
    assert!(file.exists(), "code file should be written");
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("package main"));

    let artifact = team_root.join(".artifacts").join(format!("{}_dev.md", task.id));
    assert!(artifact.exists(), "artifact should be written");
    let artifact_content = std::fs::read_to_string(&artifact).unwrap();
    assert!(artifact_content.contains("开发完成"));
    assert!(artifact_content.contains("## Files Written"));
    assert!(artifact_content.contains("src/app.go"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn init_creates_myopc_agent_layout() {
    let dir = temp_dir("init");
    init_project(&dir);

    assert!(dir.join(".myopc_agent").exists());
    assert!(dir.join(".myopc_agent/AGENTS.md").exists());
    assert!(!dir.join(".myopc_agent/myopc.json").exists(), "init 不生成项目内配置，使用全局 ~/.myopc/myopc.json");
    assert!(dir.join(".myopc_agent/.artifacts").exists());
    assert!(dir.join(".myopc_agent/workspace").exists());
    assert!(dir.join(".myopc_agent/skills").exists(), "init 创建 skills 目录，供用户添加 skill 插件");
    assert!(dir.join(".myopc_agent/communication/bus/pm/inbox").exists());

    std::fs::remove_dir_all(&dir).ok();
}
