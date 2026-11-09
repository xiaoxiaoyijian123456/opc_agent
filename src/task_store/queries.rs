use super::*;

impl TaskStore {
    pub fn workflow(&self, workflow_id: &str) -> Result<Workflow, String> {
        let connection = self.lock()?;
        connection
            .query_row(
                &format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE id = ?1"),
                [workflow_id],
                workflow_from_row,
            )
            .map_err(|error| format!("Cannot read workflow {workflow_id}: {error}"))
    }

    pub fn task(&self, task_id: &str) -> Result<StoredTask, String> {
        let connection = self.lock()?;
        connection
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                [task_id],
                task_from_row,
            )
            .map_err(|error| format!("Cannot read task {task_id}: {error}"))
    }

    pub fn tasks_for_workflow(&self, workflow_id: &str) -> Result<Vec<StoredTask>, String> {
        self.tasks_where(
            "workflow_id = ?1 ORDER BY created_at, rowid",
            &[workflow_id],
        )
    }

    pub fn completed_tasks(&self, workflow_id: &str) -> Result<Vec<StoredTask>, String> {
        self.tasks_where(
            "workflow_id = ?1 AND status = 'completed' ORDER BY created_at, rowid",
            &[workflow_id],
        )
    }

    fn tasks_where(&self, condition: &str, values: &[&str]) -> Result<Vec<StoredTask>, String> {
        let connection = self.lock()?;
        let sql = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE {condition}");
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(values), task_from_row)
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Cannot list tasks: {error}"))
    }

    pub fn list_tasks(&self) -> Result<Vec<StoredTask>, String> {
        self.tasks_where("1 = 1 ORDER BY created_at DESC, rowid DESC LIMIT 100", &[])
    }

    pub fn list_inbox(&self, role: &str) -> Result<Vec<StoredTask>, String> {
        self.tasks_where(
            "role = ?1 AND status = 'pending' ORDER BY created_at, rowid",
            &[role],
        )
    }

    pub fn status(&self) -> Result<BTreeMap<String, RoleStatus>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT role, status, COUNT(*) FROM tasks WHERE status IN ('pending', 'active') GROUP BY role, status")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, usize>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut status = BTreeMap::new();
        for role in ROLES {
            status.insert(
                role.into(),
                RoleStatus {
                    state: "idle".into(),
                    inbox: 0,
                    active: 0,
                },
            );
        }
        for row in rows {
            let (role, task_status, count) = row.map_err(|error| error.to_string())?;
            if let Some(info) = status.get_mut(&role) {
                if task_status == "active" {
                    info.active = count;
                } else {
                    info.inbox = count;
                }
            }
        }
        for info in status.values_mut() {
            info.state = if info.active > 0 {
                "working"
            } else if info.inbox > 0 {
                "queued"
            } else {
                "idle"
            }
            .into();
        }
        Ok(status)
    }

    pub fn unfinished_workflows(&self) -> Result<Vec<Workflow>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(&format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE status IN ('active', 'pending', 'waiting_approval', 'failed') ORDER BY created_at"))
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], workflow_from_row)
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }
}
