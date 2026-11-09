use crate::process::{ApprovedProcess, ProcessRequest};
use crate::ui;
use colored::Colorize;
use std::path::PathBuf;
use std::sync::Arc;

pub trait ExecutionApproval: Send + Sync {
    fn approve(&self, request: &ApprovalRequest) -> bool;
}

#[derive(Clone)]
pub struct ApprovalService {
    reviewer: Arc<dyn ExecutionApproval>,
}

pub struct CliExecutionApproval;

#[derive(Clone, Debug)]
pub enum ApprovalKind {
    Shell {
        command: String,
        description: String,
    },
    Skill {
        name: String,
        source: PathBuf,
        entry: PathBuf,
        arguments: String,
    },
    SkillInstall {
        url: String,
        destination: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    process: ProcessRequest,
}

impl ApprovalRequest {
    pub fn new(kind: ApprovalKind, process: ProcessRequest) -> Self {
        Self { kind, process }
    }

    pub fn redacted_details(&self) -> String {
        let details = match &self.kind {
            ApprovalKind::Shell {
                command,
                description,
            } => format!(
                "Type: Shell command\nCommand: {command}\nPurpose: {}",
                if description.trim().is_empty() {
                    "(not provided)"
                } else {
                    description
                }
            ),
            ApprovalKind::Skill {
                name,
                source,
                entry,
                arguments,
            } => format!(
                "Type: Skill execution\nName: {name}\nSource: {}\nEntry: {}\nArguments: {arguments}",
                source.display(),
                entry.display()
            ),
            ApprovalKind::SkillInstall { url, destination } => format!(
                "Type: Skill installation\nSource: {url}\nDestination: {}",
                destination.display()
            ),
        };
        self.process.redact(&format!(
            "{details}\nWorking directory: {}\nTimeout: {:?}\nMYOPC_API_KEY: not inherited",
            self.process.current_dir().display(),
            self.process.timeout()
        ))
    }
}

impl ExecutionApproval for CliExecutionApproval {
    fn approve(&self, request: &ApprovalRequest) -> bool {
        let prompt = ui::prompt_session();
        println!("{}", "Execution approval required".bold().yellow());
        println!("{}", request.redacted_details());
        println!("Only this execution is covered. Press Enter or anything except y/yes to deny.");
        let answer = prompt.answer();
        matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

impl ApprovalService {
    pub fn cli() -> Self {
        Self::new(Arc::new(CliExecutionApproval))
    }

    pub fn new(reviewer: Arc<dyn ExecutionApproval>) -> Self {
        Self { reviewer }
    }

    pub fn authorize(&self, request: ApprovalRequest) -> Result<ApprovedProcess, String> {
        let summary = request.redacted_details();
        if !self.reviewer.approve(&request) {
            return Err("DENIED: execution was not approved; no process was started".into());
        }
        Ok(ApprovedProcess::new(request.process, summary))
    }
}
