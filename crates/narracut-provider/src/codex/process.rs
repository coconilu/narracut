use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::mpsc,
    task::JoinHandle,
    time::Instant,
};

use super::{
    canceled_error, cancellation_failed, protocol::CodexJsonlMachine, provider_internal,
    provider_response_invalid, provider_unavailable, CodexCliRunOutput, CodexCliRunSpec,
    ProviderCancellation, ProviderError, EXECUTION_IDLE_TIMEOUT, EXECUTION_TOTAL_TIMEOUT,
    MAX_JSONL_LINE_BYTES, MAX_STDERR_BYTES, MAX_STDOUT_BYTES, PROCESS_WAIT_TIMEOUT,
};

const POST_EXIT_DRAIN_GRACE: Duration = Duration::from_secs(2);

pub(crate) type ManagedStdin = Box<dyn AsyncWrite + Unpin + Send>;
pub(crate) type ManagedStdout = Box<dyn AsyncRead + Unpin + Send>;
pub(crate) type ManagedStderr = Box<dyn AsyncRead + Unpin + Send>;
type SpawnFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ManagedJob>, ProviderError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ManagedExitStatus {
    success: bool,
    code: Option<i32>,
}

impl ManagedExitStatus {
    pub(crate) const fn new(success: bool, code: Option<i32>) -> Self {
        Self { success, code }
    }

    pub(crate) const fn success(self) -> bool {
        self.success
    }

    pub(crate) const fn code(self) -> Option<i32> {
        self.code
    }
}

pub(crate) trait ManagedJob: Send {
    fn stdin_already_supplied(&self) -> bool {
        false
    }

