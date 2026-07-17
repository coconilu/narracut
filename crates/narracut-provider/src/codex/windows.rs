use std::{
    fs::{File, OpenOptions},
    future::Future,
    io::Read,
    os::windows::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};

use processkit::{Command, OutputBufferPolicy, ProcessGroup, RunningProcess, Stdin};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::{mpsc, watch},
};

use super::{
    process::{ManagedExitStatus, ManagedJob, ManagedStderr, ManagedStdin, ManagedStdout},
    provider_response_invalid, provider_unavailable, CodexCliRunSpec, ProviderError,
    MAX_JSONL_LINE_BYTES, MAX_STDERR_BYTES, MAX_STDOUT_BYTES,
};

const FILE_SHARE_READ: u32 = 0x0000_0001;
const OUTPUT_CHANNEL_CAPACITY: usize = 32;
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const PROCESSKIT_LINE_CAP: usize = MAX_JSONL_LINE_BYTES + 8 * 1024;

/// Keeps the exact executable bytes read for identity verification locked against ordinary
/// Win32 write/delete/replace opens until `CreateProcess` has returned.
#[derive(Debug)]
pub(crate) struct GuardedExecutable {
    canonical_path: PathBuf,
    executable_hash: String,
    _file: File,
}

impl GuardedExecutable {
    pub(crate) fn open(path: &Path) -> Result<Self, ProviderError> {
        let canonical_path = std::fs::canonicalize(path)
            .map_err(|_| provider_unavailable("无法 canonicalize Codex CLI 可执行文件。", false))?;
        if !canonical_path.is_file() {
            return Err(provider_unavailable(
                "Codex CLI canonical executable 不是普通文件。",
                false,
            ));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&canonical_path)
            .map_err(|_| {
                provider_unavailable(
                    "无法以只读共享锁打开 Codex CLI 可执行文件；执行已安全停止。",
                    false,
                )
            })?;
        let executable_hash = hash_open_file(&mut file)?;
        Ok(Self {
            canonical_path,
            executable_hash,
            _file: file,
        })
    }

    pub(crate) fn open_verified(path: &Path, expected_hash: &str) -> Result<Self, ProviderError> {
        let guard = Self::open(path)?;
        if guard.executable_hash != expected_hash {
            return Err(provider_unavailable(
                "Codex CLI 可执行文件在执行前复核时发生变化；未启动进程。",
                false,
            ));
        }
        Ok(guard)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn hash(&self) -> &str {
        &self.executable_hash
    }
}

fn hash_open_file(file: &mut File) -> Result<String, ProviderError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| provider_unavailable("读取已锁定的 Codex CLI 可执行文件失败。", false))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{encoded}"))
}

#[derive(Debug, Clone, Copy)]
enum OutputFault {
    StdoutLineTooLong,
    StdoutTooLarge,
    StderrTooLarge,
    StdoutBackpressure,
    StderrBackpressure,
    ProcesskitSkippedLine,
}

impl OutputFault {
    fn into_error(self) -> ProviderError {
        let message = match self {
            Self::StdoutLineTooLong => "Codex CLI JSONL 单行超过上限。",
            Self::StdoutTooLarge => "Codex CLI stdout 超过 4 MiB 上限。",
            Self::StderrTooLarge => "Codex CLI stderr 超过 64 KiB 上限。",
            Self::StdoutBackpressure => "Codex CLI stdout 事件通道已满，执行已安全停止。",
            Self::StderrBackpressure => "Codex CLI stderr 事件通道已满，执行已安全停止。",
            Self::ProcesskitSkippedLine => {
                "Codex CLI 输出包含超过进程泵内存上限的完整行，执行已安全停止。"
            }
        };
        provider_response_invalid(message)
    }
}

#[derive(Clone)]
struct OutputPublisher {
    sender: mpsc::Sender<Vec<u8>>,
    fault: watch::Sender<Option<OutputFault>>,
    callback_lines: Arc<AtomicUsize>,
    total_bytes: Arc<AtomicUsize>,
    line_limit: Option<usize>,
    total_limit: usize,
    line_fault: OutputFault,
    total_fault: OutputFault,
    backpressure_fault: OutputFault,
}

