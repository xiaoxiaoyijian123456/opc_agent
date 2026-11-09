use crate::bus::{Bus, Task};
use crate::config::Config;
use crate::identity::IdentityEngine;
use crate::init::TEAM_DIR;
use crate::llm::{LlmBackend, LlmResponse, Msg};
use crate::redaction::contains_secret;
use crate::safe_write::{atomic_write_private, SafeFileWriter};
use crate::tools::ToolDef;
use crate::ui;
use async_trait::async_trait;
use serde_json::Value;
use std::ffi::OsStr;
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

/// 单个角色的最大工具调用轮数，防止 LLM 陷入反复调用工具的死循环
const MAX_TOOL_ROUNDS: u32 = 30;

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

/// 角色产出物根目录：统一为 `<项目根>/.myopc_agent/.artifacts`。
///
/// team_root 可能有两种形态：
///   - 已指向 `.myopc_agent` 目录（如配置 `team_root = ".myopc_agent"`）
///   - 指向项目根目录（默认 `"."`）
///     两种情况下都保证所有角色的 `.artifacts` 产出物落在项目根下的 `.myopc_agent/.artifacts`。
pub fn artifact_root(team_root: &Path) -> PathBuf {
    if team_root
        .file_name()
        .map(|n| n == OsStr::new(TEAM_DIR))
        .unwrap_or(false)
    {
        team_root.join(".artifacts")
    } else {
        team_root.join(TEAM_DIR).join(".artifacts")
    }
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
        self.workspace
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    }

    pub async fn run(&self, role: &str, task_id: &str) -> String {
        let Some(task) = self.bus.claim(role, task_id) else {
            return format!("No task {task_id} in {role} inbox");
        };
        match self.execute_task(&task).await {
            Ok(execution) => {
                self.bus.complete(role, task_id, &execution.summary);
                execution.summary
            }
            Err(error) => {
                let error_text = format!("ERROR: {error}");
                self.bus.complete(role, task_id, &error_text);
                error_text
            }
        }
    }

    pub async fn execute_task(&self, task: &Task) -> Result<AgentExecution, String> {
        let role = task.to.as_str();
        let task_id = task.id.as_str();
        let team_root = self.team_root();
        let artifact_dir = artifact_root(&team_root);
        let artifact_path = artifact_dir.join(format!("{task_id}_{role}.md"));
        let artifact_rel = artifact_path
            .strip_prefix(std::env::current_dir().unwrap_or_default())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| artifact_path.to_string_lossy().into_owned());

        let system = self.identity.build_system_prompt(role);
        let prd_protocol = if task.msg_type == "requirement" {
            "\n\nPRD response protocol: your final response must contain exactly one `REQUIRES_UI: true` or `REQUIRES_UI: false` line, followed by one `REQUIRES_UI_REASON: <non-empty reason>` line. Do not omit these lines."
        } else {
            ""
        };
        let user_msg = format!(
            "Task ID: {task_id}\nArtifact path: {artifact_rel} (written automatically by the system after you finish)\nIf you mention the artifact in your output, use exactly the path above. Do not invent a different filename or path.\n\nTask: {}\n\n{}{prd_protocol}",
            self.config.redact(&task.title),
            self.config.redact(&task.content)
        );

        let mut result = match self.run_role(role, &system, &user_msg).await {
            Ok(r) => r,
            Err(error) => {
                let error_text = format!("ERROR: {error}");
                self.write_reflection(role, task_id, task, &error_text)?;
                return Err(format!("Agent {role} failed: {error}"));
            }
        };
        result.text = self.config.redact(&result.text);

        std::fs::create_dir_all(&artifact_dir)
            .map_err(|error| format!("Cannot create artifact directory: {error}"))?;
        std::fs::set_permissions(
            &artifact_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .map_err(|error| format!("Cannot protect artifact directory: {error}"))?;
        let mut artifact_content = result.text.clone();
        if !result.files.is_empty() {
            let file_list: Vec<String> = result.files.iter().map(|f| format!("- {f}")).collect();
            artifact_content.push_str(&format!("\n\n## Files Written\n{}", file_list.join("\n")));
        }
        atomic_write_private(&artifact_path, artifact_content.as_bytes())?;

        let summary: String = result.text.chars().take(200).collect();
        let summary = if result.text.chars().count() > 200 {
            format!("{summary}...")
        } else {
            summary
        };
        if result.text.starts_with("ERROR") {
            self.write_reflection(role, task_id, task, &result.text)?;
        }
        Ok(AgentExecution {
            text: result.text,
            summary,
            artifact_path: artifact_rel,
        })
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
                    &[
                        Msg::System(system.to_string()),
                        Msg::User(user_msg.to_string()),
                    ],
                    &[],
                    &self.config,
                )
                .await?;
            return Ok(RunResult {
                text: resp.text,
                files: Vec::new(),
            });
        }
        self.run_with_tools(system, user_msg, role, &tool_names)
            .await
    }

    async fn run_with_tools(
        &self,
        system: &str,
        user_msg: &str,
        role: &str,
        tool_names: &[&'static str],
    ) -> Result<RunResult, String> {
        let mut messages: Vec<Msg> = vec![
            Msg::System(system.to_string()),
            Msg::User(user_msg.to_string()),
        ];
        let mut written_files: Vec<String> = Vec::new();
        let tools: Vec<ToolDef> = tool_names.iter().map(|&n| agent_tool_def(n)).collect();

        let mut rounds: u32 = 0;
        loop {
            rounds += 1;
            if rounds > MAX_TOOL_ROUNDS {
                return Err(format!(
                    "Agent {role} exceeded max tool rounds ({MAX_TOOL_ROUNDS}) — likely stuck in a tool loop. Interrupted."
                ));
            }
            let resp: LlmResponse = self.llm.complete(&messages, &tools, &self.config).await?;
            if resp.tool_calls.is_empty() {
                return Ok(RunResult {
                    text: resp.text,
                    files: written_files,
                });
            }
            messages.push(Msg::AssistantToolCalls(resp.tool_calls.clone()));
            for tc in resp.tool_calls {
                let result = self.handle_tool(&tc.name, &tc.arguments, role).await;
                if let Some(display) = result.written_file {
                    written_files.push(display);
                }
                messages.push(Msg::ToolResult {
                    id: tc.id,
                    content: result.message,
                });
            }
        }
    }

    async fn handle_tool(&self, name: &str, args: &Value, role: &str) -> ToolHandling {
        if name == "write_file" {
            let Some(content) = args.get("content").and_then(|value| value.as_str()) else {
                return ToolHandling::error("write_file requires a string content field");
            };
            let Some(raw_path) = args.get("path").and_then(|value| value.as_str()) else {
                return ToolHandling::error("write_file requires a string path field");
            };
            if contains_secret(content, &self.config.redactions()) {
                return ToolHandling::error("Refusing to persist the configured API key");
            }
            match self
                .file_writer()
                .and_then(|writer| writer.write(raw_path, content))
            {
                Ok(outcome) => {
                    ui::print_file_written(role, &outcome.display);
                    ToolHandling {
                        message: format!("Written: {} ({} bytes)", outcome.display, content.len()),
                        written_file: Some(outcome.display),
                    }
                }
                Err(error) => ToolHandling::error(&format!(
                    "Invalid write: {error}. Must stay inside the project code root."
                )),
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
                                o.get("label")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                o.get("value")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            ui::print_deepthink_question(role, &self.config.redact(question));
            let message = if options.is_empty() {
                ui::prompt_user_answer()
            } else {
                ui::prompt_user_select(&options)
            };
            ToolHandling {
                message,
                written_file: None,
            }
        } else {
            ToolHandling::error(&format!("Unknown tool: {name}"))
        }
    }

    fn file_writer(&self) -> Result<SafeFileWriter, String> {
        let code_base = match &self.code_root {
            Some(root) => root.clone(),
            None => self.workspace.join("code"),
        };
        SafeFileWriter::new(&code_base)
    }

    pub fn resolve_write_target(&self, raw: &str) -> Option<(PathBuf, String)> {
        self.file_writer().ok()?.resolve(raw).ok()
    }

    fn write_reflection(
        &self,
        role: &str,
        task_id: &str,
        task: &Task,
        result_text: &str,
    ) -> Result<(), String> {
        let redacted = self.config.redact(result_text);
        let summary: String = redacted.chars().take(150).collect();
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
        std::fs::create_dir_all(&archive_dir)
            .map_err(|error| format!("Cannot create reflection directory: {error}"))?;
        std::fs::set_permissions(
            &archive_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .map_err(|error| format!("Cannot protect reflection directory: {error}"))?;
        let file = archive_dir.join(format!("{task_id}_{role}_reflection.json"));
        let content = serde_json::to_vec_pretty(&reflection)
            .map_err(|error| format!("Cannot serialize reflection: {error}"))?;
        atomic_write_private(&file, &content)
    }
}

struct ToolHandling {
    message: String,
    written_file: Option<String>,
}

impl ToolHandling {
    fn error(message: &str) -> Self {
        Self {
            message: format!("ERROR: {message}"),
            written_file: None,
        }
    }
}

struct RunResult {
    text: String,
    files: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AgentExecution {
    pub text: String,
    pub summary: String,
    pub artifact_path: String,
}

fn agent_tool_def(name: &str) -> ToolDef {
    match name {
        "write_file" => ToolDef {
            name: "write_file".into(),
            description: "Write a file at the code root. Path must be relative to the current directory (code root) directly, e.g. \"cmd/server/main.go\". Do NOT create a project-name wrapper folder (like \"gin-scaffold/\"), and do not use \"workspace/\" or leading \"/\" prefixes.".into(),
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
            description: "Ask the user a question. ALWAYS provide 2-4 options and put the RECOMMENDED choice FIRST — it is the default when the user presses Enter.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "Your question" },
                    "options": {
                        "type": "array",
                        "description": "Selectable choices (2-4 items). Put the RECOMMENDED one first. Each: {label, value}.",
                        "items": {
                            "type": "object",
                            "properties": { "label": { "type": "string" }, "value": { "type": "string" } }
                        }
                    }
                },
                "required": ["question", "options"]
            }),
        },
    }
}
