use crate::tools::{skill_tool, ToolDef};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Deserialize, Clone, Debug)]
struct SkillManifest {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    entry: Option<String>,
    parameters: Option<Value>,
}

#[derive(Clone)]
struct SkillInfo {
    manifest: SkillManifest,
    path: PathBuf,
}

#[derive(Clone)]
pub struct SkillLoader {
    dir: PathBuf,
    cache: Arc<Mutex<HashMap<String, SkillInfo>>>,
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

impl SkillLoader {
    pub fn new(skills_dir: &std::path::Path) -> SkillLoader {
        std::fs::create_dir_all(skills_dir).ok();
        SkillLoader {
            dir: skills_dir.to_path_buf(),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn scan(&self) -> Vec<ToolDef> {
        let mut tool_defs = Vec::new();
        let mut cache = HashMap::new();
        if self.dir.exists() {
            let entries: Vec<PathBuf> = std::fs::read_dir(&self.dir)
                .ok()
                .into_iter()
                .flat_map(|it| it.collect::<Vec<_>>())
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            for entry in entries {
                let manifest_path = entry.join("manifest.json");
                if !manifest_path.exists() { continue; }
                if let Ok(s) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(m) = serde_json::from_str::<SkillManifest>(&s) {
                        let name = m.name.clone().unwrap_or_else(|| {
                            entry.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
                        });
                        let params = m.parameters.clone().unwrap_or_else(|| {
                            Value::Object(Default::default())
                        });
                        cache.insert(name.clone(), SkillInfo { manifest: m.clone(), path: entry });
                        tool_defs.push(skill_tool(
                            &name,
                            m.description.as_deref().unwrap_or(&name),
                            params,
                        ));
                    }
                }
            }
        }
        *self.cache.lock().unwrap() = cache;
        tool_defs
    }

    pub fn execute(&self, skill_name: &str, args: &Value) -> String {
        let info = self.cache.lock().unwrap().get(skill_name).cloned();
        let Some(info) = info else {
            return format!("Skill not found: {skill_name}");
        };
        let entry_file = info.path.join(info.manifest.entry.clone().unwrap_or_else(|| "run.js".into()));
        if !entry_file.exists() {
            return format!("Entry not found: {}", entry_file.display());
        }
        let kind = info.manifest.kind.clone().unwrap_or_else(|| "node".into());
        let cmd = match kind.as_str() {
            "python" => "python3",
            "node" => "node",
            _ => "bash",
        };

        let mut child = match Command::new(cmd)
            .arg(&entry_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&info.path)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("Skill failed: {e}"),
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(serde_json::to_string(args).unwrap_or_default().as_bytes());
        }

        let (code, stdout, stderr) = run_with_timeout(child, Duration::from_secs(60));
        if !stdout.trim().is_empty() {
            return stdout.trim().to_string();
        }
        if code != 0 && !stderr.trim().is_empty() {
            return format!("Skill error: {}", stderr.trim());
        }
        if code != 0 {
            return format!("Skill failed: exit code {code}");
        }
        "(no output)".to_string()
    }

    pub fn install(&self, github_url: &str) -> String {
        let trimmed = github_url.trim_end_matches('/');
        let name = trimmed
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .trim_end_matches(".git")
            .to_string();
        let dest = self.dir.join(&name);
        if dest.exists() {
            return format!("Skill '{name}' already exists. Remove first.");
        }

        let status = Command::new("git")
            .args(["clone", "--depth", "1", github_url])
            .arg(&dest)
            .status();

        match status {
            Ok(st) if st.success() => {}
            _ => return format!("Clone failed: check the URL and git availability"),
        }

        if !dest.join("manifest.json").exists() {
            std::fs::remove_dir_all(&dest).ok();
            return "Invalid skill: no manifest.json found. Removed.".to_string();
        }

        let req = dest.join("requirements.txt");
        if req.exists() {
            let _ = Command::new("python3")
                .args(["-m", "pip", "install", "-r"])
                .arg(&req)
                .status();
        }

        let pkg = dest.join("package.json");
        if pkg.exists() {
            let _ = Command::new("npm")
                .args(["install", "--production"])
                .current_dir(&dest)
                .status();
        }

        self.scan();
        format!("Installed skill '{name}'.")
    }

    pub fn remove(&self, skill_name: &str) -> String {
        let target = self.dir.join(skill_name);
        if !target.exists() {
            return format!("Skill '{skill_name}' not found.");
        }
        std::fs::remove_dir_all(&target).ok();
        self.cache.lock().unwrap().remove(skill_name);
        format!("Removed skill '{skill_name}'.")
    }

    pub fn list_skills(&self) -> Vec<(String, String, String)> {
        let cache = self.cache.lock().unwrap();
        if cache.is_empty() {
            drop(cache);
            self.scan();
        }
        self.cache
            .lock()
            .unwrap()
            .iter()
            .map(|(name, info)| {
                (
                    name.clone(),
                    info.manifest.description.clone().unwrap_or_default(),
                    info.manifest.version.clone().unwrap_or_default(),
                )
            })
            .collect()
    }
}
