use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct IdentityEngine {
    team_root: PathBuf,
}

fn role_dir(role: &str) -> &'static str {
    match role {
        "boss" => "boss_decide",
        "pm" => "pm_office",
        "dev" => "dev_forge",
        "ui" => "ui_ux_studio",
        "tester" => "tester",
        "admin" => "admin_bp",
        _ => "pm_office",
    }
}

fn role_identity_file(role: &str) -> &'static str {
    match role {
        "boss" => "identity.md",
        "pm" => "pm_identity.md",
        "dev" => "dev_identity.md",
        "ui" => "ui_identity.md",
        "tester" => "tester_identity.md",
        "admin" => "bp_identity.md",
        _ => "identity.md",
    }
}

impl IdentityEngine {
    pub fn new(team_root: &Path) -> IdentityEngine {
        IdentityEngine {
            team_root: team_root.to_path_buf(),
        }
    }

    fn read_or_empty(&self, path: &Path) -> String {
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub fn load_agents_md(&self) -> String {
        self.read_or_empty(&self.team_root.join("AGENTS.md"))
    }

    pub fn load_handoff(&self) -> String {
        self.read_or_empty(
            &self
                .team_root
                .join("communication")
                .join("handoff_protocols.md"),
        )
    }

    pub fn load_boundary(&self) -> String {
        self.read_or_empty(&self.team_root.join("boundary.md"))
    }

    pub fn load_knowledge(&self) -> String {
        let kb = self.team_root.join("knowledge_base");
        if !kb.exists() {
            return String::new();
        }
        self.read_md_recursive(&kb)
    }

    fn read_md_recursive(&self, dir: &Path) -> String {
        if !dir.exists() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flat_map(|it| it.collect::<Vec<_>>())
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                let sub = self.read_md_recursive(&p);
                if !sub.is_empty() {
                    parts.push(sub);
                }
            } else if p.extension().map(|e| e == "md").unwrap_or(false) {
                let text = std::fs::read_to_string(&p)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
        parts.join("\n\n")
    }

    pub fn load_identity(&self, role: &str) -> String {
        let file = self
            .team_root
            .join("staff")
            .join(role_dir(role))
            .join(role_identity_file(role));
        if file.exists() {
            return std::fs::read_to_string(&file)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        }
        format!("You are the {role} agent.")
    }

    pub fn load_boss_identity(&self) -> String {
        let file = self
            .team_root
            .join("staff")
            .join("boss_decide")
            .join("identity.md");
        if file.exists() {
            std::fs::read_to_string(&file)
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        }
    }

    pub fn read_intervention(&self) -> String {
        let file = self
            .team_root
            .join("staff")
            .join("boss_decide")
            .join("intervention.md");
        if file.exists() {
            std::fs::read_to_string(&file)
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_else(|_| "RUNNING".into())
        } else {
            "RUNNING".into()
        }
    }

    pub fn load_recent_reflections(&self, role: &str, max_count: usize) -> String {
        let archive_dir = self.team_root.join("memory_center").join("archive");
        if !archive_dir.exists() {
            return String::new();
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&archive_dir)
            .ok()
            .into_iter()
            .flat_map(|it| it.collect::<Vec<_>>())
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && !p
                        .file_name()
                        .map(|f| f.to_string_lossy().starts_with('.'))
                        .unwrap_or(false)
            })
            .collect();
        entries.sort_by_key(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        entries.reverse();

        let mut lines: Vec<String> = Vec::new();
        for d in entries {
            let mut refl_files: Vec<PathBuf> = std::fs::read_dir(&d)
                .ok()
                .into_iter()
                .flat_map(|it| it.collect::<Vec<_>>())
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|f| {
                            f.to_string_lossy()
                                .contains(&format!("_{role}_reflection.json"))
                        })
                        .unwrap_or(false)
                })
                .collect();
            refl_files.sort();
            refl_files.reverse();
            for f in refl_files {
                if let Ok(s) = std::fs::read_to_string(&f) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        let tag = if v["success"].as_bool().unwrap_or(false) {
                            "OK"
                        } else {
                            "FAIL"
                        };
                        let typ = v["type"].as_str().unwrap_or("?");
                        let title = v["title"].as_str().unwrap_or("");
                        let summary: String = v["summary"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect();
                        lines.push(format!("- [{typ}] \"{title}\" → {tag}: {summary}"));
                    }
                }
                if lines.len() >= max_count {
                    break;
                }
            }
            if lines.len() >= max_count {
                break;
            }
        }
        if lines.is_empty() {
            return String::new();
        }
        format!("## Recent Experience\n{}", lines.join("\n"))
    }

    pub fn build_system_prompt(&self, role: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        let agents_md = self.load_agents_md();
        if !agents_md.is_empty() {
            parts.push(agents_md);
            parts.push("\n---\n".to_string());
        }
        parts.push(self.load_identity(role));
        let knowledge = self.load_knowledge();
        if !knowledge.is_empty() {
            parts.push(format!("\n---\n{knowledge}"));
        }
        let reflections = self.load_recent_reflections(role, 3);
        if !reflections.is_empty() {
            parts.push(format!("\n{reflections}"));
        }
        let boundary = self.load_boundary();
        if !boundary.is_empty() {
            parts.push(format!("\n---\n{boundary}"));
        }
        parts.join("\n")
    }
}
