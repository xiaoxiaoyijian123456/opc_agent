use crate::agent_runner::{normalize_path, AgentRunner, ToolExecutor};
use crate::brain::Brain;
use crate::bus::Bus;
use crate::config::{team_root_path, validate_config, Config};
use crate::identity::IdentityEngine;
use crate::instance_lock::InstanceLock;
use crate::llm::{HttpLlm, LlmBackend};
use crate::orchestrator::{CliPrdApproval, Orchestrator};
use crate::skill_loader::SkillLoader;
use crate::task_store::TaskStore;
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
pub struct MYOPCApp {
    config: Config,
    identity: IdentityEngine,
    skills: SkillLoader,
    brain: Arc<Mutex<Brain>>,
    store: TaskStore,
    orchestrator: Arc<Orchestrator>,
    _instance_lock: Arc<InstanceLock>,
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
    pub fn new(config: Config) -> Result<MYOPCApp, String> {
        let root = team_root_path(&config);
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).ok();

        let instance_lock = Arc::new(InstanceLock::acquire(&root)?);
        let store = TaskStore::open(&root)?;
        let recovered = store.recover_active()?;
        if recovered > 0 {
            ui::print_error(&format!(
                "  ⚠ Marked {recovered} interrupted task(s) as failed. Use /tasks and /retry <task_id>."
            ));
        }
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
        let runner = Arc::new(Mutex::new(runner));
        let orchestrator = Arc::new(Orchestrator::new(
            store.clone(),
            runner,
            Arc::new(CliPrdApproval),
        ));

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
        for workflow in store.unfinished_workflows()? {
            println!(
                "  Unfinished workflow {}: {} (current task: {})",
                workflow.id,
                workflow.status,
                workflow.current_task_id.as_deref().unwrap_or("none")
            );
        }

        Ok(MYOPCApp {
            config,
            identity,
            skills,
            brain: Arc::new(Mutex::new(brain)),
            store,
            orchestrator,
            _instance_lock: instance_lock,
        })
    }

    async fn execute_tool(&self, name: &str, args: &Value) -> String {
        let root = team_root_path(&self.config);
        match name {
            "start_workflow" => {
                let state = self.identity.read_intervention();
                if state == "STOP" {
                    ui::print_intervention_block("STOP");
                    return "BLOCKED: Intervention state is STOP. All workflows halted.".into();
                }
                let request = args["request"].as_str().unwrap_or("");
                match self.orchestrator.start(request).await {
                    Ok(workflow) => serde_json::to_string(&workflow).unwrap_or_default(),
                    Err(error) => format!("ERROR: {error}"),
                }
            }

            "get_project_status" => match self.store.status() {
                Ok(status) => {
                    ui::print_status(&status);
                    serde_json::to_string(&status).unwrap_or_default()
                }
                Err(error) => format!("ERROR: {error}"),
            },

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
            "/status" => match self.store.status() {
                Ok(status) => ui::print_status(&status),
                Err(error) => ui::print_error(&error),
            },
            "/tasks" => match self.store.list_tasks() {
                Ok(tasks) => {
                    let values = tasks.iter().map(|task| task.as_value()).collect::<Vec<_>>();
                    ui::print_tasks(&values);
                }
                Err(error) => ui::print_error(&error),
            },
            "/inbox" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /inbox <role>");
                } else {
                    match self.store.list_inbox(&arg) {
                        Ok(tasks) => {
                            let values =
                                tasks.iter().map(|task| task.as_value()).collect::<Vec<_>>();
                            ui::print_inbox(&arg, &values);
                        }
                        Err(error) => ui::print_error(&error),
                    }
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
                            match self.store.create_manual(role, &desc) {
                                Ok((workflow, task)) => println!(
                                    "Dispatched [{}] to {role}. Run /resume {} to execute.",
                                    task.id, workflow.id
                                ),
                                Err(error) => ui::print_error(&error),
                            }
                        }
                    }
                }
            }
            "/resume" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /resume <workflow_id>");
                } else {
                    match self.orchestrator.resume(&arg).await {
                        Ok(workflow) => println!("Workflow {}: {}", workflow.id, workflow.status),
                        Err(error) => ui::print_error(&error),
                    }
                }
            }
            "/retry" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /retry <task_id>");
                } else {
                    match self.orchestrator.retry(&arg).await {
                        Ok(workflow) => println!("Workflow {}: {}", workflow.id, workflow.status),
                        Err(error) => ui::print_error(&error),
                    }
                }
            }
            "/cancel" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /cancel <workflow_id>");
                } else {
                    match self.orchestrator.cancel(&arg) {
                        Ok(workflow) => println!("Workflow {}: {}", workflow.id, workflow.status),
                        Err(error) => ui::print_error(&error),
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
