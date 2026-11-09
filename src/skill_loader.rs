use crate::approval::{ApprovalKind, ApprovalRequest, ApprovalService};
use crate::process::{ProcessExecutor, ProcessRequest, SHELL_TIMEOUT, SKILL_TIMEOUT};
use crate::tools::{skill_tool, ToolDef};
use crate::ui;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Deserialize, Clone, Debug)]
struct SkillManifest {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    entry: Option<String>,
    parameters: Option<Value>,
    timeout_seconds: Option<u64>,
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
    approval: ApprovalService,
    process: ProcessExecutor,
    redactions: Vec<String>,
}

impl SkillLoader {
    pub fn new(
        skills_dir: &Path,
        approval: ApprovalService,
        process: ProcessExecutor,
        redactions: Vec<String>,
    ) -> Result<SkillLoader, String> {
        std::fs::create_dir_all(skills_dir)
            .map_err(|error| format!("Cannot create skills directory {skills_dir:?}: {error}"))?;
        std::fs::set_permissions(
            skills_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .map_err(|error| format!("Cannot protect skills directory {skills_dir:?}: {error}"))?;
        Ok(SkillLoader {
            dir: skills_dir.to_path_buf(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            approval,
            process,
            redactions,
        })
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
                if !manifest_path.exists() {
                    continue;
                }
                if let Ok(s) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(m) = serde_json::from_str::<SkillManifest>(&s) {
                        let name = m.name.clone().unwrap_or_else(|| {
                            entry
                                .file_name()
                                .map(|f| f.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        });
                        let params = m
                            .parameters
                            .clone()
                            .unwrap_or_else(|| Value::Object(Default::default()));
                        cache.insert(
                            name.clone(),
                            SkillInfo {
                                manifest: m.clone(),
                                path: entry,
                            },
                        );
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
        let info = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(skill_name).cloned());
        let Some(info) = info else {
            return format!("Skill not found: {skill_name}");
        };
        let entry = info
            .manifest
            .entry
            .clone()
            .unwrap_or_else(|| "run.sh".into());
        let entry_file = match resolve_skill_entry(&self.dir, &info.path, &entry) {
            Ok(entry) => entry,
            Err(error) => return format!("Skill error: {error}"),
        };
        let kind = info.manifest.kind.clone().unwrap_or_else(|| "bash".into());
        let cmd = match kind.as_str() {
            "python" => "python3",
            "bash" => "bash",
            _ => return format!("Skill error: unsupported type {kind}"),
        };
        let arguments = match serde_json::to_string(args) {
            Ok(arguments) => arguments,
            Err(error) => return format!("Skill error: invalid arguments: {error}"),
        };
        let timeout = Duration::from_secs(info.manifest.timeout_seconds.unwrap_or(60));
        let request = match ProcessRequest::new(
            cmd,
            vec![entry_file.to_string_lossy().into_owned()],
            info.path.clone(),
            if info.manifest.timeout_seconds.is_some() {
                timeout
            } else {
                SKILL_TIMEOUT
            },
            Some(arguments.as_bytes().to_vec()),
            self.redactions.clone(),
        ) {
            Ok(request) => request,
            Err(error) => return format!("Skill error: {error}"),
        };
        let request = ApprovalRequest::new(
            ApprovalKind::Skill {
                name: skill_name.into(),
                source: info.path,
                entry: entry_file,
                arguments,
            },
            request,
        );
        let approved = match self.approval.authorize(request) {
            Ok(approved) => approved,
            Err(error) => return error,
        };
        match self.process.execute(approved) {
            Ok(outcome) => outcome.llm_summary(),
            Err(error) => format!("Skill error: {error}"),
        }
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
        if !valid_skill_directory_name(&name) {
            return "Invalid skill directory name".into();
        }
        if std::fs::symlink_metadata(&dest).is_ok() {
            return format!("Skill '{name}' already exists. Remove first.");
        }
        let request = match ProcessRequest::new(
            "git",
            vec![
                "clone".into(),
                "--depth".into(),
                "1".into(),
                "--".into(),
                github_url.into(),
                dest.to_string_lossy().into_owned(),
            ],
            self.dir.clone(),
            SHELL_TIMEOUT,
            None,
            self.redactions.clone(),
        ) {
            Ok(request) => request,
            Err(error) => return error,
        };
        let request = ApprovalRequest::new(
            ApprovalKind::SkillInstall {
                url: github_url.into(),
                destination: dest.clone(),
            },
            request,
        );
        let approved = match self.approval.authorize(request) {
            Ok(approved) => approved,
            Err(error) => return error,
        };
        match self.process.execute(approved) {
            Ok(outcome) if outcome.success() => {}
            Ok(outcome) => return outcome.llm_summary(),
            Err(error) => return error,
        }

        let valid_manifest = std::fs::read_to_string(dest.join("manifest.json"))
            .ok()
            .and_then(|source| serde_json::from_str::<SkillManifest>(&source).ok())
            .is_some();
        if !valid_manifest {
            std::fs::remove_dir_all(&dest).ok();
            return "Invalid skill: valid manifest.json is required. Removed.".to_string();
        }

        self.scan();
        format!("Installed skill '{name}'.")
    }

    pub fn remove(&self, skill_name: &str) -> String {
        if !valid_skill_directory_name(skill_name) {
            return "Invalid skill name; use an installed directory name, not a path".into();
        }
        let target = self.dir.join(skill_name);
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(_) => return format!("Skill '{skill_name}' not found."),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return format!("Skill '{skill_name}' is not an installed skill directory.");
        }
        println!("Remove skill '{skill_name}' at {}? [y/N]", target.display());
        let answer = ui::prompt_user_answer();
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            return "Cancelled.".into();
        }
        if let Err(error) = std::fs::remove_dir_all(&target) {
            return format!("Cannot remove skill '{skill_name}': {error}");
        }
        self.scan();
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

fn valid_skill_directory_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

fn resolve_skill_entry(
    skills_dir: &Path,
    skill_dir: &Path,
    entry: &str,
) -> Result<PathBuf, String> {
    let entry_path = Path::new(entry);
    if entry_path.is_absolute()
        || entry_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("Entry must stay inside the skill directory".into());
    }
    let skills_root = skills_dir
        .canonicalize()
        .map_err(|error| format!("Cannot resolve skills directory: {error}"))?;
    let skill_root = skill_dir
        .canonicalize()
        .map_err(|error| format!("Cannot resolve skill directory: {error}"))?;
    if !skill_root.starts_with(&skills_root) {
        return Err("Skill directory escapes the skills root".into());
    }
    let entry_file = skill_root.join(entry_path);
    let resolved = entry_file
        .canonicalize()
        .map_err(|error| format!("Entry not found {}: {error}", entry_file.display()))?;
    if !resolved.starts_with(&skill_root) || !resolved.is_file() {
        return Err("Entry must be a regular file inside the skill directory".into());
    }
    Ok(resolved)
}
