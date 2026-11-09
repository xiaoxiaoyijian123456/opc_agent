use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const ROLES: [&str; 5] = ["pm", "dev", "ui", "tester", "admin"];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub title: String,
    pub content: String,
    pub context: Value,
    pub status: String,
    pub ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RoleStatus {
    pub state: String, // "idle" | "working" | "queued"
    pub inbox: usize,
    pub active: usize,
}

#[derive(Clone)]
pub struct Bus {
    root: PathBuf,
    archive: PathBuf,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn today() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

impl Bus {
    pub fn new(team_root: &std::path::Path) -> Bus {
        let root = team_root.join("communication").join("bus");
        let archive = team_root.join("memory_center").join("archive");
        let bus = Bus { root, archive };
        bus.ensure_dirs();
        bus
    }

    fn ensure_dirs(&self) {
        for role in ROLES {
            std::fs::create_dir_all(self.root.join(role).join("inbox")).ok();
            std::fs::create_dir_all(self.root.join(role).join("active")).ok();
        }
        std::fs::create_dir_all(&self.archive).ok();
    }

    pub fn dispatch(&self, to_role: &str, title: &str, content: &str, opts: DispatchOpts) -> Task {
        let id: String = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let task = Task {
            id: id.clone(),
            from: opts.from_role.clone().unwrap_or_else(|| "boss".into()),
            to: to_role.to_string(),
            msg_type: opts.msg_type.clone().unwrap_or_default(),
            title: title.to_string(),
            content: content.to_string(),
            context: opts.context.unwrap_or(Value::Object(Default::default())),
            status: "pending".into(),
            ts: now_iso(),
            result: None,
            completed_at: None,
        };
        let path = self
            .root
            .join(to_role)
            .join("inbox")
            .join(format!("{id}.json"));
        std::fs::write(path, serde_json::to_string_pretty(&task).unwrap()).ok();
        task
    }

    pub fn claim(&self, role: &str, task_id: &str) -> Option<Task> {
        let src = self
            .root
            .join(role)
            .join("inbox")
            .join(format!("{task_id}.json"));
        if !src.exists() {
            return None;
        }
        let dst = self
            .root
            .join(role)
            .join("active")
            .join(format!("{task_id}.json"));
        let mut task: Task = std::fs::read_to_string(&src)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())?;
        task.status = "active".into();
        std::fs::write(&dst, serde_json::to_string_pretty(&task).unwrap()).ok();
        std::fs::remove_file(&src).ok();
        Some(task)
    }

    pub fn complete(&self, role: &str, task_id: &str, result_summary: &str) -> Option<Task> {
        let src = self
            .root
            .join(role)
            .join("active")
            .join(format!("{task_id}.json"));
        if !src.exists() {
            return None;
        }
        let mut task: Task = std::fs::read_to_string(&src)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())?;
        task.status = "completed".into();
        task.result = Some(result_summary.to_string());
        task.completed_at = Some(now_iso());
        let date_dir = self.archive.join(today());
        std::fs::create_dir_all(&date_dir).ok();
        std::fs::write(
            date_dir.join(format!("{task_id}.json")),
            serde_json::to_string_pretty(&task).unwrap(),
        )
        .ok();
        std::fs::remove_file(&src).ok();
        Some(task)
    }

    pub fn list_inbox(&self, role: &str) -> Vec<Task> {
        self.list_dir(&self.root.join(role).join("inbox"))
    }

    pub fn list_active(&self, role: &str) -> Vec<Task> {
        self.list_dir(&self.root.join(role).join("active"))
    }

    pub fn get_all_status(&self) -> BTreeMap<String, RoleStatus> {
        let mut status = BTreeMap::new();
        for role in ROLES {
            let inbox = self.list_inbox(role).len();
            let active = self.list_active(role).len();
            let state = if active > 0 {
                "working"
            } else if inbox > 0 {
                "queued"
            } else {
                "idle"
            };
            status.insert(
                role.to_string(),
                RoleStatus {
                    state: state.into(),
                    inbox,
                    active,
                },
            );
        }
        status
    }

    fn list_dir(&self, dir: &std::path::Path) -> Vec<Task> {
        let mut tasks = Vec::new();
        if !dir.exists() {
            return tasks;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flat_map(|it| it.collect::<Vec<_>>())
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .map(|e| e.path())
            .collect();
        files.sort();
        for f in files {
            if let Ok(s) = std::fs::read_to_string(&f) {
                if let Ok(t) = serde_json::from_str::<Task>(&s) {
                    tasks.push(t);
                }
            }
        }
        tasks
    }
}

#[derive(Default)]
pub struct DispatchOpts {
    pub from_role: Option<String>,
    pub msg_type: Option<String>,
    pub context: Option<Value>,
}
