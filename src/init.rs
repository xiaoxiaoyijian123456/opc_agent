use colored::Colorize;
use std::path::{Path, PathBuf};

pub const TEAM_DIR: &str = ".myopc_agent";

const ROLES: [&str; 5] = ["pm", "dev", "ui", "tester", "admin"];

const RUNTIME_DIRS: [&str; 6] = [
    "communication/bus/{role}/inbox",
    "communication/bus/{role}/active",
    "workspace",
    ".artifacts",
    "memory_center/archive",
    "memory_center/project_history",
];

fn scaffold_root() -> PathBuf {
    if let Ok(p) = std::env::var("MYOPC_SCAFFOLD") {
        let p = PathBuf::from(p);
        if p.exists() { return p; }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let from_repo = manifest_dir.join("scaffold");
    if from_repo.exists() { return from_repo; }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("../scaffold");
            if p.exists() { return p; }
        }
    }
    from_repo
}

fn get_all_files(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let full = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                results.extend(get_all_files(&full));
            } else {
                results.push(full);
            }
        }
    }
    results
}

pub fn init_project(target: &Path) {
    let project_dir = target.to_path_buf();
    let target_root = project_dir.join(TEAM_DIR);
    let scaffold = scaffold_root();

    if !scaffold.exists() {
        eprintln!("{}", "Error: scaffold data not found in package.".red());
        return;
    }

    let mut created: Vec<String> = Vec::new();
    let mut skipped: usize = 0;

    let all_files = get_all_files(&scaffold);
    for src_file in all_files {
        if src_file.file_name().map(|f| f.to_string_lossy().starts_with('.')).unwrap_or(false) {
            continue;
        }
        let rel = src_file.strip_prefix(&scaffold).unwrap().to_path_buf();
        let dst = target_root.join(&rel);
        if dst.exists() {
            skipped += 1;
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if std::fs::copy(&src_file, &dst).is_ok() {
            created.push(rel.to_string_lossy().into_owned());
        }
    }

    for pattern in RUNTIME_DIRS {
        if pattern.contains("{role}") {
            for role in ROLES {
                let p = target_root.join(pattern.replace("{role}", role));
                std::fs::create_dir_all(&p).ok();
            }
        } else {
            std::fs::create_dir_all(target_root.join(pattern)).ok();
        }
    }

    println!("\n{}", format!("MYOPC initialized in {}", target_root.display()).bold().green());
    println!("  {}\n", "Config: 全局配置 ~/.myopc/myopc.json（init 不生成项目内配置）".dimmed());
    if !created.is_empty() {
        for f in &created {
            println!("  {} {}", "+".green(), format!("{TEAM_DIR}/{f}"));
        }
    }
    if skipped > 0 {
        println!("\n  {} existing files unchanged", skipped.to_string().dimmed());
    }
    println!("\n{}", "Next steps:".bold());
    println!("  1. Edit {} — set your api_key", "~/.myopc/myopc.json".cyan());
    println!("  2. Run {} to start", "myopc".cyan());
    println!();
}
