#![allow(clippy::result_large_err)]

use narracut_contracts::{
    validate_renderer_message, CreateSceneSnapshotRequest, EnqueueSceneRenderRequest,
    EnqueueTimelineRenderRequest, GetRenderResultRequest, ProbeRendererRequest,
    RenderJobAcceptedResult, RenderResult, RendererCapabilitiesResult, RendererCommandError,
    SceneSnapshotResult, NARRACUT_RENDERER_API_VERSION,
};
use narracut_core::{
    ArtifactVerificationStatusData, CreateSceneSnapshotOptions, EnqueueRenderOptions,
    GetJobOptions, JobService, JobStatusData, RenderConfigData, RenderTargetData,
    RendererOperation, RendererService, RendererServiceError, RendererTimelineInputData,
    StorageService,
};
use narracut_renderer::{RendererIdentity as InternalRendererIdentity, MAX_LOG_BYTES, MAX_SCENES};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

use crate::renderer_runtime::{RendererRuntime, RendererRuntimeError};

const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024;
const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[tauri::command]
pub async fn probe_renderer(
    runtime: State<'_, RendererRuntime>,
    request: Value,
) -> Result<RendererCapabilitiesResult, RendererCommandError> {
    decode_request::<ProbeRendererRequest>(request, RendererOperation::ProbeRenderer)?;
    let adapter = runtime.adapter();
    let probe = adapter.probe().await;
    run_blocking(RendererOperation::ProbeRenderer, move || {
        let identity = probe.identity.ok_or_else(|| {
            error_value(
                RendererOperation::ProbeRenderer,
                if probe.available { "renderer_unsupported" } else { "renderer_unavailable" },
                probe.diagnostics.join("; "),
                true,
            )
        })?;
        if !probe.available || !probe.supported {
            return Err(error_value(
                RendererOperation::ProbeRenderer,
                if probe.available { "renderer_unsupported" } else { "renderer_unavailable" },
                probe.diagnostics.join("; "),
                true,
            ));
        }
        encode_response(
            json!({
                "apiVersion": NARRACUT_RENDERER_API_VERSION,
                "operation": "probe_renderer",
                "available": true,
                "supported": true,
                "identity": public_identity(&identity),
                "limits": {
                    "maxScenes": MAX_SCENES,
                    "maxSnapshotBytes": MAX_SNAPSHOT_BYTES,
                    "maxResourceBytes": MAX_RESOURCE_BYTES,
                    "maxLogBytes": MAX_LOG_BYTES,
                    "maxConcurrentJobs": 1
                },
                "videoCodecs": probe.video_codecs,
                "audioCodecs": probe.audio_codecs,
                "diagnostics": probe.diagnostics.into_iter().enumerate().map(|(index, message)| json!({
                    "diagnosticId": format!("renderer_diagnostic_{}", index + 1),
                    "severity": "info", "code": "renderer_probe", "message": message
                })).collect::<Vec<_>>()
            }),
            RendererOperation::ProbeRenderer,
        )
    })
    .await
}

#[tauri::command]
pub async fn create_scene_snapshot(
    service: State<'_, RendererService>,
    request: Value,
) -> Result<SceneSnapshotResult, RendererCommandError> {
    let request: CreateSceneSnapshotRequest =
        decode_request(request, RendererOperation::CreateSceneSnapshot)?;
    let service = service.inner().clone();
    run_blocking(RendererOperation::CreateSceneSnapshot, move || {
        let project_id = request.expected_project_id.clone();
        let snapshot = service
            .create_scene_snapshot(CreateSceneSnapshotOptions {
                project_path: request.project_path.to_string(),
                expected_project_id: request.expected_project_id.to_string(),
                timeline_input: timeline_input(request.timeline_input),
                scene_id: request.scene_id.to_string(),
            })
            .map_err(service_error)?;
        encode_response(
            json!({
                "apiVersion": NARRACUT_RENDERER_API_VERSION,
                "operation": "create_scene_snapshot",
                "ownerProjectId": project_id,
                "snapshot": snapshot,
            }),
            RendererOperation::CreateSceneSnapshot,
        )
    })
    .await
}

#[tauri::command]
pub async fn enqueue_scene_render(
    service: State<'_, RendererService>,
    runtime: State<'_, RendererRuntime>,
    request: Value,
) -> Result<RenderJobAcceptedResult, RendererCommandError> {
    let request: EnqueueSceneRenderRequest =
        decode_request(request, RendererOperation::EnqueueSceneRender)?;
    enqueue(
        service.inner().clone(),
        runtime.inner().clone(),
        request.project_path.to_string(),
        request.expected_project_id.to_string(),
        request.run_id.to_string(),
        timeline_input(request.timeline_input),
        RenderTargetData::Scene {
            scene_id: request.scene_id.to_string(),
        },
        render_config(request.config),
        request.idempotency_key.to_string(),
        RendererOperation::EnqueueSceneRender,
    )
    .await
}

