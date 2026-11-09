use crate::agent_runner::{artifact_root, normalize_path, AgentRunner, ToolExecutor};
use crate::brain::Brain;
use crate::bus::{Bus, DispatchOpts};
use crate::config::{team_root_path, validate_config, Config};
use crate::identity::IdentityEngine;
use crate::llm::{HttpLlm, LlmBackend};
use crate::skill_loader::SkillLoader;
use crate::tools;
use crate::ui;
use colored::Colorize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone)]
struct DoneTask {
    role: String,
    title: String,
    summary: String,
    artifact: String,
}

#[derive(Clone)]
pub struct MYOPCApp {
    config: Config,
    bus: Bus,
    identity: IdentityEngine,
    skills: SkillLoader,
    runner: Arc<Mutex<AgentRunner>>,
    brain: Arc<Mutex<Brain>>,
    done_queue: Arc<Mutex<Vec<DoneTask>>>,
}

fn run_with_timeout(mut child: std::process::Child, timeout: Duration) -> (i32, String, String) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut o, &mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return (st.code().unwrap_or(1), out, err);
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return (1, String::new(), "Timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return (1, String::new(), e.to_string()),
        }
    }
}

impl MYOPCApp {
    pub fn new(config: Config) -> MYOPCApp {
        let root = team_root_path(&config);
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).ok();

        let bus = Bus::new(&root);
        let identity = IdentityEngine::new(&root);
        let skills = SkillLoader::new(&root.join("skills"));
        let all_tools = tools::merge_tools(tools::tool_defs(), skills.scan());
        let llm: Arc<dyn LlmBackend> = Arc::new(HttpLlm::new());
        let brain = Brain::new(
            config.clone(),
            llm.clone(),
            Some(identity.clone()),
            all_tools,
        );
        let mut runner = AgentRunner::new(
            config.clone(),
            bus.clone(),
            identity.clone(),
            workspace,
            llm.clone(),
        );
        let code_root = normalize_path(&PathBuf::from(
            config.code_root.clone().unwrap_or_else(|| ".".into()),
        ));
        std::fs::create_dir_all(&code_root).ok();
        runner.code_root = Some(code_root);

        let errors = validate_config(&config);
        ui::print_welcome(
            errors.is_empty(),
            &config.provider,
            &config.default_model,
            &root.to_string_lossy(),
            config.max_tokens,
            config.temperature,
        );
        for error in &errors {
            ui::print_error(&format!("  ⚠ {error}"));
        }

