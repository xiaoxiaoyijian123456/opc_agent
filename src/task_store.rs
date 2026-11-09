use crate::bus::{RoleStatus, ROLES};
use crate::task::{Attempt, StoredTask, Workflow};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, Row, Transaction};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

mod queries;
mod recovery;

const WORKFLOW_COLUMNS: &str =
    "id, request, status, requires_ui, ui_reason, template, current_task_id, created_at, updated_at";
const TASK_COLUMNS: &str = "id, workflow_id, role, title, content, msg_type, status, depends_on, result, error, artifact_path, created_at, updated_at, completed_at";

#[derive(Clone)]
pub struct TaskStore {
    connection: Arc<Mutex<Connection>>,
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn workflow_from_row(row: &Row<'_>) -> rusqlite::Result<Workflow> {
    let requires_ui: Option<i64> = row.get(3)?;
    let template: String = row.get(5)?;
    Ok(Workflow {
        id: row.get(0)?,
        request: row.get(1)?,
        status: row.get(2)?,
        requires_ui: requires_ui.map(|value| value != 0),
        ui_reason: row.get(4)?,
        template: serde_json::from_str(&template).unwrap_or_default(),
        current_task_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<StoredTask> {
    let depends_on: String = row.get(7)?;
    Ok(StoredTask {
        id: row.get(0)?,
        workflow_id: row.get(1)?,
        role: row.get(2)?,
        title: row.get(3)?,
        content: row.get(4)?,
        msg_type: row.get(5)?,
        status: row.get(6)?,
        depends_on: serde_json::from_str(&depends_on).unwrap_or_default(),
        result: row.get(8)?,
        error: row.get(9)?,
        artifact_path: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        completed_at: row.get(13)?,
    })
}

impl TaskStore {
    pub fn open(team_root: &Path) -> Result<Self, String> {
        let path = team_root.join("myopc.db");
        let connection = Connection::open(&path)
            .map_err(|error| format!("Cannot open SQLite database {}: {error}", path.display()))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;
                 CREATE TABLE IF NOT EXISTS workflows (
                     id TEXT PRIMARY KEY,
                     request TEXT NOT NULL,
                     status TEXT NOT NULL,
                     requires_ui INTEGER,
                     ui_reason TEXT,
                     template TEXT NOT NULL,
                     current_task_id TEXT,
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS tasks (
                     id TEXT PRIMARY KEY,
                     workflow_id TEXT NOT NULL,
                     role TEXT NOT NULL,
                     title TEXT NOT NULL,
                     content TEXT NOT NULL,
                     msg_type TEXT NOT NULL,
                     status TEXT NOT NULL,
                     depends_on TEXT NOT NULL,
                     result TEXT,
                     error TEXT,
                     artifact_path TEXT,
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL,
                     completed_at TEXT,
                     FOREIGN KEY(workflow_id) REFERENCES workflows(id)
                 );
                 CREATE TABLE IF NOT EXISTS attempts (
                     id TEXT PRIMARY KEY,
                     task_id TEXT NOT NULL,
                     number INTEGER NOT NULL,
                     status TEXT NOT NULL,
                     error TEXT,
                     artifact_path TEXT,
                     started_at TEXT NOT NULL,
                     completed_at TEXT,
                     UNIQUE(task_id, number),
                     FOREIGN KEY(task_id) REFERENCES tasks(id)
                 );",
            )
            .map_err(|error| format!("Cannot initialize SQLite database: {error}"))?;
        let permissions = std::os::unix::fs::PermissionsExt::from_mode(0o600);
        std::fs::set_permissions(&path, permissions).map_err(|error| {
            format!("Cannot protect SQLite database {}: {error}", path.display())
        })?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|error| format!("SQLite mutex is poisoned: {error}"))
    }

    fn workflow_tx(tx: &Transaction<'_>, workflow_id: &str) -> Result<Workflow, String> {
        tx.query_row(
            &format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE id = ?1"),
            [workflow_id],
            workflow_from_row,
        )
        .map_err(|error| format!("Cannot read workflow {workflow_id}: {error}"))
    }

    fn task_tx(tx: &Transaction<'_>, task_id: &str) -> Result<StoredTask, String> {
        tx.query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            [task_id],
            task_from_row,
        )
        .map_err(|error| format!("Cannot read task {task_id}: {error}"))
    }

    fn insert_task(
        tx: &Transaction<'_>,
        workflow_id: &str,
        role: &str,
        title: &str,
        content: &str,
        msg_type: &str,
        depends_on: Vec<String>,
    ) -> Result<StoredTask, String> {
        let task_id = id();
        let timestamp = now();
        let dependencies = serde_json::to_string(&depends_on).map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT INTO tasks (id, workflow_id, role, title, content, msg_type, status, depends_on, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?8)",
            params![task_id, workflow_id, role, title, content, msg_type, dependencies, timestamp],
        )
        .map_err(|error| format!("Cannot create task: {error}"))?;
        tx.execute(
            "UPDATE workflows SET current_task_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![workflow_id, task_id, timestamp],
        )
        .map_err(|error| format!("Cannot update workflow: {error}"))?;
        Self::task_tx(tx, &task_id)
    }

    fn create(
        &self,
        request: &str,
        template: Vec<String>,
        role: &str,
        msg_type: &str,
    ) -> Result<(Workflow, StoredTask), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let workflow_id = id();
        let timestamp = now();
        let template_json = serde_json::to_string(&template).map_err(|error| error.to_string())?;
        let unfinished = tx
            .query_row(
                "SELECT COUNT(*) FROM workflows WHERE status IN ('active', 'pending', 'waiting_approval')",
                [],
                |row| row.get::<_, usize>(0),
            )
            .map_err(|error| error.to_string())?;
        let initial_status = if unfinished == 0 { "active" } else { "pending" };
        tx.execute(
            "INSERT INTO workflows (id, request, status, template, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                workflow_id,
                request,
                initial_status,
                template_json,
                timestamp
            ],
        )
        .map_err(|error| format!("Cannot create workflow: {error}"))?;
        let task = Self::insert_task(
            &tx,
            &workflow_id,
            role,
            &format!("{role}: {request}"),
            request,
            msg_type,
            Vec::new(),
        )?;
        tx.commit().map_err(|error| error.to_string())?;
        drop(connection);
        Ok((self.workflow(&workflow_id)?, task))
    }

    pub fn start_standard(&self, request: &str) -> Result<(Workflow, StoredTask), String> {
        self.create(request, Vec::new(), "pm", "requirement")
    }

    pub fn create_manual(
        &self,
        role: &str,
        request: &str,
    ) -> Result<(Workflow, StoredTask), String> {
        if !ROLES.contains(&role) {
            return Err(format!("Unknown role: {role}"));
        }
        self.create(request, vec![role.into()], role, "manual")
    }

    pub fn begin_attempt(&self, task_id: &str) -> Result<Attempt, String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let task = Self::task_tx(&tx, task_id)?;
        if task.status != "pending" {
            return Err(format!(
                "Task {task_id} is {}, expected pending",
                task.status
            ));
        }
        let active = tx
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status = 'active'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .map_err(|error| error.to_string())?;
        if active > 0 {
            return Err("Another task is already active in this project".into());
        }
        let number = tx
            .query_row(
                "SELECT COALESCE(MAX(number), 0) + 1 FROM attempts WHERE task_id = ?1",
                [task_id],
                |row| row.get::<_, u32>(0),
            )
            .map_err(|error| error.to_string())?;
        let attempt = Attempt {
            id: id(),
            task_id: task_id.into(),
            number,
        };
        let timestamp = now();
        tx.execute(
            "INSERT INTO attempts (id, task_id, number, status, started_at) VALUES (?1, ?2, ?3, 'active', ?4)",
            params![attempt.id, task_id, number, timestamp],
        )
        .map_err(|error| format!("Cannot create attempt: {error}"))?;
        tx.execute(
            "UPDATE tasks SET status = 'active', error = NULL, updated_at = ?2 WHERE id = ?1",
            params![task_id, timestamp],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE workflows SET status = 'active', updated_at = ?2 WHERE id = ?1",
            params![task.workflow_id, timestamp],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(attempt)
    }

    fn complete_attempt(
        tx: &Transaction<'_>,
        attempt: &Attempt,
        result: &str,
        artifact_path: &str,
    ) -> Result<(), String> {
        let timestamp = now();
        tx.execute(
            "UPDATE attempts SET status = 'completed', artifact_path = ?2, completed_at = ?3 WHERE id = ?1",
            params![attempt.id, artifact_path, timestamp],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE tasks SET status = 'completed', result = ?2, artifact_path = ?3, updated_at = ?4, completed_at = ?4 WHERE id = ?1",
            params![attempt.task_id, result, artifact_path, timestamp],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn finish_pm(
        &self,
        attempt: &Attempt,
        result: &str,
        artifact_path: &str,
        requires_ui: bool,
        reason: &str,
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let task = Self::task_tx(&tx, &attempt.task_id)?;
        Self::complete_attempt(&tx, attempt, result, artifact_path)?;
        let template = if requires_ui {
            vec!["pm", "ui", "dev", "tester", "admin"]
        } else {
            vec!["pm", "dev", "tester", "admin"]
        };
        tx.execute(
            "UPDATE workflows SET status = 'waiting_approval', requires_ui = ?2, ui_reason = ?3, template = ?4, updated_at = ?5 WHERE id = ?1",
            params![task.workflow_id, requires_ui, reason, serde_json::to_string(&template).map_err(|error| error.to_string())?, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn finish_role(
        &self,
        attempt: &Attempt,
        result: &str,
        artifact_path: &str,
        next: Option<(&str, &str)>,
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let task = Self::task_tx(&tx, &attempt.task_id)?;
        Self::complete_attempt(&tx, attempt, result, artifact_path)?;
        if let Some((role, content)) = next {
            Self::insert_task(
                &tx,
                &task.workflow_id,
                role,
                &format!("{role}: follow-up"),
                content,
                "implement",
                vec![task.id],
            )?;
            tx.execute(
                "UPDATE workflows SET status = 'active', updated_at = ?2 WHERE id = ?1",
                params![task.workflow_id, now()],
            )
            .map_err(|error| error.to_string())?;
        } else {
            tx.execute(
                "UPDATE workflows SET status = 'completed', current_task_id = NULL, updated_at = ?2 WHERE id = ?1",
                params![task.workflow_id, now()],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn approve(&self, workflow_id: &str, next_role: &str, content: &str) -> Result<(), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let workflow = Self::workflow_tx(&tx, workflow_id)?;
        if workflow.status != "waiting_approval" {
            return Err(format!(
                "Workflow {workflow_id} is not waiting for approval"
            ));
        }
        let previous = workflow.current_task_id.ok_or("PRD task is missing")?;
        Self::insert_task(
            &tx,
            workflow_id,
            next_role,
            &format!("{next_role}: approved PRD"),
            content,
            "implement",
            vec![previous],
        )?;
        tx.execute(
            "UPDATE workflows SET status = 'active', updated_at = ?2 WHERE id = ?1",
            params![workflow_id, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn revise(&self, workflow_id: &str, content: &str) -> Result<(), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let workflow = Self::workflow_tx(&tx, workflow_id)?;
        let previous = workflow.current_task_id.ok_or("PRD task is missing")?;
        Self::insert_task(
            &tx,
            workflow_id,
            "pm",
            "pm: revise PRD",
            content,
            "requirement",
            vec![previous],
        )?;
        tx.execute(
            "UPDATE workflows SET status = 'active', requires_ui = NULL, ui_reason = NULL, template = '[]', updated_at = ?2 WHERE id = ?1",
            params![workflow_id, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn fail(&self, attempt: &Attempt, error: &str) -> Result<(), String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let task = Self::task_tx(&tx, &attempt.task_id)?;
        let timestamp = now();
        tx.execute(
            "UPDATE attempts SET status = 'failed', error = ?2, completed_at = ?3 WHERE id = ?1",
            params![attempt.id, error, timestamp],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "UPDATE tasks SET status = 'failed', error = ?2, updated_at = ?3, completed_at = ?3 WHERE id = ?1",
            params![task.id, error, timestamp],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "UPDATE workflows SET status = 'failed', updated_at = ?2 WHERE id = ?1",
            params![task.workflow_id, timestamp],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.commit().map_err(|db_error| db_error.to_string())
    }

    pub fn retry(&self, task_id: &str) -> Result<String, String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let task = Self::task_tx(&tx, task_id)?;
        if task.status != "failed" {
            return Err(format!(
                "Task {task_id} is {}, expected failed",
                task.status
            ));
        }
        tx.execute(
            "UPDATE tasks SET status = 'pending', error = NULL, completed_at = NULL, updated_at = ?2 WHERE id = ?1",
            params![task_id, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE workflows SET status = 'active', updated_at = ?2 WHERE id = ?1",
            params![task.workflow_id, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(task.workflow_id)
    }

    pub fn cancel(&self, workflow_id: &str) -> Result<Workflow, String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let workflow = Self::workflow_tx(&tx, workflow_id)?;
        if let Some(task_id) = workflow.current_task_id {
            tx.execute(
                "UPDATE tasks SET status = 'cancelled', updated_at = ?2 WHERE id = ?1 AND status IN ('pending', 'waiting_approval')",
                params![task_id, now()],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.execute(
            "UPDATE workflows SET status = 'cancelled', updated_at = ?2 WHERE id = ?1",
            params![workflow_id, now()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        drop(connection);
        self.workflow(workflow_id)
    }
}
