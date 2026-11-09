use crate::bus::{Bus, Task};
use crate::config::Config;
use crate::identity::IdentityEngine;
use crate::llm::{LlmBackend, LlmResponse, Msg};
use crate::tools::ToolDef;
use crate::ui;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, name: &str, args: &Value) -> String;
}

pub fn role_tools(role: &str) -> Vec<&'static str> {
    match role {
        "pm" => vec!["ask_user"],
        "dev" => vec!["write_file"],
        "ui" => vec!["write_file"],
        _ => vec![],
    }
}

pub fn has_sync_tool(role: &str) -> bool {
    role_tools(role).contains(&"ask_user")
}

#[derive(Clone)]
pub struct AgentRunner {
    config: Config,
    bus: Bus,
    identity: IdentityEngine,
    workspace: PathBuf,
    llm: Arc<dyn LlmBackend>,
    pub code_root: Option<PathBuf>,
}

pub fn normalize_path(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for comp in absolute.components() {
        match comp {
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::Normal(c) => out.push(c),
            Component::Prefix(c) => out.push(c.as_os_str()),
        }
    }
    out
}

impl AgentRunner {
    pub fn new(
        config: Config,
        bus: Bus,
        identity: IdentityEngine,
        workspace: PathBuf,
        llm: Arc<dyn LlmBackend>,
    ) -> AgentRunner {
        AgentRunner {
            config,
            bus,
            identity,
            workspace,
            llm,
            code_root: None,
        }
    }

    fn team_root(&self) -> PathBuf {
        self.workspace.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    }

    pub async fn run(&self, role: &str, task_id: &str) -> String {
        let Some(task) = self.bus.claim(role, task_id) else {
            return format!("No task {task_id} in {role} inbox");
        };

        let team_root = self.team_root();
        let artifact_dir = team_root.join(".artifacts");
        let artifact_path = artifact_dir.join(format!("{task_id}_{role}.md"));
        let artifact_rel = artifact_path
            .strip_prefix(&std::env::current_dir().unwrap_or_default())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| artifact_path.to_string_lossy().into_owned());

        let system = self.identity.build_system_prompt(role);
        let user_msg = format!(
            "Task ID: {task_id}\nArtifact path: {artifact_rel} (written automatically by the system after you finish)\nIf you mention the artifact in your output, use exactly the path above. Do not invent a different filename or path.\n\nTask: {}\n\n{}",
            task.title, task.content
        );

        let result = match self.run_role(role, &system, &user_msg).await {
            Ok(r) => r,
            Err(e) => {
                let error_text = format!("ERROR: {e}");
                self.bus.complete(role, task_id, &error_text);
                self.write_reflection(role, task_id, &task, &error_text);
                return format!("Agent {role} failed: {e}");
            }
        };

        std::fs::create_dir_all(&artifact_dir).ok();
        let mut artifact_content = result.text.clone();
        if !result.files.is_empty() {
            let file_list: Vec<String> = result.files.iter().map(|f| format!("- {f}")).collect();
            artifact_content.push_str(&format!("\n\n## Files Written\n{}", file_list.join("\n")));
        }
        std::fs::write(&artifact_path, artifact_content).ok();

