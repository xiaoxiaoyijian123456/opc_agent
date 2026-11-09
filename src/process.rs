mod outcome;
mod request;

pub use outcome::ProcessOutcome;
pub use request::{ProcessRequest, MAX_OUTPUT_BYTES, MAX_TIMEOUT, SHELL_TIMEOUT, SKILL_TIMEOUT};

use outcome::ExitState;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const SUMMARY_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub struct ApprovedProcess {
    request: ProcessRequest,
    approval_summary: String,
}

impl ApprovedProcess {
    pub(crate) fn new(request: ProcessRequest, approval_summary: String) -> Self {
        Self {
            request,
            approval_summary,
        }
    }
}

#[derive(Clone)]
pub struct ProcessExecutor {
    log_dir: PathBuf,
    active: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
}

#[derive(Default)]
struct OutputBuffers {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    total: usize,
    limit_exceeded: bool,
    error: Option<String>,
}

enum OutputStream {
    Stdout,
    Stderr,
}

impl ProcessExecutor {
    pub fn new(log_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(log_dir).map_err(|error| {
            format!("Cannot create log directory {}: {error}", log_dir.display())
        })?;
        std::fs::set_permissions(log_dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(|error| {
                format!(
                    "Cannot protect log directory {}: {error}",
                    log_dir.display()
                )
            })?;
        Ok(Self {
            log_dir: log_dir.to_path_buf(),
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn execute(&self, approved: ApprovedProcess) -> Result<ProcessOutcome, String> {
        let request = approved.request;
        let execution_id = uuid::Uuid::new_v4().simple().to_string();
        let log_path = self.log_dir.join(format!("{execution_id}.log"));
        let mut log = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|error| {
                format!(
                    "Cannot create execution log {}: {error}",
                    log_path.display()
                )
            })?;

        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(&request.current_dir)
            .envs(&request.environment)
            .env_remove("MYOPC_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command.stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = error.to_string();
                write_log(
                    &mut log,
                    &request,
                    &approved.approval_summary,
                    LogData {
                        state: ExitState::SpawnError,
                        exit_code: None,
                        error: Some(&message),
                        stdout: &[],
                        stderr: &[],
                    },
                )?;
                return Ok(ProcessOutcome {
                    exit_code: None,
                    state: ExitState::SpawnError,
                    error: Some(message),
                    stdout: String::new(),
                    stderr: String::new(),
                    log_path,
                });
            }
        };

        let pid = child.id();
        let terminated = Arc::new(AtomicBool::new(false));
        self.active
            .lock()
            .map_err(|error| format!("Process registry is poisoned: {error}"))?
            .insert(pid, terminated.clone());

        let buffers = Arc::new(Mutex::new(OutputBuffers::default()));
        let stdout = child.stdout.take().ok_or("Cannot capture process stdout")?;
        let stderr = child.stderr.take().ok_or("Cannot capture process stderr")?;
        let stdout_reader = spawn_reader(stdout, OutputStream::Stdout, buffers.clone());
        let stderr_reader = spawn_reader(stderr, OutputStream::Stderr, buffers.clone());
        let stdin_writer = match (child.stdin.take(), request.stdin.clone()) {
            (Some(mut stdin), Some(input)) => Some(thread::spawn(move || {
                stdin.write_all(&input).err().and_then(|error| {
                    if error.kind() == std::io::ErrorKind::BrokenPipe {
                        None
                    } else {
                        Some(error.to_string())
                    }
                })
            })),
            _ => None,
        };

        let started = Instant::now();
        let mut exit_status = None;
        let mut forced_state = None;
        let mut forced_error = None;
        loop {
            let read_error = buffers
                .lock()
                .map_err(|error| format!("Output buffer is poisoned: {error}"))?
                .error
                .clone();
            let limit_exceeded = buffers
                .lock()
                .map_err(|error| format!("Output buffer is poisoned: {error}"))?
                .limit_exceeded;
            let next_forced_state = if terminated.load(Ordering::SeqCst) {
                Some(ExitState::Terminated)
            } else if limit_exceeded {
                Some(ExitState::OutputLimit)
            } else if read_error.is_some() {
                Some(ExitState::IoError)
            } else if started.elapsed() >= request.timeout {
                Some(ExitState::TimedOut)
            } else {
                None
            };
            if let Some(state) = next_forced_state {
                forced_error = read_error;
                forced_state = Some(state);
                terminate_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_status = Some(status);
                    terminate_process_group(pid);
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    forced_state = Some(ExitState::IoError);
                    forced_error = Some(error.to_string());
                    terminate_process_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }

        if stdout_reader.join().is_err() || stderr_reader.join().is_err() {
            forced_state = Some(ExitState::IoError);
            forced_error = Some("Output reader thread panicked".into());
        }
        if let Some(writer) = stdin_writer {
            match writer.join() {
                Ok(Some(error)) if forced_state.is_none() => {
                    forced_state = Some(ExitState::IoError);
                    forced_error = Some(error);
                }
                Err(_) if forced_state.is_none() => {
                    forced_state = Some(ExitState::IoError);
                    forced_error = Some("Stdin writer thread panicked".into());
                }
                _ => {}
            }
        }
        self.active
            .lock()
            .map_err(|error| format!("Process registry is poisoned: {error}"))?
            .remove(&pid);

        let output_problem = {
            let buffers = buffers
                .lock()
                .map_err(|error| format!("Output buffer is poisoned: {error}"))?;
            (buffers.limit_exceeded, buffers.error.clone())
        };
        if forced_state.is_none() {
            if output_problem.0 {
                forced_state = Some(ExitState::OutputLimit);
            } else if let Some(error) = output_problem.1 {
                forced_state = Some(ExitState::IoError);
                forced_error = Some(error);
            }
        }

        let buffers = buffers
            .lock()
            .map_err(|error| format!("Output buffer is poisoned: {error}"))?;
        let stdout = redact_bytes(&buffers.stdout, &request.redactions);
        let stderr = redact_bytes(&buffers.stderr, &request.redactions);
        let state = forced_state.unwrap_or_else(|| {
            if exit_status.as_ref().is_some_and(|status| status.success()) {
                ExitState::Completed
            } else {
                ExitState::Failed
            }
        });
        let exit_code = exit_status.as_ref().and_then(|status| status.code());
        write_log(
            &mut log,
            &request,
            &approved.approval_summary,
            LogData {
                state,
                exit_code,
                error: forced_error.as_deref(),
                stdout: &stdout,
                stderr: &stderr,
            },
        )?;

        Ok(ProcessOutcome {
            exit_code,
            state,
            error: forced_error,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            log_path,
        })
    }

    pub fn kill_all(&self) {
        let processes = self.active.lock().ok().map(|active| {
            active
                .iter()
                .map(|(pid, terminated)| (*pid, terminated.clone()))
                .collect::<Vec<_>>()
        });
        for (pid, terminated) in processes.unwrap_or_default() {
            terminated.store(true, Ordering::SeqCst);
            terminate_process_group(pid);
        }
    }
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    stream: OutputStream,
    buffers: Arc<Mutex<OutputBuffers>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let Ok(mut buffers) = buffers.lock() else {
                        break;
                    };
                    buffers.total = buffers.total.saturating_add(count);
                    if buffers.total > MAX_OUTPUT_BYTES {
                        buffers.limit_exceeded = true;
                        continue;
                    }
                    match stream {
                        OutputStream::Stdout => buffers.stdout.extend_from_slice(&chunk[..count]),
                        OutputStream::Stderr => buffers.stderr.extend_from_slice(&chunk[..count]),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    if let Ok(mut buffers) = buffers.lock() {
                        buffers.error = Some(error.to_string());
                    }
                    break;
                }
            }
        }
    })
}

