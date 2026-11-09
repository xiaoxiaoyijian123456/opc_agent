use colored::Colorize;
use opc::agent_runner::{normalize_path, AgentRunner, ToolExecutor};
use opc::brain::Brain;
use opc::bus::{Bus, DispatchOpts};
use opc::config::{team_root_path, validate_config, Config};
use opc::identity::IdentityEngine;
use opc::init::init_project;
use opc::llm::{HttpLlm, LlmBackend};
use opc::skill_loader::SkillLoader;
use opc::tools;
use opc::ui;
use rustyline::error::ReadlineError;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone)]
struct DoneTask {
    role: String,
    task_id: String,
    title: String,
    summary: String,
    artifact: String,
}

#[derive(Clone)]
pub struct OPCApp {
    config: Config,
    bus: Bus,
    identity: IdentityEngine,
    skills: SkillLoader,
    runner: Arc<Mutex<AgentRunner>>,
    brain: Arc<Mutex<Brain>>,
    compact: Arc<AtomicBool>,
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

impl OPCApp {
    pub fn new() -> OPCApp {
        let placeholder = PathBuf::from(".");
        OPCApp {
            config: Config {
                provider: "openai".into(),
                api_key: String::new(),
                base_url: String::new(),
                default_model: "gpt-4o".into(),
                team_root: ".".into(),
                code_root: None,
                max_tokens: 8192,
                temperature: 0.7,
            },
            bus: Bus::new(&placeholder),
            identity: IdentityEngine::new(&placeholder),
            skills: SkillLoader::new(&placeholder.join("skills")),
            runner: Arc::new(Mutex::new(AgentRunner::new(
                Config {
                    provider: "openai".into(),
                    api_key: String::new(),
                    base_url: String::new(),
                    default_model: "gpt-4o".into(),
                    team_root: ".".into(),
                    code_root: None,
                    max_tokens: 8192,
                    temperature: 0.7,
                },
                Bus::new(&placeholder),
                IdentityEngine::new(&placeholder),
                placeholder.clone(),
                Arc::new(HttpLlm::new()),
            ))),
            brain: Arc::new(Mutex::new(Brain::new(
                Config {
                    provider: "openai".into(),
                    api_key: String::new(),
                    base_url: String::new(),
                    default_model: "gpt-4o".into(),
                    team_root: ".".into(),
                    code_root: None,
                    max_tokens: 8192,
                    temperature: 0.7,
                },
                Arc::new(HttpLlm::new()),
                None,
                tools::tool_defs(),
            ))),
            compact: Arc::new(AtomicBool::new(false)),
            done_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn init(&mut self) {
        self.config = opc::config::load_config();
        let root = team_root_path(&self.config);

        self.bus = Bus::new(&root);
        self.identity = IdentityEngine::new(&root);

        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).ok();

        self.skills = SkillLoader::new(&root.join("skills"));
        let skill_tools = self.skills.scan();
        let base = if self.config.code_root.is_some() {
            tools::tool_defs().into_iter().filter(|t| t.name != "create_project").collect()
        } else {
            tools::tool_defs()
        };
        let all_tools = tools::merge_tools(base, skill_tools);

        let llm: Arc<dyn LlmBackend> = Arc::new(HttpLlm::new());
        self.brain = Arc::new(Mutex::new(Brain::new(
            self.config.clone(),
            llm.clone(),
            Some(self.identity.clone()),
            all_tools,
        )));

        let mut runner = AgentRunner::new(
            self.config.clone(),
            self.bus.clone(),
            self.identity.clone(),
            workspace,
            llm,
        );
        if let Some(cr) = &self.config.code_root {
            let cr = normalize_path(&PathBuf::from(cr));
            std::fs::create_dir_all(&cr).ok();
            runner.code_root = Some(cr);
        }
        self.runner = Arc::new(Mutex::new(runner));

        let errors = validate_config(&self.config);
        ui::print_welcome(
            errors.is_empty(),
            &self.config.provider,
            &self.config.default_model,
            &root.to_string_lossy(),
            self.config.max_tokens,
            self.config.temperature,
        );
        for e in &errors {
            ui::print_error(&format!("  ⚠ {e}"));
        }
        if !root.join("AGENTS.md").exists() {
            ui::print_error("  ⚠ Team directory not initialized. Run 'opc init' first.");
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
                        return format!("BLOCKED: User did not approve dispatch. Response: {answer}");
                    }
                }

                let mut ctx = serde_json::Map::new();
                if let Some(d) = args.get("depends_on") {
                    ctx.insert("depends_on".into(), d.clone());
                }
                if let Some(a) = args.get("artifacts") {
                    ctx.insert("artifacts".into(), a.clone());
                }
                let context = if ctx.is_empty() { None } else { Some(Value::Object(ctx)) };
                let role = args["role"].as_str().unwrap_or("").to_string();
                let title = args["title"].as_str().unwrap_or("").to_string();
                let content = args["content"].as_str().unwrap_or("").to_string();
                let msg_type = args["msg_type"].as_str().unwrap_or("").to_string();
                let task = self.bus.dispatch(
                    &role,
                    &title,
                    &content,
                    DispatchOpts { from_role: None, msg_type: Some(msg_type), context },
                );
                ui::print_agent_working(&role, &title);
                serde_json::to_string(&task).unwrap_or_default()
            }