#[tauri::command]
pub async fn enqueue_timeline_render(
    service: State<'_, RendererService>,
    runtime: State<'_, RendererRuntime>,
    request: Value,
) -> Result<RenderJobAcceptedResult, RendererCommandError> {
    let request: EnqueueTimelineRenderRequest =
        decode_request(request, RendererOperation::EnqueueTimelineRender)?;
    enqueue(
        service.inner().clone(),
        runtime.inner().clone(),
        request.project_path.to_string(),
        request.expected_project_id.to_string(),
        request.run_id.to_string(),
        timeline_input(request.timeline_input),
        RenderTargetData::Timeline,
        render_config(request.config),
        request.idempotency_key.to_string(),
        RendererOperation::EnqueueTimelineRender,
    )
    .await
}

#[tauri::command]
pub async fn get_render_result(
    jobs: State<'_, JobService>,
    storage: State<'_, StorageService>,
    request: Value,
) -> Result<RenderResult, RendererCommandError> {
    let request: GetRenderResultRequest =
        decode_request(request, RendererOperation::GetRenderResult)?;
    let jobs = jobs.inner().clone();
    let storage = storage.inner().clone();
    run_blocking(RendererOperation::GetRenderResult, move || {
        let snapshot = jobs
            .get_job(GetJobOptions {
                project_path: request.project_path.to_string(),
                expected_project_id: request.expected_project_id.to_string(),
                job_id: request.job_id.to_string(),
            })
            .map_err(|error| {
                error_value(
                    RendererOperation::GetRenderResult,
                    "artifact_not_found",
                    error.message,
                    false,
                )
            })?;
        if snapshot.status != JobStatusData::Succeeded {
            return Err(error_value(
                RendererOperation::GetRenderResult,
                "artifact_not_found",
                "The render result is available only after the Job succeeds.",
                snapshot.status.is_terminal(),
            ));
        }
        let artifact_id = snapshot
            .artifact_ids
            .iter()
            .find(|artifact_id| {
                storage
                    .get_artifact(request.project_path.to_string(), artifact_id.as_str())
                    .ok()
                    .and_then(|read| {
                        read.artifact
                            .get("kind")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .as_deref()
                    == Some("render_log")
            })
            .cloned()
            .ok_or_else(|| {
                error_value(
                    RendererOperation::GetRenderResult,
                    "artifact_not_found",
                    "The succeeded Job has no render_log Artifact.",
                    false,
                )
            })?;
        let verification = storage
            .verify_artifact(request.project_path.to_string(), &artifact_id)
            .map_err(|error| {
                error_value(
                    RendererOperation::GetRenderResult,
                    "io_error",
                    error.message,
                    true,
                )
            })?;
        if verification.status != ArtifactVerificationStatusData::Verified {
            return Err(error_value(
                RendererOperation::GetRenderResult,
                "input_hash_mismatch",
                "The render_log Artifact failed content-hash verification.",
                false,
            ));
        }
        let bytes = storage
            .read_artifact_content_bounded(
                request.project_path.to_string(),
                &request.expected_project_id.to_string(),
                &artifact_id,
                MAX_SNAPSHOT_BYTES,
            )
            .map_err(|error| {
                error_value(
                    RendererOperation::GetRenderResult,
                    "io_error",
                    error.message,
                    true,
                )
            })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            error_value(
                RendererOperation::GetRenderResult,
                "internal_contract_error",
                "The render_log Artifact is not valid JSON.",
                false,
            )
        })?;
        encode_response(value, RendererOperation::GetRenderResult)
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn enqueue(
    service: RendererService,
    runtime: RendererRuntime,
    project_path: String,
    expected_project_id: String,
    run_id: String,
    timeline_input: RendererTimelineInputData,
    target: RenderTargetData,
    config: RenderConfigData,
    idempotency_key: String,
    operation: RendererOperation,
) -> Result<RenderJobAcceptedResult, RendererCommandError> {
    let identity = require_supported_identity(&runtime, operation).await?;
    run_blocking(operation, move || {
        let accepted = service
            .enqueue_render(EnqueueRenderOptions {
                project_path: project_path.clone(),
                expected_project_id: expected_project_id.clone(),
                run_id,
                timeline_input,
                target,
                config,
                renderer_identity: Some(identity),
                idempotency_key,
            })
            .map_err(service_error)?;
        runtime
            .schedule_supported_job(project_path, expected_project_id, accepted.job_id.clone())
            .map_err(|error| runtime_error(error, operation))?;
        encode_response(accepted, operation)
    })
    .await
}

async fn require_supported_identity(
    runtime: &RendererRuntime,
    operation: RendererOperation,
) -> Result<InternalRendererIdentity, RendererCommandError> {
    let probe = runtime.adapter().probe().await;
    if !probe.available || !probe.supported {
        return Err(error_value(
            operation,
            if probe.available {
                "renderer_unsupported"
            } else {
                "renderer_unavailable"
            },
            probe.diagnostics.join("; "),
            true,
        ));
    }
    probe.identity.ok_or_else(|| {
        error_value(
            operation,
            "renderer_unavailable",
            "Renderer probe returned no executable identity.",
            true,
        )
    })
}

fn timeline_input(
    value: narracut_contracts::RendererTimelineInputReference,
) -> RendererTimelineInputData {
    serde_json::from_value(
        serde_json::to_value(value).expect("generated TimelineInputReference must serialize"),
    )
    .expect("generated TimelineInputReference must map to the internal contract")
}

fn render_config(value: narracut_contracts::RendererConfig) -> RenderConfigData {
    serde_json::from_value(
        serde_json::to_value(value).expect("generated RenderConfig must serialize"),
    )
    .expect("generated RenderConfig must map to the internal contract")
}

fn public_identity(identity: &InternalRendererIdentity) -> Value {
    json!({
        "adapterId": identity.adapter_id,
        "adapterVersion": identity.adapter_version,
        "executableFileName": identity.executable_file_name,
        "executableHash": identity.executable_hash,
        "ffmpegVersion": identity.ffmpeg_version,
        "ffprobeFileName": identity.ffprobe_file_name,
        "ffprobeHash": identity.ffprobe_hash,
        "ffprobeVersion": identity.ffprobe_version,
        "capabilityHash": identity.capability_hash,
    })
}

async fn run_blocking<T, F>(
    operation: RendererOperation,
    task: F,
) -> Result<T, RendererCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RendererCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| {
            error_value(
                operation,
                "internal_contract_error",
                "The renderer command worker terminated unexpectedly.",
                false,
            )
        })?
}

fn decode_request<T>(
    request: Value,
    operation: RendererOperation,
) -> Result<T, RendererCommandError>
where
    T: DeserializeOwned + Serialize,
{
    validate_renderer_message(&request).map_err(|error| {
        error_value(
            operation,
            "invalid_request",
            format!("The request violates Renderer v1: {error}"),
            false,
        )
    })?;
    serde_json::from_value(request).map_err(|error| {
        error_value(
            operation,
            "invalid_request",
            format!("The request cannot be decoded: {error}"),
            false,
        )
    })
}

fn encode_response<TInternal, TContract>(
    value: TInternal,
    operation: RendererOperation,
) -> Result<TContract, RendererCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(value).map_err(|error| {
        error_value(
            operation,
            "internal_contract_error",
            format!("Renderer response serialization failed: {error}"),
            false,
        )
    })?;
    validate_renderer_message(&value).map_err(|error| {
        error_value(
            operation,
            "internal_contract_error",
            format!("Renderer response violates v1: {error}"),
            false,
        )
    })?;
    serde_json::from_value(value).map_err(|error| {
        error_value(
            operation,
            "internal_contract_error",
            format!("Renderer response conversion failed: {error}"),
            false,
        )
    })
}

