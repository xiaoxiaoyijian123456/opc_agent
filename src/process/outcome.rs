use super::{bounded_summary, MAX_OUTPUT_BYTES};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExitState {
    Completed,
    Failed,
    TimedOut,
    OutputLimit,
    Terminated,
    IoError,
    SpawnError,
}

#[derive(Debug)]
pub struct ProcessOutcome {
    pub(super) exit_code: Option<i32>,
    pub(super) state: ExitState,
    pub(super) error: Option<String>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) log_path: PathBuf,
}

impl ProcessOutcome {
    pub fn success(&self) -> bool {
        self.state == ExitState::Completed && self.exit_code == Some(0)
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn stdout_summary(&self) -> String {
        bounded_summary(&self.stdout)
    }

    pub fn stderr_summary(&self) -> String {
        bounded_summary(&self.stderr)
    }

    pub fn llm_summary(&self) -> String {
        let mut sections = Vec::new();
        if self.success() {
            let stdout = self.stdout_summary();
            sections.push(if stdout.trim().is_empty() {
                "(no output)".into()
            } else {
                stdout.trim().to_string()
            });
        } else {
            let reason = match self.state {
                ExitState::Failed => match self.exit_code {
                    Some(code) => format!("Process exited with code {code}"),
                    None => "Process was terminated by a signal".into(),
                },
                ExitState::TimedOut => "Process timed out".into(),
                ExitState::OutputLimit => format!(
                    "Process exceeded the {} MiB output limit",
                    MAX_OUTPUT_BYTES / 1024 / 1024
                ),
                ExitState::Terminated => "Process was terminated".into(),
                ExitState::IoError => "Process I/O failed".into(),
                ExitState::SpawnError => "Process could not be started".into(),
                ExitState::Completed => unreachable!(),
            };
            if let Some(error) = &self.error {
                sections.push(format!("ERROR: {reason}: {error}"));
            } else {
                sections.push(format!("ERROR: {reason}"));
            }
            let stdout = self.stdout_summary();
            if !stdout.trim().is_empty() {
                sections.push(format!("STDOUT:\n{}", stdout.trim()));
            }
        }
        let stderr = self.stderr_summary();
        if !stderr.trim().is_empty() {
            sections.push(format!("STDERR:\n{}", stderr.trim()));
        }
        sections.push(format!("Log: {}", self.log_path.display()));
        sections.join("\n")
    }
}