impl OutputPublisher {
    fn publish(&self, line: &str) {
        self.callback_lines.fetch_add(1, Ordering::SeqCst);
        if self.line_limit.is_some_and(|limit| line.len() > limit) {
            self.raise(self.line_fault);
            return;
        }

        // processkit normalizes LF/CRLF and strips the terminator. Charging two bytes per
        // decoded line is deliberately conservative and therefore never under-counts raw CRLF.
        let charged = line.len().saturating_add(2);
        let total = self
            .total_bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_add(charged))
            })
            .unwrap_or(usize::MAX)
            .saturating_add(charged);
        if total > self.total_limit {
            self.raise(self.total_fault);
            return;
        }

        let mut framed = Vec::with_capacity(line.len().saturating_add(1));
        framed.extend_from_slice(line.as_bytes());
        framed.push(b'\n');
        if self.sender.try_send(framed).is_err() {
            self.raise(self.backpressure_fault);
        }
    }

    fn raise(&self, fault: OutputFault) {
        self.fault.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(fault);
                true
            } else {
                false
            }
        });
    }
}

struct LineChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
}

impl LineChannelReader {
    fn new(receiver: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            current: Vec::new(),
            offset: 0,
        }
    }
}

impl AsyncRead for LineChannelReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.offset < self.current.len() {
                let remaining = &self.current[self.offset..];
                let copied = remaining.len().min(buffer.remaining());
                buffer.put_slice(&remaining[..copied]);
                self.offset += copied;
                return Poll::Ready(Ok(()));
            }
            match self.receiver.poll_recv(cx) {
                Poll::Ready(Some(next)) => {
                    self.current = next;
                    self.offset = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct ProcesskitJob {
    run: Option<RunningProcess>,
    group: ProcessGroup,
    cached_status: Option<ManagedExitStatus>,
    stdout: Option<ManagedStdout>,
    stderr: Option<ManagedStderr>,
    fault: watch::Receiver<Option<OutputFault>>,
    stdout_callback_lines: Arc<AtomicUsize>,
    stderr_callback_lines: Arc<AtomicUsize>,
}

impl ProcesskitJob {
    fn current_fault(&self) -> Option<ProviderError> {
        self.fault.borrow().map(OutputFault::into_error)
    }

    fn processkit_skipped_line(&self) -> bool {
        self.run.as_ref().is_some_and(|run| {
            run.stdout_line_count() > self.stdout_callback_lines.load(Ordering::SeqCst)
                || run.stderr_line_count() > self.stderr_callback_lines.load(Ordering::SeqCst)
        })
    }

    async fn wait_impl(
        &mut self,
        observe_output_faults: bool,
    ) -> Result<ManagedExitStatus, ProviderError> {
        if let Some(status) = self.cached_status {
            return Ok(status);
        }

        let outcome = loop {
            if observe_output_faults {
                if let Some(error) = self.current_fault() {
                    return Err(error);
                }
                if self.processkit_skipped_line() {
                    return Err(OutputFault::ProcesskitSkippedLine.into_error());
                }
            }

            enum Selected {
                Exited(Result<processkit::Outcome, processkit::Error>),
                FaultChanged,
                PollOutput,
            }

            let selected = {
                let run = self
                    .run
                    .as_mut()
                    .ok_or_else(|| provider_unavailable("Codex CLI 进程句柄已丢失。", false))?;
                let mut processes = [run];
                tokio::select! {
                    result = processkit::wait_any(&mut processes) => {
                        Selected::Exited(result.map(|(_, outcome)| outcome))
                    }
                    changed = self.fault.changed(), if observe_output_faults => {
                        let _ = changed;
                        Selected::FaultChanged
                    }
                    _ = tokio::time::sleep(OUTPUT_POLL_INTERVAL), if observe_output_faults => {
                        Selected::PollOutput
                    }
                }
            };

            match selected {
                Selected::Exited(Ok(outcome)) => break outcome,
                Selected::Exited(Err(_)) => {
                    return Err(provider_unavailable(
                        "无法读取 Codex CLI JobObject 状态。",
                        true,
                    ));
                }
                Selected::FaultChanged | Selected::PollOutput => continue,
            }
        };

        if observe_output_faults {
            if let Some(error) = self.current_fault() {
                return Err(error);
            }
            if self.processkit_skipped_line() {
                return Err(OutputFault::ProcesskitSkippedLine.into_error());
            }
        }

        let status = ManagedExitStatus::new(outcome.code() == Some(0), outcome.code());
        self.cached_status = Some(status);
        let run = self
            .run
            .take()
            .ok_or_else(|| provider_unavailable("Codex CLI 进程句柄已丢失。", false))?;
        let _finished = run
            .finish()
            .await
            .map_err(|_| provider_unavailable("Codex CLI 进程完成收尾失败。", true))?;
        if observe_output_faults {
            if let Some(error) = self.current_fault() {
                return Err(error);
            }
        }
        Ok(status)
    }
}

impl Drop for ProcesskitJob {
    fn drop(&mut self) {
        let _ = self.group.kill_all();
    }
}

impl ManagedJob for ProcesskitJob {
    fn stdin_already_supplied(&self) -> bool {
        true
    }

    fn take_stdin(&mut self) -> Option<ManagedStdin> {
        None
    }

    fn take_stdout(&mut self) -> Option<ManagedStdout> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<ManagedStderr> {
        self.stderr.take()
    }

    fn start_kill(&mut self) -> std::io::Result<()> {
        self.group
            .kill_all()
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn wait(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>> {
        Box::pin(self.wait_impl(true))
    }

    fn reap(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>> {
        Box::pin(self.wait_impl(false))
    }

    fn tree_members(&self) -> Result<Vec<u32>, ProviderError> {
        self.group
            .members()
            .map_err(|_| provider_unavailable("无法读取 Codex CLI JobObject 成员。", false))
    }
}

pub(crate) async fn spawn(spec: &CodexCliRunSpec) -> Result<Box<dyn ManagedJob>, ProviderError> {
    spawn_with_hook(spec, |_| {}).await
}

#[cfg(test)]
async fn spawn_with_test_hook(
    spec: &CodexCliRunSpec,
    before_spawn: impl FnOnce(&GuardedExecutable),
) -> Result<Box<dyn ManagedJob>, ProviderError> {
    spawn_with_hook(spec, before_spawn).await
}

async fn spawn_with_hook(
    spec: &CodexCliRunSpec,
    before_spawn: impl FnOnce(&GuardedExecutable),
) -> Result<Box<dyn ManagedJob>, ProviderError> {
    let guard =
        GuardedExecutable::open_verified(&spec.executable, spec.expected_executable_hash.as_str())?;
    before_spawn(&guard);

    let (fault_tx, fault_rx) = watch::channel(None);
    let (stdout_tx, stdout_rx) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
    let (stderr_tx, stderr_rx) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
    let stdout_callback_lines = Arc::new(AtomicUsize::new(0));
    let stderr_callback_lines = Arc::new(AtomicUsize::new(0));
    let stdout_publisher = OutputPublisher {
        sender: stdout_tx,
        fault: fault_tx.clone(),
        callback_lines: stdout_callback_lines.clone(),
        total_bytes: Arc::new(AtomicUsize::new(0)),
        line_limit: Some(MAX_JSONL_LINE_BYTES),
        total_limit: MAX_STDOUT_BYTES,
        line_fault: OutputFault::StdoutLineTooLong,
        total_fault: OutputFault::StdoutTooLarge,
        backpressure_fault: OutputFault::StdoutBackpressure,
    };
    let stderr_publisher = OutputPublisher {
        sender: stderr_tx,
        fault: fault_tx,
        callback_lines: stderr_callback_lines.clone(),
        total_bytes: Arc::new(AtomicUsize::new(0)),
        line_limit: None,
        total_limit: MAX_STDERR_BYTES,
        line_fault: OutputFault::StderrTooLarge,
        total_fault: OutputFault::StderrTooLarge,
        backpressure_fault: OutputFault::StderrBackpressure,
    };

    let command = Command::new(guard.path())
        .args(&spec.argv)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.environment)
        .stdin(Stdin::from_bytes(spec.stdin.clone()))
        .create_no_window()
        .output_buffer(
            OutputBufferPolicy::bounded(OUTPUT_CHANNEL_CAPACITY)
                .with_max_bytes(PROCESSKIT_LINE_CAP),
        )
        .on_stdout_line(move |line| stdout_publisher.publish(line))
        .on_stderr_line(move |line| stderr_publisher.publish(line));
    let group = ProcessGroup::new()
        .map_err(|_| provider_unavailable("无法创建 Codex CLI JobObject。", false))?;
    let mut run = group
        .start(&command)
        .await
        .map_err(|_| provider_unavailable("无法启动已复核的 Codex CLI JobObject 进程。", true))?;
    let stdout_stream = run
        .stdout_lines()
        .map_err(|_| provider_unavailable("无法启动 Codex CLI 有界输出泵。", false))?;
    drop(stdout_stream);

    // The same-handle guard stays alive until processkit has created the child suspended,
    // assigned it to the Job Object, resumed it, and armed both bounded output pumps.
    drop(guard);
    Ok(Box::new(ProcesskitJob {
        run: Some(run),
        group,
        cached_status: None,
        stdout: Some(Box::new(LineChannelReader::new(stdout_rx))),
        stderr: Some(Box::new(LineChannelReader::new(stderr_rx))),
        fault: fault_rx,
        stdout_callback_lines,
        stderr_callback_lines,
    }))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs,
        io::Write as _,
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::OnceLock,
        time::{Duration, Instant as StdInstant},
    };

    use tempfile::{tempdir, TempDir};
    use tokio::io::AsyncReadExt as _;

    use super::*;
    use crate::{ProviderCancellation, ProviderErrorCode};

    fn compiled_helper() -> PathBuf {
        static HELPER: OnceLock<PathBuf> = OnceLock::new();
        HELPER
            .get_or_init(|| {
                let directory = tempdir().expect("helper build directory").keep();
                let executable = directory.join("windows-process-helper.exe");
                let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests")
                    .join("fixtures")
                    .join("windows-process-helper.rs");
                let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
                let output = Command::new(rustc)
                    .arg(source)
                    .arg("-O")
                    .arg("-o")
                    .arg(&executable)
                    .stdin(Stdio::null())
                    .output()
                    .expect("compile Windows helper");
                assert!(
                    output.status.success(),
                    "helper compilation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                executable
            })
            .clone()
    }

    fn copied_helper(directory: &TempDir, name: &str) -> PathBuf {
        let executable = directory.path().join(name);
        fs::copy(compiled_helper(), &executable).expect("copy Windows helper");
        executable
    }

    fn helper_spec(executable: &Path, cwd: &Path, argv: Vec<String>) -> CodexCliRunSpec {
        let executable_hash = GuardedExecutable::open(executable)
            .expect("hash helper")
            .hash()
            .to_owned();
        CodexCliRunSpec {
            executable: executable.to_owned(),
            expected_executable_hash: executable_hash,
            argv,
            cwd: cwd.to_owned(),
            stdin: Vec::new(),
            environment: super::super::sanitized_environment(),
        }
    }

    async fn run_with_limits(
        spec: CodexCliRunSpec,
        cancellation: ProviderCancellation,
        idle: Duration,
        total: Duration,
    ) -> Result<super::super::CodexCliRunOutput, ProviderError> {
        super::super::process::run_codex_process_with_limits(
            spec,
            cancellation,
            idle,
            total,
            Duration::from_secs(1),
            Duration::from_millis(250),
        )
        .await
    }

    async fn wait_state(path: &Path) -> Vec<(u32, u16)> {
        let deadline = StdInstant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                let entries = text
                    .lines()
                    .filter_map(|line| {
                        let mut fields = line.split_whitespace();
                        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
                    })
                    .collect::<Vec<_>>();
                if entries.len() == 2 {
                    return entries;
                }
            }
            assert!(StdInstant::now() < deadline, "process state timeout");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn assert_ports_released(entries: &[(u32, u16)]) {
        let deadline = StdInstant::now() + Duration::from_secs(5);
        loop {
            let released = entries
                .iter()
                .all(|(_, port)| TcpListener::bind(("127.0.0.1", *port)).map(drop).is_ok());
            if released {
                return;
            }
            assert!(
                StdInstant::now() < deadline,
                "parent/child marker ports must be released"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn windows_jobobject_cancel_terminates_parent_and_child() {
        let directory = tempdir().expect("cancel fixture directory");
        let executable = copied_helper(&directory, "cancel-helper.exe");
        let state = directory.path().join("cancel.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec!["parent-block".to_owned(), state.display().to_string()],
        );
        let cancellation = ProviderCancellation::default();
        let run_cancellation = cancellation.clone();
        let run = tokio::spawn(async move {
            run_with_limits(
                spec,
                run_cancellation,
                Duration::from_secs(10),
                Duration::from_secs(15),
            )
            .await
        });
        let entries = wait_state(&state).await;
        cancellation.cancel();
        let error = run
            .await
            .expect("run task")
            .expect_err("cancellation fails the run");
        assert_eq!(error.code, ProviderErrorCode::Canceled);
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_jobobject_total_timeout_terminates_parent_and_child() {
        let directory = tempdir().expect("timeout fixture directory");
        let executable = copied_helper(&directory, "timeout-helper.exe");
        let state = directory.path().join("timeout.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec!["parent-block".to_owned(), state.display().to_string()],
        );
        let error = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_secs(10),
            Duration::from_millis(1500),
        )
        .await
        .expect_err("total timeout fails");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        let entries = wait_state(&state).await;
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_jobobject_kill_on_drop_terminates_parent_and_child() {
        let directory = tempdir().expect("drop fixture directory");
        let executable = copied_helper(&directory, "drop-helper.exe");
        let state = directory.path().join("drop.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec!["parent-block".to_owned(), state.display().to_string()],
        );
        let job = spawn(&spec).await.expect("spawn JobObject helper");
        let entries = wait_state(&state).await;
        drop(job);
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_inherited_pipe_is_bounded_and_tree_is_reaped() {
        let directory = tempdir().expect("pipe fixture directory");
        let executable = copied_helper(&directory, "pipe-helper.exe");
        let state = directory.path().join("pipe.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec![
                "inherited-pipe-parent".to_owned(),
                state.display().to_string(),
            ],
        );
        let error = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_millis(150),
            Duration::from_secs(2),
        )
        .await
        .expect_err("inherited pipe cannot hang indefinitely");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        let entries = wait_state(&state).await;
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_forbidden_then_block_stops_before_delayed_sentinel() {
        let directory = tempdir().expect("forbidden fixture directory");
        let executable = copied_helper(&directory, "forbidden-helper.exe");
        let state = directory.path().join("forbidden.state");
        let sentinel = directory.path().join("delayed.sentinel");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec![
                "forbidden-block".to_owned(),
                state.display().to_string(),
                sentinel.display().to_string(),
            ],
        );
        let started = StdInstant::now();
        let error = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .await
        .expect_err("forbidden event fails closed");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!sentinel.exists(), "delayed sentinel must never execute");
        let entries = wait_state(&state).await;
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_live_oversize_terminates_parent_and_child() {
        let directory = tempdir().expect("oversize fixture directory");
        let executable = copied_helper(&directory, "oversize-helper.exe");
        let state = directory.path().join("oversize.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec!["oversize-block".to_owned(), state.display().to_string()],
        );
        let error = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .await
        .expect_err("oversized line fails before EOF");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        let entries = wait_state(&state).await;
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_unterminated_oversize_is_memory_bounded_and_times_out_tree() {
        let directory = tempdir().expect("unterminated oversize fixture directory");
        let executable = copied_helper(&directory, "unterminated-oversize-helper.exe");
        let state = directory.path().join("unterminated-oversize.state");
        let spec = helper_spec(
            &executable,
            directory.path(),
            vec![
                "unterminated-oversize-block".to_owned(),
                state.display().to_string(),
            ],
        );
        let error = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_millis(150),
            Duration::from_secs(2),
        )
        .await
        .expect_err("an unterminated fragment is bounded by the idle deadline");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        let entries = wait_state(&state).await;
        assert_ports_released(&entries).await;
    }

    #[tokio::test]
    async fn windows_crash_is_bounded_and_reports_exit_code() {
        let directory = tempdir().expect("crash fixture directory");
        let executable = copied_helper(&directory, "crash-helper.exe");
        let spec = helper_spec(&executable, directory.path(), vec!["crash".to_owned()]);
        let output = run_with_limits(
            spec,
            ProviderCancellation::default(),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .await
        .expect("nonzero exit is returned to Provider classification");
        assert!(!output.success);
        assert_eq!(output.exit_code, Some(7));
        assert!(output.completed_turn.is_none());
    }

    #[test]
    fn windows_guard_blocks_write_delete_rename_and_replace() {
        let directory = tempdir().expect("guard fixture directory");
        let executable = copied_helper(&directory, "guarded.exe");
        let replacement = copied_helper(&directory, "replacement.exe");
        let renamed = directory.path().join("renamed.exe");
        let guard = GuardedExecutable::open(&executable).expect("open executable guard");

        assert!(OpenOptions::new().write(true).open(&executable).is_err());
        assert!(fs::remove_file(&executable).is_err());
        assert!(fs::rename(&executable, &renamed).is_err());
        assert!(fs::copy(&replacement, &executable).is_err());
        assert!(executable.exists());

        drop(guard);
        fs::rename(&executable, &renamed).expect("rename succeeds after guard drop");
        assert!(renamed.exists());
    }

    #[tokio::test]
    async fn windows_guard_closes_probe_to_spawn_replacement_race() {
        let directory = tempdir().expect("race fixture directory");
        let executable = copied_helper(&directory, "race.exe");
        let replacement = directory.path().join("malicious.exe");
        fs::write(&replacement, b"not an executable").expect("write replacement");
        let spec = helper_spec(&executable, directory.path(), vec!["success".to_owned()]);
        let mut replacement_result = None;
        let mut job = spawn_with_test_hook(&spec, |_| {
            replacement_result = Some(fs::rename(&replacement, &executable));
        })
        .await
        .expect("spawn original guarded executable");
        assert!(
            replacement_result.expect("replacement attempted").is_err(),
            "replacement must fail while guard is alive"
        );
        let mut stdout = job.take_stdout().expect("stdout");
        let mut stderr = job.take_stderr().expect("stderr");
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.expect("read stdout");
            bytes
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.expect("read stderr");
            bytes
        });
        let status = job.wait().await.expect("wait original helper");
        assert!(status.success());
        let stdout = stdout_task.await.expect("stdout task");
        let stderr = stderr_task.await.expect("stderr task");
        assert!(stderr.is_empty());
        assert!(String::from_utf8_lossy(&stdout).contains("thread.started"));
    }

    #[test]
    fn windows_same_handle_hash_mismatch_fails_before_spawn() {
        let directory = tempdir().expect("mismatch fixture directory");
        let executable = copied_helper(&directory, "mismatch.exe");
        let original_hash = GuardedExecutable::open(&executable)
            .expect("original hash")
            .hash()
            .to_owned();
        let mut file = OpenOptions::new()
            .append(true)
            .open(&executable)
            .expect("mutate before execution guard");
        file.write_all(b"changed").expect("append mutation");
        let error = GuardedExecutable::open_verified(&executable, &original_hash)
            .expect_err("hash mismatch fails closed");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        assert!(!error.retryable);
    }

    #[test]
    fn helper_spec_environment_does_not_add_test_controls() {
        let directory = tempdir().expect("environment fixture directory");
        let executable = copied_helper(&directory, "environment.exe");
        let spec = helper_spec(&executable, directory.path(), vec!["success".to_owned()]);
        let forbidden = [
            OsString::from("NARRACUT_HELPER_MODE"),
            OsString::from("NARRACUT_FAKE_CODEX_VERSION"),
        ];
        assert!(forbidden
            .iter()
            .all(|key| !spec.environment.contains_key(key)));
        let _: &BTreeMap<OsString, OsString> = &spec.environment;
    }
}