    fn take_stdin(&mut self) -> Option<ManagedStdin>;
    fn take_stdout(&mut self) -> Option<ManagedStdout>;
    fn take_stderr(&mut self) -> Option<ManagedStderr>;
    fn start_kill(&mut self) -> std::io::Result<()>;
    fn wait(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>>;
    fn reap(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>>;

    fn tree_members(&self) -> Result<Vec<u32>, ProviderError> {
        Ok(Vec::new())
    }
}

pub(crate) trait CodexProcessFactory: Send + Sync {
    fn spawn<'a>(&'a self, spec: &'a CodexCliRunSpec) -> SpawnFuture<'a>;
}

#[derive(Debug, Default)]
pub(crate) struct SystemCodexProcessFactory;

impl CodexProcessFactory for SystemCodexProcessFactory {
    fn spawn<'a>(&'a self, spec: &'a CodexCliRunSpec) -> SpawnFuture<'a> {
        Box::pin(async move {
            #[cfg(windows)]
            {
                return super::windows::spawn(spec).await;
            }
            #[cfg(not(windows))]
            {
                let _ = spec;
                Err(provider_unavailable(
                    "本机 Codex CLI Provider 当前仅支持 Windows Alpha，未启动进程。",
                    false,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ExecutionLimits {
    idle: Duration,
    total: Duration,
    cleanup_reserve: Duration,
    drain_grace: Duration,
}

impl ExecutionLimits {
    const fn production() -> Self {
        Self {
            idle: EXECUTION_IDLE_TIMEOUT,
            total: EXECUTION_TOTAL_TIMEOUT,
            cleanup_reserve: PROCESS_WAIT_TIMEOUT,
            drain_grace: POST_EXIT_DRAIN_GRACE,
        }
    }
}

enum RunOutcome {
    JobWait(Result<ManagedExitStatus, ProviderError>),
    PrimaryFailure(ProviderError),
}

enum IoEvent {
    StdoutLine(Vec<u8>),
    StdinFinished(Result<(), ProviderError>),
    StdoutFinished(Result<(), ProviderError>),
    StderrFinished(Result<usize, ProviderError>),
}

#[derive(Default)]
struct IoState {
    stdin_finished: bool,
    stdout_finished: bool,
    stderr_bytes: Option<usize>,
}

impl IoState {
    fn all_finished(&self) -> bool {
        self.stdin_finished && self.stdout_finished && self.stderr_bytes.is_some()
    }
}

struct IoTasks {
    stdin: Option<JoinHandle<()>>,
    stdout: JoinHandle<()>,
    stderr: JoinHandle<()>,
}

impl IoTasks {
    async fn join(self) -> Result<(), ProviderError> {
        let tasks = [self.stdin, Some(self.stdout), Some(self.stderr)];
        for task in tasks.into_iter().flatten() {
            task.await
                .map_err(|_| provider_internal("Codex CLI IO task 异常退出。"))?;
        }
        Ok(())
    }

    async fn abort_and_join(self) -> Result<(), ProviderError> {
        let mut tasks = [self.stdin, Some(self.stdout), Some(self.stderr)];
        for task in tasks.iter().flatten() {
            task.abort();
        }
        let mut join_failed = false;
        for task in tasks.iter_mut().flatten() {
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    join_failed = true;
                }
            }
        }
        if join_failed {
            Err(cancellation_failed(
                "Codex CLI IO task 清理期间发生异常，无法确认完整回收。",
            ))
        } else {
            Ok(())
        }
    }
}

pub(crate) async fn run_codex_process(
    spec: CodexCliRunSpec,
    cancellation: ProviderCancellation,
) -> Result<CodexCliRunOutput, ProviderError> {
    run_codex_process_with(
        Arc::new(SystemCodexProcessFactory),
        spec,
        cancellation,
        ExecutionLimits::production(),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_codex_process_with_limits(
    spec: CodexCliRunSpec,
    cancellation: ProviderCancellation,
    idle: Duration,
    total: Duration,
    cleanup_reserve: Duration,
    drain_grace: Duration,
) -> Result<CodexCliRunOutput, ProviderError> {
    run_codex_process_with(
        Arc::new(SystemCodexProcessFactory),
        spec,
        cancellation,
        ExecutionLimits {
            idle,
            total,
            cleanup_reserve,
            drain_grace,
        },
    )
    .await
}

async fn run_codex_process_with(
    factory: Arc<dyn CodexProcessFactory>,
    spec: CodexCliRunSpec,
    cancellation: ProviderCancellation,
    limits: ExecutionLimits,
) -> Result<CodexCliRunOutput, ProviderError> {
    let mut child = factory.spawn(&spec).await?;
    let started = Instant::now();
    let hard_deadline = started + limits.total;
    let execution_budget = limits.total.saturating_sub(limits.cleanup_reserve);
    let execution_deadline = started + execution_budget;

    let stdin_already_supplied = child.stdin_already_supplied();
    let stdin = if stdin_already_supplied {
        None
    } else {
        match child.take_stdin() {
            Some(stdin) => Some(stdin),
            None => {
                return finish_without_io(
                    child.as_mut(),
                    provider_internal("Codex CLI 执行缺少 stdin pipe。"),
                    hard_deadline,
                    limits.cleanup_reserve,
                )
                .await;
            }
        }
    };
    let stdout = match child.take_stdout() {
        Some(stdout) => stdout,
        None => {
            return finish_without_io(
                child.as_mut(),
                provider_internal("Codex CLI 执行缺少 stdout pipe。"),
                hard_deadline,
                limits.cleanup_reserve,
            )
            .await;
        }
    };
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            return finish_without_io(
                child.as_mut(),
                provider_internal("Codex CLI 执行缺少 stderr pipe。"),
                hard_deadline,
                limits.cleanup_reserve,
            )
            .await;
        }
    };

    let (tasks, mut events) = spawn_io_tasks(stdin, stdout, stderr, spec.stdin);
    let mut io_state = IoState {
        stdin_finished: stdin_already_supplied,
        ..IoState::default()
    };
    let mut machine = CodexJsonlMachine::default();
    let mut idle_deadline = Instant::now() + limits.idle;

    let outcome = {
        let mut wait = child.wait();
        loop {
            let selected = tokio::select! {
                biased;
                _ = cancellation.cancelled() => RunOutcome::PrimaryFailure(canceled_error()),
                event = events.recv() => {
                    match event {
                        Some(event) => match handle_io_event(event, &mut machine, &mut io_state) {
                            Ok(true) => {
                                idle_deadline = Instant::now() + limits.idle;
                                continue;
                            }
                            Ok(false) => continue,
                            Err(error) => RunOutcome::PrimaryFailure(error),
                        },
                        None if io_state.all_finished() => continue,
                        None => RunOutcome::PrimaryFailure(provider_internal(
                            "Codex CLI IO channel 在所有 task 完成前关闭。",
                        )),
                    }
                }
                status = &mut wait => RunOutcome::JobWait(status),
                _ = tokio::time::sleep_until(idle_deadline) => RunOutcome::PrimaryFailure(
                    provider_unavailable("Codex CLI JSONL 输出空闲超时。", true),
                ),
                _ = tokio::time::sleep_until(execution_deadline) => RunOutcome::PrimaryFailure(
                    provider_unavailable("Codex CLI 执行超过总时限。", true),
                ),
            };
            break selected;
        }
    };
    let status = match outcome {
        RunOutcome::JobWait(Ok(status)) => status,
        RunOutcome::JobWait(Err(error)) => {
            return finish_failed_run(
                child.as_mut(),
                error,
                tasks,
                hard_deadline,
                limits.cleanup_reserve,
            )
            .await;
        }
        RunOutcome::PrimaryFailure(primary) => {
            return finish_failed_run(
                child.as_mut(),
                primary,
                tasks,
                hard_deadline,
                limits.cleanup_reserve,
            )
            .await;
        }
    };

    let drain_deadline = Instant::now() + limits.drain_grace;
    while !io_state.all_finished() {
        let drain_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(canceled_error()),
            event = events.recv() => {
                match event {
                    Some(event) => handle_io_event(event, &mut machine, &mut io_state).map(|_| ()),
                    None if io_state.all_finished() => Ok(()),
                    None => Err(provider_internal(
                        "Codex CLI IO channel 在 drain 完成前关闭。",
                    )),
                }
            }
            _ = tokio::time::sleep_until(drain_deadline) => Err(provider_response_invalid(
                "Codex CLI Job 已退出，但 IO 未在 drain grace 内完成。",
            )),
            _ = tokio::time::sleep_until(hard_deadline) => Err(provider_unavailable(
                "Codex CLI 执行达到绝对总时限。",
                true,
            )),
        };
        if let Err(primary) = drain_result {
            let io_cleanup = tasks.abort_and_join().await;
            return Err(resolve_cleanup_priority(primary, Ok(()), io_cleanup));
        }
    }

    tasks.join().await?;
    let tree_deadline = capped_deadline(Instant::now() + limits.drain_grace, hard_deadline);
    match tree_empty_before(child.as_mut(), tree_deadline).await {
        Ok(true) => {}
        Ok(false) => {
            let primary =
                provider_response_invalid("Codex CLI 主进程退出后仍有 JobObject 子进程存活。");
            let cleanup = terminate_and_wait(
                child.as_mut(),
                cleanup_deadline(hard_deadline, limits.cleanup_reserve),
            )
            .await;
            return Err(resolve_cleanup_priority(primary, cleanup, Ok(())));
        }
        Err(cleanup) => return Err(cleanup),
    }
    let stderr_bytes = io_state.stderr_bytes.unwrap_or_default();
    let completed_turn = if status.success() {
        Some(machine.finish()?)
    } else {
        None
    };
    Ok(CodexCliRunOutput {
        success: status.success(),
        exit_code: status.code(),
        completed_turn,
        stderr_bytes,
    })
}

fn handle_io_event(
    event: IoEvent,
    machine: &mut CodexJsonlMachine,
    state: &mut IoState,
) -> Result<bool, ProviderError> {
    match event {
        IoEvent::StdoutLine(line) => {
            machine.feed_line(&line)?;
            Ok(true)
        }
        IoEvent::StdinFinished(result) => {
            result?;
            state.stdin_finished = true;
            Ok(false)
        }
        IoEvent::StdoutFinished(result) => {
            result?;
            state.stdout_finished = true;
            Ok(false)
        }
        IoEvent::StderrFinished(result) => {
            state.stderr_bytes = Some(result?);
            Ok(false)
        }
    }
}

fn spawn_io_tasks(
    stdin: Option<ManagedStdin>,
    stdout: ManagedStdout,
    stderr: ManagedStderr,
    stdin_bytes: Vec<u8>,
) -> (IoTasks, mpsc::Receiver<IoEvent>) {
    let (events, receiver) = mpsc::channel(32);
    let stdin_task = stdin.map(|mut stdin| {
        let stdin_events = events.clone();
        tokio::spawn(async move {
            let result = async {
                stdin
                    .write_all(&stdin_bytes)
                    .await
                    .map_err(|_| provider_unavailable("Codex CLI stdin 写入失败。", true))?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|_| provider_unavailable("Codex CLI stdin 关闭失败。", true))
            }
            .await;
            let _ = stdin_events.send(IoEvent::StdinFinished(result)).await;
        })
    });
    let stdout_events = events.clone();
    let stdout_task = tokio::spawn(async move {
        let result = read_stdout_lines(stdout, &stdout_events).await;
        let _ = stdout_events.send(IoEvent::StdoutFinished(result)).await;
    });
    let stderr_task = tokio::spawn(async move {
        let result = read_stderr_bounded(stderr).await;
        let _ = events.send(IoEvent::StderrFinished(result)).await;
    });
    (
        IoTasks {
            stdin: stdin_task,
            stdout: stdout_task,
            stderr: stderr_task,
        },
        receiver,
    )
}

async fn read_stdout_lines(
    mut reader: ManagedStdout,
    events: &mpsc::Sender<IoEvent>,
) -> Result<(), ProviderError> {
    let mut total = 0_usize;
    let mut pending = Vec::with_capacity(8 * 1024);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| provider_unavailable("Codex CLI stdout 读取失败。", true))?;
        if read == 0 {
            if !pending.is_empty() {
                events
                    .send(IoEvent::StdoutLine(pending))
                    .await
                    .map_err(|_| provider_internal("Codex CLI JSONL channel 已关闭。"))?;
            }
            return Ok(());
        }
        total = total.saturating_add(read);
        if total > MAX_STDOUT_BYTES {
            return Err(provider_response_invalid(
                "Codex CLI stdout 超过 4 MiB 上限。",
            ));
        }
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                if pending.last() == Some(&b'\r') {
                    pending.pop();
                }
                let line = std::mem::replace(&mut pending, Vec::with_capacity(8 * 1024));
                events
                    .send(IoEvent::StdoutLine(line))
                    .await
                    .map_err(|_| provider_internal("Codex CLI JSONL channel 已关闭。"))?;
            } else {
                if pending.len() >= MAX_JSONL_LINE_BYTES {
                    return Err(provider_response_invalid("Codex CLI JSONL 单行超过上限。"));
                }
                pending.push(*byte);
            }
        }
    }
}

async fn read_stderr_bounded(mut reader: ManagedStderr) -> Result<usize, ProviderError> {
    let mut total = 0_usize;
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| provider_unavailable("Codex CLI stderr 读取失败。", true))?;
        if read == 0 {
            return Ok(total);
        }
        total = total.saturating_add(read);
        if total > MAX_STDERR_BYTES {
            return Err(provider_response_invalid(
                "Codex CLI stderr 超过 64 KiB 上限。",
            ));
        }
    }
}