        MYOPCApp {
            config,
            bus,
            identity,
            skills,
            runner: Arc::new(Mutex::new(runner)),
            brain: Arc::new(Mutex::new(brain)),
            done_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn execute_tool(&self, name: &str, args: &Value) -> String {
        let root = team_root_path(&self.config);
        match name {
            "dispatch_task" => {
                let state = self.identity.read_intervention();
                if state == "STOP" {
                    ui::print_intervention_block("STOP");
                    return "BLOCKED: Intervention state is STOP. All dispatches halted.".into();
                }
                if state == "PAUSE" {
                    ui::print_intervention_block("PAUSE");
                    ui::print_deepthink_question(
                        "brain",
                        &format!(
                            "Dispatch to {}: {} — approve? (y/n)",
                            args["role"].as_str().unwrap_or(""),
                            args["title"].as_str().unwrap_or("")
                        ),
                    );
                    let answer = ui::prompt_user_answer().to_lowercase();
                    if !["y", "yes", "ok", "approve"].contains(&answer.as_str()) {
                        return format!(
                            "BLOCKED: User did not approve dispatch. Response: {answer}"
                        );
                    }
                }

                let mut ctx = serde_json::Map::new();
                if let Some(d) = args.get("depends_on") {
                    ctx.insert("depends_on".into(), d.clone());
                }
                if let Some(a) = args.get("artifacts") {
                    ctx.insert("artifacts".into(), a.clone());
                }
                let context = if ctx.is_empty() {
                    None
                } else {
                    Some(Value::Object(ctx))
                };
                let role = args["role"].as_str().unwrap_or("").to_string();
                let title = args["title"].as_str().unwrap_or("").to_string();
                let content = args["content"].as_str().unwrap_or("").to_string();
                let msg_type = args["msg_type"].as_str().unwrap_or("").to_string();
                let task = self.bus.dispatch(
                    &role,
                    &title,
                    &content,
                    DispatchOpts {
                        from_role: None,
                        msg_type: Some(msg_type),
                        context,
                    },
                );
                ui::print_agent_working(&role, &title);
                serde_json::to_string(&task).unwrap_or_default()
            }

            "get_project_status" => {
                let status = self.bus.get_all_status();
                ui::print_status(&status);
                serde_json::to_string(&status).unwrap_or_default()
            }

            "execute_role_task" => {
                let role = args["role"].as_str().unwrap_or("").to_string();
                let task_id = args["task_id"].as_str().unwrap_or("").to_string();
                if crate::agent_runner::has_sync_tool(&role) {
                    let summary = self.runner.lock().await.run(&role, &task_id).await;
                    ui::print_agent_done(&role, &summary);
                    summary
                } else {
                    self.run_in_background(&role, &task_id);
                    format!("Task {task_id} started in background for {role}. You'll be notified when done.")
                }
            }

            "ask_user" => {
                let question = args["question"].as_str().unwrap_or("").to_string();
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
                ui::print_deepthink_question("brain", &question);
                if options.is_empty() {
                    ui::prompt_user_answer()
                } else {
                    ui::prompt_user_select(&options)
                }
            }

            "run_bash" => {
                let command = args["command"].as_str().unwrap_or("").to_string();
                let description = args["description"].as_str().unwrap_or("").to_string();
                self.execute_bash(&command, &description, &root).await
            }

            _ if name.starts_with("skill_") => {
                let skill_name = &name["skill_".len()..];
                let result = self.skills.execute(skill_name, args);
                let preview: String = result.chars().take(120).collect();
                println!("  {} Skill {}: {}", "⚡".cyan(), skill_name, preview);
                result
            }

            _ => format!("Unknown tool: {name}"),
        }
    }

    fn run_in_background(&self, role: &str, task_id: &str) {
        let runner = self.runner.clone();
        let queue = self.done_queue.clone();
        let config = self.config.clone();
        let bus = self.bus.clone();
        let role = role.to_string();
        let task_id = task_id.to_string();
        ui::print_agent_working(&role, &format!("{task_id} (background)"));
        tokio::spawn(async move {
            let done = {
                let r = runner.lock().await;
                let summary = r.run(&role, &task_id).await;
                let root = team_root_path(&config);
                let artifact = artifact_root(&root).join(format!("{task_id}_{role}.md"));
                let title = bus
                    .list_active(&role)
                    .iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| task_id.clone());
                DoneTask {
                    role,
                    title,
                    summary,
                    artifact: artifact.to_string_lossy().into_owned(),
                }
            };
            queue.lock().await.push(done);
        });
    }

    pub(crate) async fn drain_notifications(&self) {
        let mut queue = self.done_queue.lock().await;
        while !queue.is_empty() {
            let done = queue.remove(0);
            ui::print_task_notification(&done.role, &done.title, &done.summary, &done.artifact);
        }
    }

