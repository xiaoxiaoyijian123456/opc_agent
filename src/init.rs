use colored::Colorize;
use std::path::Path;

pub const TEAM_DIR: &str = ".myopc_agent";

const ROLES: [&str; 5] = ["pm", "dev", "ui", "tester", "admin"];

const RUNTIME_DIRS: [&str; 7] = [
    "communication/bus/{role}/inbox",
    "communication/bus/{role}/active",
    "workspace",
    ".artifacts",
    "skills",
    "memory_center/archive",
    "memory_center/project_history",
];

const SCAFFOLD_FILES: &[(&str, &str)] = &[
    ("AGENTS.md", include_str!("../scaffold/AGENTS.md")),
    ("boundary.md", include_str!("../scaffold/boundary.md")),
    (
        "communication/handoff_protocols.md",
        include_str!("../scaffold/communication/handoff_protocols.md"),
    ),
    (
        "knowledge_base/corp_style/workstyle.md",
        include_str!("../scaffold/knowledge_base/corp_style/workstyle.md"),
    ),
    (
        "memory_center/reflection.log",
        include_str!("../scaffold/memory_center/reflection.log"),
    ),
    (
        "memory_center/user.md",
        include_str!("../scaffold/memory_center/user.md"),
    ),
    (
        "myopc.json.example",
        include_str!("../scaffold/myopc.json.example"),
    ),
    (
        "staff/admin_bp/bp_identity.md",
        include_str!("../scaffold/staff/admin_bp/bp_identity.md"),
    ),
    (
        "staff/admin_bp/output_template.md",
        include_str!("../scaffold/staff/admin_bp/output_template.md"),
    ),
    (
        "staff/boss_decide/identity.md",
        include_str!("../scaffold/staff/boss_decide/identity.md"),
    ),
    (
        "staff/boss_decide/intervention.md",
        include_str!("../scaffold/staff/boss_decide/intervention.md"),
    ),
    (
        "staff/dev_forge/backend_spec.md",
        include_str!("../scaffold/staff/dev_forge/backend_spec.md"),
    ),
    (
        "staff/dev_forge/dev_identity.md",
        include_str!("../scaffold/staff/dev_forge/dev_identity.md"),
    ),
    (
        "staff/dev_forge/frontend_spec.md",
        include_str!("../scaffold/staff/dev_forge/frontend_spec.md"),
    ),
    (
        "staff/pm_office/pm_identity.md",
        include_str!("../scaffold/staff/pm_office/pm_identity.md"),
    ),
    (
        "staff/pm_office/pm_rules.md",
        include_str!("../scaffold/staff/pm_office/pm_rules.md"),
    ),
    (
        "staff/tester/tester_identity.md",
        include_str!("../scaffold/staff/tester/tester_identity.md"),
    ),
    (
        "staff/tester/tester_spec.md",
        include_str!("../scaffold/staff/tester/tester_spec.md"),
    ),
    (
        "staff/ui_ux_studio/design_spec.md",
        include_str!("../scaffold/staff/ui_ux_studio/design_spec.md"),
    ),
    (
        "staff/ui_ux_studio/design_system_template.md",
        include_str!("../scaffold/staff/ui_ux_studio/design_system_template.md"),
    ),
    (
        "staff/ui_ux_studio/ui_identity.md",
        include_str!("../scaffold/staff/ui_ux_studio/ui_identity.md"),
    ),
];

pub fn init_project(target: &Path) {
    let target_root = target.join(TEAM_DIR);
    let mut created: Vec<String> = Vec::new();
    let mut skipped: usize = 0;

    for (relative_path, content) in SCAFFOLD_FILES {
        let destination = target_root.join(relative_path);
        if destination.exists() {
            skipped += 1;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if std::fs::write(&destination, content).is_ok() {
            created.push(relative_path.to_string());
        }
    }

    for pattern in RUNTIME_DIRS {
        if pattern.contains("{role}") {
            for role in ROLES {
                let path = target_root.join(pattern.replace("{role}", role));
                std::fs::create_dir_all(&path).ok();
            }
        } else {
            std::fs::create_dir_all(target_root.join(pattern)).ok();
        }
    }

    println!(
        "\n{}",
        format!("MYOPC initialized in {}", target_root.display())
            .bold()
            .green()
    );
    println!(
        "  {}\n",
        "Config: 全局配置 ~/.myopc/myopc.json（init 不生成项目内配置）".dimmed()
    );
    if !created.is_empty() {
        for file in &created {
            println!("  {} {TEAM_DIR}/{file}", "+".green());
        }
    }
    if skipped > 0 {
        println!(
            "\n  {} existing files unchanged",
            skipped.to_string().dimmed()
        );
    }
    println!("\n{}", "Next steps:".bold());
    println!(
        "  1. Edit {} — set your api_key",
        "~/.myopc/myopc.json".cyan()
    );
    println!("  2. Run {} to start", "myopc".cyan());
    println!();
}
