use myopc::approval::{ApprovalKind, ApprovalRequest, ApprovalService, ExecutionApproval};
use myopc::process::{ProcessExecutor, ProcessRequest};
use myopc::safe_write::SafeFileWriter;
use myopc::skill_loader::SkillLoader;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct FixedApproval {
    allowed: bool,
}

impl ExecutionApproval for FixedApproval {
    fn approve(&self, _request: &ApprovalRequest) -> bool {
        self.allowed
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "myopc-execution-{tag}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn approval(allowed: bool) -> ApprovalService {
    ApprovalService::new(Arc::new(FixedApproval { allowed }))
}

fn shell(command: &str, cwd: &Path, timeout: Duration) -> ProcessRequest {
    ProcessRequest::shell(command, cwd.to_path_buf(), timeout, Vec::new()).unwrap()
}

fn authorize(
    service: &ApprovalService,
    command: &str,
    request: ProcessRequest,
) -> myopc::process::ApprovedProcess {
    service
        .authorize(ApprovalRequest::new(
            ApprovalKind::Shell {
                command: command.into(),
                description: "test command".into(),
            },
            request,
        ))
        .unwrap()
}

#[test]
fn denied_shell_does_not_start_a_process() {
    let dir = temp_dir("denied");
    let marker = dir.join("marker");
    let command = format!("printf executed > {}", marker.display());
    let _executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let request = shell(&command, &dir, Duration::from_secs(5));
    let error = approval(false)
        .authorize(ApprovalRequest::new(
            ApprovalKind::Shell {
                command,
                description: "must not run".into(),
            },
            request,
        ))
        .err()
        .unwrap();

    assert!(error.contains("DENIED"));
    assert!(!marker.exists());
    assert_eq!(std::fs::read_dir(dir.join("logs")).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn shell_redacts_secrets_and_drops_api_key_environment() {
    let dir = temp_dir("redaction");
    let secret = "myopc-test-secret";
    let command = format!("printf 'literal=%s env=%s' '{secret}' \"$MYOPC_API_KEY\"");
    let request = ProcessRequest::shell(
        &command,
        dir.clone(),
        Duration::from_secs(5),
        vec![secret.into()],
    )
    .unwrap()
    .with_env("MYOPC_API_KEY", secret);
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let outcome = executor
        .execute(authorize(&approval(true), &command, request))
        .unwrap();

    assert!(outcome.success(), "{}", outcome.llm_summary());
    let summary = outcome.llm_summary();
    assert!(summary.contains("literal=*** env="), "{summary}");
    assert!(!summary.contains(secret));
    let log = std::fs::read_to_string(outcome.log_path()).unwrap();
    assert!(!log.contains(secret));
    assert!(log.contains("Command:"));
    assert_eq!(
        std::fs::metadata(outcome.log_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn large_stdout_and_stderr_are_read_concurrently() {
    let dir = temp_dir("large-output");
    let command = "i=0; while [ \"$i\" -lt 2000 ]; do printf 'out-%s-abcdefghijklmnopqrstuvwxyz\\n' \"$i\"; printf 'err-%s-abcdefghijklmnopqrstuvwxyz\\n' \"$i\" >&2; i=$((i+1)); done";
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let outcome = executor
        .execute(authorize(
            &approval(true),
            command,
            shell(command, &dir, Duration::from_secs(10)),
        ))
        .unwrap();

    assert!(outcome.success(), "{}", outcome.llm_summary());
    assert!(outcome.stdout_summary().contains("summary truncated"));
    assert!(outcome.stderr_summary().contains("summary truncated"));
    let log = std::fs::read_to_string(outcome.log_path()).unwrap();
    assert!(log.contains("out-1999-abcdefghijklmnopqrstuvwxyz"));
    assert!(log.contains("err-1999-abcdefghijklmnopqrstuvwxyz"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn timeout_kills_the_process_and_preserves_a_log() {
    let dir = temp_dir("timeout");
    let marker = dir.join("survived");
    let command = format!(
        "(sleep 0.3; printf survived > {}) & sleep 5",
        marker.display()
    );
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let started = Instant::now();
    let outcome = executor
        .execute(authorize(
            &approval(true),
            &command,
            shell(&command, &dir, Duration::from_millis(80)),
        ))
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!outcome.success());
    assert!(outcome.llm_summary().contains("timed out"));
    assert!(outcome.log_path().is_file());
    std::thread::sleep(Duration::from_millis(450));
    assert!(
        !marker.exists(),
        "a descendant escaped process-group cleanup"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn kill_all_terminates_a_running_process() {
    let dir = temp_dir("kill-all");
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let running = executor.clone();
    let worker_dir = dir.clone();
    let worker = std::thread::spawn(move || {
        running.execute(authorize(
            &approval(true),
            "sleep 5",
            shell("sleep 5", &worker_dir, Duration::from_secs(10)),
        ))
    });
    std::thread::sleep(Duration::from_millis(100));
    executor.kill_all();
    let outcome = worker.join().unwrap().unwrap();

    assert!(!outcome.success());
    assert!(outcome.llm_summary().contains("terminated"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn nonzero_exit_code_and_stderr_are_reported() {
    let dir = temp_dir("nonzero");
    let command = "printf visible; printf problem >&2; exit 7";
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let outcome = executor
        .execute(authorize(
            &approval(true),
            command,
            shell(command, &dir, Duration::from_secs(5)),
        ))
        .unwrap();

    assert_eq!(outcome.exit_code(), Some(7));
    let summary = outcome.llm_summary();
    assert!(summary.contains("code 7"), "{summary}");
    assert!(summary.contains("visible"));
    assert!(summary.contains("problem"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn output_limit_terminates_the_process_explicitly() {
    let dir = temp_dir("output-limit");
    let command = "dd if=/dev/zero bs=1048576 count=11 2>/dev/null";
    let executor = ProcessExecutor::new(&dir.join("logs")).unwrap();
    let outcome = executor
        .execute(authorize(
            &approval(true),
            command,
            shell(command, &dir, Duration::from_secs(10)),
        ))
        .unwrap();

    assert!(!outcome.success());
    assert!(outcome.llm_summary().contains("output limit"));
    assert!(outcome.log_path().is_file());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn safe_writes_reject_escape_and_use_atomic_replacement() {
    let dir = temp_dir("safe-write");
    let root = dir.join("code");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let writer = SafeFileWriter::new(&root).unwrap();

    let written = writer.write("src/main.rs", "fn main() {}\n").unwrap();
    assert_eq!(written.display, "src/main.rs");
    assert_eq!(
        std::fs::read_to_string(written.path).unwrap(),
        "fn main() {}\n"
    );
    for path in [
        "../escape",
        "/absolute",
        ".git/config",
        ".myopc_agent/task",
        ".artifacts/report",
        "artifacts/report",
    ] {
        assert!(writer.write(path, "bad").is_err(), "accepted {path}");
    }

    std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
    assert!(writer.write("linked/escape", "bad").is_err());
    std::fs::create_dir(root.join("directory")).unwrap();
    assert!(writer.write("directory", "bad").is_err());

    let original = outside.join("original");
    std::fs::write(&original, "original").unwrap();
    std::fs::hard_link(&original, root.join("hardlink")).unwrap();
    writer.write("hardlink", "replacement").unwrap();
    assert_eq!(std::fs::read_to_string(original).unwrap(), "original");
    assert_eq!(
        std::fs::read_to_string(root.join("hardlink")).unwrap(),
        "replacement"
    );
    assert_eq!(
        std::fs::metadata(root.join("src/main.rs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    std::fs::remove_dir_all(dir).ok();
}

fn skill_loader(dir: &Path, allowed: bool) -> SkillLoader {
    let process = ProcessExecutor::new(&dir.join("logs")).unwrap();
    SkillLoader::new(&dir.join("skills"), approval(allowed), process, Vec::new()).unwrap()
}

fn write_skill(dir: &Path, script: &str) {
    let skill = dir.join("skills/demo");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("manifest.json"),
        r#"{"name":"demo","type":"bash","entry":"run.sh","parameters":{"type":"object"}}"#,
    )
    .unwrap();
    std::fs::write(skill.join("run.sh"), script).unwrap();
}

#[test]
fn denied_skill_does_not_execute_its_entry() {
    let dir = temp_dir("skill-denied");
    write_skill(&dir, "printf executed > \"$PWD/marker\"\n");
    let loader = skill_loader(&dir, false);
    loader.scan();

    let result = loader.execute("demo", &json!({"input": "value"}));
    assert!(result.contains("DENIED"), "{result}");
    assert!(!dir.join("skills/demo/marker").exists());
    assert!(loader.remove("../escape").contains("Invalid skill name"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn approved_skill_receives_json_through_stdin() {
    let dir = temp_dir("skill-approved");
    write_skill(&dir, "input=$(cat); printf '%s' \"$input\"\n");
    let loader = skill_loader(&dir, true);
    loader.scan();

    let result = loader.execute("demo", &json!({"input": "hello"}));
    assert!(result.contains("{\"input\":\"hello\"}"), "{result}");
    assert!(result.contains("Log:"));
    std::fs::remove_dir_all(dir).ok();
}
