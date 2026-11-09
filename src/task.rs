use crate::bus::Task;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Workflow {
    pub id: String,
    pub request: String,
    pub status: String,
    pub requires_ui: Option<bool>,
    pub ui_reason: Option<String>,
    pub template: Vec<String>,
    pub current_task_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct StoredTask {
    pub id: String,
    pub workflow_id: String,
    pub role: String,
    pub title: String,
    pub content: String,
    pub msg_type: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub artifact_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl StoredTask {
    pub fn runner_task(&self) -> Task {
        Task {
            id: self.id.clone(),
            from: "orchestrator".into(),
            to: self.role.clone(),
            msg_type: self.msg_type.clone(),
            title: self.title.clone(),
            content: self.content.clone(),
            context: json!({ "depends_on": self.depends_on }),
            status: self.status.clone(),
            ts: self.created_at.clone(),
            result: self.result.clone(),
            completed_at: self.completed_at.clone(),
        }
    }

    pub fn as_value(&self) -> Value {
        json!({
            "id": self.id,
            "workflow_id": self.workflow_id,
            "to": self.role,
            "title": self.title,
            "status": self.status,
            "error": self.error,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attempt {
    pub id: String,
    pub task_id: String,
    pub number: u32,
}
