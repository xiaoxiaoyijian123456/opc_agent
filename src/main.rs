use colored::Colorize;
use myopc::app::MYOPCApp;
use myopc::config::{load_config, team_root_path};
use myopc::init::{init_project, TEAM_DIR};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("init") {
        if let Some(dir) = args.get(2) {
            let target = PathBuf::from(dir);
            if !target.exists() {
                if let Err(error) = std::fs::create_dir_all(&target) {
                    eprintln!("Error: cannot create {}: {error}", target.display());
                    std::process::exit(1);
                }
                println!("  {} Created {}", "+".green(), target.display());
            }
            if target.join(TEAM_DIR).exists() {
                eprintln!(
                    "{}",
                    format!(
                        "Error: {} is already initialized ({TEAM_DIR} exists).",
                        target.display()
                    )
                    .red()
                );
                std::process::exit(1);
            }
            init_project(&target);
            if let Err(error) = std::env::set_current_dir(&target) {
                eprintln!("Error: cannot cd into {}: {error}", target.display());
                std::process::exit(1);
            }
        } else {
            let cwd = std::env::current_dir().unwrap_or_default();
            init_project(&cwd);
            return;
        }
    }

    let config = match load_config() {
        Ok(config) => config,
        Err(errors) => {
            for error in errors {
                eprintln!("Configuration error: {error}");
            }
            std::process::exit(1);
        }
    };
    let root = team_root_path(&config);
    if !root.join("AGENTS.md").is_file() {
        eprintln!(
            "Team directory is not initialized at {}. Run 'myopc init' first.",
            root.display()
        );
        std::process::exit(1);
    }

    let app = match MYOPCApp::new(config) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Startup error: {error}");
            std::process::exit(1);
        }
    };
    myopc::cli::run(app).await;
}
