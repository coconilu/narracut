#![allow(clippy::result_large_err)]

use narracut_contracts::media_command_types::{
    EnqueueAudioImportRequest, EnqueueCaptionsImportRequest, GenerateScenePlanRequest,
    GenerateTimelineRequest, GetMediaDocumentRequest, MediaCommandError, MediaDocumentResult,
    MediaJobAcceptedResult, MediaSaveResult, ReauthorizeMediaRequest, SaveScenePlanRequest,
    SaveTimelineRequest,
};
use narracut_contracts::{
    validate_media_command_message, validate_media_document, NARRACUT_MEDIA_COMMAND_API_VERSION,
};
use narracut_core::{
    FrozenArtifactInputData, GetMediaDocumentOptions, JobErrorCode, MediaErrorCode, MediaOperation,
    MediaRightsData, MediaService, MediaServiceError, PcmWavParseLimits, ReauthorizeMediaOptions,
    SaveScenePlanOptions, SaveTimelineOptions, ScenePlanEditData, SrtParseLimits, StorageErrorCode,
    TimelineCanvasData, TimelineEditData, TimelineSafeAreaData,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

use crate::media_runtime::{
    AudioImportEnqueueOptions, CaptionsImportEnqueueOptions, MediaJobEnqueueOutcome, MediaRuntime,
    MediaRuntimeError, ScenePlanEnqueueOptions, TimelineEnqueueOptions,
};

#[tauri::command]
pub async fn enqueue_audio_import(
    state: State<'_, MediaRuntime>,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let runtime = state.inner().clone();
    let enqueue_runtime = runtime.clone();
    let project_path = request
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let accepted = run_blocking(MediaOperation::ImportAudio, move || {
        handle_enqueue_audio_import(&enqueue_runtime, request)
    })
    .await?;
    let _ = runtime
        .schedule_supported_job(
            project_path,
            accepted.owner_project_id.to_string(),
            accepted.job_id.to_string(),
        )
        .map_err(|error| media_runtime_error_to_contract(error, MediaOperation::ImportAudio))?;
    Ok(accepted)
}

#[tauri::command]
pub async fn enqueue_captions_import(
    state: State<'_, MediaRuntime>,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let runtime = state.inner().clone();
    let enqueue_runtime = runtime.clone();
    let project_path = request
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let accepted = run_blocking(MediaOperation::ImportCaptions, move || {
        handle_enqueue_captions_import(&enqueue_runtime, request)
    })
    .await?;
    let _ = runtime
        .schedule_supported_job(
            project_path,
            accepted.owner_project_id.to_string(),
            accepted.job_id.to_string(),
        )
        .map_err(|error| media_runtime_error_to_contract(error, MediaOperation::ImportCaptions))?;
    Ok(accepted)
}

#[tauri::command]
pub async fn generate_scene_plan(
    state: State<'_, MediaRuntime>,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let runtime = state.inner().clone();
    let enqueue_runtime = runtime.clone();
    let project_path = request
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let accepted = run_blocking(MediaOperation::GenerateScenePlan, move || {
        handle_generate_scene_plan(&enqueue_runtime, request)
    })
    .await?;
    let _ = runtime
        .schedule_supported_job(
            project_path,
            accepted.owner_project_id.to_string(),
            accepted.job_id.to_string(),
        )
        .map_err(|error| {
            media_runtime_error_to_contract(error, MediaOperation::GenerateScenePlan)
        })?;
    Ok(accepted)
}

#[tauri::command]
pub async fn generate_timeline(
    state: State<'_, MediaRuntime>,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let runtime = state.inner().clone();
    let enqueue_runtime = runtime.clone();
    let project_path = request
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let accepted = run_blocking(MediaOperation::GenerateTimeline, move || {
        handle_generate_timeline(&enqueue_runtime, request)
    })
    .await?;
    let _ = runtime
        .schedule_supported_job(
            project_path,
            accepted.owner_project_id.to_string(),
            accepted.job_id.to_string(),
        )
        .map_err(|error| {
            media_runtime_error_to_contract(error, MediaOperation::GenerateTimeline)
        })?;
    Ok(accepted)
}

#[tauri::command]
pub async fn get_media_document(
    state: State<'_, MediaService>,
    request: Value,
) -> Result<MediaDocumentResult, MediaCommandError> {
    let service = state.inner().clone();
    run_blocking(MediaOperation::ReadMediaDocument, move || {
        handle_get_media_document(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn save_scene_plan(
    state: State<'_, MediaService>,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let service = state.inner().clone();
    run_blocking(MediaOperation::SaveScenePlan, move || {
        handle_save_scene_plan(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn save_timeline(
    state: State<'_, MediaService>,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let service = state.inner().clone();
    run_blocking(MediaOperation::SaveTimeline, move || {
        handle_save_timeline(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn reauthorize_media(
    state: State<'_, MediaService>,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let service = state.inner().clone();
    run_blocking(MediaOperation::ReauthorizeMedia, move || {
        handle_reauthorize_media(&service, request)
    })
    .await
}

fn handle_enqueue_audio_import(
    runtime: &MediaRuntime,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    reject_legacy_import_rights(&request, MediaOperation::ImportAudio)?;
    let request: EnqueueAudioImportDto =
        decode_request::<EnqueueAudioImportRequest, _>(request, MediaOperation::ImportAudio)?;
    let limits = PcmWavParseLimits {
        max_bytes: request.limits.max_bytes,
    };
    let outcome = runtime
        .enqueue_audio_import(AudioImportEnqueueOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            source_path: request.source_path,
            expected_source_content_hash: request.expected_source_content_hash,
            script_input: request.script_input,
            rights: request.rights,
            limits,
            config_snapshot: json!({
                "runtimeVersion": "1.0.0",
                "parser": "pcm_wav_v1",
                "limits": limits,
            }),
            idempotency_key: request.idempotency_key,
        })
        .map_err(|error| media_runtime_error_to_contract(error, MediaOperation::ImportAudio))?;
    encode_job_accepted_response(outcome, MediaOperation::ImportAudio)
}

fn handle_enqueue_captions_import(
    runtime: &MediaRuntime,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    reject_legacy_import_rights(&request, MediaOperation::ImportCaptions)?;
    let request: EnqueueCaptionsImportDto =
        decode_request::<EnqueueCaptionsImportRequest, _>(request, MediaOperation::ImportCaptions)?;
    let defaults = SrtParseLimits::default();
    let limits = SrtParseLimits {
        max_bytes: request.limits.max_bytes,
        max_cue_count: request
            .limits
            .max_cue_count
            .unwrap_or(defaults.max_cue_count),
        max_cue_text_bytes: request
            .limits
            .max_cue_text_bytes
            .unwrap_or(defaults.max_cue_text_bytes),
    };
    let audio_duration_ms = request.audio_duration_ms;
    let outcome = runtime
        .enqueue_captions_import(CaptionsImportEnqueueOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            source_path: request.source_path,
            expected_source_content_hash: request.expected_source_content_hash,
            script_input: request.script_input,
            audio_input: request.audio_input,
            audio_duration_ms,
            rights: request.rights,
            limits,
            config_snapshot: json!({
                "runtimeVersion": "1.0.0",
                "parser": "srt_v1",
                "audioDurationMs": audio_duration_ms,
                "limits": limits,
            }),
            idempotency_key: request.idempotency_key,
        })
        .map_err(|error| media_runtime_error_to_contract(error, MediaOperation::ImportCaptions))?;
    encode_job_accepted_response(outcome, MediaOperation::ImportCaptions)
}

fn handle_generate_scene_plan(
    runtime: &MediaRuntime,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let request: GenerateScenePlanDto =
        decode_request::<GenerateScenePlanRequest, _>(request, MediaOperation::GenerateScenePlan)?;
    let outcome = runtime
        .generate_scene_plan(ScenePlanEnqueueOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            research_input: request.research_input,
            script_input: request.script_input,
            captions_input: request.captions_input,
            config_snapshot: json!({
                "runtimeVersion": "1.0.0",
                "planner": "deterministic_scene_plan_v1",
            }),
            idempotency_key: request.idempotency_key,
        })
        .map_err(|error| {
            media_runtime_error_to_contract(error, MediaOperation::GenerateScenePlan)
        })?;
    encode_job_accepted_response(outcome, MediaOperation::GenerateScenePlan)
}

fn handle_generate_timeline(
    runtime: &MediaRuntime,
    request: Value,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    let request: GenerateTimelineDto =
        decode_request::<GenerateTimelineRequest, _>(request, MediaOperation::GenerateTimeline)?;
    let canvas = request.canvas;
    let safe_area = request.safe_area;
    let outcome = runtime
        .generate_timeline(TimelineEnqueueOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            audio_input: request.audio_input,
            captions_input: request.captions_input,
            scene_plan_input: request.scene_plan_input,
            canvas,
            safe_area,
            config_snapshot: json!({
                "runtimeVersion": "1.0.0",
                "builder": "deterministic_timeline_v1",
                "canvas": canvas,
                "safeArea": safe_area,
            }),
            idempotency_key: request.idempotency_key,
        })
        .map_err(|error| {
            media_runtime_error_to_contract(error, MediaOperation::GenerateTimeline)
        })?;
    encode_job_accepted_response(outcome, MediaOperation::GenerateTimeline)
}

fn handle_get_media_document(
    service: &MediaService,
    request: Value,
) -> Result<MediaDocumentResult, MediaCommandError> {
    let request: GetMediaDocumentDto =
        decode_request::<GetMediaDocumentRequest, _>(request, MediaOperation::ReadMediaDocument)?;
    let result = service
        .get_media_document(GetMediaDocumentOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            artifact_id: request.artifact_id,
        })
        .map_err(media_error_to_contract)?;
    encode_media_document_response(result, MediaOperation::ReadMediaDocument)
}

fn handle_save_scene_plan(
    service: &MediaService,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let request: SaveScenePlanDto =
        decode_request::<SaveScenePlanRequest, _>(request, MediaOperation::SaveScenePlan)?;
    let base = service
        .get_media_document(GetMediaDocumentOptions {
            project_path: request.project_path.clone(),
            expected_project_id: request.expected_project_id.clone(),
            artifact_id: request.base_artifact_id.clone(),
        })
        .map_err(media_error_to_contract)?;
    let edits = convert_scene_plan_edits(request.edits, &base.document)?;
    let result = service
        .save_scene_plan(SaveScenePlanOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            base_artifact_id: request.base_artifact_id,
            edits,
            change_summary: request.change_summary,
            idempotency_key: request.idempotency_key,
        })
        .map_err(media_error_to_contract)?;
    encode_response(result, MediaOperation::SaveScenePlan)
}

fn handle_save_timeline(
    service: &MediaService,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let request: SaveTimelineDto =
        decode_request::<SaveTimelineRequest, _>(request, MediaOperation::SaveTimeline)?;
    let edits = request.edits.into_iter().map(Into::into).collect();
    let result = service
        .save_timeline(SaveTimelineOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            base_artifact_id: request.base_artifact_id,
            edits,
            change_summary: request.change_summary,
            idempotency_key: request.idempotency_key,
        })
        .map_err(media_error_to_contract)?;
    encode_response(result, MediaOperation::SaveTimeline)
}

fn handle_reauthorize_media(
    service: &MediaService,
    request: Value,
) -> Result<MediaSaveResult, MediaCommandError> {
    let request: ReauthorizeMediaDto =
        decode_request::<ReauthorizeMediaRequest, _>(request, MediaOperation::ReauthorizeMedia)?;
    let result = service
        .reauthorize_media(ReauthorizeMediaOptions {
            project_path: request.project_path,
            expected_project_id: request.expected_project_id,
            run_id: request.run_id,
            base_artifact_id: request.base_artifact_id,
            rights: request.rights,
            idempotency_key: request.idempotency_key,
        })
        .map_err(media_error_to_contract)?;
    encode_response(result, MediaOperation::ReauthorizeMedia)
}

async fn run_blocking<T, F>(operation: MediaOperation, task: F) -> Result<T, MediaCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, MediaCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| internal_contract_error(operation, "媒体后台操作异常终止。"))?
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: MediaOperation,
) -> Result<TDto, MediaCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_media_command_message(&request)
        .map_err(|_| invalid_request_error(operation, "请求未通过 media-command v1 契约。"))?;
    let generated: TContract = serde_json::from_value(request)
        .map_err(|_| invalid_request_error(operation, "请求无法解析为生成契约。"))?;
    let value = serde_json::to_value(generated)
        .map_err(|_| internal_contract_error(operation, "生成请求无法重新序列化。"))?;
    serde_json::from_value(value)
        .map_err(|_| invalid_request_error(operation, "请求字段无法转换为媒体输入。"))
}

fn encode_media_document_response(
    response: narracut_core::MediaDocumentReadResultData,
    operation: MediaOperation,
) -> Result<MediaDocumentResult, MediaCommandError> {
    validate_media_document(&response.document)
        .map_err(|_| internal_contract_error(operation, "媒体文档响应违反 media v1 契约。"))?;
    encode_response(
        json!({
            "apiVersion": NARRACUT_MEDIA_COMMAND_API_VERSION,
            "ownerProjectId": response.owner_project_id,
            "artifactId": response.artifact_id,
            "contentHash": response.content_hash,
            "document": response.document,
        }),
        operation,
    )
}

fn encode_job_accepted_response(
    outcome: MediaJobEnqueueOutcome,
    operation: MediaOperation,
) -> Result<MediaJobAcceptedResult, MediaCommandError> {
    encode_response(
        json!({
            "apiVersion": NARRACUT_MEDIA_COMMAND_API_VERSION,
            "operation": operation_name(operation),
            "ownerProjectId": outcome.owner_project_id,
            "runId": outcome.run_id,
            "jobId": outcome.job_id,
            "idempotentReplay": outcome.idempotent_replay,
        }),
        operation,
    )
}

fn encode_response<TInternal, TContract>(
    response: TInternal,
    operation: MediaOperation,
) -> Result<TContract, MediaCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response)
        .map_err(|_| internal_contract_error(operation, "媒体响应序列化失败。"))?;
    validate_media_command_message(&value)
        .map_err(|_| internal_contract_error(operation, "媒体响应违反 command v1 契约。"))?;
    serde_json::from_value(value)
        .map_err(|_| internal_contract_error(operation, "媒体响应无法转换为生成类型。"))
}

fn media_error_to_contract(error: MediaServiceError) -> MediaCommandError {
    let operation = error.operation;
    let (code, retryable) = contract_error_code(error.code);
    let mut object = Map::from_iter([
        (
            "apiVersion".to_owned(),
            json!(NARRACUT_MEDIA_COMMAND_API_VERSION),
        ),
        ("code".to_owned(), json!(code)),
        ("operation".to_owned(), json!(operation_name(operation))),
        ("message".to_owned(), json!(error.message)),
        ("retryable".to_owned(), json!(retryable)),
    ]);
    if let Some(stage_id) = error.stage_id {
        object.insert("stageId".to_owned(), json!(stage_id));
    }
    if let Some(run_id) = error.run_id {
        object.insert("runId".to_owned(), json!(run_id));
    }
    if let Some(artifact_id) = error.artifact_id {
        object.insert("artifactId".to_owned(), json!(artifact_id));
    }
    contract_error_from_value(Value::Object(object), operation)
}

fn media_runtime_error_to_contract(
    error: MediaRuntimeError,
    operation: MediaOperation,
) -> MediaCommandError {
    let (code, message, retryable) = match error {
        MediaRuntimeError::Storage(error) => match error.code {
            StorageErrorCode::InvalidRequest | StorageErrorCode::InvalidPath => {
                ("invalid_request", "媒体源请求无效。", false)
            }
            StorageErrorCode::PathContainsSymlink => (
                "source_link_rejected",
                "媒体源路径包含不允许的符号链接或重解析点。",
                false,
            ),
            StorageErrorCode::ProjectNotFound => ("project_not_found", "目标项目不存在。", false),
            StorageErrorCode::ProjectIdentityMismatch => (
                "project_identity_mismatch",
                "目标项目身份与请求不一致。",
                false,
            ),
            StorageErrorCode::SourceNotFound => {
                ("source_not_file", "媒体源不存在或不是普通文件。", false)
            }
            StorageErrorCode::SourceChanged => (
                "source_changed",
                "媒体源内容与调用方确认的哈希不一致。",
                true,
            ),
            StorageErrorCode::SourceTooLarge | StorageErrorCode::ArtifactTooLarge => {
                ("source_too_large", "媒体源超过本次导入的大小上限。", false)
            }
            StorageErrorCode::IoError
            | StorageErrorCode::IndexUnavailable
            | StorageErrorCode::IndexMigrationFailed => {
                ("io_error", "媒体源暂存或任务持久化失败。", true)
            }
            StorageErrorCode::ContentCorrupt => (
                "input_hash_mismatch",
                "已暂存媒体源未通过完整性校验。",
                false,
            ),
            _ => ("internal_contract_error", "媒体源暂存未能完成。", false),
        },
        MediaRuntimeError::Job(error) => match error.code {
            JobErrorCode::InvalidRequest | JobErrorCode::InvalidPath => {
                ("invalid_request", "媒体任务请求无效。", false)
            }
            JobErrorCode::ProjectNotFound => ("project_not_found", "目标项目不存在。", false),
            JobErrorCode::ProjectIdentityMismatch => (
                "project_identity_mismatch",
                "目标项目身份与请求不一致。",
                false,
            ),
            JobErrorCode::IdempotencyConflict => (
                "job_conflict",
                "该幂等键已绑定到不同的媒体任务请求。",
                false,
            ),
            JobErrorCode::IoError => ("io_error", "媒体任务持久化失败。", true),
            _ => (
                "internal_contract_error",
                "媒体任务未能安全进入队列。",
                false,
            ),
        },
        MediaRuntimeError::Serialization(_) | MediaRuntimeError::InvalidSnapshot(_) => (
            "internal_contract_error",
            "媒体任务内部契约处理失败。",
            false,
        ),
    };
    error_value(operation, code, message, retryable)
}

fn contract_error_code(code: MediaErrorCode) -> (&'static str, bool) {
    match code {
        MediaErrorCode::InvalidRequest
        | MediaErrorCode::RightsRequired
        | MediaErrorCode::VoiceCloneNotAllowed => ("invalid_request", false),
        MediaErrorCode::RightsUpgradeRequired => ("rights_upgrade_required", false),
        MediaErrorCode::InvalidSourceName => ("source_not_file", false),
        MediaErrorCode::SourceHashMismatch => ("input_hash_mismatch", false),
        MediaErrorCode::SourceChanged => ("source_changed", true),
        MediaErrorCode::InputNotApproved => ("review_required", false),
        MediaErrorCode::InputReferenceMismatch => ("traceability_incomplete", false),
        MediaErrorCode::CrossProjectReference => ("cross_project_reference", false),
        MediaErrorCode::ArtifactVerificationFailed => ("artifact_not_found", false),
        MediaErrorCode::IdempotencyConflict => ("job_conflict", false),
        MediaErrorCode::ResourceLimitExceeded => ("source_too_large", false),
        MediaErrorCode::InvalidMedia => ("unsupported_media", false),
        MediaErrorCode::ContractViolation => ("internal_contract_error", false),
        MediaErrorCode::StorageUnavailable | MediaErrorCode::Io => ("io_error", true),
    }
}

fn invalid_request_error(
    operation: MediaOperation,
    message: impl Into<String>,
) -> MediaCommandError {
    error_value(operation, "invalid_request", message, false)
}

fn internal_contract_error(
    operation: MediaOperation,
    message: impl Into<String>,
) -> MediaCommandError {
    error_value(operation, "internal_contract_error", message, false)
}

fn error_value(
    operation: MediaOperation,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
) -> MediaCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": NARRACUT_MEDIA_COMMAND_API_VERSION,
            "code": code,
            "operation": operation_name(operation),
            "message": message.into(),
            "retryable": retryable,
        }),
        operation,
    )
}

