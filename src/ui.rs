use colored::Colorize;
use comfy_table::Table;
use std::collections::BTreeMap;
use std::io::Write;

use crate::bus::RoleStatus;

fn role_label(role: &str) -> &str {
    match role {
        "pm" => "PM",
        "dev" => "Dev",
        "ui" => "UI",
        "tester" => "Test",
        "admin" => "Admin",
        "brain" => "MYOPC",
        _ => role,
    }
}

pub fn draw_box(lines: &[String], title: Option<&str>) -> String {
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    if let Some(t) = title {
        out.push_str(&format!("{} {}\n", "─".repeat(width + 4), t));
    }
    out.push_str(&format!("┌─{}─┐\n", "─".repeat(width + 2)));
    for l in lines {
        let pad = width - l.chars().count();
        out.push_str(&format!("│ {} {}│\n", l, " ".repeat(pad)));
    }
    out.push_str(&format!("└─{}─┘", "─".repeat(width + 2)));
    out
}

pub fn print_welcome(
    config_ok: bool,
    provider: &str,
    model: &str,
    team_root: &str,
    max_tokens: u32,
    temperature: f32,
) {
    let lines = vec![
        "MYOPC Team Agent v0.1".bold().to_string(),
        String::new(),
        format!("  Provider   : {}", provider.cyan()),
        format!("  Model      : {}", model.cyan()),
        format!("  Max Tokens : {}", max_tokens.to_string().cyan()),
        format!("  Temperature: {}", temperature.to_string().cyan()),
        format!("  Team Root  : {}", team_root.cyan()),
        format!(
            "  Config     : {}",
            if config_ok {
                "OK".green()
            } else {
                "Missing api_key".red()
            }
        ),
        String::new(),
        "  /help for commands, /exit to quit"
            .dimmed()
            .to_string(),
    ];
    let boxed = draw_box(&lines, None);
    println!("{}", boxed.blue());
}

pub fn print_status(status: &BTreeMap<String, RoleStatus>) {
    let parts: Vec<String> = status
        .iter()
        .map(|(role, info)| {
            let icon = match info.state.as_str() {
                "working" => "●".green(),
                "queued" => "●".yellow(),
                _ => "○".dimmed(),
            };
            format!("{} {}", icon, role_label(role))
        })
        .collect();
    println!("{}", parts.join(" │ "));
}

pub fn print_tasks(tasks: &[serde_json::Value]) {
    if tasks.is_empty() {
        println!("{}", "No tasks".dimmed());
        return;
    }
    let mut table = Table::new();
    table.set_header(vec!["ID", "To", "Title", "Status"]);
    for t in tasks {
        table.add_row(vec![
            t["id"].as_str().unwrap_or(""),
            t["to"].as_str().unwrap_or(""),
            t["title"].as_str().unwrap_or(""),
            t["status"].as_str().unwrap_or(""),
        ]);
    }
    println!("{table}");
}

pub fn print_inbox(role: &str, tasks: &[serde_json::Value]) {
    if tasks.is_empty() {
        println!("{} inbox is empty", role_label(role).dimmed());
        return;
    }
    for t in tasks {
        println!(
            "  [{}] {}",
            t["id"].as_str().unwrap_or(""),
            t["title"].as_str().unwrap_or("")
        );
    }
}

pub fn print_help() {
    let cmds: [(&str, &str); 11] = [
        ("/status", "Show agent status"),
        ("/tasks", "Show task queue"),
        ("/inbox <role>", "View role inbox (pm/dev/ui/tester/admin)"),
        ("/dispatch", "Manually dispatch a task"),
        ("/skills", "List available tools"),
        ("/skill list", "List installed skills"),
        ("/skill install <url>", "Install skill from GitHub"),
        ("/skill remove <name>", "Remove a skill"),
        ("/uninstall", "Remove MYOPC from system"),
        ("/help", "This help"),
        ("/exit", "Quit"),
    ];
    let mut table = Table::new();
    table.set_header(vec!["Command", "Description"]);
    for (cmd, desc) in cmds {
        table.add_row(vec![cmd.cyan().to_string(), desc.to_string()]);
    }
    println!("{table}");
}

