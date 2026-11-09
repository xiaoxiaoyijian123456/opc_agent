use crate::redaction::redact_text;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SHELL_TIMEOUT: Duration = Duration::from_secs(120);
pub const SKILL_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProcessRequest {
    pub(super) program: String,
    pub(super) args: Vec<String>,
    pub(super) current_dir: PathBuf,
    pub(super) timeout: Duration,
    pub(super) stdin: Option<Vec<u8>>,
    pub(super) redactions: Vec<String>,
    pub(super) environment: BTreeMap<String, String>,
}

impl ProcessRequest {
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        current_dir: PathBuf,
        timeout: Duration,
        stdin: Option<Vec<u8>>,
        redactions: Vec<String>,
    ) -> Result<Self, String> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err("Process program cannot be empty".into());
        }
        if timeout.is_zero() || timeout > MAX_TIMEOUT {
            return Err(format!(
                "Process timeout must be greater than zero and at most {} seconds",
                MAX_TIMEOUT.as_secs()
            ));
        }
        Ok(Self {
            program,
            args,
            current_dir,
            timeout,
            stdin,
            redactions,
            environment: BTreeMap::new(),
        })
    }

    pub fn shell(
        command: &str,
        current_dir: PathBuf,
        timeout: Duration,
        redactions: Vec<String>,
    ) -> Result<Self, String> {
        if command.trim().is_empty() {
            return Err("Shell command cannot be empty".into());
        }
        Self::new(
            "sh",
            vec!["-c".into(), command.into()],
            current_dir,
            timeout,
            None,
            redactions,
        )
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn redact(&self, text: &str) -> String {
        redact_text(text, &self.redactions)
    }

    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }
}