async fn finish_without_io<T>(
    child: &mut dyn ManagedJob,
    primary: ProviderError,
    hard_deadline: Instant,
    cleanup_reserve: Duration,
) -> Result<T, ProviderError> {
    let cleanup = terminate_and_wait(child, cleanup_deadline(hard_deadline, cleanup_reserve)).await;
    Err(resolve_cleanup_priority(primary, cleanup, Ok(())))
}

async fn finish_failed_run(
    child: &mut dyn ManagedJob,
    primary: ProviderError,
    tasks: IoTasks,
    hard_deadline: Instant,
    cleanup_reserve: Duration,
) -> Result<CodexCliRunOutput, ProviderError> {
    let process_cleanup =
        terminate_and_wait(child, cleanup_deadline(hard_deadline, cleanup_reserve)).await;
    let io_cleanup = tasks.abort_and_join().await;
    Err(resolve_cleanup_priority(
        primary,
        process_cleanup,
        io_cleanup,
    ))
}

fn resolve_cleanup_priority(
    primary: ProviderError,
    process_cleanup: Result<(), ProviderError>,
    io_cleanup: Result<(), ProviderError>,
) -> ProviderError {
    match process_cleanup {
        Err(cleanup) => cleanup,
        Ok(()) => match io_cleanup {
            Err(cleanup) => cleanup,
            Ok(()) => primary,
        },
    }
}