pub fn print_skills(extra_skills: &[(String, String)]) {
    let builtins: [(&str, &str); 5] = [
        ("dispatch_task", "Send task to a role"),
        ("get_project_status", "Get all agent status"),
        ("execute_role_task", "Run role agent on task"),
        ("run_bash", "Run shell command (requires user approval)"),
        ("ask_user", "Ask user for approval or clarification"),
    ];
    println!("{}", "  Built-in:".bold());
    for (name, desc) in builtins {
        println!("    {} — {}", name.cyan(), desc);
    }
    if !extra_skills.is_empty() {
        println!("{}", "  Skills:".bold());
        for (name, desc) in extra_skills {
            println!("    {} — {}", format!("skill_{name}").cyan(), desc);
        }
    }
}

pub fn print_error(msg: &str) {
    println!("{}", msg.red());
}

pub fn print_agent_working(role: &str, task_title: &str) {
    println!(
        "  {} {} working on: {}",
        "▶".green(),
        role_label(role),
        task_title
    );
}

pub fn print_agent_done(role: &str, summary: &str) {
    let s: String = summary.chars().take(120).collect();
    println!("  {} {} done: {}", "✓".green(), role_label(role), s);
}

pub fn print_file_written(role: &str, file_path: &str) {
    println!(
        "  {} {} wrote: {}",
        "📄".cyan(),
        role_label(role),
        file_path
    );
}

pub fn print_intervention_block(state: &str) {
    let lines = vec![
        format!("Intervention state: {}", state.bold().red()),
        "All dispatches are blocked. Change intervention.md to RUNNING to resume.".to_string(),
    ];
    println!("{}", draw_box(&lines, Some("⚠ BLOCKED")).red());
}

pub fn print_deepthink_question(role: &str, question: &str) {
    println!(
        "\n  {} {} asks: {}",
        "?".yellow(),
        role_label(role).bold(),
        question
    );
}

pub fn print_task_notification(role: &str, task_title: &str, summary: &str, artifact_path: &str) {
    let s: String = summary.chars().take(300).collect();
    let mut lines = vec![format!(
        "{} {} completed: {}",
        "✓".green(),
        role_label(role).bold(),
        task_title
    )];
    lines.push(String::new());
    lines.push(s);
    if !artifact_path.is_empty() {
        lines.push(String::new());
        lines.push(format!("Artifact: {}", artifact_path.dimmed()));
    }
    println!("{}", draw_box(&lines, Some("Task Completed")).green());
}

pub fn print_bash_output(stdout: &str, stderr: &str, return_code: i32) {
    if !stdout.is_empty() {
        for line in stdout.lines() {
            println!("  {line}");
        }
    }
    if !stderr.is_empty() {
        for line in stderr.lines() {
            println!("  {}", line.red());
        }
    }
    if return_code != 0 {
        println!("  {}", format!("Exit code: {return_code}").red());
    }
}

pub fn prompt_user_answer() -> String {
    print!("  Your answer: ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    match std::io::stdin().read_line(&mut s) {
        Ok(_) => {
            let s = s.trim().to_string();
            if s.is_empty() {
                "(no answer)".to_string()
            } else {
                s
            }
        }
        Err(_) => "(skipped)".to_string(),
    }
}

pub fn prompt_user_select(options: &[(String, String)]) -> String {
    if options.is_empty() {
        return prompt_user_answer();
    }
    println!();
    for (i, (label, _)) in options.iter().enumerate() {
        if i == 0 {
            println!("    {}: {}（建议，回车默认选中）", i + 1, label);
        } else {
            println!("    {}: {}", i + 1, label);
        }
    }
    println!("    {}: 自由输入...", options.len() + 1);
    print!("  请选择 (1-{}，回车选中建议项): ", options.len() + 1);
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    match std::io::stdin().read_line(&mut s) {
        Ok(_) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                // 回车 → 默认选中建议项（第一个选项）
                return options[0].1.clone();
            }
            match t.parse::<usize>() {
                Ok(n) if n >= 1 && n <= options.len() => options[n - 1].1.clone(),
                Ok(n) if n == options.len() + 1 => prompt_user_answer(),
                _ => t, // 非数字文本 → 视为自由输入
            }
        }
        Err(_) => "(skipped)".to_string(),
    }
}