            "read_workspace_file" => {
                let path = args["path"].as_str().unwrap_or("");
                let fp = root.join("workspace").join(path);
                if fp.exists() {
                    std::fs::read_to_string(&fp).unwrap_or_default()
                } else {
                    format!("File not found: {path}")
                }
            }

            "write_workspace_file" => {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                let fp = root.join("workspace").join(path);
                if let Some(parent) = fp.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&fp, content).ok();
                format!("Written to {path}")
            }

            "get_project_status" => {
                let status = self.bus.get_all_status();
                ui::print_status(&status);
                serde_json::to_string(&status).unwrap_or_default()
            }

            "execute_role_task" => {
                let role = args["role"].as_str().unwrap_or("").to_string();
                let task_id = args["task_id"].as_str().unwrap_or("").to_string();
                if opc::agent_runner::has_sync_tool(&role) {
                    let summary = self.runner.lock().await.run(&role, &task_id).await;
                    ui::print_agent_done(&role, &summary);
                    summary
                } else {
                    self.run_in_background(&role, &task_id);
                    format!("Task {task_id} started in background for {role}. You'll be notified when done.")
                }
            }

            "list_projects" => {
                let registry = self.load_registry(&root);
                if registry.is_empty() {
                    "No existing projects.".into()
                } else {
                    serde_json::to_string(&registry).unwrap_or_default()
                }
            }

