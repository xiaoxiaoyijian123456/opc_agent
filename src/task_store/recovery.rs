use super::*;

impl TaskStore {
    pub fn recover_active(&self) -> Result<usize, String> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let timestamp = now();
        let count = tx
            .execute(
                "UPDATE tasks SET status = 'failed', error = '执行被中断', updated_at = ?1, completed_at = ?1 WHERE status = 'active'",
                [&timestamp],
            )
            .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE attempts SET status = 'failed', error = '执行被中断', completed_at = ?1 WHERE status = 'active'",
            [&timestamp],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE workflows SET status = 'failed', updated_at = ?1 WHERE status = 'active' AND current_task_id IN (SELECT id FROM tasks WHERE status = 'failed' AND error = '执行被中断')",
            [&timestamp],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(count)
    }

    pub fn attempt_count(&self, task_id: &str) -> Result<u32, String> {
        self.lock()?
            .query_row(
                "SELECT COUNT(*) FROM attempts WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }
}