fn contract_error_from_value(value: Value, operation: MediaOperation) -> MediaCommandError {
    validate_media_command_message(&value).unwrap_or_else(|error| {
        panic!(
            "media error adapter produced invalid schema for {}: {error}",
            operation_name(operation)
        )
    });
    serde_json::from_value(value).unwrap_or_else(|error| {
        panic!(
            "media error adapter failed generated conversion for {}: {error}",
            operation_name(operation)
        )
    })
}

fn operation_name(operation: MediaOperation) -> &'static str {
    match operation {
        MediaOperation::ImportAudio => "enqueue_audio_import",
        MediaOperation::ImportCaptions => "enqueue_captions_import",
        MediaOperation::GenerateScenePlan => "generate_scene_plan",
        MediaOperation::GenerateTimeline => "generate_timeline",
        MediaOperation::SaveScenePlan => "save_scene_plan",
        MediaOperation::SaveTimeline => "save_timeline",
        MediaOperation::ReauthorizeMedia => "reauthorize_media",
        MediaOperation::ReadMediaDocument => "get_media_document",
        MediaOperation::ValidateApprovedInputs => "generate_scene_plan",
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetMediaDocumentDto {
    project_path: String,
    expected_project_id: String,
    artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueAudioImportDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    source_path: String,
    expected_source_content_hash: Option<String>,
    script_input: FrozenArtifactInputData,
    rights: MediaRightsData,
    limits: MediaImportLimitsDto,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueCaptionsImportDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    source_path: String,
    expected_source_content_hash: Option<String>,
    script_input: FrozenArtifactInputData,
    audio_input: FrozenArtifactInputData,
    audio_duration_ms: u64,
    rights: MediaRightsData,
    limits: MediaImportLimitsDto,
    idempotency_key: String,
}

fn reject_legacy_import_rights(
    request: &Value,
    operation: MediaOperation,
) -> Result<(), MediaCommandError> {
    if request.get("apiVersion").and_then(Value::as_str) != Some("1.0.0") {
        return Ok(());
    }
    validate_media_command_message(request)
        .map_err(|_| invalid_request_error(operation, "Legacy media request is malformed."))?;
    Err(error_value(
        operation,
        "rights_upgrade_required",
        "Legacy media rights cannot be executed. Reauthorize the historical media into a new schema 1.2 run.",
        false,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReauthorizeMediaDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    base_artifact_id: String,
    rights: MediaRightsData,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaImportLimitsDto {
    max_bytes: u64,
    max_cue_count: Option<usize>,
    max_cue_text_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateScenePlanDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    research_input: FrozenArtifactInputData,
    script_input: FrozenArtifactInputData,
    captions_input: FrozenArtifactInputData,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateTimelineDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    audio_input: FrozenArtifactInputData,
    captions_input: FrozenArtifactInputData,
    scene_plan_input: FrozenArtifactInputData,
    canvas: TimelineCanvasData,
    safe_area: TimelineSafeAreaData,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveScenePlanDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    base_artifact_id: String,
    edits: Vec<ScenePlanEditDto>,
    change_summary: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "editType",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum ScenePlanEditDto {
    Split {
        scene_id: String,
        split_at_ms: u64,
    },
    Merge {
        first_scene_id: String,
        second_scene_id: String,
    },
    Update {
        scene_id: String,
        title: Option<String>,
        narrative_role: Option<String>,
    },
    MoveBoundary {
        left_scene_id: String,
        right_scene_id: String,
        boundary_ms: u64,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveTimelineDto {
    project_path: String,
    expected_project_id: String,
    run_id: String,
    base_artifact_id: String,
    edits: Vec<TimelineEditDto>,
    change_summary: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "editType",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum TimelineEditDto {
    MoveBoundary {
        left_scene_id: String,
        right_scene_id: String,
        boundary_ms: u64,
    },
    SetSafeArea {
        safe_area: TimelineSafeAreaData,
    },
    SetCaptionVisibility {
        visible: bool,
    },
}

impl From<TimelineEditDto> for TimelineEditData {
    fn from(value: TimelineEditDto) -> Self {
        match value {
            TimelineEditDto::MoveBoundary {
                left_scene_id,
                right_scene_id,
                boundary_ms,
            } => Self::MoveSceneBoundary {
                left_scene_id,
                right_scene_id,
                boundary_ms,
            },
            TimelineEditDto::SetSafeArea { safe_area } => Self::SetSafeArea { safe_area },
            TimelineEditDto::SetCaptionVisibility { visible } => {
                Self::SetCaptionVisibility { visible }
            }
        }
    }
}

fn convert_scene_plan_edits(
    edits: Vec<ScenePlanEditDto>,
    base: &Value,
) -> Result<Vec<ScenePlanEditData>, MediaCommandError> {
    let scenes = base
        .get("scenes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_scene_boundary_error("基础 Scene Plan 缺少有效的 scenes。"))?;

    edits
        .into_iter()
        .map(|edit| match edit {
            ScenePlanEditDto::Split {
                scene_id,
                split_at_ms,
            } => {
                let scene = find_scene_for_boundary(scenes, &scene_id)?;
                let boundary_cue_id = resolve_boundary_cue_id(
                    scene,
                    scene,
                    split_at_ms,
                    None,
                    "拆分时间不是该 Scene 的可用 cue 边界。",
                )?;
                Ok(ScenePlanEditData::Split {
                    scene_id,
                    boundary_cue_id,
                })
            }
            ScenePlanEditDto::Merge {
                first_scene_id,
                second_scene_id,
            } => Ok(ScenePlanEditData::Merge {
                first_scene_id,
                second_scene_id,
            }),
            ScenePlanEditDto::Update {
                scene_id,
                title,
                narrative_role,
            } => Ok(ScenePlanEditData::Update {
                scene_id,
                title,
                narrative_role,
            }),
            ScenePlanEditDto::MoveBoundary {
                left_scene_id,
                right_scene_id,
                boundary_ms,
            } => {
                let left_index = scenes
                    .iter()
                    .position(|scene| {
                        scene.get("sceneId").and_then(Value::as_str) == Some(&left_scene_id)
                    })
                    .ok_or_else(|| invalid_scene_boundary_error("左侧 Scene 不存在。"))?;
                let left = &scenes[left_index];
                let right = scenes.get(left_index + 1).filter(|scene| {
                    scene.get("sceneId").and_then(Value::as_str) == Some(&right_scene_id)
                });
                let right = right.ok_or_else(|| {
                    invalid_scene_boundary_error("移动边界只支持相邻的左右 Scene。")
                })?;
                let current_left_count = scene_cue_ids(left)?.len();
                let boundary_cue_id = resolve_boundary_cue_id(
                    left,
                    right,
                    boundary_ms,
                    Some(current_left_count),
                    "目标时间不是这两个 Scene 的可用 cue 边界。",
                )?;
                Ok(ScenePlanEditData::MoveBoundary {
                    left_scene_id,
                    right_scene_id,
                    boundary_cue_id,
                })
            }
        })
        .collect()
}

fn find_scene_for_boundary<'a>(
    scenes: &'a [Value],
    scene_id: &str,
) -> Result<&'a Value, MediaCommandError> {
    scenes
        .iter()
        .find(|scene| scene.get("sceneId").and_then(Value::as_str) == Some(scene_id))
        .ok_or_else(|| invalid_scene_boundary_error("目标 Scene 不存在。"))
}

fn resolve_boundary_cue_id(
    left: &Value,
    right: &Value,
    requested_ms: u64,
    excluded_position: Option<usize>,
    message: &'static str,
) -> Result<String, MediaCommandError> {
    let start = left
        .get("suggestedStartMs")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_scene_boundary_error("Scene 起始时间无效。"))?;
    let end = right
        .get("suggestedEndMs")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_scene_boundary_error("Scene 结束时间无效。"))?;
    let mut cue_ids = scene_cue_ids(left)?;
    if !std::ptr::eq(left, right) {
        cue_ids.extend(scene_cue_ids(right)?);
    }
    let total_count = cue_ids.len();

    (1..total_count)
        .filter(|position| Some(*position) != excluded_position)
        .find_map(|position| {
            proportional_boundary_ms(start, end, position, total_count)
                .filter(|boundary_ms| *boundary_ms == requested_ms)
                .map(|_| cue_ids[position].clone())
        })
        .ok_or_else(|| invalid_scene_boundary_error(message))
}

fn scene_cue_ids(scene: &Value) -> Result<Vec<String>, MediaCommandError> {
    scene
        .get("cueIds")
        .and_then(Value::as_array)
        .filter(|cue_ids| !cue_ids.is_empty())
        .and_then(|cue_ids| {
            cue_ids
                .iter()
                .map(|cue_id| cue_id.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| invalid_scene_boundary_error("Scene cueIds 无效。"))
}

fn proportional_boundary_ms(
    start: u64,
    end: u64,
    left_count: usize,
    total_count: usize,
) -> Option<u64> {
    let duration = end.checked_sub(start)?;
    if left_count == 0 || left_count >= total_count || duration < 2 {
        return None;
    }
    let offset = (u128::from(duration) * left_count as u128) / total_count as u128;
    let boundary = start.checked_add(u64::try_from(offset).ok()?)?;
    Some(boundary.clamp(start + 1, end - 1))
}

fn invalid_scene_boundary_error(message: impl Into<String>) -> MediaCommandError {
    error_value(
        MediaOperation::SaveScenePlan,
        "invalid_scene_boundary",
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use super::{
        convert_scene_plan_edits, decode_request, encode_media_document_response, encode_response,
        handle_enqueue_audio_import, handle_enqueue_captions_import, handle_generate_scene_plan,
        handle_generate_timeline, media_error_to_contract, GenerateTimelineDto,
        GetMediaDocumentDto, ScenePlanEditDto,
    };
    use narracut_contracts::media_command_types::{
        GetMediaDocumentRequest, MediaDocumentResult, MediaSaveResult,
    };
    use narracut_contracts::{validate_media_command_message, ArtifactDraft};
    use narracut_core::{
        CancelJobOptions, CreateProjectOptions, FrozenArtifactInputData, GetJobOptions,
        InitializeWorkflowOptions, JobService, JobSnapshotData, JobStatusData,
        MediaDocumentReadResultData, MediaErrorCode, MediaOperation, MediaSaveResultData,
        MediaService, MediaServiceError, PrepareStageRunOptions, ProjectDescriptorData,
        ProjectService, RecordStageRunOptions, ReviewDecisionData, ReviewStageRunOptions,
        ReviewerReferenceData, ScenePlanEditData, StorageService, StoreArtifactFileOptions,
        TerminalRunStatusData, WorkflowService,
    };
    use serde_json::{json, Value};
    use tempfile::TempDir;

    use crate::media_runtime::{
        MediaExecutionTestGate, MediaRetryOptions, MediaRuntime, TimelineEnqueueOptions,
    };

    struct EnqueueFixture {
        _temp: TempDir,
        external_dir: std::path::PathBuf,
        project: ProjectDescriptorData,
        storage: StorageService,
        workflow: WorkflowService,
        jobs: JobService,
        media: MediaService,
        runtime: MediaRuntime,
        reviewed_inputs: BTreeMap<String, FrozenArtifactInputData>,
    }

    impl EnqueueFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("media enqueue project parent");
            let external_dir = temp.path().join("EXTERNAL_ABSOLUTE_PATH_CANARY");
            fs::create_dir(&external_dir).expect("create external source directory");
            fs::write(
                external_dir.join("voice-source.wav"),
                pcm_wave(16_000, 1, 16, 16_000),
            )
            .expect("write audio source");
            fs::write(
                external_dir.join("captions-source.srt"),
                b"1\n00:00:00,000 --> 00:00:01,000\nhello\n",
            )
            .expect("write captions source");

            let projects = ProjectService::default();
            let project = projects
                .create_project(CreateProjectOptions {
                    parent_path: temp.path().to_string_lossy().into_owned(),
                    directory_name: "media-enqueue".to_owned(),
                    name: "Media enqueue fixture".to_owned(),
                    workflow_definition_id: "workflow_standard_v1".to_owned(),
                    default_locale: Some("zh-CN".to_owned()),
                })
                .expect("create media enqueue project");
            let storage = StorageService::new(temp.path().join("index.sqlite3"), projects.clone());
            let workflow = WorkflowService::new(projects.clone(), storage.clone());
            workflow
                .initialize_project_workflow(InitializeWorkflowOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                })
                .expect("initialize media enqueue workflow");
            let brief = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "brief",
                "brief",
                vec![],
            );
            let research = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "research",
                "claim_set",
                vec![workflow_input_ref(&brief, "brief")],
            );
            let script = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "script",
                "script",
                vec![workflow_input_ref(&research, "claim_set")],
            );
            let audio = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "audio",
                "voice_audio",
                vec![workflow_input_ref(&script, "script")],
            );
            let captions = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "captions",
                "captions",
                vec![
                    workflow_input_ref(&script, "script"),
                    workflow_input_ref(&audio, "voice_audio"),
                ],
            );
            let scene_plan = approve_fixture_stage(
                &external_dir,
                &project,
                &storage,
                &workflow,
                "scene_plan",
                "scene_plan",
                vec![
                    workflow_input_ref(&research, "claim_set"),
                    workflow_input_ref(&script, "script"),
                    workflow_input_ref(&captions, "captions"),
                ],
            );
            let reviewed_inputs = [brief, research, script, audio, captions, scene_plan]
                .into_iter()
                .map(|input| (input.stage_id.clone(), input))
                .collect();
            let media = MediaService::new(projects.clone(), storage.clone(), workflow.clone());
            let jobs = JobService::new(projects, storage.clone(), workflow.clone());
            let runtime = MediaRuntime::new(media.clone(), storage.clone(), jobs.clone());
            Self {
                _temp: temp,
                external_dir,
                project,
                storage,
                workflow,
                jobs,
                media,
                runtime,
                reviewed_inputs,
            }
        }

        fn audio_request(&self, key: &str, run_id: &str) -> Value {
            json!({
                "apiVersion": "1.1.0",
                "command": "enqueue_audio_import",
                "projectPath": self.project.project_path,
                "expectedProjectId": self.project.project_id,
                "runId": run_id,
                "sourcePath": self.external_dir.join("voice-source.wav"),
                "scriptInput": self.reviewed_input("script"),
                "rights": rights("fixture-audio"),
                "limits": {"maxBytes": 65536},
                "idempotencyKey": key,
            })
        }

        fn captions_request(&self, key: &str, run_id: &str) -> Value {
            json!({
                "apiVersion": "1.1.0",
                "command": "enqueue_captions_import",
                "projectPath": self.project.project_path,
                "expectedProjectId": self.project.project_id,
                "runId": run_id,
                "sourcePath": self.external_dir.join("captions-source.srt"),
                "scriptInput": self.reviewed_input("script"),
                "audioInput": self.reviewed_input("audio"),
                "audioDurationMs": 1_000,
                "rights": rights("fixture-captions"),
                "limits": {
                    "maxBytes": 1024,
                    "maxCueCount": 100,
                    "maxCueTextBytes": 1024
                },
                "idempotencyKey": key,
            })
        }

        fn scene_plan_request(&self, key: &str, run_id: &str) -> Value {
            json!({
                "apiVersion": "1.0.0",
                "command": "generate_scene_plan",
                "projectPath": self.project.project_path,
                "expectedProjectId": self.project.project_id,
                "runId": run_id,
                "researchInput": self.reviewed_input("research"),
                "scriptInput": self.reviewed_input("script"),
                "captionsInput": self.reviewed_input("captions"),
                "idempotencyKey": key,
            })
        }

        fn timeline_request(&self, key: &str, run_id: &str) -> Value {
            json!({
                "apiVersion": "1.0.0",
                "command": "generate_timeline",
                "projectPath": self.project.project_path,
                "expectedProjectId": self.project.project_id,
                "runId": run_id,
                "audioInput": self.reviewed_input("audio"),
                "captionsInput": self.reviewed_input("captions"),
                "scenePlanInput": self.reviewed_input("scene_plan"),
                "canvas": {
                    "width": 1920,
                    "height": 1080,
                    "frameRateNumerator": 30,
                    "frameRateDenominator": 1
                },
                "safeArea": {"x": 96, "y": 54, "width": 1728, "height": 972},
                "idempotencyKey": key,
            })
        }

        fn receipt(&self, accepted: &Value) -> Value {
            let job_id = accepted["jobId"].as_str().expect("accepted job id");
            let bytes = fs::read(
                Path::new(&self.project.project_path)
                    .join("requests/jobs")
                    .join(format!("{job_id}.json")),
            )
            .expect("read persisted media request receipt");
            serde_json::from_slice(&bytes).expect("decode persisted media request receipt")
        }

        fn reviewed_input(&self, stage_id: &str) -> Value {
            serde_json::to_value(
                self.reviewed_inputs
                    .get(stage_id)
                    .expect("approved fixture input"),
            )
            .expect("serialize approved fixture input")
        }

        fn approve_and_freeze_worker_output(
            &self,
            stage_id: &str,
            run_id: &str,
            snapshot: &JobSnapshotData,
            expected_kind: &str,
        ) -> FrozenArtifactInputData {
            let artifact_id = self.artifact_id_for_kind(snapshot, expected_kind);
            let review_id = format!("review_{stage_id}_worker_chain");
            self.workflow
                .review_stage_run(ReviewStageRunOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.to_owned(),
                    review_id: review_id.clone(),
                    decision: ReviewDecisionData::Approved,
                    reviewer: ReviewerReferenceData {
                        kind: "human".to_owned(),
                        reviewer_id: "reviewer_worker_chain".to_owned(),
                        display_name: "Worker Chain Reviewer".to_owned(),
                    },
                    comments: "approved real media worker output".to_owned(),
                    artifact_ids: snapshot.artifact_ids.clone(),
                })
                .expect("approve real media worker output");
            let read = self
                .storage
                .get_artifact(&self.project.project_path, &artifact_id)
                .expect("read approved real worker artifact");
            assert_eq!(read.artifact["stageId"], stage_id);
            assert_eq!(read.artifact["runId"], run_id);
            let provenance = read.artifact["provenance"]
                .as_array()
                .expect("worker artifact provenance");
            let claim_ids = provenance
                .iter()
                .filter_map(|item| item["claimId"].as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            let evidence_refs = provenance
                .iter()
                .filter_map(|item| item["evidenceRef"].as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            assert!(!claim_ids.is_empty());
            assert_eq!(claim_ids.len(), evidence_refs.len());
            FrozenArtifactInputData {
                stage_id: stage_id.to_owned(),
                run_id: run_id.to_owned(),
                artifact_id,
                content_hash: read.artifact["contentHash"]
                    .as_str()
                    .expect("worker artifact content hash")
                    .to_owned(),
                review_record_id: review_id,
                claim_ids,
                evidence_refs,
            }
        }

        fn artifact_id_for_kind(&self, snapshot: &JobSnapshotData, expected_kind: &str) -> String {
            snapshot
                .artifact_ids
                .iter()
                .find(|artifact_id| {
                    self.storage
                        .get_artifact(&self.project.project_path, artifact_id)
                        .is_ok_and(|read| read.artifact["kind"] == expected_kind)
                })
                .cloned()
                .unwrap_or_else(|| panic!("job output must contain kind {expected_kind}"))
        }

        async fn run_job_to_success(&self, accepted: &Value) -> JobSnapshotData {
            let job_id = accepted["jobId"]
                .as_str()
                .expect("accepted media job id")
                .to_owned();
            self.runtime
                .run_until_terminal(
                    &self.project.project_path,
                    &self.project.project_id,
                    &job_id,
                )
                .await;
            let snapshot = self
                .jobs
                .get_job(GetJobOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    job_id,
                })
                .expect("read terminal media worker job");
            assert_eq!(
                snapshot.status,
                JobStatusData::Succeeded,
                "media worker failure: {:?}",
                snapshot.last_error
            );
            assert_eq!(snapshot.progress, 1.0);
            let expected_artifacts = match snapshot.job["stageId"].as_str() {
                Some("audio" | "captions") => 2,
                _ => 1,
            };
            assert_eq!(snapshot.artifact_ids.len(), expected_artifacts);
            assert!(snapshot.last_error.is_none());
            snapshot
        }
    }

    fn assert_run_artifacts_match_snapshot(run: &Value, snapshot: &JobSnapshotData) {
        let mut run_ids = run["artifactIds"]
            .as_array()
            .expect("StageRun artifact ids")
            .iter()
            .map(|value| value.as_str().expect("StageRun artifact id").to_owned())
            .collect::<Vec<_>>();
        let mut snapshot_ids = snapshot.artifact_ids.clone();
        run_ids.sort();
        snapshot_ids.sort();
        assert_eq!(run_ids, snapshot_ids);
    }

    fn approve_fixture_stage(
        external_dir: &Path,
        project: &ProjectDescriptorData,
        storage: &StorageService,
        workflow: &WorkflowService,
        stage_id: &str,
        kind: &str,
        input_refs: Vec<Value>,
    ) -> FrozenArtifactInputData {
        let run_id = format!("run_{stage_id}_approved_fixture");
        let review_id = format!("review_{stage_id}_approved_fixture");
        workflow
            .prepare_stage_run(PrepareStageRunOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.clone(),
                job_id: format!("job_fixture_{stage_id}"),
                input_refs,
                executor: json!({
                    "providerId": "fixture",
                    "providerVersion": "1.0.0",
                    "executionMode": "local"
                }),
            })
            .expect("prepare approved fixture stage");
        let source = external_dir.join(format!("approved-{stage_id}.json"));
        let claim_id = format!("claim_{stage_id}_fixture");
        let evidence_ref = format!("evidence_{stage_id}_fixture");
        let payload = if stage_id == "script" {
            json!({
                "schemaVersion": "narracut.script/v1",
                "title": "Worker fixture script",
                "language": "en",
                "summary": "Traceable worker fixture.",
                "estimatedDurationSeconds": 1.0,
                "segments": [{
                    "segmentId": "segment_worker_fixture",
                    "order": 0,
                    "title": "Fixture",
                    "narration": "hello",
                    "provenance": [{
                        "claimId": claim_id,
                        "evidenceRef": evidence_ref,
                    }],
                }],
            })
        } else {
            json!({"stage": stage_id})
        };
        fs::write(
            &source,
            serde_json::to_vec(&payload).expect("serialize approved fixture artifact"),
        )
        .expect("write approved fixture artifact");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": stage_id,
            "runId": run_id,
            "kind": kind,
            "mediaType": "application/json",
            "evidenceRole": "non_evidence",
            "source": {"origin":"generated","providerId":"fixture","model":"fixture"},
            "provenance": [{"claimId": claim_id, "evidenceRef": evidence_ref}],
        }))
        .expect("build approved fixture artifact draft");
        let committed = storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import approved fixture artifact");
        let artifact_id = committed.artifact["artifactId"]
            .as_str()
            .expect("approved fixture artifact id")
            .to_owned();
        let content_hash = committed.artifact["contentHash"]
            .as_str()
            .expect("approved fixture content hash")
            .to_owned();
        workflow
            .record_stage_run(RecordStageRunOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.clone(),
                status: TerminalRunStatusData::Succeeded,
                job_id: format!("job_fixture_{stage_id}"),
                artifact_ids: vec![artifact_id.clone()],
                log_summary: json!({"message":"fixture complete","warnings":[],"errors":[]}),
            })
            .expect("record approved fixture stage");
        workflow
            .review_stage_run(ReviewStageRunOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.clone(),
                review_id: review_id.clone(),
                decision: ReviewDecisionData::Approved,
                reviewer: ReviewerReferenceData {
                    kind: "human".to_owned(),
                    reviewer_id: "reviewer_fixture".to_owned(),
                    display_name: "Fixture Reviewer".to_owned(),
                },
                comments: "approved for enqueue fixture".to_owned(),
                artifact_ids: vec![artifact_id.clone()],
            })
            .expect("approve fixture stage");
        FrozenArtifactInputData {
            stage_id: stage_id.to_owned(),
            run_id,
            artifact_id,
            content_hash,
            review_record_id: review_id,
            claim_ids: vec![claim_id],
            evidence_refs: vec![evidence_ref],
        }
    }

    fn workflow_input_ref(input: &FrozenArtifactInputData, kind: &str) -> Value {
        json!({
            "refId": format!("fixture_{}_{}", input.stage_id, input.artifact_id),
            "referenceType": "artifact",
            "kind": kind,
            "contentHash": input.content_hash,
            "artifactId": input.artifact_id,
            "sourceRunId": input.run_id,
            "reviewRecordId": input.review_record_id,
            "claimIds": input.claim_ids,
            "evidenceRefs": input.evidence_refs,
        })
    }

    fn pcm_wave(sample_rate: u32, channels: u16, bits_per_sample: u16, frames: u32) -> Vec<u8> {
        let block_align = channels * (bits_per_sample / 8);
        let data_bytes = frames * u32::from(block_align);
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0".to_vec();
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        bytes.resize(bytes.len() + data_bytes as usize, 0);
        let riff_size = u32::try_from(bytes.len() - 8).expect("small fixture WAV");
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes
    }

    fn rights(license_id: &str) -> Value {
        json!({
            "ownership": "self_recorded",
            "author": "Fixture Author",
            "rightsStatement": "Fixture-owned media source.",
            "licenseId": license_id,
            "attributionText": "",
            "authorizationRecords": [{
                "authorizationRecordId": format!("authorization_{license_id}"),
                "authorizationType": "material_use",
                "grantor": "Fixture Author",
                "scope": "Fixture-owned media source.",
                "evidenceRef": license_id,
                "recordedAt": "2026-07-18T00:00:00Z"
            }],
            "voiceAuthorization": {
                "applicability": "not_applicable",
                "reason": "not_voice_clone"
            },
        })
    }

    #[test]
    fn four_media_enqueue_commands_return_schema_valid_accepted_results() {
        let fixture = EnqueueFixture::new();
        let accepted = [
            serde_json::to_value(
                handle_generate_timeline(
                    &fixture.runtime,
                    fixture.timeline_request("enqueue-timeline", "run_timeline_enqueue"),
                )
                .expect("enqueue timeline"),
            )
            .expect("serialize timeline accepted"),
            serde_json::to_value(
                handle_generate_scene_plan(
                    &fixture.runtime,
                    fixture.scene_plan_request("enqueue-scene", "run_scene_plan_enqueue"),
                )
                .expect("enqueue scene plan"),
            )
            .expect("serialize scene-plan accepted"),
            serde_json::to_value(
                handle_enqueue_captions_import(
                    &fixture.runtime,
                    fixture.captions_request("enqueue-captions", "run_captions_enqueue"),
                )
                .expect("enqueue captions"),
            )
            .expect("serialize captions accepted"),
            serde_json::to_value(
                handle_enqueue_audio_import(
                    &fixture.runtime,
                    fixture.audio_request("enqueue-audio", "run_audio_enqueue"),
                )
                .expect("enqueue audio"),
            )
            .expect("serialize audio accepted"),
        ];

        for (result, operation) in accepted.iter().zip([
            "generate_timeline",
            "generate_scene_plan",
            "enqueue_captions_import",
            "enqueue_audio_import",
        ]) {
            validate_media_command_message(result).expect("accepted result follows media schema");
            assert_eq!(result["operation"], operation);
            assert_eq!(result["ownerProjectId"], fixture.project.project_id);
            assert_eq!(result["idempotentReplay"], false);
            assert_eq!(
                result["jobId"].as_str().map(str::len),
                Some("job_".len() + 64)
            );
        }
    }

    #[tokio::test]
    async fn audio_worker_executes_off_thread_and_commits_auditable_terminal_state() {
        let fixture = EnqueueFixture::new();
        let run_id = "run_audio_worker_success";
        let accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("audio-worker-success", run_id),
            )
            .expect("enqueue worker audio import"),
        )
        .expect("serialize accepted audio job");
        let job_id = accepted["jobId"]
            .as_str()
            .expect("accepted audio job id")
            .to_owned();
        let receipt = fixture.receipt(&accepted);

        fixture
            .runtime
            .run_until_terminal(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &job_id,
            )
            .await;

        let snapshot = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id,
            })
            .expect("read terminal audio job");
        assert_eq!(
            snapshot.status,
            JobStatusData::Succeeded,
            "audio worker failure: {:?}",
            snapshot.last_error
        );
        assert_eq!(snapshot.progress, 1.0);
        assert_eq!(snapshot.artifact_ids.len(), 2);
        assert!(snapshot.last_error.is_none());

        let raw_artifact_id = fixture.artifact_id_for_kind(&snapshot, "audio_source");
        let derived_artifact_id = fixture.artifact_id_for_kind(&snapshot, "voice_audio");
        let derived = fixture
            .storage
            .get_artifact(&fixture.project.project_path, &derived_artifact_id)
            .expect("read derived Audio artifact");
        assert_eq!(derived.artifact["kind"], "voice_audio");
        let source_artifact_ids = derived.artifact["source"]["sourceArtifactIds"]
            .as_array()
            .expect("derived Audio sourceArtifactIds");
        assert!(source_artifact_ids
            .iter()
            .any(|value| value.as_str() == Some(raw_artifact_id.as_str())));
        let raw = fixture
            .storage
            .get_artifact(&fixture.project.project_path, &raw_artifact_id)
            .expect("read raw Audio source artifact");
        assert_eq!(raw.artifact["kind"], "audio_source");
        let derived_document: Value = serde_json::from_slice(
            &fixture
                .storage
                .read_artifact_content_bounded(
                    &fixture.project.project_path,
                    &fixture.project.project_id,
                    &derived_artifact_id,
                    1024 * 1024,
                )
                .expect("read derived Audio document"),
        )
        .expect("decode derived Audio document");
        assert_eq!(derived_document["artifactUri"], raw.content_uri);

        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&fixture.project.project_path)
                    .join("runs/audio")
                    .join(run_id)
                    .join("run.json"),
            )
            .expect("read terminal Audio StageRun"),
        )
        .expect("decode terminal Audio StageRun");
        assert_eq!(run["status"], "succeeded");
        assert_run_artifacts_match_snapshot(&run, &snapshot);
        assert_eq!(run["logSummary"]["errors"], json!([]));

        let persisted = format!(
            "{}{}{}",
            serde_json::to_string(&receipt).expect("serialize receipt"),
            serde_json::to_string(&snapshot).expect("serialize terminal snapshot"),
            serde_json::to_string(&run).expect("serialize terminal run"),
        );
        assert!(!persisted.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!persisted.contains(&fixture.external_dir.to_string_lossy().into_owned()));
    }

    #[tokio::test]
    async fn four_stage_media_workers_execute_from_real_approved_outputs() {
        let fixture = EnqueueFixture::new();

        let audio_run_id = "run_audio_worker_chain";
        let audio_accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("worker-chain-audio", audio_run_id),
            )
            .expect("enqueue chain Audio"),
        )
        .expect("serialize chain Audio accepted");
        let audio_job = fixture.run_job_to_success(&audio_accepted).await;
        let audio_input = fixture.approve_and_freeze_worker_output(
            "audio",
            audio_run_id,
            &audio_job,
            "voice_audio",
        );

        let captions_run_id = "run_captions_worker_chain";
        let mut captions_request =
            fixture.captions_request("worker-chain-captions", captions_run_id);
        captions_request["audioInput"] =
            serde_json::to_value(&audio_input).expect("serialize chain Audio input");
        let captions_accepted = serde_json::to_value(
            handle_enqueue_captions_import(&fixture.runtime, captions_request)
                .expect("enqueue chain Captions"),
        )
        .expect("serialize chain Captions accepted");
        let captions_job = fixture.run_job_to_success(&captions_accepted).await;
        let captions_input = fixture.approve_and_freeze_worker_output(
            "captions",
            captions_run_id,
            &captions_job,
            "captions",
        );

        let scene_run_id = "run_scene_plan_worker_chain";
        let mut scene_request = fixture.scene_plan_request("worker-chain-scene", scene_run_id);
        scene_request["captionsInput"] =
            serde_json::to_value(&captions_input).expect("serialize chain Captions input");
        let scene_accepted = serde_json::to_value(
            handle_generate_scene_plan(&fixture.runtime, scene_request)
                .expect("enqueue chain Scene Plan"),
        )
        .expect("serialize chain Scene Plan accepted");
        let scene_job = fixture.run_job_to_success(&scene_accepted).await;
        let scene_input = fixture.approve_and_freeze_worker_output(
            "scene_plan",
            scene_run_id,
            &scene_job,
            "scene_plan",
        );

        let timeline_run_id = "run_timeline_worker_chain";
        let mut timeline_request =
            fixture.timeline_request("worker-chain-timeline", timeline_run_id);
        timeline_request["audioInput"] =
            serde_json::to_value(&audio_input).expect("serialize chain Timeline Audio input");
        timeline_request["captionsInput"] =
            serde_json::to_value(&captions_input).expect("serialize chain Timeline Captions input");
        timeline_request["scenePlanInput"] =
            serde_json::to_value(&scene_input).expect("serialize chain Timeline Scene Plan input");
        let timeline_accepted = serde_json::to_value(
            handle_generate_timeline(&fixture.runtime, timeline_request)
                .expect("enqueue chain Timeline"),
        )
        .expect("serialize chain Timeline accepted");
        let timeline_job = fixture.run_job_to_success(&timeline_accepted).await;
        let _timeline_input = fixture.approve_and_freeze_worker_output(
            "timeline",
            timeline_run_id,
            &timeline_job,
            "timeline",
        );

        for (stage_id, run_id, expected_kind, job) in [
            ("audio", audio_run_id, "voice_audio", &audio_job),
            ("captions", captions_run_id, "captions", &captions_job),
            ("scene_plan", scene_run_id, "scene_plan", &scene_job),
            ("timeline", timeline_run_id, "timeline", &timeline_job),
        ] {
            let artifact_id = fixture.artifact_id_for_kind(job, expected_kind);
            let artifact = fixture
                .storage
                .get_artifact(&fixture.project.project_path, &artifact_id)
                .expect("read real chain output artifact");
            assert_eq!(artifact.artifact["stageId"], stage_id);
            assert_eq!(artifact.artifact["runId"], run_id);
            assert_eq!(artifact.artifact["kind"], expected_kind);
            let run: Value = serde_json::from_slice(
                &fs::read(
                    Path::new(&fixture.project.project_path)
                        .join("runs")
                        .join(stage_id)
                        .join(run_id)
                        .join("run.json"),
                )
                .expect("read real chain StageRun"),
            )
            .expect("decode real chain StageRun");
            assert_eq!(run["status"], "succeeded");
            assert_run_artifacts_match_snapshot(&run, job);
        }
    }

    #[tokio::test]
    async fn media_retry_restores_verified_request_and_succeeds_after_source_repair() {
        let fixture = EnqueueFixture::new();
        let source_accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("retry-source-failed", "run_audio_retry_source_failed"),
            )
            .expect("enqueue failed retry source"),
        )
        .expect("serialize failed retry source");
        let source_job_id = source_accepted["jobId"]
            .as_str()
            .expect("failed source job id")
            .to_owned();
        let source_receipt = fixture.receipt(&source_accepted);
        let staged_source_uri = source_receipt["stagedSourceUri"]
            .as_str()
            .expect("staged source URI");
        let staged_source_path = Path::new(&fixture.project.project_path).join(staged_source_uri);
        let original_staged_bytes = fs::read(&staged_source_path).expect("read staged source");
        fs::write(&staged_source_path, b"tampered staged media source")
            .expect("tamper staged source");

        fixture
            .runtime
            .run_until_terminal(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &source_job_id,
            )
            .await;
        let failed_source = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: source_job_id.clone(),
            })
            .expect("read failed source job");
        assert_eq!(failed_source.status, JobStatusData::Failed);
        let source_error = failed_source
            .last_error
            .as_ref()
            .expect("tampered source failure");
        assert_eq!(source_error.code, "media_source_content_corrupt");
        assert!(!source_error.retryable);
        assert!(source_error.details.is_empty());
        let failed_serialized =
            serde_json::to_string(&failed_source).expect("serialize failed source job");
        assert!(!failed_serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!failed_serialized.contains(staged_source_uri));
        assert!(!failed_serialized.contains(&staged_source_path.to_string_lossy().into_owned()));
        assert!(!failed_serialized.contains(&fixture.external_dir.to_string_lossy().into_owned()));

        fs::write(&staged_source_path, original_staged_bytes).expect("restore staged source");
        let retry_options = MediaRetryOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_job_id,
            new_run_id: "run_audio_retry_after_source_repair".to_owned(),
            idempotency_key: "media-retry-after-source-repair".to_owned(),
        };
        let retry = fixture
            .runtime
            .retry_media_job(retry_options.clone())
            .expect("retry repaired media job");
        assert_eq!(retry.status, JobStatusData::Queued);
        let retry_job_id = retry.job["jobId"]
            .as_str()
            .expect("retry job id")
            .to_owned();
        let retry_receipt = fixture.receipt(&json!({"jobId": retry_job_id}));
        let mut expected_retry_receipt = source_receipt.clone();
        expected_retry_receipt["runId"] = json!(retry_options.new_run_id);
        expected_retry_receipt["idempotencyKey"] = json!(retry_options.idempotency_key);
        assert_eq!(retry_receipt, expected_retry_receipt);
        for field in [
            "stagedSourceUri",
            "sourceContentHash",
            "rights",
            "limits",
            "inputRefs",
            "configSnapshot",
        ] {
            assert_eq!(retry_receipt[field], source_receipt[field], "field {field}");
        }
        let retry_receipt_serialized =
            serde_json::to_string(&retry_receipt).expect("serialize retry receipt");
        assert!(!retry_receipt_serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!retry_receipt_serialized
            .contains(&fixture.external_dir.to_string_lossy().into_owned()));

        fixture
            .runtime
            .run_until_terminal(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &retry_job_id,
            )
            .await;
        let succeeded_retry = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: retry_job_id,
            })
            .expect("read succeeded retry job");
        assert_eq!(
            succeeded_retry.status,
            JobStatusData::Succeeded,
            "retried media job failure: {:?}",
            succeeded_retry.last_error
        );
        assert_eq!(succeeded_retry.artifact_ids.len(), 2);
    }

    #[tokio::test]
    async fn queued_media_cancellation_is_terminal_and_never_scheduled() {
        let fixture = EnqueueFixture::new();
        let run_id = "run_audio_canceled_before_schedule";
        let accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("audio-canceled-before-schedule", run_id),
            )
            .expect("enqueue queued media job"),
        )
        .expect("serialize queued media job");
        let job_id = accepted["jobId"]
            .as_str()
            .expect("queued media job id")
            .to_owned();

        let canceled = fixture
            .jobs
            .cancel_job(CancelJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: job_id.clone(),
                message: "cancel before worker scheduling".to_owned(),
            })
            .expect("cancel queued media job");
        assert_eq!(canceled.status, JobStatusData::Canceled);
        assert!(canceled.artifact_ids.is_empty());
        assert!(canceled.cancellation_requested);
        assert!(!canceled.finalization_pending);

        assert!(!fixture
            .runtime
            .schedule_supported_job(
                fixture.project.project_path.clone(),
                fixture.project.project_id.clone(),
                job_id.clone(),
            )
            .expect("terminal canceled job scheduling decision"));
        tokio::task::yield_now().await;
        let reread = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id,
            })
            .expect("reread canceled media job");
        assert_eq!(reread.status, JobStatusData::Canceled);
        assert!(reread.artifact_ids.is_empty());

        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&fixture.project.project_path)
                    .join("runs/audio")
                    .join(run_id)
                    .join("run.json"),
            )
            .expect("read canceled media StageRun"),
        )
        .expect("decode canceled media StageRun");
        assert_eq!(run["status"], "canceled");
        assert_eq!(run["artifactIds"], json!([]));
        assert_eq!(run["logSummary"]["errors"], json!([]));
    }

    #[tokio::test]
    async fn running_media_cancellation_waits_for_safe_boundary_and_keeps_artifact_history() {
        let fixture = EnqueueFixture::new();
        let gate = MediaExecutionTestGate::new();
        let runtime = fixture
            .runtime
            .clone()
            .with_execution_test_gate(gate.clone());
        let run_id = "run_audio_canceled_at_safe_boundary";
        let accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &runtime,
                fixture.audio_request("audio-canceled-at-safe-boundary", run_id),
            )
            .expect("enqueue gated media job"),
        )
        .expect("serialize gated media job");
        let job_id = accepted["jobId"]
            .as_str()
            .expect("gated media job id")
            .to_owned();
        assert!(runtime
            .schedule_supported_job(
                fixture.project.project_path.clone(),
                fixture.project.project_id.clone(),
                job_id.clone(),
            )
            .expect("schedule gated media job"));
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            gate.wait_until_entered(),
        )
        .await
        .expect("worker enters spawn_blocking execution gate");

        let running = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: job_id.clone(),
            })
            .expect("read gated running media job");
        assert_eq!(running.status, JobStatusData::Running);
        assert!(!running.cancellation_requested);
        assert!(running.artifact_ids.is_empty());

        let cancellation_requested = fixture
            .jobs
            .cancel_job(CancelJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: job_id.clone(),
                message: "cancel while blocking execution is at a safe boundary".to_owned(),
            })
            .expect("request running media cancellation");
        assert_eq!(cancellation_requested.status, JobStatusData::Running);
        assert!(cancellation_requested.cancellation_requested);
        assert!(cancellation_requested.artifact_ids.is_empty());

        gate.release();
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let snapshot = fixture
                    .jobs
                    .get_job(GetJobOptions {
                        project_path: fixture.project.project_path.clone(),
                        expected_project_id: fixture.project.project_id.clone(),
                        job_id: job_id.clone(),
                    })
                    .expect("poll gated media cancellation");
                if snapshot.status.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("gated media cancellation reaches terminal state");
        assert_eq!(terminal.status, JobStatusData::Canceled);
        assert!(terminal.cancellation_requested);
        assert!(!terminal.finalization_pending);
        assert_eq!(terminal.artifact_ids.len(), 2);
        assert!(terminal.last_error.is_none());

        let derived_artifact_id = fixture.artifact_id_for_kind(&terminal, "voice_audio");
        let derived = fixture
            .storage
            .get_artifact(&fixture.project.project_path, &derived_artifact_id)
            .expect("read derived artifact completed at cancellation boundary");
        assert_eq!(derived.artifact["stageId"], "audio");
        assert_eq!(derived.artifact["runId"], run_id);
        assert_eq!(derived.artifact["kind"], "voice_audio");
        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&fixture.project.project_path)
                    .join("runs/audio")
                    .join(run_id)
                    .join("run.json"),
            )
            .expect("read safe-boundary canceled StageRun"),
        )
        .expect("decode safe-boundary canceled StageRun");
        assert_eq!(run["status"], "canceled");
        assert_run_artifacts_match_snapshot(&run, &terminal);

        let terminal_serialized = format!(
            "{}{}{}",
            serde_json::to_string(&terminal).expect("serialize canceled terminal snapshot"),
            serde_json::to_string(&run).expect("serialize canceled terminal StageRun"),
            serde_json::to_string(&derived.artifact).expect("serialize canceled output artifact"),
        );
        assert!(!terminal_serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!terminal_serialized.contains(&fixture.external_dir.to_string_lossy().into_owned()));

        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        let stable = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id,
            })
            .expect("reread stable canceled media job");
        assert_eq!(stable.status, JobStatusData::Canceled);
        assert_eq!(stable.last_sequence, terminal.last_sequence);
        assert_eq!(stable.artifact_ids, terminal.artifact_ids);
    }

    #[tokio::test]
    async fn fresh_media_runtime_recovers_persisted_queued_receipt_to_success() {
        let fixture = EnqueueFixture::new();
        let run_id = "run_audio_recovered_by_fresh_runtime";
        let accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("audio-fresh-runtime-recovery", run_id),
            )
            .expect("enqueue media job without scheduling"),
        )
        .expect("serialize recoverable media job");
        let job_id = accepted["jobId"]
            .as_str()
            .expect("recoverable media job id")
            .to_owned();
        let receipt_before_restart = fixture.receipt(&accepted);
        let queued = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: job_id.clone(),
            })
            .expect("read persisted queued media job");
        assert_eq!(queued.status, JobStatusData::Queued);
        assert!(queued.artifact_ids.is_empty());

        let fresh_runtime = MediaRuntime::new(
            fixture.media.clone(),
            fixture.storage.clone(),
            fixture.jobs.clone(),
        );
        assert_eq!(
            fresh_runtime
                .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id,)
                .expect("resume media jobs from persisted project"),
            1
        );
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let snapshot = fixture
                    .jobs
                    .get_job(GetJobOptions {
                        project_path: fixture.project.project_path.clone(),
                        expected_project_id: fixture.project.project_id.clone(),
                        job_id: job_id.clone(),
                    })
                    .expect("poll fresh-runtime media recovery");
                if snapshot.status.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fresh runtime completes persisted queued job");
        assert_eq!(
            terminal.status,
            JobStatusData::Succeeded,
            "fresh-runtime recovery failure: {:?}",
            terminal.last_error
        );
        assert_eq!(terminal.artifact_ids.len(), 2);
        assert_eq!(fixture.receipt(&accepted), receipt_before_restart);

        let artifact_id = fixture.artifact_id_for_kind(&terminal, "voice_audio");
        let artifact = fixture
            .storage
            .get_artifact(&fixture.project.project_path, &artifact_id)
            .expect("read fresh-runtime recovered output");
        assert_eq!(artifact.artifact["stageId"], "audio");
        assert_eq!(artifact.artifact["runId"], run_id);
        assert_eq!(artifact.artifact["kind"], "voice_audio");
        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&fixture.project.project_path)
                    .join("runs/audio")
                    .join(run_id)
                    .join("run.json"),
            )
            .expect("read fresh-runtime recovered StageRun"),
        )
        .expect("decode fresh-runtime recovered StageRun");
        assert_eq!(run["status"], "succeeded");
        assert_run_artifacts_match_snapshot(&run, &terminal);
        let persisted = format!(
            "{}{}{}",
            serde_json::to_string(&receipt_before_restart).expect("serialize recovered receipt"),
            serde_json::to_string(&terminal).expect("serialize recovered terminal job"),
            serde_json::to_string(&run).expect("serialize recovered StageRun"),
        );
        assert!(!persisted.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!persisted.contains(&fixture.external_dir.to_string_lossy().into_owned()));
    }

    #[tokio::test]
    async fn canceled_media_retry_is_exactly_replayable_and_conflict_safe() {
        let fixture = EnqueueFixture::new();
        let source_accepted = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("retry-canceled-source", "run_audio_retry_canceled_source"),
            )
            .expect("enqueue canceled retry source"),
        )
        .expect("serialize canceled retry source");
        let source_job_id = source_accepted["jobId"]
            .as_str()
            .expect("canceled retry source job id")
            .to_owned();
        let source_receipt = fixture.receipt(&source_accepted);
        let source = fixture
            .jobs
            .cancel_job(CancelJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: source_job_id.clone(),
                message: "cancel source before dedicated media retry".to_owned(),
            })
            .expect("cancel media retry source");
        assert_eq!(source.status, JobStatusData::Canceled);
        assert!(source.artifact_ids.is_empty());

        let retry_options = MediaRetryOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_job_id: source_job_id.clone(),
            new_run_id: "run_audio_retry_from_canceled_source".to_owned(),
            idempotency_key: "media-retry-canceled-source".to_owned(),
        };
        let retry = fixture
            .runtime
            .retry_media_job(retry_options.clone())
            .expect("retry canceled media job");
        assert_eq!(retry.status, JobStatusData::Queued);
        let retry_job_id = retry.job["jobId"]
            .as_str()
            .expect("canceled-source retry job id")
            .to_owned();
        let retry_receipt = fixture.receipt(&json!({"jobId": retry_job_id}));
        let mut expected_retry_receipt = source_receipt.clone();
        expected_retry_receipt["runId"] = json!(retry_options.new_run_id);
        expected_retry_receipt["idempotencyKey"] = json!(retry_options.idempotency_key);
        assert_eq!(retry_receipt, expected_retry_receipt);

        let replay = fixture
            .runtime
            .retry_media_job(retry_options.clone())
            .expect("exact canceled media retry replay");
        assert_eq!(replay.job, retry.job);
        assert_eq!(replay.status, retry.status);
        assert_eq!(replay.last_sequence, retry.last_sequence);
        assert_eq!(
            fixture.receipt(&json!({"jobId": retry_job_id})),
            retry_receipt
        );

        let conflict = fixture
            .runtime
            .retry_media_job(MediaRetryOptions {
                new_run_id: "run_audio_retry_conflicting_identity".to_owned(),
                ..retry_options
            })
            .expect_err("same retry key cannot bind a different run");
        assert!(matches!(
            conflict,
            crate::media_runtime::MediaRuntimeError::Job(ref error)
                if error.code == narracut_core::JobErrorCode::IdempotencyConflict
        ));

        fixture
            .runtime
            .run_until_terminal(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &retry_job_id,
            )
            .await;
        let succeeded = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: retry_job_id,
            })
            .expect("read succeeded canceled-source retry");
        assert_eq!(
            succeeded.status,
            JobStatusData::Succeeded,
            "canceled-source retry failure: {:?}",
            succeeded.last_error
        );
        assert_eq!(succeeded.artifact_ids.len(), 2);
        assert_eq!(
            fixture
                .jobs
                .get_job(GetJobOptions {
                    project_path: fixture.project.project_path.clone(),
                    expected_project_id: fixture.project.project_id.clone(),
                    job_id: source_job_id,
                })
                .expect("reread original canceled source")
                .status,
            JobStatusData::Canceled
        );
        let persisted = format!(
            "{}{}{}",
            serde_json::to_string(&source_receipt).expect("serialize canceled source receipt"),
            serde_json::to_string(&retry_receipt).expect("serialize canceled retry receipt"),
            serde_json::to_string(&succeeded).expect("serialize canceled retry result"),
        );
        assert!(!persisted.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
        assert!(!persisted.contains(&fixture.external_dir.to_string_lossy().into_owned()));
    }

    #[test]
    fn staged_import_receipts_exclude_external_paths_and_preserve_frozen_traceability() {
        let fixture = EnqueueFixture::new();
        let captions = serde_json::to_value(
            handle_enqueue_captions_import(
                &fixture.runtime,
                fixture.captions_request("path-free-captions", "run_captions_path_free"),
            )
            .expect("enqueue captions"),
        )
        .expect("serialize captions accepted");
        let audio = serde_json::to_value(
            handle_enqueue_audio_import(
                &fixture.runtime,
                fixture.audio_request("path-free-audio", "run_audio_path_free"),
            )
            .expect("enqueue audio"),
        )
        .expect("serialize audio accepted");

        for (receipt, expected_name, expected_ref_count) in [
            (fixture.receipt(&audio), "source-voice-source.wav", 1),
            (fixture.receipt(&captions), "source-captions-source.srt", 2),
        ] {
            let serialized = serde_json::to_string(&receipt).expect("serialize receipt");
            assert!(receipt.get("sourcePath").is_none());
            assert!(!serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
            assert!(!serialized.contains(&fixture.external_dir.to_string_lossy().into_owned()));
            assert_eq!(receipt["sourceFileName"], expected_name);
            assert!(receipt["stagedSourceUri"]
                .as_str()
                .is_some_and(|uri| uri.starts_with("requests/media-sources/sha256/")));
            assert!(receipt["sourceContentHash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("sha256:")));
            assert!(receipt["sourceByteLength"]
                .as_u64()
                .is_some_and(|size| size > 0));
            let refs = receipt["inputRefs"].as_array().expect("frozen input refs");
            assert_eq!(refs.len(), expected_ref_count);
            for input in refs {
                assert!(input["reviewRecordId"].as_str().is_some());
                assert!(!input["claimIds"].as_array().expect("claim ids").is_empty());
                assert!(!input["evidenceRefs"]
                    .as_array()
                    .expect("evidence refs")
                    .is_empty());
            }
            assert!(receipt["configSnapshot"].is_object());
            assert!(receipt["idempotencyKey"].as_str().is_some());
        }
    }

    #[test]
    fn enqueue_replays_exact_requests_and_rejects_conflicting_idempotency_keys() {
        let fixture = EnqueueFixture::new();
        let request = fixture.scene_plan_request("stable-scene-key", "run_scene_plan_stable");
        let first = handle_generate_scene_plan(&fixture.runtime, request.clone())
            .expect("enqueue first scene plan");
        let replay =
            handle_generate_scene_plan(&fixture.runtime, request).expect("replay exact scene plan");
        let first = serde_json::to_value(first).expect("serialize first accepted");
        let replay = serde_json::to_value(replay).expect("serialize replay accepted");
        assert_eq!(first["jobId"], replay["jobId"]);
        assert_eq!(first["idempotentReplay"], false);
        assert_eq!(replay["idempotentReplay"], true);

        let conflict = handle_generate_scene_plan(
            &fixture.runtime,
            fixture.scene_plan_request("stable-scene-key", "run_scene_plan_conflict"),
        )
        .expect_err("same key with different request must conflict");
        let conflict = serde_json::to_value(conflict).expect("serialize conflict");
        validate_media_command_message(&conflict).expect("conflict follows media schema");
        assert_eq!(conflict["code"], "job_conflict");
        assert!(conflict.get("path").is_none());
        assert!(!serde_json::to_string(&conflict)
            .expect("serialize conflict string")
            .contains(&fixture._temp.path().to_string_lossy().into_owned()));
    }

    #[test]
    fn media_runtime_accepts_only_owned_media_stages_and_fixed_local_executor() {
        let fixture = EnqueueFixture::new();
        let request: GenerateTimelineDto =
            decode_request::<narracut_contracts::media_command_types::GenerateTimelineRequest, _>(
                fixture.timeline_request("supports-timeline", "run_timeline_supports"),
                MediaOperation::GenerateTimeline,
            )
            .expect("decode timeline request");
        let outcome = fixture
            .runtime
            .generate_timeline(TimelineEnqueueOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                run_id: request.run_id,
                audio_input: request.audio_input,
                captions_input: request.captions_input,
                scene_plan_input: request.scene_plan_input,
                canvas: request.canvas,
                safe_area: request.safe_area,
                config_snapshot: json!({"runtimeVersion":"1.0.0"}),
                idempotency_key: request.idempotency_key,
            })
            .expect("enqueue timeline directly");
        let snapshot = fixture
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id,
            })
            .expect("read media job");
        assert!(fixture.runtime.supports_media_job(&snapshot));

        let mut script_job = snapshot.clone();
        script_job.job["stageId"] = json!("script");
        assert!(!fixture.runtime.supports_media_job(&script_job));

        let mut provider_job = snapshot.clone();
        provider_job.job["executor"] = json!({
            "providerId": "openai_api",
            "providerVersion": "1.0.0",
            "executionMode": "remote_api",
            "model": "gpt-5.6-terra"
        });
        assert!(!fixture.runtime.supports_media_job(&provider_job));

        let mut foreign_local_job = snapshot;
        foreign_local_job.job["executor"] = json!({
            "providerId": "local-test",
            "providerVersion": "1.0.0",
            "executionMode": "local",
            "model": "bounded_media_v1"
        });
        assert!(!fixture.runtime.supports_media_job(&foreign_local_job));
    }

    #[test]
    fn raw_requests_are_schema_checked_before_dto_conversion() {
        let decoded: GetMediaDocumentDto = decode_request::<GetMediaDocumentRequest, _>(
            json!({
                "apiVersion": "1.0.0",
                "command": "get_media_document",
                "projectPath": "C:/Videos/demo",
                "expectedProjectId": "project_demo",
                "artifactId": "artifact_scene_plan_demo"
            }),
            MediaOperation::ReadMediaDocument,
        )
        .expect("valid request");
        assert_eq!(decoded.expected_project_id, "project_demo");

        for invalid in [
            json!({
                "apiVersion": "1.0.0",
                "command": "get_media_document",
                "projectPath": "C:/Videos/demo",
                "expectedProjectId": "project_demo",
                "artifactId": "artifact_scene_plan_demo",
                "unexpected": true
            }),
            json!({
                "apiVersion": "1.0.0",
                "command": "save_timeline",
                "projectPath": "C:/Videos/demo",
                "expectedProjectId": "project_demo",
                "artifactId": "artifact_scene_plan_demo"
            }),
        ] {
            let error = decode_request::<GetMediaDocumentRequest, GetMediaDocumentDto>(
                invalid,
                MediaOperation::ReadMediaDocument,
            )
            .expect_err("invalid request must fail at the command boundary");
            let value = serde_json::to_value(error).expect("serialize error");
            assert_eq!(value["code"], "invalid_request");
            assert_eq!(value["operation"], "get_media_document");
            validate_media_command_message(&value).expect("error follows schema");
        }
    }

    #[test]
    fn get_and_save_responses_follow_the_generated_contract() {
        let documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../packages/contracts/fixtures/valid-media-documents.json"
        ))
        .expect("valid media fixture");
        let document = documents
            .into_iter()
            .find(|document| document["documentType"] == "scene_plan")
            .expect("scene plan fixture");
        let read: MediaDocumentResult = encode_media_document_response(
            MediaDocumentReadResultData {
                owner_project_id: "project_demo".to_owned(),
                artifact_id: "artifact_scene_plan_demo".to_owned(),
                content_hash: format!("sha256:{}", "a".repeat(64)),
                document,
            },
            MediaOperation::ReadMediaDocument,
        )
        .expect("encode read response");
        let read_value = serde_json::to_value(read).expect("serialize read response");
        assert_eq!(read_value["ownerProjectId"], "project_demo");
        validate_media_command_message(&read_value).expect("read response follows schema");

        let save: MediaSaveResult = encode_response(
            MediaSaveResultData {
                api_version: "1.0.0".to_owned(),
                operation: "save_timeline".to_owned(),
                owner_project_id: "project_demo".to_owned(),
                run_id: "run_timeline_saved".to_owned(),
                artifact_id: "artifact_timeline_saved".to_owned(),
                changed_scene_ids: vec!["scene_0001".to_owned()],
                stale_because_stage_ids: vec!["timeline".to_owned()],
                idempotent_replay: false,
            },
            MediaOperation::SaveTimeline,
        )
        .expect("encode save response");
        let save_value = serde_json::to_value(save).expect("serialize save response");
        assert_eq!(save_value["operation"], "save_timeline");
        validate_media_command_message(&save_value).expect("save response follows schema");
    }

    #[test]
    fn core_errors_are_mapped_to_schema_valid_redacted_errors() {
        let cases = [
            (MediaErrorCode::InvalidRequest, "invalid_request", false),
            (MediaErrorCode::SourceChanged, "source_changed", true),
            (MediaErrorCode::InputNotApproved, "review_required", false),
            (
                MediaErrorCode::InputReferenceMismatch,
                "traceability_incomplete",
                false,
            ),
            (MediaErrorCode::IdempotencyConflict, "job_conflict", false),
            (MediaErrorCode::StorageUnavailable, "io_error", true),
        ];

        for (source_code, expected_code, retryable) in cases {
            let contract = media_error_to_contract(MediaServiceError {
                code: source_code,
                operation: MediaOperation::SaveScenePlan,
                message: "安全错误摘要".to_owned(),
                project_id: Some("project_private".into()),
                stage_id: Some("scene_plan".into()),
                run_id: Some("run_scene_plan_001".into()),
                artifact_id: Some("artifact_scene_plan_001".into()),
            });
            let value = serde_json::to_value(contract).expect("serialize mapped error");
            assert_eq!(value["code"], expected_code);
            assert_eq!(value["retryable"], retryable);
            assert_eq!(value["stageId"], "scene_plan");
            assert_eq!(value["runId"], "run_scene_plan_001");
            assert_eq!(value["artifactId"], "artifact_scene_plan_001");
            assert!(value.get("projectId").is_none());
            validate_media_command_message(&value).expect("mapped error follows schema");
        }
    }

    #[test]
    fn scene_plan_millisecond_boundaries_resolve_to_core_cue_ids() {
        let edits = convert_scene_plan_edits(
            vec![
                ScenePlanEditDto::Split {
                    scene_id: "scene_a".to_owned(),
                    split_at_ms: 50,
                },
                ScenePlanEditDto::MoveBoundary {
                    left_scene_id: "scene_a".to_owned(),
                    right_scene_id: "scene_b".to_owned(),
                    boundary_ms: 125,
                },
            ],
            &boundary_fixture(),
        )
        .expect("resolve exact millisecond boundaries");

        assert_eq!(
            edits,
            [
                ScenePlanEditData::Split {
                    scene_id: "scene_a".to_owned(),
                    boundary_cue_id: "cue_3".to_owned(),
                },
                ScenePlanEditData::MoveBoundary {
                    left_scene_id: "scene_a".to_owned(),
                    right_scene_id: "scene_b".to_owned(),
                    boundary_cue_id: "cue_6".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn scene_plan_boundary_conversion_rejects_non_exact_current_and_non_adjacent_boundaries() {
        let fixture = boundary_fixture();
        for edit in [
            ScenePlanEditDto::Split {
                scene_id: "scene_a".to_owned(),
                split_at_ms: 51,
            },
            ScenePlanEditDto::MoveBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_b".to_owned(),
                boundary_ms: 100,
            },
            ScenePlanEditDto::MoveBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_c".to_owned(),
                boundary_ms: 125,
            },
        ] {
            let error = convert_scene_plan_edits(vec![edit], &fixture)
                .expect_err("unsafe boundary must be rejected");
            let value = serde_json::to_value(error).expect("serialize boundary error");
            assert_eq!(value["code"], "invalid_scene_boundary");
            assert_eq!(value["operation"], "save_scene_plan");
            validate_media_command_message(&value).expect("boundary error follows schema");
        }
    }

    fn boundary_fixture() -> Value {
        json!({
            "scenes": [
                {
                    "sceneId": "scene_a",
                    "suggestedStartMs": 0,
                    "suggestedEndMs": 100,
                    "cueIds": ["cue_1", "cue_2", "cue_3", "cue_4"]
                },
                {
                    "sceneId": "scene_b",
                    "suggestedStartMs": 100,
                    "suggestedEndMs": 200,
                    "cueIds": ["cue_5", "cue_6", "cue_7", "cue_8"]
                },
                {
                    "sceneId": "scene_c",
                    "suggestedStartMs": 200,
                    "suggestedEndMs": 300,
                    "cueIds": ["cue_9", "cue_10"]
                }
            ]
        })
    }
}