fn service_error(error: RendererServiceError) -> RendererCommandError {
    let operation = error.operation;
    let mut object = Map::from_iter([
        (
            "apiVersion".to_owned(),
            json!(NARRACUT_RENDERER_API_VERSION),
        ),
        ("code".to_owned(), json!(error.code.as_str())),
        ("operation".to_owned(), json!(operation.as_str())),
        ("message".to_owned(), json!(error.message)),
        ("retryable".to_owned(), json!(error.retryable)),
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
    error_from_value(Value::Object(object), operation)
}

fn runtime_error(
    error: RendererRuntimeError,
    operation: RendererOperation,
) -> RendererCommandError {
    error_value(operation, "io_error", error.to_string(), true)
}

fn error_value(
    operation: RendererOperation,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
) -> RendererCommandError {
    error_from_value(
        json!({
            "apiVersion": NARRACUT_RENDERER_API_VERSION,
            "code": code,
            "operation": operation.as_str(),
            "message": message.into(),
            "retryable": retryable,
        }),
        operation,
    )
}

fn error_from_value(value: Value, operation: RendererOperation) -> RendererCommandError {
    validate_renderer_message(&value).unwrap_or_else(|error| {
        panic!(
            "renderer error adapter violated {}: {error}",
            operation.as_str()
        )
    });
    serde_json::from_value(value).unwrap_or_else(|error| {
        panic!(
            "renderer error conversion failed for {}: {error}",
            operation.as_str()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_request, RendererOperation};
    use narracut_contracts::ProbeRendererRequest;

    #[test]
    fn renderer_requests_reject_executable_and_argv_fields() {
        for field in ["executablePath", "argv", "filterComplex", "outputPath"] {
            let mut value =
                serde_json::json!({ "apiVersion": "1.0.0", "command": "probe_renderer" });
            value
                .as_object_mut()
                .expect("object")
                .insert(field.to_owned(), serde_json::json!("attacker-controlled"));
            assert!(decode_request::<ProbeRendererRequest>(
                value,
                RendererOperation::ProbeRenderer
            )
            .is_err());
        }
    }
}