            "create_project" => {
                self.create_project(&root, args).await
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
                                    o.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                    o.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string(),
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

    async fn create_project(&self, root: &Path, args: &Value) -> String {
        let proj_name = args["name"].as_str().unwrap_or("project").to_string();
        let location = args["location"].as_str().unwrap_or("workspace").to_string();
        let parent = args["parent"].as_str().map(|s| s.to_string());
        let title = args["title"].as_str().unwrap_or(&proj_name).to_string();
        let keywords: Vec<String> = args
            .get("keywords")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|k| k.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let ws = root.join("workspace");
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();

        let code_root: PathBuf = if let Some(p) = &parent {
            let registry = self.load_registry(root);
            let entry = registry.iter().find(|e| e["name"] == Value::String(p.clone()));
            match entry {
                Some(e) => {
                    let dir = Path::new(e["code_path"].as_str().unwrap_or(""))
                        .parent()
                        .map(|d| d.to_path_buf())
                        .unwrap_or_default();
                    dir.join(&proj_name)
                }
                None if location == "desktop" => home.join("Desktop").join(&proj_name),
                _ => ws.join(&proj_name).join("code"),
            }
        } else if location == "desktop" {
            home.join("Desktop").join(&proj_name)
        } else {
            ws.join(&proj_name).join("code")
        };

        let artifacts_path = root.join(".artifacts").join(&proj_name);
        std::fs::create_dir_all(&artifacts_path).ok();
        std::fs::create_dir_all(&code_root).ok();

        {
            let mut runner = self.runner.lock().await;
            runner.project_id = Some(proj_name.clone());
            runner.code_root = Some(code_root.clone());
        }

        let entry = serde_json::json!({
            "name": proj_name,
            "title": title,
            "code_path": code_root.to_string_lossy(),
            "artifacts_path": artifacts_path.to_string_lossy(),
            "created_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "keywords": keywords,
        });
        self.save_to_registry(root, entry);

        println!("  {} Project {} created", "📁".green(), proj_name.bold());
        println!("      Artifacts: {}", artifacts_path.to_string_lossy());
        println!("      Code:      {}", code_root.to_string_lossy());
        if let Some(p) = &parent {
            println!("      Sibling of: {p}");
        }

        serde_json::json!({
            "project": proj_name,
            "artifacts_path": artifacts_path.to_string_lossy(),
            "code_path": code_root.to_string_lossy(),
        })
        .to_string()
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
                let mut r = runner.lock().await;
                let summary = r.run(&role, &task_id).await;
                let root = team_root_path(&config);
                let artifact = match r.project_id.clone() {
                    Some(pid) => root.join(".artifacts").join(pid).join(format!("{task_id}_{role}.md")),
                    None => root.join(".artifacts").join(format!("{task_id}_{role}.md")),
                };
                let title = bus
                    .list_active(&role)
                    .iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| task_id.clone());
                DoneTask {
                    role,
                    task_id,
                    title,
                    summary,
                    artifact: artifact.to_string_lossy().into_owned(),
                }
            };
            queue.lock().await.push(done);
        });
    }

    async fn drain_notifications(&self) {
        let mut queue = self.done_queue.lock().await;
        while !queue.is_empty() {
            let done = queue.remove(0);
            ui::print_task_notification(&done.role, &done.title, &done.summary, &done.artifact);
        }
    }

    fn load_registry(&self, root: &Path) -> Vec<Value> {
        let p = root.join("workspace").join("project_registry.json");
        if p.exists() {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn save_to_registry(&self, root: &Path, entry: Value) {
        let p = root.join("workspace").join("project_registry.json");
        let mut registry = self.load_registry(root);
        registry.retain(|e| e["name"] != entry["name"]);
        registry.push(entry);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(p, serde_json::to_string_pretty(&registry).unwrap()).ok();
    }

    async fn execute_bash(&self, command: &str, description: &str, root: &Path) -> String {
        let approved = ui::prompt_bash_approval(command, description);
        if !approved {
            return "User denied execution.".into();
        }
        let mut child = match Command::new("sh")
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
            if t.is_empty() { "(no output)".into() } else { t }
        } else {
            ui::print_bash_output(&stdout, &stderr, code);
            let mut output = stdout;
            if !stderr.is_empty() {
                output.push_str(&format!("\nSTDERR:\n{stderr}"));
            }
            output.push_str(&format!("\nExit code: {code}"));
            let t = output.trim().to_string();
            if t.is_empty() { "(no output)".into() } else { t }
        }
    }

    async fn handle_slash(&self, cmd: &str) -> bool {
        let mut parts = cmd.trim().split_whitespace();
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
                for role in opc::bus::ROLES {
                    all.extend(self.bus.list_inbox(role).iter().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)));
                    all.extend(self.bus.list_active(role).iter().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)));
                }
                ui::print_tasks(&all);
            }
            "/inbox" => {
                if arg.is_empty() {
                    ui::print_error("Usage: /inbox <role>");
                } else {
                    let tasks = self.bus.list_inbox(&arg);
                    let values: Vec<Value> = tasks.iter().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).collect();
                    ui::print_inbox(&arg, &values);
                }
            }
            "/skills" => {
                ui::print_skills(&self.skills.list_skills().iter().map(|s| (s.0.clone(), s.1.clone())).collect::<Vec<_>>());
            }
            "/help" => ui::print_help(),
            "/compact" => {
                let new_val = !self.compact.load(Ordering::SeqCst);
                self.compact.store(new_val, Ordering::SeqCst);
                println!("Compact mode: {}", if new_val { "on" } else { "off" });
            }
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
                            let task = self.bus.dispatch(role, &desc, &desc, DispatchOpts::default());
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
        println!("{}", "⚠ Uninstall OPC".bold().red());
        println!("This will:");
        println!("  1. Remove ~/.opc/ config directory");
        println!("  (binary: run `cargo uninstall opc-cli` or delete the binary)");
        let answer = ui::prompt_user_answer();
        if answer.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return;
        }
        let opc_home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join(".opc");
        if opc_home.exists() {
            std::fs::remove_dir_all(&opc_home).ok();
            println!("Removed {}", opc_home.to_string_lossy());
        }
        println!("{}", "OPC uninstalled.".green());
    }

    pub async fn chat(&self, user_input: &str) {
        if self.config.api_key.is_empty() {
            ui::print_error("No api_key configured. Edit opc.json or set OPC_API_KEY.");
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
impl ToolExecutor for OPCApp {
    async fn execute(&self, name: &str, args: &Value) -> String {
        self.execute_tool(name, args).await
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("init") {
        let cwd = std::env::current_dir().unwrap_or_default();
        init_project(&cwd);
        return;
    }

    let mut app = OPCApp::new();
    app.init();

    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let opc_dir = home.join(".opc");
    std::fs::create_dir_all(&opc_dir).ok();
    let history_file = opc_dir.join("history");

    let mut rl = rustyline::DefaultEditor::new().expect("failed to init readline");
    if let Err(_) = rl.load_history(&history_file) {
        // first run — no history yet
    }

    loop {
        app.drain_notifications().await;
        match rl.readline("opc > ") {
            Ok(line) => {
                rl.add_history_entry(line.as_str()).ok();
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with('/') {
                    let should_exit = app.handle_slash(&trimmed).await;
                    if should_exit {
                        break;
                    }
                } else {
                    app.chat(&trimmed).await;
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        }
    }
    let _ = rl.save_history(&history_file);
    println!("\nBye.");
}