async fn terminate_and_wait(
    child: &mut dyn ManagedJob,
    deadline: Instant,
) -> Result<(), ProviderError> {
    let kill_result = child.start_kill();
    let wait_result = tokio::time::timeout_at(deadline, child.reap()).await;
    let members_result = tree_empty_before(child, deadline).await;
    if kill_result.is_err() {
        return Err(cancellation_failed(
            "无法确认 Codex CLI JobObject 已接收整树终止请求。",
        ));
    }
    match wait_result {
        Ok(Ok(_)) => {}
        _ => Err(cancellation_failed(
            "Codex CLI JobObject 终止后未在绝对时限内完成整树 wait。",
        ))?,
    }
    match members_result {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(cancellation_failed(
            "Codex CLI JobObject 终止后仍无法确认成员列表已清空。",
        )),
    }
}

async fn tree_empty_before(
    child: &mut dyn ManagedJob,
    deadline: Instant,
) -> Result<bool, ProviderError> {
    loop {
        if child.tree_members()?.is_empty() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn cleanup_deadline(hard_deadline: Instant, cleanup_reserve: Duration) -> Instant {
    capped_deadline(Instant::now() + cleanup_reserve, hard_deadline)
}

fn capped_deadline(candidate: Instant, hard_deadline: Instant) -> Instant {
    std::cmp::min(candidate, hard_deadline)
}

#[derive(Debug)]
pub(crate) struct ProbeProcessOutput {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr_bytes: usize,
}

#[cfg(windows)]
pub(crate) fn run_probe_process(
    spec: CodexCliRunSpec,
    timeout: Duration,
    output_limit: usize,
) -> Result<ProbeProcessOutput, ProviderError> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| provider_internal("无法创建 Codex CLI 探测 runtime。"))?;
        runtime.block_on(run_probe_process_async(spec, timeout, output_limit))
    })
    .join()
    .map_err(|_| provider_internal("Codex CLI 探测线程异常。"))?
}