        let summary: String = result.text.chars().take(200).collect();
        let summary = if result.text.chars().count() > 200 {
            format!("{summary}...")
        } else {
            summary
        };
        self.bus.complete(role, task_id, &summary);
        if result.text.starts_with("ERROR") {
            self.write_reflection(role, task_id, &task, &result.text);
        }
        summary
    }

    async fn run_role(
        &self,
        role: &str,
        system: &str,
        user_msg: &str,
    ) -> Result<RunResult, String> {
        let tool_names = role_tools(role);
        if tool_names.is_empty() {
            let resp = self
                .llm
                .complete(
                    &[Msg::System(system.to_string()), Msg::User(user_msg.to_string())],
                    &[],
                    &self.config,
                )
                .await?;
            return Ok(RunResult { text: resp.text, files: Vec::new() });
        }
        self.run_with_tools(system, user_msg, role, &tool_names).await
    }

    async fn run_with_tools(
        &self,
        system: &str,
        user_msg: &str,
        role: &str,
        tool_names: &[&'static str],
    ) -> Result<RunResult, String> {
        let mut messages: Vec<Msg> =
            vec![Msg::System(system.to_string()), Msg::User(user_msg.to_string())];
        let mut written_files: Vec<String> = Vec::new();
        let tools: Vec<ToolDef> = tool_names
            .iter()
            .map(|&n| agent_tool_def(n))
            .collect();

        loop {
            let resp: LlmResponse = self.llm.complete(&messages, &tools, &self.config).await?;
            if resp.tool_calls.is_empty() {
                return Ok(RunResult { text: resp.text, files: written_files });
            }
            messages.push(Msg::AssistantToolCalls(resp.tool_calls.clone()));
            for tc in resp.tool_calls {
                let result = self.handle_tool(&tc.name, &tc.arguments, role).await;
                if tc.name == "write_file" {
                    if let Some((_, display)) = self.resolve_write_target(
                        tc.arguments.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    ) {
                        written_files.push(display);
                    }
                }
                messages.push(Msg::ToolResult { id: tc.id, content: result });
            }
        }
    }

    async fn handle_tool(&self, name: &str, args: &Value, role: &str) -> String {
        if name == "write_file" {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            match self.resolve_write_target(raw_path) {
                Some((full, display)) => {
                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    std::fs::write(&full, content).ok();
                    ui::print_file_written(role, &display);
                    format!("Written: {display} ({} bytes)", content.len())
                }
                None => format!("Invalid path: {raw_path}. Must stay inside the project code root."),
            }
        } else if name == "ask_user" {
            let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
            let options: Vec<(String, String)> = args
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|o| {
                            (
                                o.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                o.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            ui::print_deepthink_question(role, question);
            if options.is_empty() {
                ui::prompt_user_answer()
            } else {
                ui::prompt_user_select(&options)
            }
        } else {
            format!("Unknown tool: {name}")
        }
    }

    pub fn resolve_write_target(&self, raw: &str) -> Option<(PathBuf, String)> {
        let code_base = match &self.code_root {
            Some(cr) => cr.clone(),
            None => self.workspace.join("code"),
        };

        let mut rel = raw.replace('\\', "/").trim().to_string();
        if let Some(s) = rel.strip_prefix("./") {
            rel = s.to_string();
        }
        if let Some(s) = rel.strip_prefix("workspace/") {
            rel = s.to_string();
        }

        let mut base = code_base;
        if rel.starts_with("artifacts/") {
            base = self.team_root().join(".artifacts");
            rel = rel["artifacts/".len()..].to_string();
        } else if rel.starts_with("code/") {
            rel = rel["code/".len()..].to_string();
        }

        if rel.is_empty() || rel.starts_with('/') {
            return None;
        }

        let base_resolved = normalize_path(&base);
        let full = normalize_path(&base_resolved.join(&rel));
        if full != base_resolved && !full.starts_with(&base_resolved) {
            return None;
        }
        let display = full
            .strip_prefix(&base_resolved)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| rel.clone());
        Some((full, display))
    }

    fn write_reflection(&self, role: &str, task_id: &str, task: &Task, result_text: &str) {
        let summary: String = result_text.chars().take(150).collect();
        let reflection = serde_json::json!({
            "task_id": task_id,
            "role": role,
            "title": task.title,
            "type": task.msg_type,
            "success": false,
            "summary": summary,
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        let date_str = chrono::Utc::now().format("%Y-%m-%d");
        let archive_dir = self
            .team_root()
            .join("memory_center")
            .join("archive")
            .join(date_str.to_string());
        std::fs::create_dir_all(&archive_dir).ok();
        let file = archive_dir.join(format!("{task_id}_{role}_reflection.json"));
        std::fs::write(file, serde_json::to_string_pretty(&reflection).unwrap()).ok();
    }
}

struct RunResult {
    text: String,
    files: Vec<String>,
}

fn agent_tool_def(name: &str) -> ToolDef {
    match name {
        "write_file" => ToolDef {
            name: "write_file".into(),
            description: "Write a file under the project code root. Path must be relative to the code root (e.g. \"cmd/server/main.go\"), without any \"workspace/\", or leading \"/\" prefix.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the project code root, e.g. \"internal/handler/user.go\"" },
                    "content": { "type": "string", "description": "Full file content" }
                },
                "required": ["path", "content"]
            }),
        },
        _ => ToolDef {
            name: "ask_user".into(),
            description: "Ask the user a question. Provide options for common choices.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "Your question" },
                    "options": {
                        "type": "array",
                        "description": "Selectable choices. Each: {label, value}. Omit for free-text.",
                        "items": {
                            "type": "object",
                            "properties": { "label": { "type": "string" }, "value": { "type": "string" } }
                        }
                    }
                },
                "required": ["question"]
            }),
        },
    }
}