    async fn execute_bash(&self, command: &str, description: &str, root: &Path) -> String {
        // 直接执行，不再向用户确认（需求确认后流程自动执行）
        if !description.is_empty() {
            println!("  {} {}", "$".bold().yellow(), command.white());
            println!("  {}", description.dimmed());
        }
        let child = match Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("Error: {e}"),
        };
        let (code, stdout, stderr) = run_with_timeout(child, Duration::from_secs(120));
        if code == 0 {
            ui::print_bash_output(&stdout, &stderr, 0);
            let t = stdout.trim().to_string();
            if t.is_empty() {
                "(no output)".into()
            } else {
                t
            }
        } else {
            ui::print_bash_output(&stdout, &stderr, code);
            let mut output = stdout;
            if !stderr.is_empty() {
                output.push_str(&format!("\nSTDERR:\n{stderr}"));
            }
            output.push_str(&format!("\nExit code: {code}"));
            let t = output.trim().to_string();
            if t.is_empty() {
                "(no output)".into()
            } else {
                t
            }
        }
    }

    pub(crate) async fn handle_slash(&self, cmd: &str) -> bool {
        let mut parts = cmd.split_whitespace();
        let command = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.collect::<Vec<_>>().join(" ").trim().to_string();

        match command.as_str() {
            "/exit" => return true,
            "/status" => {
                let status = self.bus.get_all_status();
                ui::print_status(&status);
            }
            "/tasks" => {
                let mut all: Vec<Value> = Vec::new();
                for role in crate::bus::ROLES {
                    all.extend(
                        self.bus
                            .list_inbox(role)
                            .iter()
                            .map(|t| serde_json::to_value(t).unwrap_or(Value::Null)),
                    );
                    all.extend(
                        self.bus
                            .list_active(role)
                            .iter()
                            .map(|t| serde_json::to_value(t).unwrap_or(Value::Null)),
                    );
                }
                ui::print_tasks(&all);
            }
            "/inbox" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /inbox <role>");
                } else {
                    let tasks = self.bus.list_inbox(&arg);
                    let values: Vec<Value> = tasks
                        .iter()
                        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
                        .collect();
                    ui::print_inbox(&arg, &values);
                }
            }
            "/skills" => {
                ui::print_skills(
                    &self
                        .skills
                        .list_skills()
                        .iter()
                        .map(|s| (s.0.clone(), s.1.clone()))
                        .collect::<Vec<_>>(),
                );
            }
            "/help" => ui::print_help(),
            "/dispatch" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /dispatch <role> <task description>");
                } else {
                    let mut dp = arg.split_whitespace();
                    if let Some(role) = dp.next() {
                        let desc = dp.collect::<Vec<_>>().join(" ");
                        if desc.is_empty() {
                            ui::print_error("Usage: /dispatch <role> <task description>");
                        } else {
                            let task =
                                self.bus
                                    .dispatch(role, &desc, &desc, DispatchOpts::default());
                            println!("Dispatched [{}] to {role}", task.id);
                        }
                    }
                }
            }
            "/skill" => self.handle_skill_cmd(&arg).await,
            "/uninstall" => {
                self.uninstall().await;
                return true;
            }
            _ => ui::print_error(&format!("Unknown command: {command}")),
        }
        false
    }

    async fn handle_skill_cmd(&self, arg: &str) {
        let mut parts = arg.split_whitespace();
        let sub = parts.next().unwrap_or("list").to_string();
        let sub_arg = parts.collect::<Vec<_>>().join(" ").trim().to_string();

        match sub.as_str() {
            "list" => {
                let skills = self.skills.list_skills();
                if skills.is_empty() {
                    println!("{}", "No skills installed".dimmed());
                } else {
                    for (name, desc, version) in &skills {
                        println!("  {} v{} — {desc}", name.cyan(), version);
                    }
                }
            }
            "install" => {
                if sub_arg.is_empty() {
                    ui::print_error("Usage: /skill install <github_url>");
                } else {
                    println!("Installing from {sub_arg}...");
                    let result = self.skills.install(&sub_arg);
                    println!("{result}");
                    self.reload_skills().await;
                }
            }
            "remove" => {
                if sub_arg.is_empty() {
                    ui::print_error("Usage: /skill remove <name>");
                } else {
                    let result = self.skills.remove(&sub_arg);
                    println!("{result}");
                    self.reload_skills().await;
                }
            }
            _ => ui::print_error("Usage: /skill [list|install|remove]"),
        }
    }

    async fn reload_skills(&self) {
        let skill_tools = self.skills.scan();
        let merged = tools::merge_tools(tools::tool_defs(), skill_tools);
        self.brain.lock().await.set_tools(merged);
    }

    async fn uninstall(&self) {
        println!("{}", "⚠ Uninstall MYOPC".bold().red());
        println!("This will:");
        println!("  1. Remove ~/.myopc/ config directory");
        println!("  (binary: run `cargo uninstall myopc` or delete the binary)");
        let answer = ui::prompt_user_answer();
        if answer.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return;
        }
        let opc_home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".myopc");
        if opc_home.exists() {
            std::fs::remove_dir_all(&opc_home).ok();
            println!("Removed {}", opc_home.to_string_lossy());
        }
        println!("{}", "MYOPC uninstalled.".green());
    }

    pub async fn chat(&self, user_input: &str) {
        if self.config.api_key.is_empty() {
            ui::print_error("No api_key configured. Edit myopc.json or set MYOPC_API_KEY.");
            return;
        }
        let executor: Arc<dyn ToolExecutor> = Arc::new(self.clone());
        match self.brain.lock().await.chat(user_input, executor).await {
            Ok(r) => println!("{r}"),
            Err(e) => ui::print_error(&format!("Error: {e}")),
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for MYOPCApp {
    async fn execute(&self, name: &str, args: &Value) -> String {
        self.execute_tool(name, args).await
    }
}