#[cfg(windows)]
async fn run_probe_process_async(
    spec: CodexCliRunSpec,
    timeout: Duration,
    output_limit: usize,
) -> Result<ProbeProcessOutput, ProviderError> {
    let factory = SystemCodexProcessFactory;
    let mut child = factory.spawn(&spec).await?;
    let started = Instant::now();
    let hard_deadline = started + timeout + PROCESS_WAIT_TIMEOUT;
    let execution_deadline = started + timeout;
    let (Some(mut stdout), Some(mut stderr)) = (child.take_stdout(), child.take_stderr()) else {
        let cleanup = terminate_and_wait(child.as_mut(), hard_deadline).await;
        return Err(resolve_cleanup_priority(
            provider_internal("Codex CLI 探测缺少标准 IO pipe。"),
            cleanup,
            Ok(()),
        ));
    };
    if !child.stdin_already_supplied() {
        let Some(mut stdin) = child.take_stdin() else {
            let cleanup = terminate_and_wait(child.as_mut(), hard_deadline).await;
            return Err(resolve_cleanup_priority(
                provider_internal("Codex CLI 探测缺少 stdin pipe。"),
                cleanup,
                Ok(()),
            ));
        };
        if stdin.shutdown().await.is_err() {
            let cleanup = terminate_and_wait(child.as_mut(), hard_deadline).await;
            return Err(resolve_cleanup_priority(
                provider_unavailable("Codex CLI 探测 stdin 无法关闭。", false),
                cleanup,
                Ok(()),
            ));
        }
    }
    let mut stdout_task =
        tokio::spawn(async move { read_probe_output(&mut stdout, output_limit, "stdout").await });
    let mut stderr_task =
        tokio::spawn(async move { read_probe_output(&mut stderr, output_limit, "stderr").await });
    let wait_result = {
        let wait = child.wait();
        tokio::time::timeout_at(execution_deadline, wait).await
    };
    let status = match wait_result {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            let cleanup = terminate_and_wait(child.as_mut(), hard_deadline).await;
            let io_cleanup = abort_probe_tasks(stdout_task, stderr_task).await;
            return Err(resolve_cleanup_priority(error, cleanup, io_cleanup));
        }
        Err(_) => {
            let cleanup = terminate_and_wait(child.as_mut(), hard_deadline).await;
            let io_cleanup = abort_probe_tasks(stdout_task, stderr_task).await;
            return Err(resolve_cleanup_priority(
                provider_unavailable("Codex CLI 探测超时。", true),
                cleanup,
                io_cleanup,
            ));
        }
    };
    let joined = tokio::time::timeout_at(hard_deadline, async {
        let (stdout, stderr) = tokio::join!(&mut stdout_task, &mut stderr_task);
        let stdout =
            stdout.map_err(|_| provider_internal("Codex CLI 探测 stdout task 异常。"))??;
        let stderr =
            stderr.map_err(|_| provider_internal("Codex CLI 探测 stderr task 异常。"))??;
        Ok::<_, ProviderError>((stdout, stderr))
    })
    .await;
    let (stdout, stderr) = match joined {
        Ok(result) => result?,
        Err(_) => {
            let io_cleanup = abort_probe_tasks(stdout_task, stderr_task).await;
            return Err(resolve_cleanup_priority(
                provider_response_invalid("Codex CLI 探测 IO 未在绝对时限内到达 EOF。"),
                Ok(()),
                io_cleanup,
            ));
        }
    };
    Ok(ProbeProcessOutput {
        success: status.success(),
        stdout,
        stderr_bytes: stderr.len(),
    })
}

