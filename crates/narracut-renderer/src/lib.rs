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
pub const MAX_SCENES: usize = 1_000;
pub const MAX_LOG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererIdentity {
    pub adapter_id: String,
    pub adapter_version: String,
    pub executable_file_name: String,
    pub executable_path: PathBuf,
    pub executable_hash: String,
    pub ffmpeg_version: String,
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
    fn probe(&self) -> RendererProbe;
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
    fn probe(&self) -> RendererProbe {
        match probe_ffmpeg() {
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

fn probe_ffmpeg() -> Result<RendererProbe, RendererError> {
    let executable = locate_ffmpeg()?;
    let guarded = GuardedExecutable::open(&executable)?;
    let version_output = std::process::Command::new(guarded.path())
        .args(["-hide_banner", "-version"])
        .env_clear()
        .output()
        .map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unavailable,
                "FFmpeg 版本探测失败。",
                true,
            )
        })?;
    if !version_output.status.success() || version_output.stdout.len() > 256 * 1024 {
        return Err(RendererError::new(
            RendererErrorCode::Unsupported,
            "FFmpeg 未返回受支持的有界版本信息。",
            false,
        ));
    }
    let version_text = String::from_utf8_lossy(&version_output.stdout);
    let version = parse_ffmpeg_version(&version_text)?;
    let encoders_output = std::process::Command::new(guarded.path())
        .args(["-hide_banner", "-encoders"])
        .env_clear()
        .output()
        .map_err(|_| {
            RendererError::new(
                RendererErrorCode::Unavailable,
                "FFmpeg 编码能力探测失败。",
                true,
            )
        })?;
    if !encoders_output.status.success() || encoders_output.stdout.len() > 4 * 1024 * 1024 {
        return Err(RendererError::new(
            RendererErrorCode::Unsupported,
            "FFmpeg 编码器清单无效或超出上限。",
            false,
        ));
    }
    let encoders = String::from_utf8_lossy(&encoders_output.stdout);
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
    let identity = RendererIdentity {
        adapter_id: ADAPTER_ID.to_owned(),
        adapter_version: ADAPTER_VERSION.to_owned(),
        executable_file_name: file_name,
        executable_path: guarded.path().to_path_buf(),
        executable_hash: guarded.hash().to_owned(),
        ffmpeg_version: version.to_string(),
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

fn parse_ffmpeg_version(output: &str) -> Result<Version, RendererError> {
    let token = output
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("ffmpeg version "))
        .and_then(|tail| tail.split_whitespace().next())
        .ok_or_else(|| {
            RendererError::new(
                RendererErrorCode::Unsupported,
                "FFmpeg 版本行无法解析。",
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
                "FFmpeg 版本不是可识别语义版本。",
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
    progress(1.0, "FFmpeg 渲染完成，等待原子提交".to_owned());
    let duration_ms = spec
        .scenes
        .last()
        .map(|scene| scene.end_ms)
        .unwrap_or_default()
        - spec
            .scenes
            .first()
            .map(|scene| scene.start_ms)
            .unwrap_or_default();
    Ok(RenderProcessResult {
        duration_ms,
        width: spec.canvas.width,
        height: spec.canvas.height,
        has_audio: true,
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
                start_ms: 0,
                end_ms: 1000,
                color: "0x123456".into(),
            }],
        };
        let argv = build_ffmpeg_argv(&spec);
        assert_eq!(argv.last().map(String::as_str), Some("out.partial.mp4"));
        assert!(!argv
            .iter()
            .any(|arg| arg.contains("&&") || arg.contains('|') || arg.contains(';')));
        assert!(argv.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
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
}
