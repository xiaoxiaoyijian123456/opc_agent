use async_trait::async_trait;
use myopc::agent_runner::AgentRunner;
use myopc::bus::Bus;
use myopc::config::Config;
use myopc::identity::IdentityEngine;
use myopc::init::init_project;
use myopc::llm::{LlmBackend, LlmResponse, Msg, ToolCallMsg};
use myopc::orchestrator::{parse_requires_ui, Orchestrator, PrdApproval, PrdDecision, PrdReview};
use myopc::task_store::TaskStore;
use myopc::tools::ToolDef;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct MockLlm {
    responses: Arc<Mutex<VecDeque<LlmResponse>>>,
}

#[async_trait]
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
            .ok_or_else(|| "no more scripted responses".into())
    }
}

struct ScriptedApproval {
    decisions: Mutex<VecDeque<PrdDecision>>,
}

#[async_trait]
impl PrdApproval for ScriptedApproval {
    async fn review(&self, _review: PrdReview) -> PrdDecision {
        self.decisions.lock().unwrap().pop_front().unwrap()
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "myopc-orchestrator-{tag}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn response(text: &str) -> LlmResponse {
    LlmResponse {
        text: text.into(),
        tool_calls: Vec::<ToolCallMsg>::new(),
    }
}

fn prd(requires_ui: bool) -> LlmResponse {
    response(&format!(
        "PRD contents\nREQUIRES_UI: {requires_ui}\nREQUIRES_UI_REASON: scripted reason"
    ))
}

fn harness(
    dir: &Path,
    responses: Vec<LlmResponse>,
    decisions: Vec<PrdDecision>,
) -> (Orchestrator, TaskStore) {
    init_project(dir);
    let team_root = dir.join(".myopc_agent");
    let config = Config {
        provider: "openai".into(),
        api_key: "mock-key".into(),
        base_url: "http://localhost:0".into(),
        default_model: "mock-model".into(),
        team_root: team_root.to_string_lossy().into_owned(),
        code_root: Some(dir.join("code").to_string_lossy().into_owned()),
        max_tokens: 8192,
        temperature: 0.7,
    };
    let llm = Arc::new(MockLlm {
        responses: Arc::new(Mutex::new(VecDeque::from(responses))),
    });
    let runner = AgentRunner::new(
        config,
        Bus::new(&team_root),
        IdentityEngine::new(&team_root),
        team_root.join("workspace"),
        llm,
    );
    let store = TaskStore::open(&team_root).unwrap();
    let orchestrator = Orchestrator::new(
        store.clone(),
        Arc::new(tokio::sync::Mutex::new(runner)),
        Arc::new(ScriptedApproval {
            decisions: Mutex::new(VecDeque::from(decisions)),
        }),
    );
    (orchestrator, store)
}

#[tokio::test]
async fn standard_workflow_runs_all_roles_in_order() {
    let dir = temp_dir("standard");
    let (orchestrator, store) = harness(
        &dir,
        vec![
            prd(false),
            response("implemented"),
            response("tested"),
            response("summarized"),
        ],
        vec![PrdDecision::Approve],
    );

    let workflow = orchestrator.start("build a CLI").await.unwrap();
    assert_eq!(workflow.status, "completed");
    let tasks = store.tasks_for_workflow(&workflow.id).unwrap();
    let roles = tasks
        .iter()
        .map(|task| task.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["pm", "dev", "tester", "admin"]);
    assert!(tasks.iter().all(|task| task.status == "completed"));
    assert_eq!(tasks[1].depends_on, [tasks[0].id.clone()]);
    assert!(tasks[2].content.contains("PRD contents"));
    assert!(tasks[2].content.contains("implemented"));
    for task in tasks {
        assert_eq!(store.attempt_count(&task.id).unwrap(), 1);
    }

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn visual_workflow_inserts_ui_before_dev() {
    let dir = temp_dir("visual");
    let (orchestrator, store) = harness(
        &dir,
        vec![
            prd(true),
            response("design"),
            response("implemented"),
            response("tested"),
            response("summarized"),
        ],
        vec![PrdDecision::Approve],
    );

    let workflow = orchestrator.start("build a web UI").await.unwrap();
    let tasks = store.tasks_for_workflow(&workflow.id).unwrap();
    let roles = tasks
        .iter()
        .map(|task| task.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["pm", "ui", "dev", "tester", "admin"]);
    assert_eq!(workflow.status, "completed");

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn revision_returns_to_pm_before_approval() {
    let dir = temp_dir("revision");
    let (orchestrator, store) = harness(
        &dir,
        vec![
            prd(false),
            prd(true),
            response("design"),
            response("implemented"),
            response("tested"),
            response("summarized"),
        ],
        vec![
            PrdDecision::Revise("include mobile".into()),
            PrdDecision::Approve,
        ],
    );

    let workflow = orchestrator.start("build an interface").await.unwrap();
    let tasks = store.tasks_for_workflow(&workflow.id).unwrap();
    assert_eq!(tasks.len(), 6);
    assert!(tasks[1].content.contains("include mobile"));
    assert_eq!(workflow.status, "completed");

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn terminating_prd_creates_no_downstream_tasks() {
    let dir = temp_dir("terminate");
    let (orchestrator, store) = harness(&dir, vec![prd(false)], vec![PrdDecision::Terminate]);

    let workflow = orchestrator.start("stop after PRD").await.unwrap();
    assert_eq!(workflow.status, "cancelled");
    assert_eq!(store.tasks_for_workflow(&workflow.id).unwrap().len(), 1);

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn invalid_prd_protocol_fails_without_downstream_tasks() {
    let dir = temp_dir("invalid-prd");
    let (orchestrator, store) = harness(&dir, vec![response("unstructured PRD")], vec![]);

    let workflow = orchestrator.start("invalid PRD").await.unwrap();
    assert_eq!(workflow.status, "failed");
    let tasks = store.tasks_for_workflow(&workflow.id).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "failed");
    assert!(tasks[0].error.as_deref().unwrap().contains("REQUIRES_UI"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn retry_and_crash_recovery_preserve_attempt_history() {
    let dir = temp_dir("recovery");
    let team_root = dir.join(".myopc_agent");
    std::fs::create_dir_all(&team_root).unwrap();
    let store = TaskStore::open(&team_root).unwrap();
    let (workflow, task) = store.start_standard("recover this").unwrap();
    let first = store.begin_attempt(&task.id).unwrap();
    store.fail(&first, "simulated failure").unwrap();
    let workflow_id = store.retry(&task.id).unwrap();
    assert_eq!(workflow_id, workflow.id);
    let second = store.begin_attempt(&task.id).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.number, 2);

    assert_eq!(store.recover_active().unwrap(), 1);
    assert_eq!(store.task(&task.id).unwrap().status, "failed");
    assert_eq!(store.workflow(&workflow.id).unwrap().status, "failed");
    assert_eq!(store.attempt_count(&task.id).unwrap(), 2);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn waiting_approval_survives_recovery() {
    let dir = temp_dir("waiting");
    let team_root = dir.join(".myopc_agent");
    std::fs::create_dir_all(&team_root).unwrap();
    let store = TaskStore::open(&team_root).unwrap();
    let (workflow, task) = store.start_standard("wait for approval").unwrap();
    let attempt = store.begin_attempt(&task.id).unwrap();
    store
        .finish_pm(&attempt, "PRD", ".artifacts/prd.md", false, "CLI only")
        .unwrap();

    assert_eq!(store.recover_active().unwrap(), 0);
    assert_eq!(
        store.workflow(&workflow.id).unwrap().status,
        "waiting_approval"
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn additional_workflows_wait_and_never_run_concurrently() {
    let dir = temp_dir("queued");
    let team_root = dir.join(".myopc_agent");
    std::fs::create_dir_all(&team_root).unwrap();
    let store = TaskStore::open(&team_root).unwrap();
    let (first_workflow, first_task) = store.start_standard("first").unwrap();
    let (second_workflow, second_task) = store.start_standard("second").unwrap();
    assert_eq!(first_workflow.status, "active");
    assert_eq!(second_workflow.status, "pending");

    let _attempt = store.begin_attempt(&first_task.id).unwrap();
    assert!(store.begin_attempt(&second_task.id).is_err());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn prd_protocol_parser_rejects_ambiguity() {
    assert_eq!(
        parse_requires_ui("REQUIRES_UI: true\nREQUIRES_UI_REASON: visual").unwrap(),
        (true, "visual".into())
    );
    assert!(parse_requires_ui("REQUIRES_UI: maybe\nREQUIRES_UI_REASON: unknown").is_err());
    assert!(parse_requires_ui("REQUIRES_UI: false").is_err());
}