#[cfg(windows)]
async fn read_probe_output(
    reader: &mut (dyn AsyncRead + Unpin + Send),
    limit: usize,
    stream: &str,
) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await.map_err(|_| {
            provider_unavailable(format!("Codex CLI 探测 {stream} 读取失败。"), false)
        })?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(provider_response_invalid(format!(
                "Codex CLI 探测 {stream} 超过有界上限。"
            )));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

#[cfg(windows)]
async fn abort_probe_tasks(
    mut stdout: JoinHandle<Result<Vec<u8>, ProviderError>>,
    mut stderr: JoinHandle<Result<Vec<u8>, ProviderError>>,
) -> Result<(), ProviderError> {
    stdout.abort();
    stderr.abort();
    let stdout_result = (&mut stdout).await;
    let stderr_result = (&mut stderr).await;
    if stdout_result.is_err_and(|error| !error.is_cancelled())
        || stderr_result.is_err_and(|error| !error.is_cancelled())
    {
        Err(cancellation_failed("Codex CLI 探测 reader task 清理失败。"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    use serde_json::json;
    use tokio::io::{AsyncWriteExt as _, DuplexStream};

    use super::*;
    use crate::ProviderErrorCode;

    #[derive(Default)]
    struct FakeSignals {
        killed: AtomicBool,
        kill_calls: AtomicUsize,
        wait_calls: AtomicUsize,
        reap_calls: AtomicUsize,
        members_calls: AtomicUsize,
    }

    struct FakeJob {
        stdin: Option<ManagedStdin>,
        stdout: Option<ManagedStdout>,
        stderr: Option<ManagedStderr>,
        signals: Arc<FakeSignals>,
        initially_exited: bool,
        kill_fails: bool,
        reap_hangs: bool,
        members_never_empty: bool,
    }

    impl FakeJob {
        fn status_result(
            &self,
            force_pending: bool,
        ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>>
        {
            let initially_exited = self.initially_exited;
            let signals = self.signals.clone();
            Box::pin(async move {
                if !force_pending && (initially_exited || signals.killed.load(Ordering::SeqCst)) {
                    Ok(ManagedExitStatus::new(true, Some(0)))
                } else {
                    std::future::pending().await
                }
            })
        }
    }

    impl ManagedJob for FakeJob {
        fn take_stdin(&mut self) -> Option<ManagedStdin> {
            self.stdin.take()
        }

        fn take_stdout(&mut self) -> Option<ManagedStdout> {
            self.stdout.take()
        }

        fn take_stderr(&mut self) -> Option<ManagedStderr> {
            self.stderr.take()
        }

        fn start_kill(&mut self) -> std::io::Result<()> {
            self.signals.kill_calls.fetch_add(1, Ordering::SeqCst);
            self.signals.killed.store(true, Ordering::SeqCst);
            if self.kill_fails {
                Err(std::io::Error::other("injected kill failure"))
            } else {
                Ok(())
            }
        }

        fn wait(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>>
        {
            self.signals.wait_calls.fetch_add(1, Ordering::SeqCst);
            self.status_result(false)
        }

        fn reap(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<ManagedExitStatus, ProviderError>> + Send + '_>>
        {
            self.signals.reap_calls.fetch_add(1, Ordering::SeqCst);
            self.status_result(self.reap_hangs)
        }

        fn tree_members(&self) -> Result<Vec<u32>, ProviderError> {
            self.signals.members_calls.fetch_add(1, Ordering::SeqCst);
            if self.members_never_empty {
                Ok(vec![4242])
            } else {
                Ok(Vec::new())
            }
        }
    }

    struct FakeFactory {
        job: Mutex<Option<FakeJob>>,
    }

    impl CodexProcessFactory for FakeFactory {
        fn spawn<'a>(&'a self, _spec: &'a CodexCliRunSpec) -> SpawnFuture<'a> {
            Box::pin(async move {
                self.job
                    .lock()
                    .expect("fake factory lock")
                    .take()
                    .map(|job| Box::new(job) as Box<dyn ManagedJob>)
                    .ok_or_else(|| provider_internal("fake job already spawned"))
            })
        }
    }

    struct FakeHandles {
        stdout: DuplexStream,
        _stderr: DuplexStream,
        _stdin: DuplexStream,
    }

    fn fake_factory(
        initially_exited: bool,
        kill_fails: bool,
    ) -> (Arc<dyn CodexProcessFactory>, FakeHandles, Arc<FakeSignals>) {
        fake_factory_with(initially_exited, kill_fails, false, false)
    }

    fn fake_factory_with(
        initially_exited: bool,
        kill_fails: bool,
        reap_hangs: bool,
        members_never_empty: bool,
    ) -> (Arc<dyn CodexProcessFactory>, FakeHandles, Arc<FakeSignals>) {
        let (stdin, stdin_peer) = tokio::io::duplex(1024);
        let (stdout_peer, stdout) = tokio::io::duplex(1024 * 1024);
        let (stderr_peer, stderr) = tokio::io::duplex(1024);
        let signals = Arc::new(FakeSignals::default());
        let job = FakeJob {
            stdin: Some(Box::new(stdin)),
            stdout: Some(Box::new(stdout)),
            stderr: Some(Box::new(stderr)),
            signals: signals.clone(),
            initially_exited,
            kill_fails,
            reap_hangs,
            members_never_empty,
        };
        (
            Arc::new(FakeFactory {
                job: Mutex::new(Some(job)),
            }),
            FakeHandles {
                stdout: stdout_peer,
                _stderr: stderr_peer,
                _stdin: stdin_peer,
            },
            signals,
        )
    }

    fn fixture_spec() -> CodexCliRunSpec {
        CodexCliRunSpec {
            executable: PathBuf::from("fixture-codex.exe"),
            expected_executable_hash: format!("sha256:{}", "a".repeat(64)),
            argv: vec!["exec".to_owned()],
            cwd: PathBuf::from("fixture-capsule"),
            stdin: Vec::new(),
            environment: BTreeMap::new(),
        }
    }

    fn test_limits(idle: Duration, total: Duration, cleanup_reserve: Duration) -> ExecutionLimits {
        ExecutionLimits {
            idle,
            total,
            cleanup_reserve,
            drain_grace: Duration::from_millis(30),
        }
    }

    #[tokio::test]
    async fn bounded_jsonl_reader_rejects_live_line_overflow() {
        let (mut writer, reader) = tokio::io::duplex(MAX_JSONL_LINE_BYTES + 16);
        let (events, mut receiver) = mpsc::channel(8);
        let reader_events = events.clone();
        let reader_task =
            tokio::spawn(async move { read_stdout_lines(Box::new(reader), &reader_events).await });
        let writer_task = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; MAX_JSONL_LINE_BYTES + 1])
                .await
                .expect("write oversized line");
        });
        writer_task.await.expect("writer task");
        let error = reader_task
            .await
            .expect("reader task")
            .expect_err("line overflow fails before EOF");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn forbidden_event_kills_job_immediately() {
        let (factory, mut handles, signals) = fake_factory(false, false);
        for event in [
            json!({"type":"thread.started","thread_id":"thread_fixture"}),
            json!({"type":"turn.started"}),
            json!({
                "type":"item.started",
                "item":{"id":"tool","type":"command_execution","command":"secret"}
            }),
        ] {
            handles
                .stdout
                .write_all(&serde_json::to_vec(&event).expect("JSON line"))
                .await
                .expect("write event");
            handles.stdout.write_all(b"\n").await.expect("newline");
        }
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_millis(200),
            ),
        )
        .await
        .expect_err("forbidden event fails");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert_eq!(signals.kill_calls.load(Ordering::SeqCst), 1);
        assert_eq!(signals.wait_calls.load(Ordering::SeqCst), 1);
        assert_eq!(signals.reap_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn idle_timeout_has_bounded_cleanup() {
        let (factory, _handles, signals) = fake_factory(false, false);
        let started = Instant::now();
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_millis(30),
                Duration::from_millis(250),
                Duration::from_millis(80),
            ),
        )
        .await
        .expect_err("silent job idles out");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        assert!(error.message.contains("空闲"));
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(signals.kill_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn absolute_total_reserves_cleanup_before_hard_deadline() {
        let (factory, _handles, signals) = fake_factory(false, false);
        let total = Duration::from_millis(140);
        let cleanup = Duration::from_millis(50);
        let started = Instant::now();
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(Duration::from_secs(1), total, cleanup),
        )
        .await
        .expect_err("absolute execution deadline fires");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        assert!(error.message.contains("总时限"));
        assert!(started.elapsed() < total + Duration::from_millis(50));
        assert_eq!(signals.kill_calls.load(Ordering::SeqCst), 1);
        assert_eq!(signals.wait_calls.load(Ordering::SeqCst), 1);
        assert_eq!(signals.reap_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn post_exit_drain_is_bounded_and_does_not_kill_reaped_job() {
        let (factory, _handles, signals) = fake_factory(true, false);
        let started = Instant::now();
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_secs(1),
                Duration::from_millis(250),
                Duration::from_millis(80),
            ),
        )
        .await
        .expect_err("inherited pipe is bounded by drain grace");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert!(error.message.contains("drain grace"));
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(signals.kill_calls.load(Ordering::SeqCst), 0);
        assert_eq!(signals.wait_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cleanup_error_overrides_forbidden_protocol_error() {
        let (factory, mut handles, signals) = fake_factory(false, true);
        for event in [
            json!({"type":"thread.started","thread_id":"thread_fixture"}),
            json!({"type":"turn.started"}),
            json!({
                "type":"item.started",
                "item":{"id":"tool","type":"command_execution"}
            }),
        ] {
            handles
                .stdout
                .write_all(&serde_json::to_vec(&event).expect("JSON line"))
                .await
                .expect("write event");
            handles.stdout.write_all(b"\n").await.expect("newline");
        }
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_millis(100),
            ),
        )
        .await
        .expect_err("cleanup failure wins");
        assert_eq!(error.code, ProviderErrorCode::CancellationFailed);
        assert!(!error.retryable);
        assert_eq!(signals.kill_calls.load(Ordering::SeqCst), 1);
        assert_eq!(signals.reap_calls.load(Ordering::SeqCst), 1);
        assert!(signals.members_calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn reap_timeout_is_bounded_and_overrides_primary_error() {
        let (factory, _handles, signals) = fake_factory_with(false, false, true, false);
        let started = Instant::now();
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_millis(20),
                Duration::from_millis(180),
                Duration::from_millis(60),
            ),
        )
        .await
        .expect_err("a stuck reap is a cleanup failure");
        assert_eq!(error.code, ProviderErrorCode::CancellationFailed);
        assert!(started.elapsed() < Duration::from_millis(180));
        assert_eq!(signals.reap_calls.load(Ordering::SeqCst), 1);
        assert!(signals.members_calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn nonempty_tree_after_reap_is_cleanup_failure() {
        let (factory, _handles, signals) = fake_factory_with(false, false, false, true);
        let error = run_codex_process_with(
            factory,
            fixture_spec(),
            ProviderCancellation::default(),
            test_limits(
                Duration::from_millis(20),
                Duration::from_millis(180),
                Duration::from_millis(60),
            ),
        )
        .await
        .expect_err("a nonempty JobObject is a cleanup failure");
        assert_eq!(error.code, ProviderErrorCode::CancellationFailed);
        assert_eq!(signals.reap_calls.load(Ordering::SeqCst), 1);
        assert!(signals.members_calls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn cleanup_failure_has_priority_over_primary_error() {
        let primary = provider_unavailable("retryable primary", true);
        let cleanup = cancellation_failed("cleanup failed");
        let selected = resolve_cleanup_priority(primary, Err(cleanup), Ok(()));
        assert_eq!(selected.code, ProviderErrorCode::CancellationFailed);
        assert!(!selected.retryable);
    }

    #[test]
    fn accepted_line_is_the_only_idle_activity() {
        let mut machine = CodexJsonlMachine::default();
        let mut state = IoState::default();
        let accepted = handle_io_event(
            IoEvent::StdoutLine(
                serde_json::to_vec(&json!({
                    "type":"thread.started",
                    "thread_id":"thread_fixture"
                }))
                .expect("line"),
            ),
            &mut machine,
            &mut state,
        )
        .expect("valid line");
        assert!(accepted);
        assert!(
            !handle_io_event(IoEvent::StdinFinished(Ok(())), &mut machine, &mut state,)
                .expect("stdin terminal is valid")
        );
    }
}