fn terminate_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

struct LogData<'a> {
    state: ExitState,
    exit_code: Option<i32>,
    error: Option<&'a str>,
    stdout: &'a [u8],
    stderr: &'a [u8],
}

fn write_log(
    log: &mut File,
    request: &ProcessRequest,
    approval_summary: &str,
    data: LogData<'_>,
) -> Result<(), String> {
    let header = format!(
        "Approval:\n{}\nWorking directory: {}\nTimeout: {:?}\nState: {:?}\nExit code: {:?}\nError: {}\n",
        request.redact(approval_summary),
        request.redact(&request.current_dir.display().to_string()),
        request.timeout,
        data.state,
        data.exit_code,
        request.redact(data.error.unwrap_or("none"))
    );
    log.write_all(header.as_bytes())
        .and_then(|_| log.write_all(b"\n--- stdout ---\n"))
        .and_then(|_| log.write_all(data.stdout))
        .and_then(|_| log.write_all(b"\n--- stderr ---\n"))
        .and_then(|_| log.write_all(data.stderr))
        .and_then(|_| log.sync_all())
        .map_err(|error| format!("Cannot write execution log: {error}"))
}

fn redact_bytes(input: &[u8], secrets: &[String]) -> Vec<u8> {
    let mut output = input.to_vec();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        let needle = secret.as_bytes();
        if needle.is_empty() || needle.len() > output.len() {
            continue;
        }
        let mut redacted = Vec::with_capacity(output.len());
        let mut index = 0;
        while index < output.len() {
            if index + needle.len() <= output.len()
                && &output[index..index + needle.len()] == needle
            {
                redacted.extend_from_slice(b"***");
                index += needle.len();
            } else {
                redacted.push(output[index]);
                index += 1;
            }
        }
        output = redacted;
    }
    output
}

fn bounded_summary(text: &str) -> String {
    if text.len() <= SUMMARY_BYTES {
        return text.to_string();
    }
    let mut end = SUMMARY_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n...[output summary truncated; see the execution log]",
        &text[..end]
    )
}
