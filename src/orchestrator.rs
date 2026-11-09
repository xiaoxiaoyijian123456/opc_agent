use crate::agent_runner::AgentRunner;
use crate::redaction::redact_text;
use crate::task::Workflow;
use crate::task_store::TaskStore;
use crate::ui;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum PrdDecision {
    Approve,
    Revise(String),
    Terminate,
}

#[derive(Clone, Debug)]
pub struct PrdReview {
    pub workflow_id: String,
    pub artifact_path: String,
    pub requires_ui: bool,
    pub reason: String,
    pub template: Vec<String>,
}

#[async_trait]
pub trait PrdApproval: Send + Sync {
    async fn review(&self, review: PrdReview) -> PrdDecision;
}

#[derive(Clone)]
pub struct CliPrdApproval {
    redactions: Vec<String>,
}

impl CliPrdApproval {
    pub fn new(redactions: Vec<String>) -> Self {
        Self { redactions }
    }
}

#[async_trait]
impl PrdApproval for CliPrdApproval {
    async fn review(&self, review: PrdReview) -> PrdDecision {
        println!("\nPRD ready: {}", review.artifact_path);
        println!("Workflow: {}", review.template.join(" -> "));
        println!("Requires UI: {} — {}", review.requires_ui, review.reason);
        println!("Choose explicitly: [a]pprove / [r]evise / [t]erminate");
        loop {
            let answer = ui::prompt_user_answer();
            match answer.trim().to_lowercase().as_str() {
                "a" | "approve" => return PrdDecision::Approve,
                "r" | "revise" => loop {
                    println!("Revision feedback (required):");
                    let feedback = ui::prompt_user_answer();
                    if feedback.trim() == "/terminate" {
                        return PrdDecision::Terminate;
                    }
                    if !feedback.trim().is_empty() {
                        return PrdDecision::Revise(redact_text(feedback.trim(), &self.redactions));
                    }
                    println!("Feedback cannot be empty. Enter /terminate to stop instead.");
                },
                "" | "t" | "terminate" => return PrdDecision::Terminate,
                _ => println!("Invalid choice. Enter a, r, or t."),
            }
        }
    }
}

#[derive(Clone)]
pub struct Orchestrator {
    store: TaskStore,
    runner: Arc<Mutex<AgentRunner>>,
    approval: Arc<dyn PrdApproval>,
}

pub fn parse_requires_ui(text: &str) -> Result<(bool, String), String> {
    let mut values = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("REQUIRES_UI:").map(str::trim));
    let value = values.next().ok_or("PRD is missing REQUIRES_UI")?;
    if values.next().is_some() {
        return Err("PRD contains multiple REQUIRES_UI lines".into());
    }
    let requires_ui = match value {
        "true" => true,
        "false" => false,
        _ => return Err("REQUIRES_UI must be true or false".into()),
    };
    let mut reasons = text.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("REQUIRES_UI_REASON:")
            .map(str::trim)
    });
    let reason = reasons
        .next()
        .filter(|reason| !reason.is_empty())
        .ok_or("PRD is missing a non-empty REQUIRES_UI_REASON")?;
    if reasons.next().is_some() {
        return Err("PRD contains multiple REQUIRES_UI_REASON lines".into());
    }
    Ok((requires_ui, reason.to_string()))
}

impl Orchestrator {
    pub fn new(
        store: TaskStore,
        runner: Arc<Mutex<AgentRunner>>,
        approval: Arc<dyn PrdApproval>,
    ) -> Self {
        Self {
            store,
            runner,
            approval,
        }
    }

    pub fn store(&self) -> &TaskStore {
        &self.store
    }

    pub async fn start(&self, request: &str) -> Result<Workflow, String> {
        if request.trim().is_empty() {
            return Err("Workflow request cannot be empty".into());
        }
        let (workflow, _) = self.store.start_standard(request.trim())?;
        if workflow.status == "pending" {
            return Ok(workflow);
        }
        self.drive(&workflow.id).await
    }

    pub async fn resume(&self, workflow_id: &str) -> Result<Workflow, String> {
        let workflow = self.store.workflow(workflow_id)?;
        if workflow.status == "failed" {
            return Err("Failed workflows require /retry <task_id>".into());
        }
        self.drive(workflow_id).await
    }

    pub async fn retry(&self, task_id: &str) -> Result<Workflow, String> {
        let workflow_id = self.store.retry(task_id)?;
        self.drive(&workflow_id).await
    }

