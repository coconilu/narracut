#![forbid(unsafe_code)]

//! NarraCut Renderer v1 adapter boundary.
//!
//! The UI never supplies an executable, argv, filter graph, environment, or output path.
//! Callers freeze a probed [`RendererIdentity`] and pass only validated high-level config.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ADAPTER_ID: &str = "narracut.ffmpeg";
pub const ADAPTER_VERSION: &str = "1.0.0";
pub const MIN_FFMPEG_MAJOR: u64 = 6;
pub const MAX_FFMPEG_MAJOR: u64 = 8;
/// A render commits one immutable snapshot per scene plus the video and render log.
/// Job/StageRun v1 accepts at most 256 artifacts, so the Renderer must stop at 254
/// scenes before accepting work.
pub const MAX_SCENES: usize = 254;
pub const MAX_LOG_BYTES: usize = 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_VERSION_BYTES: usize = 256 * 1024;
const MAX_ENCODERS_BYTES: usize = 4 * 1024 * 1024;
const MAX_FFPROBE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererIdentity {
    pub adapter_id: String,
    pub adapter_version: String,
    pub executable_file_name: String,
    pub executable_path: PathBuf,
    pub executable_hash: String,
    pub ffmpeg_version: String,
    pub ffprobe_file_name: String,
    pub ffprobe_path: PathBuf,
    pub ffprobe_hash: String,
    pub ffprobe_version: String,
    pub capability_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererProbe {
    pub available: bool,
    pub supported: bool,
    pub identity: Option<RendererIdentity>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderCanvas {
    pub width: u32,
    pub height: u32,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderEncoding {
    pub preset: String,
    pub crf: u8,
    pub timeout_ms: u64,
    pub max_temporary_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSceneSpec {
    pub scene_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSpec {
    pub identity: RendererIdentity,
    pub working_directory: PathBuf,
    pub output_path: PathBuf,
    pub audio_path: PathBuf,
    pub canvas: RenderCanvas,
    pub encoding: RenderEncoding,
    pub scenes: Vec<RenderSceneSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderProcessResult {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub stderr_tail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererErrorCode {
    Unavailable,
    Unsupported,
    IdentityChanged,
    InvalidSpec,
    ResourceLimit,
    SpawnFailed,
    ProcessFailed,
    Timeout,
    Canceled,
    CleanupFailed,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererError {
    pub code: RendererErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl RendererError {
    fn new(code: RendererErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for RendererError {}

#[derive(Debug, Clone, Default)]
pub struct RenderCancellation(Arc<AtomicBool>);

impl RenderCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[async_trait]
pub trait RendererAdapter: Send + Sync {
    async fn probe(&self) -> RendererProbe;
    async fn render(
        &self,
        spec: RenderSpec,
        cancellation: RenderCancellation,
        progress: Arc<dyn Fn(f64, String) + Send + Sync>,
    ) -> Result<RenderProcessResult, RendererError>;
}

#[derive(Debug, Default)]
pub struct FfmpegRenderer;

#[async_trait]
impl RendererAdapter for FfmpegRenderer {
    async fn probe(&self) -> RendererProbe {
        match probe_ffmpeg().await {
            Ok(probe) => probe,
            Err(error) => RendererProbe {
                available: false,
                supported: false,
                identity: None,
                video_codecs: Vec::new(),
                audio_codecs: Vec::new(),
                diagnostics: vec![error.message],
            },
        }
    }

    async fn render(
        &self,
        spec: RenderSpec,
        cancellation: RenderCancellation,
        progress: Arc<dyn Fn(f64, String) + Send + Sync>,
    ) -> Result<RenderProcessResult, RendererError> {
        validate_render_spec(&spec)?;
        if cancellation.is_canceled() {
            return Err(RendererError::new(
                RendererErrorCode::Canceled,
                "渲染在启动前已取消。",
                false,
            ));
        }
        execute_ffmpeg(spec, cancellation, progress).await
    }
}

async fn probe_ffmpeg() -> Result<RendererProbe, RendererError> {
    let executable = locate_ffmpeg()?;
    let guarded = GuardedExecutable::open(&executable)?;
    let version_output = run_managed_command(
        &guarded,
        &["-hide_banner".into(), "-version".into()],
        None,
        PROBE_TIMEOUT,
        MAX_VERSION_BYTES,
        "FFmpeg 版本探测",
    )
    .await?;
    if version_output.exit_code != Some(0) {
        return Err(RendererError::new(
            RendererErrorCode::Unsupported,
            "FFmpeg 未返回受支持的有界版本信息。",
            false,
        ));
    }
    let version = parse_ffmpeg_version(&version_output.stdout)?;
    let encoders_output = run_managed_command(
        &guarded,
        &["-hide_banner".into(), "-encoders".into()],
        None,
        PROBE_TIMEOUT,
        MAX_ENCODERS_BYTES,
        "FFmpeg 编码能力探测",
    )
    .await?;
    if encoders_output.exit_code != Some(0) {
        return Err(RendererError::new(
            RendererErrorCode::Unsupported,
            "FFmpeg 编码器清单无效或超出上限。",
            false,
        ));
    }
    let encoders = &encoders_output.stdout;
    let has_x264 = encoders
        .lines()
        .any(|line| line.split_whitespace().any(|field| field == "libx264"));
    let has_aac = encoders
        .lines()
        .any(|line| line.split_whitespace().any(|field| field == "aac"));
    let supported =
        (MIN_FFMPEG_MAJOR..=MAX_FFMPEG_MAJOR).contains(&version.major) && has_x264 && has_aac;
    let capability_hash = hash_bytes(
        format!(
            "{ADAPTER_VERSION}\n{}\nlibx264={has_x264}\naac={has_aac}",
            version
        )
        .as_bytes(),
    );
    let file_name = guarded
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("ffmpeg.exe")
        .to_owned();
    let ffprobe_path = locate_ffprobe(guarded.path())?;
    let ffprobe_guard = GuardedExecutable::open(&ffprobe_path)?;
    let ffprobe_output = run_managed_command(
        &ffprobe_guard,
        &["-hide_banner".into(), "-version".into()],
        None,
        PROBE_TIMEOUT,
        MAX_VERSION_BYTES,
        "FFprobe 版本探测",
    )
    .await?;
    if ffprobe_output.exit_code != Some(0) {
        return Err(RendererError::new(
            RendererErrorCode::Unsupported,
            "FFprobe 未返回受支持的有界版本信息。",
            false,
        ));
    }
    let ffprobe_version = parse_tool_version(&ffprobe_output.stdout, "ffprobe version ")?;
    let ffprobe_file_name = ffprobe_guard
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("ffprobe.exe")
        .to_owned();
    let identity = RendererIdentity {
        adapter_id: ADAPTER_ID.to_owned(),
        adapter_version: ADAPTER_VERSION.to_owned(),
        executable_file_name: file_name,
        executable_path: guarded.path().to_path_buf(),
        executable_hash: guarded.hash().to_owned(),
        ffmpeg_version: version.to_string(),
        ffprobe_file_name,
        ffprobe_path: ffprobe_guard.path().to_path_buf(),
        ffprobe_hash: ffprobe_guard.hash().to_owned(),
        ffprobe_version: ffprobe_version.to_string(),
        capability_hash,
    };
    Ok(RendererProbe {
        available: true,
        supported,
        identity: Some(identity),
        video_codecs: has_x264.then(|| "libx264".to_owned()).into_iter().collect(),
        audio_codecs: has_aac.then(|| "aac".to_owned()).into_iter().collect(),
        diagnostics: if supported {
            Vec::new()
        } else {
            vec![format!(
                "需要 FFmpeg {MIN_FFMPEG_MAJOR}..={MAX_FFMPEG_MAJOR} 且包含 libx264/aac。"
            )]
        },
    })
}

fn locate_ffmpeg() -> Result<PathBuf, RendererError> {
    let path = env::var_os("PATH").ok_or_else(|| {
        RendererError::new(
            RendererErrorCode::Unavailable,
            "PATH 未配置，无法探测 FFmpeg。",
            true,
        )
    })?;
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe"]
    } else {
        &["ffmpeg"]
    };
    for directory in env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return fs::canonicalize(&candidate).map_err(|_| {
                    RendererError::new(
                        RendererErrorCode::Unavailable,
                        "FFmpeg 路径无法规范化。",
                        true,
                    )
                });
            }
        }
    }
    Err(RendererError::new(
        RendererErrorCode::Unavailable,
        "PATH 中未找到 ffmpeg；请安装受支持版本并重新启动 NarraCut。",
        true,
    ))
}

fn locate_ffprobe(ffmpeg_path: &Path) -> Result<PathBuf, RendererError> {
    let file_name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let candidate = ffmpeg_path.with_file_name(file_name);
    if !candidate.is_file() {
        return Err(RendererError::new(
            RendererErrorCode::Unavailable,
            "FFmpeg 同目录中未找到受控 ffprobe。",
            true,
        ));
    }
    fs::canonicalize(candidate).map_err(|_| {
        RendererError::new(
            RendererErrorCode::Unavailable,
            "FFprobe 路径无法规范化。",
            true,
        )
    })
}

fn parse_ffmpeg_version(output: &str) -> Result<Version, RendererError> {
    parse_tool_version(output, "ffmpeg version ")
}

fn parse_tool_version(output: &str, prefix: &str) -> Result<Version, RendererError> {
    let token = output
        .lines()
        .next()
        .and_then(|line| line.strip_prefix(prefix))
        .and_then(|tail| tail.split_whitespace().next())
        .ok_or_else(|| {
            RendererError::new(
                RendererErrorCode::Unsupported,
                "FFmpeg/FFprobe 版本行无法解析。",
                false,
            )
        })?;
    let numeric = token
        .trim_start_matches('n')
        .split('-')
        .next()
        .unwrap_or(token);
    Version::parse(numeric)
        .or_else(|_| {
            let mut parts = numeric.split('.');
            let major = parts.next().unwrap_or("0");
            let minor = parts.next().unwrap_or("0");
            Version::parse(&format!("{major}.{minor}.0"))
        })
        .map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unsupported,
                "FFmpeg/FFprobe 版本不是可识别语义版本。",
                false,
            )
        })
}

fn validate_render_spec(spec: &RenderSpec) -> Result<(), RendererError> {
    if spec.identity.adapter_id != ADAPTER_ID
        || spec.identity.adapter_version != ADAPTER_VERSION
        || spec.scenes.is_empty()
        || spec.scenes.len() > MAX_SCENES
        || spec.canvas.width < 320
        || spec.canvas.width > 3840
        || spec.canvas.height < 180
        || spec.canvas.height > 2160
        || spec.canvas.frame_rate_numerator == 0
        || spec.canvas.frame_rate_denominator == 0
        || !(18..=35).contains(&spec.encoding.crf)
        || !matches!(
            spec.encoding.preset.as_str(),
            "veryfast" | "faster" | "fast" | "medium"
        )
        || spec.encoding.timeout_ms < 1_000
        || spec.encoding.timeout_ms > 7_200_000
        || spec.encoding.max_temporary_bytes < 1024 * 1024
        || spec.encoding.max_temporary_bytes > 20 * 1024 * 1024 * 1024
        || !spec.audio_path.is_file()
        || spec
            .scenes
            .iter()
            .any(|scene| scene.start_ms >= scene.end_ms || !valid_color(&scene.color))
    {
        return Err(RendererError::new(
            RendererErrorCode::InvalidSpec,
            "渲染配置、场景或冻结音频不合法。",
            false,
        ));
    }
    require_child_path(&spec.working_directory, &spec.output_path)?;
    require_child_path(&spec.working_directory, &spec.audio_path)?;
    Ok(())
}

fn require_child_path(root: &Path, candidate: &Path) -> Result<(), RendererError> {
    let root = fs::canonicalize(root)
        .map_err(|_| RendererError::new(RendererErrorCode::Io, "渲染临时目录不可用。", true))?;
    let parent = candidate.parent().ok_or_else(|| {
        RendererError::new(
            RendererErrorCode::InvalidSpec,
            "渲染路径缺少父目录。",
            false,
        )
    })?;
    let parent = fs::canonicalize(parent)
        .map_err(|_| RendererError::new(RendererErrorCode::Io, "渲染路径无法规范化。", true))?;
    if !parent.starts_with(&root) {
        return Err(RendererError::new(
            RendererErrorCode::InvalidSpec,
            "渲染读写路径必须位于受控临时目录。",
            false,
        ));
    }
    Ok(())
}

fn valid_color(value: &str) -> bool {
    value.len() == 8
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn build_ffmpeg_argv(spec: &RenderSpec) -> Vec<String> {
    let frame_rate = format!(
        "{}/{}",
        spec.canvas.frame_rate_numerator, spec.canvas.frame_rate_denominator
    );
    let size = format!("{}x{}", spec.canvas.width, spec.canvas.height);
    let mut argv = vec!["-hide_banner".into(), "-nostdin".into(), "-y".into()];
    for scene in &spec.scenes {
        let duration = (scene.end_ms - scene.start_ms) as f64 / 1000.0;
        argv.extend([
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!(
                "color=c={}:s={size}:r={frame_rate}:d={duration:.3}",
                scene.color
            ),
        ]);
    }
    let audio_start_ms = spec
        .scenes
        .first()
        .map(|scene| scene.start_ms)
        .unwrap_or_default();
    if audio_start_ms > 0 {
        argv.extend([
            "-ss".into(),
            format!("{:.3}", audio_start_ms as f64 / 1000.0),
        ]);
    }
    argv.extend(["-i".into(), spec.audio_path.to_string_lossy().into_owned()]);
    if spec.scenes.len() > 1 {
        let inputs = (0..spec.scenes.len())
            .map(|index| format!("[{index}:v]"))
            .collect::<String>();
        argv.extend([
            "-filter_complex".into(),
            format!("{inputs}concat=n={}:v=1:a=0[v]", spec.scenes.len()),
            "-map".into(),
            "[v]".into(),
        ]);
    } else {
        argv.extend(["-map".into(), "0:v:0".into()]);
    }
    argv.extend([
        "-map".into(),
        format!("{}:a:0", spec.scenes.len()),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        spec.encoding.preset.clone(),
        "-crf".into(),
        spec.encoding.crf.to_string(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-shortest".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        spec.output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

#[derive(Debug)]
struct ManagedCommandOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[cfg(windows)]
async fn run_managed_command(
    executable: &GuardedExecutable,
    argv: &[String],
    current_dir: Option<&Path>,
    timeout: Duration,
    max_output_bytes: usize,
    label: &str,
) -> Result<ManagedCommandOutput, RendererError> {
    use narracut_windows_process::ProcessTerminationBarrier;
    use processkit::{Command, OutputBufferPolicy, ProcessGroup};
    use std::sync::{atomic::AtomicUsize, Mutex};

    let stdout = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
    let total_bytes = Arc::new(AtomicUsize::new(0));
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_callbacks = Arc::new(AtomicUsize::new(0));
    let stderr_callbacks = Arc::new(AtomicUsize::new(0));
    let stdout_capture = stdout.clone();
    let stderr_capture = stderr.clone();
    let stdout_bytes = total_bytes.clone();
    let stderr_bytes = total_bytes.clone();
    let stdout_overflow = overflow.clone();
    let stderr_overflow = overflow.clone();
    let stdout_count = stdout_callbacks.clone();
    let stderr_count = stderr_callbacks.clone();
    let capture_line = move |line: &str,
                             output: &Arc<Mutex<Vec<String>>>,
                             bytes: &Arc<AtomicUsize>,
                             overflow: &Arc<AtomicBool>,
                             count: &Arc<AtomicUsize>| {
        count.fetch_add(1, Ordering::AcqRel);
        let previous = bytes.fetch_add(line.len().saturating_add(1), Ordering::AcqRel);
        if previous.saturating_add(line.len()).saturating_add(1) > max_output_bytes {
            overflow.store(true, Ordering::Release);
            return;
        }
        if let Ok(mut captured) = output.lock() {
            captured.push(line.to_owned());
        }
    };
    let stdout_capture_line = capture_line;
    let mut command = Command::new(executable.path())
        .args(argv)
        .env_clear()
        .output_buffer(OutputBufferPolicy::bounded(1024).with_max_bytes(max_output_bytes.max(1)))
        .create_no_window()
        .on_stdout_line(move |line| {
            stdout_capture_line(
                line,
                &stdout_capture,
                &stdout_bytes,
                &stdout_overflow,
                &stdout_count,
            )
        })
        .on_stderr_line(move |line| {
            capture_line(
                line,
                &stderr_capture,
                &stderr_bytes,
                &stderr_overflow,
                &stderr_count,
            )
        });
    if let Some(current_dir) = current_dir {
        command = command.current_dir(current_dir);
    }
    let group = ProcessGroup::new().map_err(|_| {
        RendererError::new(
            RendererErrorCode::SpawnFailed,
            format!("无法为{label}创建 JobObject。"),
            true,
        )
    })?;
    let mut run = group.start(&command).await.map_err(|_| {
        RendererError::new(
            RendererErrorCode::SpawnFailed,
            format!("无法启动受控{label}。"),
            true,
        )
    })?;
    let stdout_stream = run.stdout_lines().map_err(|_| {
        RendererError::new(
            RendererErrorCode::SpawnFailed,
            format!("无法启动{label}的有界输出泵。"),
            false,
        )
    })?;
    drop(stdout_stream);
    let deadline = tokio::time::Instant::now() + timeout;
    let outcome = loop {
        let skipped_output = run.stdout_line_count() > stdout_callbacks.load(Ordering::Acquire)
            || run.stderr_line_count() > stderr_callbacks.load(Ordering::Acquire);
        if overflow.load(Ordering::Acquire) || skipped_output {
            break Err(RendererError::new(
                RendererErrorCode::ResourceLimit,
                format!("{label}输出超过 {max_output_bytes} 字节上限。"),
                false,
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            break Err(RendererError::new(
                RendererErrorCode::Timeout,
                format!("{label}超过受限执行时间。"),
                true,
            ));
        }
        let mut processes = [&mut run];
        match tokio::time::timeout(
            Duration::from_millis(20),
            processkit::wait_any(&mut processes),
        )
        .await
        {
            Ok(Ok((_, outcome))) => break Ok(outcome),
            Ok(Err(_)) => {
                break Err(RendererError::new(
                    RendererErrorCode::ProcessFailed,
                    format!("无法读取{label} JobObject 状态。"),
                    true,
                ));
            }
            Err(_) => continue,
        }
    };
    if let Err(error) = outcome {
        let members = group.members().unwrap_or_default();
        let barriers = members
            .iter()
            .filter_map(|pid| ProcessTerminationBarrier::open(*pid).ok())
            .collect::<Vec<_>>();
        group.kill_all().map_err(|_| {
            RendererError::new(
                RendererErrorCode::CleanupFailed,
                format!("{label}进程树终止失败。"),
                false,
            )
        })?;
        let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < cleanup_deadline
            && barriers
                .iter()
                .any(|barrier| !barrier.is_signaled().unwrap_or(false))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if barriers
            .iter()
            .any(|barrier| !barrier.is_signaled().unwrap_or(false))
        {
            return Err(RendererError::new(
                RendererErrorCode::CleanupFailed,
                format!("{label}进程树未在清理时限内退出。"),
                false,
            ));
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), run.finish()).await;
        return Err(error);
    }
    let outcome = outcome.expect("managed command outcome checked above");
    let code = outcome.code();
    let skipped_output = run.stdout_line_count() > stdout_callbacks.load(Ordering::Acquire)
        || run.stderr_line_count() > stderr_callbacks.load(Ordering::Acquire);
    let _finished = run.finish().await.map_err(|_| {
        RendererError::new(
            RendererErrorCode::CleanupFailed,
            format!("{label}完成收尾失败。"),
            true,
        )
    })?;
    if overflow.load(Ordering::Acquire) || skipped_output {
        return Err(RendererError::new(
            RendererErrorCode::ResourceLimit,
            format!("{label}输出超过 {max_output_bytes} 字节上限。"),
            false,
        ));
    }
    Ok(ManagedCommandOutput {
        exit_code: code,
        stdout: stdout
            .lock()
            .map(|value| value.join("\n"))
            .unwrap_or_default(),
        stderr: stderr
            .lock()
            .map(|value| value.join("\n"))
            .unwrap_or_default(),
    })
}

#[cfg(not(windows))]
async fn run_managed_command(
    _executable: &GuardedExecutable,
    _argv: &[String],
    _current_dir: Option<&Path>,
    _timeout: Duration,
    _max_output_bytes: usize,
    _label: &str,
) -> Result<ManagedCommandOutput, RendererError> {
    Err(RendererError::new(
        RendererErrorCode::Unsupported,
        "受控 Renderer 进程当前仅支持 Windows Alpha。",
        false,
    ))
}

#[derive(Debug, Deserialize)]
struct FfprobeDocument {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_name: Option<String>,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VerifiedMedia {
    duration_ms: u64,
    width: u32,
    height: u32,
    has_audio: bool,
}

async fn inspect_rendered_media(spec: &RenderSpec) -> Result<VerifiedMedia, RendererError> {
    let guard =
        GuardedExecutable::open_verified(&spec.identity.ffprobe_path, &spec.identity.ffprobe_hash)?;
    let argv = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "stream=codec_name,codec_type,width,height,r_frame_rate:format=format_name,duration".into(),
        "-of".into(),
        "json".into(),
        spec.output_path.to_string_lossy().into_owned(),
    ];
    let output = run_managed_command(
        &guard,
        &argv,
        Some(&spec.working_directory),
        FFPROBE_TIMEOUT,
        MAX_FFPROBE_BYTES,
        "FFprobe 输出校验",
    )
    .await?;
    if output.exit_code != Some(0) {
        return Err(RendererError::new(
            RendererErrorCode::ProcessFailed,
            format!(
                "FFprobe 拒绝候选输出：{}",
                bounded_tail(&output.stderr, 2048)
            ),
            false,
        ));
    }
    let document: FfprobeDocument = serde_json::from_str(&output.stdout).map_err(|_| {
        RendererError::new(
            RendererErrorCode::ProcessFailed,
            "FFprobe 返回的媒体真值不是合法有界 JSON。",
            false,
        )
    })?;
    verify_ffprobe_document(spec, &document)
}

fn verify_ffprobe_document(
    spec: &RenderSpec,
    document: &FfprobeDocument,
) -> Result<VerifiedMedia, RendererError> {
    let format = document.format.as_ref().ok_or_else(media_mismatch)?;
    let format_name = format.format_name.as_deref().ok_or_else(media_mismatch)?;
    if !format_name.split(',').any(|name| name == "mp4") {
        return Err(media_mismatch());
    }
    let video_streams = document
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    let audio_streams = document
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect::<Vec<_>>();
    if video_streams.len() != 1 || audio_streams.len() != 1 {
        return Err(media_mismatch());
    }
    let video = video_streams[0];
    let audio = audio_streams[0];
    if video.codec_name.as_deref() != Some("h264")
        || video.width != Some(spec.canvas.width)
        || video.height != Some(spec.canvas.height)
        || audio.codec_name.as_deref() != Some("aac")
        || !frame_rate_matches(
            video.r_frame_rate.as_deref().unwrap_or_default(),
            spec.canvas.frame_rate_numerator,
            spec.canvas.frame_rate_denominator,
        )
    {
        return Err(media_mismatch());
    }
    let duration_seconds = format
        .duration
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(media_mismatch)?;
    let duration_ms = (duration_seconds * 1_000.0).round() as u64;
    let expected_ms = spec
        .scenes
        .last()
        .map(|scene| scene.end_ms)
        .unwrap_or_default()
        .saturating_sub(
            spec.scenes
                .first()
                .map(|scene| scene.start_ms)
                .unwrap_or_default(),
        );
    let two_frames_ms = (2_000_u64 * u64::from(spec.canvas.frame_rate_denominator))
        .div_ceil(u64::from(spec.canvas.frame_rate_numerator));
    let tolerance_ms = two_frames_ms.max(100);
    if duration_ms.abs_diff(expected_ms) > tolerance_ms {
        return Err(media_mismatch());
    }
    Ok(VerifiedMedia {
        duration_ms,
        width: video.width.expect("width checked above"),
        height: video.height.expect("height checked above"),
        has_audio: true,
    })
}

fn frame_rate_matches(value: &str, expected_numerator: u32, expected_denominator: u32) -> bool {
    let Some((numerator, denominator)) = value.split_once('/') else {
        return false;
    };
    let Ok(numerator) = numerator.parse::<u64>() else {
        return false;
    };
    let Ok(denominator) = denominator.parse::<u64>() else {
        return false;
    };
    denominator != 0
        && numerator * u64::from(expected_denominator)
            == u64::from(expected_numerator) * denominator
}

fn media_mismatch() -> RendererError {
    RendererError::new(
        RendererErrorCode::ProcessFailed,
        "候选输出未通过 MP4/H.264/AAC/画布/帧率/时长真值校验。",
        false,
    )
}

#[cfg(windows)]
async fn execute_ffmpeg(
    spec: RenderSpec,
    cancellation: RenderCancellation,
    progress: Arc<dyn Fn(f64, String) + Send + Sync>,
) -> Result<RenderProcessResult, RendererError> {
    use narracut_windows_process::ProcessTerminationBarrier;
    use processkit::{Command, OutputBufferPolicy, ProcessGroup};
    use std::sync::Mutex;

    let guard = GuardedExecutable::open_verified(
        &spec.identity.executable_path,
        &spec.identity.executable_hash,
    )?;
    let argv = build_ffmpeg_argv(&spec);
    let stdout = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
    let stdout_capture = stdout.clone();
    let stderr_capture = stderr.clone();
    let command = Command::new(guard.path())
        .args(&argv)
        .current_dir(&spec.working_directory)
        .env_clear()
        .output_buffer(OutputBufferPolicy::bounded(1024).with_max_bytes(MAX_LOG_BYTES))
        .create_no_window()
        .on_stdout_line(move |line| {
            if let Ok(mut output) = stdout_capture.lock() {
                if output.iter().map(String::len).sum::<usize>() < MAX_LOG_BYTES {
                    output.push(line.to_owned());
                }
            }
        })
        .on_stderr_line(move |line| {
            if let Ok(mut output) = stderr_capture.lock() {
                if output.iter().map(String::len).sum::<usize>() < MAX_LOG_BYTES {
                    output.push(line.to_owned());
                }
            }
        });
    let group = ProcessGroup::new().map_err(|_| {
        RendererError::new(
            RendererErrorCode::SpawnFailed,
            "无法创建 FFmpeg JobObject。",
            true,
        )
    })?;
    let mut run = group.start(&command).await.map_err(|_| {
        RendererError::new(
            RendererErrorCode::SpawnFailed,
            "无法启动已复核的 FFmpeg。",
            true,
        )
    })?;
    drop(guard);
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_millis(spec.encoding.timeout_ms);
    let outcome = loop {
        if cancellation.is_canceled() {
            break Err(RendererError::new(
                RendererErrorCode::Canceled,
                "渲染已取消。",
                false,
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            break Err(RendererError::new(
                RendererErrorCode::Timeout,
                "FFmpeg 超过受限执行时间。",
                true,
            ));
        }
        let elapsed = started.elapsed().as_millis() as u64;
        let estimate = spec
            .scenes
            .last()
            .map(|scene| scene.end_ms)
            .unwrap_or(1)
            .max(1);
        progress(
            (elapsed as f64 / estimate as f64).clamp(0.05, 0.95),
            "FFmpeg 正在渲染受控内容层".to_owned(),
        );
        let mut processes = [&mut run];
        match tokio::time::timeout(
            Duration::from_millis(100),
            processkit::wait_any(&mut processes),
        )
        .await
        {
            Ok(Ok((_, outcome))) => break Ok(outcome),
            Ok(Err(_)) => {
                break Err(RendererError::new(
                    RendererErrorCode::ProcessFailed,
                    "无法读取 FFmpeg JobObject 状态。",
                    true,
                ))
            }
            Err(_) => continue,
        }
    };
    if outcome.is_err() {
        let members = group.members().unwrap_or_default();
        let barriers = members
            .iter()
            .filter_map(|pid| ProcessTerminationBarrier::open(*pid).ok())
            .collect::<Vec<_>>();
        group.kill_all().map_err(|_| {
            RendererError::new(
                RendererErrorCode::CleanupFailed,
                "FFmpeg 进程树终止失败。",
                false,
            )
        })?;
        let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < cleanup_deadline {
            if barriers
                .iter()
                .all(|barrier| barrier.is_signaled().unwrap_or(false))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if barriers
            .iter()
            .any(|barrier| !barrier.is_signaled().unwrap_or(false))
        {
            return Err(RendererError::new(
                RendererErrorCode::CleanupFailed,
                "FFmpeg 进程树未在清理时限内退出。",
                false,
            ));
        }
    }
    let outcome = outcome?;
    let code = outcome.code();
    let _finished = run.finish().await.map_err(|_| {
        RendererError::new(
            RendererErrorCode::CleanupFailed,
            "FFmpeg 完成收尾失败。",
            true,
        )
    })?;
    if code != Some(0) {
        let tail = bounded_tail(
            &stderr
                .lock()
                .map(|value| value.join("\n"))
                .unwrap_or_default(),
            8192,
        );
        return Err(RendererError::new(
            RendererErrorCode::ProcessFailed,
            format!("FFmpeg 退出码 {code:?}：{tail}"),
            false,
        ));
    }
    if !spec.output_path.is_file() {
        return Err(RendererError::new(
            RendererErrorCode::ProcessFailed,
            "FFmpeg 未生成候选输出。",
            false,
        ));
    }
    let bytes = fs::metadata(&spec.output_path)
        .map_err(|_| RendererError::new(RendererErrorCode::Io, "无法读取渲染输出元数据。", true))?
        .len();
    if bytes == 0 || bytes > spec.encoding.max_temporary_bytes {
        let _ = fs::remove_file(&spec.output_path);
        return Err(RendererError::new(
            RendererErrorCode::ResourceLimit,
            "渲染输出为空或超过临时磁盘上限。",
            false,
        ));
    }
    let verified = match inspect_rendered_media(&spec).await {
        Ok(verified) => verified,
        Err(error) => {
            let _ = fs::remove_file(&spec.output_path);
            return Err(error);
        }
    };
    progress(1.0, "FFmpeg 渲染完成，等待原子提交".to_owned());
    Ok(RenderProcessResult {
        duration_ms: verified.duration_ms,
        width: verified.width,
        height: verified.height,
        has_audio: verified.has_audio,
        stderr_tail: bounded_tail(
            &stderr
                .lock()
                .map(|value| value.join("\n"))
                .unwrap_or_default(),
            8192,
        ),
    })
}

#[cfg(not(windows))]
async fn execute_ffmpeg(
    _spec: RenderSpec,
    _cancellation: RenderCancellation,
    _progress: Arc<dyn Fn(f64, String) + Send + Sync>,
) -> Result<RenderProcessResult, RendererError> {
    Err(RendererError::new(
        RendererErrorCode::Unsupported,
        "FFmpeg Renderer 当前仅支持 Windows Alpha。",
        false,
    ))
}

fn bounded_tail(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    value[value.len() - limit..].to_owned()
}

struct GuardedExecutable {
    path: PathBuf,
    hash: String,
    _file: File,
}

impl GuardedExecutable {
    fn open(path: &Path) -> Result<Self, RendererError> {
        let path = fs::canonicalize(path).map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unavailable,
                "FFmpeg 可执行文件无法规范化。",
                false,
            )
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unavailable,
                "FFmpeg 可执行文件不可读。",
                false,
            )
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(RendererError::new(
                RendererErrorCode::Unavailable,
                "FFmpeg 必须是普通文件，不能是链接。",
                false,
            ));
        }
        let mut file = open_guarded_file(&path)?;
        let hash = hash_open_file(&mut file)?;
        Ok(Self {
            path,
            hash,
            _file: file,
        })
    }

    fn open_verified(path: &Path, expected_hash: &str) -> Result<Self, RendererError> {
        let guard = Self::open(path)?;
        if guard.hash != expected_hash {
            return Err(RendererError::new(
                RendererErrorCode::IdentityChanged,
                "FFmpeg 可执行文件身份已变化；拒绝静默替换。",
                false,
            ));
        }
        Ok(guard)
    }

    fn path(&self) -> &Path {
        &self.path
    }
    fn hash(&self) -> &str {
        &self.hash
    }
}

#[cfg(windows)]
fn open_guarded_file(path: &Path) -> Result<File, RendererError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_SHARE_READ: u32 = 1;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unavailable,
                "无法以只读共享锁打开 FFmpeg。",
                false,
            )
        })
}

#[cfg(not(windows))]
fn open_guarded_file(path: &Path) -> Result<File, RendererError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| RendererError::new(RendererErrorCode::Unavailable, "无法打开 FFmpeg。", false))
}

fn hash_open_file(file: &mut File) -> Result<String, RendererError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            RendererError::new(RendererErrorCode::Io, "读取 FFmpeg 身份失败。", true)
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in bytes {
        value.push_str(&format!("{byte:02x}"));
    }
    value
}

pub fn deterministic_scene_color(scene_id: &str) -> String {
    let digest = Sha256::digest(scene_id.as_bytes());
    format!(
        "0x{:02x}{:02x}{:02x}",
        digest[0] / 2,
        digest[1] / 2,
        digest[2] / 2
    )
}

pub fn sanitized_environment() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_adapter_owned_and_has_no_shell_fragments() {
        let spec = RenderSpec {
            identity: RendererIdentity {
                adapter_id: ADAPTER_ID.into(),
                adapter_version: ADAPTER_VERSION.into(),
                executable_file_name: "ffmpeg.exe".into(),
                executable_path: "ffmpeg.exe".into(),
                executable_hash: format!("sha256:{}", "a".repeat(64)),
                ffmpeg_version: "7.1.1".into(),
                ffprobe_file_name: "ffprobe.exe".into(),
                ffprobe_path: "ffprobe.exe".into(),
                ffprobe_hash: format!("sha256:{}", "c".repeat(64)),
                ffprobe_version: "7.1.1".into(),
                capability_hash: format!("sha256:{}", "b".repeat(64)),
            },
            working_directory: ".".into(),
            output_path: "out.partial.mp4".into(),
            audio_path: "audio.wav".into(),
            canvas: RenderCanvas {
                width: 1920,
                height: 1080,
                frame_rate_numerator: 30,
                frame_rate_denominator: 1,
            },
            encoding: RenderEncoding {
                preset: "fast".into(),
                crf: 23,
                timeout_ms: 60_000,
                max_temporary_bytes: 1024 * 1024,
            },
            scenes: vec![RenderSceneSpec {
                scene_id: "scene_1".into(),
                start_ms: 1_000,
                end_ms: 2_000,
                color: "0x123456".into(),
            }],
        };
        let argv = build_ffmpeg_argv(&spec);
        assert_eq!(argv.last().map(String::as_str), Some("out.partial.mp4"));
        assert!(!argv
            .iter()
            .any(|arg| arg.contains("&&") || arg.contains('|') || arg.contains(';')));
        assert!(argv.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
        assert!(argv.windows(2).any(|pair| pair == ["-ss", "1.000"]));
        assert!(!argv.iter().any(|arg| arg == "-filter_script"));
    }

    #[test]
    fn deterministic_colors_are_stable_and_bounded() {
        let color = deterministic_scene_color("scene_0001");
        assert_eq!(color, deterministic_scene_color("scene_0001"));
        assert!(valid_color(&color));
    }

    #[test]
    fn version_parser_handles_release_and_git_suffixes() {
        assert_eq!(
            parse_ffmpeg_version("ffmpeg version 7.1.1 Copyright")
                .unwrap()
                .major,
            7
        );
        assert_eq!(
            parse_ffmpeg_version("ffmpeg version 8.0-full_build Copyright")
                .unwrap()
                .major,
            8
        );
    }

    #[test]
    fn render_spec_rejects_paths_outside_the_controlled_root() {
        let root = tempfile::tempdir().expect("controlled root");
        let outside = tempfile::tempdir().expect("outside root");
        let audio_path = root.path().join("audio.wav");
        fs::write(&audio_path, b"fixture").expect("audio fixture");
        let spec = RenderSpec {
            identity: RendererIdentity {
                adapter_id: ADAPTER_ID.into(),
                adapter_version: ADAPTER_VERSION.into(),
                executable_file_name: "ffmpeg.exe".into(),
                executable_path: "ffmpeg.exe".into(),
                executable_hash: format!("sha256:{}", "a".repeat(64)),
                ffmpeg_version: "7.1.1".into(),
                ffprobe_file_name: "ffprobe.exe".into(),
                ffprobe_path: "ffprobe.exe".into(),
                ffprobe_hash: format!("sha256:{}", "c".repeat(64)),
                ffprobe_version: "7.1.1".into(),
                capability_hash: format!("sha256:{}", "b".repeat(64)),
            },
            working_directory: root.path().to_path_buf(),
            output_path: outside.path().join("escaped.mp4"),
            audio_path,
            canvas: RenderCanvas {
                width: 640,
                height: 360,
                frame_rate_numerator: 30,
                frame_rate_denominator: 1,
            },
            encoding: RenderEncoding {
                preset: "fast".into(),
                crf: 23,
                timeout_ms: 60_000,
                max_temporary_bytes: 64 * 1024 * 1024,
            },
            scenes: vec![RenderSceneSpec {
                scene_id: "scene_1".into(),
                start_ms: 0,
                end_ms: 1_000,
                color: "0x123456".into(),
            }],
        };
        let error = validate_render_spec(&spec).expect_err("output escape must fail");
        assert_eq!(error.code, RendererErrorCode::InvalidSpec);
    }

    #[test]
    fn ffprobe_truth_rejects_corruption_and_media_drift() {
        let root = tempfile::tempdir().expect("controlled root");
        let spec = fixture_spec(root.path());
        let valid = ffprobe_document("h264", 640, 360, "30/1", "aac", "2.000000");
        assert_eq!(
            verify_ffprobe_document(&spec, &valid).expect("valid media truth"),
            VerifiedMedia {
                duration_ms: 2_000,
                width: 640,
                height: 360,
                has_audio: true,
            }
        );
        for drifted in [
            ffprobe_document("hevc", 640, 360, "30/1", "aac", "2.000000"),
            ffprobe_document("h264", 1280, 720, "30/1", "aac", "2.000000"),
            ffprobe_document("h264", 640, 360, "24/1", "aac", "2.000000"),
            ffprobe_document("h264", 640, 360, "30/1", "mp3", "2.000000"),
            ffprobe_document("h264", 640, 360, "30/1", "aac", "9.000000"),
        ] {
            assert_eq!(
                verify_ffprobe_document(&spec, &drifted)
                    .expect_err("media drift must fail closed")
                    .code,
                RendererErrorCode::ProcessFailed
            );
        }
        assert!(serde_json::from_str::<FfprobeDocument>("{corrupt").is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn managed_probe_times_out_and_reaps_a_hanging_process_tree() {
        let executable = std::env::current_exe().expect("current test executable");
        let guard = GuardedExecutable::open(&executable).expect("guard helper identity");
        let error = run_managed_command(
            &guard,
            &[
                "--exact".into(),
                "tests::managed_probe_helper_hangs".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            executable.parent(),
            Duration::from_millis(200),
            64 * 1024,
            "挂死探测测试",
        )
        .await
        .expect_err("hanging probe must time out");
        assert_eq!(error.code, RendererErrorCode::Timeout);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn managed_probe_stops_and_reaps_infinite_output() {
        let executable = std::env::current_exe().expect("current test executable");
        let guard = GuardedExecutable::open(&executable).expect("guard helper identity");
        let error = run_managed_command(
            &guard,
            &[
                "--exact".into(),
                "tests::managed_probe_helper_floods_output".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            executable.parent(),
            Duration::from_secs(5),
            8 * 1024,
            "无限输出探测测试",
        )
        .await
        .expect_err("output flood must hit the streaming limit");
        assert_eq!(error.code, RendererErrorCode::ResourceLimit);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "child helper launched by managed_probe_times_out_and_reaps_a_hanging_process_tree"]
    fn managed_probe_helper_hangs() {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "child helper launched by managed_probe_stops_and_reaps_infinite_output"]
    fn managed_probe_helper_floods_output() {
        loop {
            println!("{}", "x".repeat(4 * 1024));
        }
    }

    fn fixture_spec(root: &Path) -> RenderSpec {
        RenderSpec {
            identity: RendererIdentity {
                adapter_id: ADAPTER_ID.into(),
                adapter_version: ADAPTER_VERSION.into(),
                executable_file_name: "ffmpeg.exe".into(),
                executable_path: root.join("ffmpeg.exe"),
                executable_hash: format!("sha256:{}", "a".repeat(64)),
                ffmpeg_version: "7.1.1".into(),
                ffprobe_file_name: "ffprobe.exe".into(),
                ffprobe_path: root.join("ffprobe.exe"),
                ffprobe_hash: format!("sha256:{}", "c".repeat(64)),
                ffprobe_version: "7.1.1".into(),
                capability_hash: format!("sha256:{}", "b".repeat(64)),
            },
            working_directory: root.into(),
            output_path: root.join("output.partial.mp4"),
            audio_path: root.join("audio.wav"),
            canvas: RenderCanvas {
                width: 640,
                height: 360,
                frame_rate_numerator: 30,
                frame_rate_denominator: 1,
            },
            encoding: RenderEncoding {
                preset: "fast".into(),
                crf: 23,
                timeout_ms: 60_000,
                max_temporary_bytes: 64 * 1024 * 1024,
            },
            scenes: vec![RenderSceneSpec {
                scene_id: "scene_001".into(),
                start_ms: 0,
                end_ms: 2_000,
                color: "0x123456".into(),
            }],
        }
    }

    fn ffprobe_document(
        video_codec: &str,
        width: u32,
        height: u32,
        frame_rate: &str,
        audio_codec: &str,
        duration: &str,
    ) -> FfprobeDocument {
        serde_json::from_value(serde_json::json!({
            "streams": [
                { "codec_name": video_codec, "codec_type": "video", "width": width, "height": height, "r_frame_rate": frame_rate },
                { "codec_name": audio_codec, "codec_type": "audio" }
            ],
            "format": { "format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": duration }
        }))
        .expect("ffprobe fixture")
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a locally installed supported FFmpeg/FFprobe"]
    async fn real_ffmpeg_smoke_produces_playable_h264_aac_mp4() {
        let adapter = FfmpegRenderer;
        let probe = adapter.probe().await;
        assert!(
            probe.available && probe.supported,
            "{:#?}",
            probe.diagnostics
        );
        let identity = probe
            .identity
            .expect("supported probe must freeze identity");
        let temp = tempfile::tempdir().expect("create smoke directory");
        let audio_path = temp.path().join("audio.wav");
        write_silent_wav(&audio_path, 48_000, 2);
        let output_path = temp.path().join("output.partial.mp4");
        let spec = RenderSpec {
            identity: identity.clone(),
            working_directory: temp.path().to_path_buf(),
            output_path: output_path.clone(),
            audio_path,
            canvas: RenderCanvas {
                width: 640,
                height: 360,
                frame_rate_numerator: 30,
                frame_rate_denominator: 1,
            },
            encoding: RenderEncoding {
                preset: "veryfast".into(),
                crf: 23,
                timeout_ms: 60_000,
                max_temporary_bytes: 64 * 1024 * 1024,
            },
            scenes: vec![
                RenderSceneSpec {
                    scene_id: "scene_smoke_1".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                    color: deterministic_scene_color("scene_smoke_1"),
                },
                RenderSceneSpec {
                    scene_id: "scene_smoke_2".into(),
                    start_ms: 1_000,
                    end_ms: 2_000,
                    color: deterministic_scene_color("scene_smoke_2"),
                },
            ],
        };
        let result = adapter
            .render(spec, RenderCancellation::default(), Arc::new(|_, _| {}))
            .await
            .expect("real FFmpeg render must succeed");
        assert_eq!((result.width, result.height), (640, 360));
        assert!(result.duration_ms.abs_diff(2_000) <= 100);
        assert!(result.has_audio);
        assert!(fs::metadata(&output_path).expect("output metadata").len() > 0);
    }

    #[cfg(windows)]
    fn write_silent_wav(path: &Path, sample_rate: u32, seconds: u32) {
        let data_len = sample_rate * seconds * 2;
        let mut bytes = Vec::with_capacity(data_len as usize + 44);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(data_len as usize + 44, 0);
        fs::write(path, bytes).expect("write silent WAV fixture");
    }
}