    pub fn cancel(&self, workflow_id: &str) -> Result<Workflow, String> {
        self.store.cancel(workflow_id)
    }

    fn next_role(template: &[String], role: &str) -> Option<String> {
        template
            .iter()
            .position(|candidate| candidate == role)
            .and_then(|position| template.get(position + 1))
            .cloned()
    }

    fn downstream_content(
        &self,
        workflow_id: &str,
        extra: Option<(&str, &str)>,
        feedback: Option<&str>,
    ) -> Result<String, String> {
        let workflow = self.store.workflow(workflow_id)?;
        let mut content = format!("Original request:\n{}", workflow.request);
        for task in self.store.completed_tasks(workflow_id)? {
            if let Some(result) = task.result {
                content.push_str(&format!("\n\n[{} output]\n{}", task.role, result));
            }
        }
        if let Some((role, result)) = extra {
            content.push_str(&format!("\n\n[{role} output]\n{result}"));
        }
        if let Some(feedback) = feedback {
            content.push_str(&format!("\n\n[Requested revision]\n{feedback}"));
        }
        Ok(content)
    }

    async fn drive(&self, workflow_id: &str) -> Result<Workflow, String> {
        loop {
            let workflow = self.store.workflow(workflow_id)?;
            match workflow.status.as_str() {
                "completed" | "cancelled" | "failed" => return Ok(workflow),
                "waiting_approval" => {
                    let task_id = workflow
                        .current_task_id
                        .clone()
                        .ok_or("PRD task is missing")?;
                    let task = self.store.task(&task_id)?;
                    let review = PrdReview {
                        workflow_id: workflow.id.clone(),
                        artifact_path: task.artifact_path.unwrap_or_default(),
                        requires_ui: workflow.requires_ui.ok_or("requires_ui is missing")?,
                        reason: workflow.ui_reason.clone().unwrap_or_default(),
                        template: workflow.template.clone(),
                    };
                    match self.approval.review(review).await {
                        PrdDecision::Approve => {
                            let next_role = workflow
                                .template
                                .get(1)
                                .cloned()
                                .ok_or("Approved workflow has no downstream role")?;
                            let content = self.downstream_content(&workflow.id, None, None)?;
                            self.store.approve(&workflow.id, &next_role, &content)?;
                        }
                        PrdDecision::Revise(feedback) => {
                            let content =
                                self.downstream_content(&workflow.id, None, Some(&feedback))?;
                            self.store.revise(&workflow.id, &content)?;
                        }
                        PrdDecision::Terminate => {
                            return self.store.cancel(&workflow.id);
                        }
                    }
                }
                "active" | "pending" => {
                    let task_id = workflow
                        .current_task_id
                        .clone()
                        .ok_or("Active workflow has no current task")?;
                    let task = self.store.task(&task_id)?;
                    let attempt = self.store.begin_attempt(&task.id)?;
                    ui::print_agent_working(&task.role, &task.title);
                    let execution = {
                        let runner = self.runner.lock().await;
                        runner.execute_task(&task.runner_task()).await
                    };
                    let execution = match execution {
                        Ok(execution) => execution,
                        Err(error) => {
                            self.store.fail(&attempt, &error)?;
                            continue;
                        }
                    };
                    ui::print_agent_done(&task.role, &execution.summary);
                    if task.role == "pm" && task.msg_type == "requirement" {
                        match parse_requires_ui(&execution.text) {
                            Ok((requires_ui, reason)) => self.store.finish_pm(
                                &attempt,
                                &execution.text,
                                &execution.artifact_path,
                                requires_ui,
                                &reason,
                            )?,
                            Err(error) => self.store.fail(&attempt, &error)?,
                        }
                        continue;
                    }
                    let next = Self::next_role(&workflow.template, &task.role)
                        .map(|role| {
                            let content = self.downstream_content(
                                &workflow.id,
                                Some((&task.role, &execution.text)),
                                None,
                            )?;
                            Ok::<_, String>((role, content))
                        })
                        .transpose()?;
                    self.store.finish_role(
                        &attempt,
                        &execution.text,
                        &execution.artifact_path,
                        next.as_ref()
                            .map(|(role, content)| (role.as_str(), content.as_str())),
                    )?;
                }
                status => return Err(format!("Unknown workflow status: {status}")),
            }
        }
    }
}
