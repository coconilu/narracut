use std::{collections::BTreeSet, io::Write};

use narracut_contracts::{validate_media_document, ArtifactDraft};
use narracut_renderer::{
    deterministic_scene_color, hash_bytes, RenderSceneSpec, RendererIdentity,
    MAX_SCENES as RENDERER_MAX_SCENES,
};
use serde_json::{json, Value};
use tempfile::NamedTempFile;

use crate::job_service::MAX_JOB_ARTIFACTS;
use crate::{
    validate_timeline_semantics, ApprovedArtifactInputData, ArtifactCommitPlanEntryData,
    ArtifactTransferObserver, ArtifactVerificationStatusData, ClaimStageJobRequestOptions,
    CommitRenderOptions, EnqueueRenderOptions, EnqueueStageJobOptions, GetJobOptions, JobService,
    NoopArtifactTransferObserver, PreparedRenderData, ProjectErrorCode, ProjectService,
    ProvenanceReferenceData, RenderCommitResultData, RenderEnqueueResultData, RenderTargetData,
    RendererOperation, RendererServiceError, RendererServiceErrorCode, RendererTimelineInputData,
    RetryPolicyData, SceneSnapshotData, StorageErrorCode, StorageService, StoreArtifactFileOptions,
    ValidateApprovedMediaInputsOptions, WorkflowErrorCode, WorkflowService,
    RENDERER_COMMAND_API_VERSION,
};

const MAX_TIMELINE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MEDIA_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
// StorageService 的有界读取上限是 64 MiB；Renderer 必须使用同一边界，
// 否则任何真实音频在 prepare 阶段都会被底层拒绝。
const MAX_AUDIO_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
const RENDER_FIXED_ARTIFACTS: usize = 2;
const SNAPSHOT_CSP: &str = "default-src 'none'; img-src data: narracut:; media-src narracut:; style-src 'unsafe-inline'; font-src narracut:; script-src 'none'; connect-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

struct DerivedArtifactData<'a> {
    source_path: &'a str,
    kind: &'a str,
    media_type: &'a str,
    source_ids: &'a [String],
    provenance: &'a [ProvenanceReferenceData],
    artifact_id: &'a str,
}

struct SceneTraceabilityData {
    provenance: Vec<ProvenanceReferenceData>,
    claim_ids: Vec<String>,
    evidence_refs: Vec<String>,
}

#[derive(Clone)]
pub struct RendererService {
    project_service: ProjectService,
    storage_service: StorageService,
    workflow_service: WorkflowService,
    job_service: JobService,
}

impl RendererService {
    pub fn new(
        project_service: ProjectService,
        storage_service: StorageService,
        workflow_service: WorkflowService,
        job_service: JobService,
    ) -> Self {
        Self {
            project_service,
            storage_service,
            workflow_service,
            job_service,
        }
    }

    pub fn create_scene_snapshot(
        &self,
        options: crate::CreateSceneSnapshotOptions,
    ) -> Result<SceneSnapshotData, RendererServiceError> {
        let prepared = self.prepare_render(
            EnqueueRenderOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                run_id: "run_render_preview".to_owned(),
                timeline_input: options.timeline_input,
                target: RenderTargetData::Scene {
                    scene_id: options.scene_id,
                },
                config: default_config_from_timeline_placeholder(),
                renderer_identity: None,
                idempotency_key: "renderer-preview-only".to_owned(),
            },
            true,
        )?;
        prepared.snapshots.into_iter().next().ok_or_else(|| {
            RendererServiceError::new(
                RendererServiceErrorCode::SceneNotFound,
                RendererOperation::CreateSceneSnapshot,
                "目标场景不存在。",
            )
        })
    }

    pub fn enqueue_render(
        &self,
        options: EnqueueRenderOptions,
    ) -> Result<RenderEnqueueResultData, RendererServiceError> {
        let operation = match options.target {
            RenderTargetData::Scene { .. } => RendererOperation::EnqueueSceneRender,
            RenderTargetData::Timeline => RendererOperation::EnqueueTimelineRender,
        };
        validate_request_shape(&options, operation)?;
        if options.renderer_identity.is_none() {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::InvalidRequest,
                operation,
                "Renderer identity must be frozen before enqueueing a render.",
            ));
        }
        let input_refs = vec![workflow_input_ref(&options.timeline_input)];
        self.workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "render".to_owned(),
                inputs: vec![approved_input(&options.timeline_input)],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        let timeline = self.read_verified_json(
            &options.project_path,
            &options.expected_project_id,
            &options.timeline_input.artifact_id,
            MAX_TIMELINE_BYTES,
            operation,
        )?;
        validate_target_scene_capacity(&options.target, &timeline, operation)?;
        let request = serde_json::to_value(&options).map_err(|_| {
            RendererServiceError::new(
                RendererServiceErrorCode::ContractViolation,
                operation,
                "无法冻结 Renderer 任务请求。",
            )
        })?;
        let claim = self
            .job_service
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                idempotency_key: options.idempotency_key.clone(),
                request: request.clone(),
            })
            .map_err(|error| map_job_error(error, operation))?;
        let snapshot = self
            .job_service
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: options.project_path,
                    expected_project_id: options.expected_project_id.clone(),
                    stage_id: "render".to_owned(),
                    run_id: options.run_id.clone(),
                    input_refs,
                    executor: json!({
                        "providerId": "narracut_renderer",
                        "providerVersion": "1.0.0",
                        "executionMode": "local",
                        "model": "ffmpeg"
                    }),
                    idempotency_key: options.idempotency_key,
                    retry_policy: RetryPolicyData {
                        max_attempts: 3,
                        initial_backoff_ms: 1_000,
                        backoff_multiplier: 2,
                        max_backoff_ms: 15_000,
                    },
                },
                request,
            )
            .map_err(|error| map_job_error(error, operation))?;
        let job_id = snapshot
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RendererServiceError::new(
                    RendererServiceErrorCode::ContractViolation,
                    operation,
                    "Renderer Job 缺少 jobId。",
                )
            })?
            .to_owned();
        if job_id != claim.job_id {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::JobConflict,
                operation,
                "Renderer 请求收据与 Job 身份不一致。",
            ));
        }
        Ok(RenderEnqueueResultData {
            api_version: RENDERER_COMMAND_API_VERSION.to_owned(),
            operation: operation.as_str().to_owned(),
            owner_project_id: snapshot.owner_project_id,
            run_id: options.run_id,
            job_id,
            idempotent_replay: claim.idempotent_replay,
        })
    }

    pub fn prepare_render(
        &self,
        mut options: EnqueueRenderOptions,
        preview_only: bool,
    ) -> Result<PreparedRenderData, RendererServiceError> {
        let operation = if preview_only {
            RendererOperation::CreateSceneSnapshot
        } else {
            RendererOperation::ExecuteRender
        };
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::ProjectIdentityMismatch,
                operation,
                "Renderer 请求项目身份不匹配。",
            ));
        }
        self.workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "render".to_owned(),
                inputs: vec![approved_input(&options.timeline_input)],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        let timeline = self.read_verified_json(
            &options.project_path,
            &options.expected_project_id,
            &options.timeline_input.artifact_id,
            MAX_TIMELINE_BYTES,
            operation,
        )?;
        validate_media_document(&timeline)
            .map_err(|_| contract_error(operation, "Timeline 未通过 media 契约。"))?;
        validate_timeline_semantics(&timeline)
            .map_err(|_| contract_error(operation, "Timeline 语义无效。"))?;
        if timeline.get("documentType").and_then(Value::as_str) != Some("timeline")
            || timeline.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || timeline.get("runId").and_then(Value::as_str)
                != Some(options.timeline_input.run_id.as_str())
        {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::CrossProjectReference,
                operation,
                "Timeline 文档身份与冻结引用不一致。",
            ));
        }
        let duration_ms = required_u64(&timeline, "durationMs", operation)?;
        let timeline_canvas = timeline
            .get("canvas")
            .cloned()
            .ok_or_else(|| contract_error(operation, "Timeline 缺少 canvas。"))?;
        if preview_only {
            options.config = render_config_from_timeline(&timeline)?;
        } else {
            validate_request_shape(&options, operation)?;
            if serde_json::to_value(options.config.canvas).ok().as_ref() != Some(&timeline_canvas)
                || duration_ms > options.config.max_duration_ms
            {
                return Err(RendererServiceError::new(
                    RendererServiceErrorCode::InvalidRequest,
                    operation,
                    "渲染画布/帧率必须与冻结 Timeline 完全一致，且时长不能超过配置上限。",
                ));
            }
        }
        let input_refs = timeline
            .get("inputRefs")
            .and_then(Value::as_array)
            .ok_or_else(|| contract_error(operation, "Timeline 缺少 inputRefs。"))?;
        let mut source_artifact_ids = vec![options.timeline_input.artifact_id.clone()];
        let mut scene_plan = None;
        let mut audio_document = None;
        for input in input_refs {
            let artifact_id = required_string(input, "artifactId", operation)?;
            let stage_id = required_string(input, "stageId", operation)?;
            let content_hash = required_string(input, "contentHash", operation)?;
            let read = self
                .storage_service
                .get_artifact(&options.project_path, &artifact_id)
                .map_err(|error| map_storage_error(error, operation))?;
            if read.owner_project_id != options.expected_project_id
                || read.artifact.get("projectId").and_then(Value::as_str)
                    != Some(options.expected_project_id.as_str())
                || read.artifact.get("stageId").and_then(Value::as_str) != Some(stage_id.as_str())
                || read.artifact.get("runId").and_then(Value::as_str)
                    != input.get("runId").and_then(Value::as_str)
                || read.artifact.get("contentHash").and_then(Value::as_str)
                    != Some(content_hash.as_str())
            {
                return Err(RendererServiceError::new(
                    RendererServiceErrorCode::InputHashMismatch,
                    operation,
                    "Timeline 的上游引用发生身份或哈希漂移。",
                )
                .for_artifact(artifact_id));
            }
            let document = self.read_verified_json(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_MEDIA_DOCUMENT_BYTES,
                operation,
            )?;
            match stage_id.as_str() {
                "scene_plan" => scene_plan = Some(document),
                "audio" => audio_document = Some((read.artifact, document)),
                "captions" => {
                    validate_media_document(&document)
                        .map_err(|_| contract_error(operation, "Captions 文档无效。"))?;
                }
                _ => return Err(contract_error(operation, "Timeline 包含未声明的上游阶段。")),
            }
            source_artifact_ids.push(artifact_id);
        }
        let scene_plan = scene_plan
            .ok_or_else(|| contract_error(operation, "Timeline 缺少 Scene Plan 引用。"))?;
        validate_media_document(&scene_plan)
            .map_err(|_| contract_error(operation, "Scene Plan 文档无效。"))?;
        let (audio_metadata, audio_document) = audio_document
            .ok_or_else(|| contract_error(operation, "Timeline 缺少 Audio 引用。"))?;
        validate_media_document(&audio_document)
            .map_err(|_| contract_error(operation, "Audio 文档无效。"))?;
        let raw_audio_artifact_id = audio_metadata
            .get("source")
            .and_then(|value| value.get("sourceArtifactIds"))
            .and_then(Value::as_array)
            .and_then(|ids| {
                ids.iter().filter_map(Value::as_str).find(|id| {
                    self.storage_service
                        .get_artifact(&options.project_path, id)
                        .ok()
                        .and_then(|read| {
                            read.artifact
                                .get("kind")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .as_deref()
                        == Some("audio_source")
                })
            })
            .ok_or_else(|| {
                contract_error(operation, "Audio 文档未闭合到原始 audio_source Artifact。")
            })?
            .to_owned();
        let audio_bytes = self
            .storage_service
            .verify_artifact(&options.project_path, &raw_audio_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if audio_bytes.status != ArtifactVerificationStatusData::Verified {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::InputHashMismatch,
                operation,
                "The source audio Artifact failed content-hash verification.",
            )
            .for_artifact(&raw_audio_artifact_id));
        }
        let audio_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &raw_audio_artifact_id,
                MAX_AUDIO_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        source_artifact_ids.push(raw_audio_artifact_id);
        validate_target_scene_capacity(&options.target, &timeline, operation)?;
        let target_scene_ids = match &options.target {
            RenderTargetData::Scene { scene_id } => vec![scene_id.clone()],
            RenderTargetData::Timeline => timeline
                .get("sceneTrack")
                .and_then(Value::as_array)
                .ok_or_else(|| contract_error(operation, "Timeline 缺少 sceneTrack。"))?
                .iter()
                .map(|item| required_string(item, "sceneId", operation))
                .collect::<Result<Vec<_>, _>>()?,
        };
        let snapshots = target_scene_ids
            .iter()
            .map(|scene_id| {
                build_snapshot(
                    &options.expected_project_id,
                    &options.timeline_input,
                    &timeline,
                    &scene_plan,
                    scene_id,
                    operation,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreparedRenderData {
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            timeline_input: options.timeline_input,
            config: options.config,
            target: options.target,
            snapshots,
            audio_bytes,
            source_artifact_ids,
        })
    }

    pub fn render_scene_specs(prepared: &PreparedRenderData) -> Vec<RenderSceneSpec> {
        prepared
            .snapshots
            .iter()
            .map(|snapshot| RenderSceneSpec {
                scene_id: snapshot.scene_id.clone(),
                start_ms: snapshot.start_ms,
                end_ms: snapshot.end_ms,
                color: deterministic_scene_color(&snapshot.scene_id),
            })
            .collect()
    }

    pub fn commit_render(
        &self,
        options: CommitRenderOptions,
    ) -> Result<RenderCommitResultData, RendererServiceError> {
        self.commit_render_with_control(options, &NoopArtifactTransferObserver)
    }

    pub fn commit_render_with_control(
        &self,
        options: CommitRenderOptions,
        observer: &dyn ArtifactTransferObserver,
    ) -> Result<RenderCommitResultData, RendererServiceError> {
        let operation = RendererOperation::CommitRender;
        let job = self
            .job_service
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: options.job_id.clone(),
            })
            .map_err(|error| map_job_error(error, operation))?;
        if job.job.get("stageRunId").and_then(Value::as_str)
            != Some(options.prepared.run_id.as_str())
        {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::JobConflict,
                operation,
                "Render commit 的 Job 与冻结 runId 不一致。",
            ));
        }
        let mut commit_plan = options
            .prepared
            .snapshots
            .iter()
            .map(|snapshot| ArtifactCommitPlanEntryData {
                artifact_id: stable_render_artifact_id(
                    &options.job_id,
                    &format!("snapshot:{}", snapshot.scene_id),
                ),
                kind: "scene_snapshot".to_owned(),
            })
            .collect::<Vec<_>>();
        let video_kind = match options.prepared.target {
            RenderTargetData::Scene { .. } => "rendered_scene",
            RenderTargetData::Timeline => "rendered_video",
        };
        commit_plan.push(ArtifactCommitPlanEntryData {
            artifact_id: stable_render_artifact_id(&options.job_id, "video"),
            kind: video_kind.to_owned(),
        });
        commit_plan.push(ArtifactCommitPlanEntryData {
            artifact_id: stable_render_artifact_id(&options.job_id, "render_log"),
            kind: "render_log".to_owned(),
        });
        self.storage_service
            .begin_artifact_commit_journal(
                &options.project_path,
                &options.expected_project_id,
                &options.job_id,
                &options.prepared.run_id,
                &job.created_at,
                &commit_plan,
            )
            .map_err(|error| map_storage_error(error, operation))?;

        let mut artifact_ids = Vec::new();
        let mut manifest = Vec::new();
        for (snapshot, plan) in options.prepared.snapshots.iter().zip(commit_plan.iter()) {
            let mut file = NamedTempFile::new()
                .map_err(|_| io_error(operation, "无法创建 Snapshot 临时文件。"))?;
            file.write_all(snapshot.html.as_bytes())
                .map_err(|_| io_error(operation, "无法写入 Snapshot 临时文件。"))?;
            let commit = self.import_derived(
                &options,
                DerivedArtifactData {
                    source_path: file.path().to_string_lossy().as_ref(),
                    kind: "scene_snapshot",
                    media_type: "text/html",
                    source_ids: &options.prepared.source_artifact_ids,
                    provenance: &snapshot.provenance,
                    artifact_id: &plan.artifact_id,
                },
                &job.created_at,
                observer,
            )?;
            let artifact_id = artifact_string(&commit.artifact, "artifactId", operation)?;
            artifact_ids.push(artifact_id.clone());
            manifest.push(manifest_entry(
                &commit.artifact,
                "scene_snapshot",
                snapshot.end_ms - snapshot.start_ms,
                options.prepared.config.canvas.width,
                options.prepared.config.canvas.height,
                false,
                vec![snapshot.scene_id.clone()],
            ));
        }
        let provenance = target_provenance(&options.prepared.snapshots);
        let video_plan = &commit_plan[options.prepared.snapshots.len()];
        let video_commit = self.import_derived(
            &options,
            DerivedArtifactData {
                source_path: &options.rendered_file_path,
                kind: video_kind,
                media_type: "video/mp4",
                source_ids: &options.prepared.source_artifact_ids,
                provenance: &provenance,
                artifact_id: &video_plan.artifact_id,
            },
            &job.created_at,
            observer,
        )?;
        let video_artifact_id = artifact_string(&video_commit.artifact, "artifactId", operation)?;
        artifact_ids.push(video_artifact_id.clone());
        let scene_ids = options
            .prepared
            .snapshots
            .iter()
            .map(|snapshot| snapshot.scene_id.clone())
            .collect::<Vec<_>>();
        manifest.push(manifest_entry(
            &video_commit.artifact,
            video_kind,
            options.process_result.duration_ms,
            options.process_result.width,
            options.process_result.height,
            options.process_result.has_audio,
            scene_ids.clone(),
        ));
        let identity = renderer_identity_json(&options.renderer_identity);
        let target = match options.prepared.target {
            RenderTargetData::Scene { .. } => "scene",
            RenderTargetData::Timeline => "timeline",
        };
        let renderer_log_summary = json!({
            "rendererIdentity": identity,
            "timelineInput": options.prepared.timeline_input,
            "config": options.prepared.config,
            "sceneSnapshotHashes": options.prepared.snapshots.iter().map(|snapshot| snapshot.content_hash.clone()).collect::<Vec<_>>(),
            "artifactManifest": manifest,
            "affectedSceneIds": scene_ids,
            "reusedSceneIds": [],
            "stderrTail": options.process_result.stderr_tail,
            "message": "Renderer v1 已完成原子 Artifact 提交。",
            "warnings": [], "errors": []
        });
        let result = json!({
            "apiVersion": RENDERER_COMMAND_API_VERSION,
            "operation": "get_render_result",
            "ownerProjectId": options.expected_project_id,
            "runId": options.prepared.run_id,
            "jobId": options.job_id,
            "status": "succeeded",
            "target": target,
            "timelineInput": options.prepared.timeline_input,
            "config": options.prepared.config,
            "rendererIdentity": identity,
            "snapshotHashes": options.prepared.snapshots.iter().map(|snapshot| snapshot.content_hash.clone()).collect::<Vec<_>>(),
            "artifacts": manifest,
            "affectedSceneIds": scene_ids,
            "reusedSceneIds": [], "diagnostics": [], "logSummary": renderer_log_summary
        });
        narracut_contracts::validate_renderer_message(&result)
            .map_err(|_| contract_error(operation, "RenderResult 未通过 Renderer v1 契约。"))?;
        let mut result_file = NamedTempFile::new()
            .map_err(|_| io_error(operation, "无法创建 RenderResult 临时文件。"))?;
        serde_json::to_writer(&mut result_file, &result)
            .map_err(|_| io_error(operation, "无法序列化 RenderResult。"))?;
        result_file
            .write_all(b"\n")
            .map_err(|_| io_error(operation, "无法写入 RenderResult。"))?;
        let result_commit = self.import_derived(
            &options,
            DerivedArtifactData {
                source_path: result_file.path().to_string_lossy().as_ref(),
                kind: "render_log",
                media_type: "application/json",
                source_ids: &options.prepared.source_artifact_ids,
                provenance: &provenance,
                artifact_id: &commit_plan[options.prepared.snapshots.len() + 1].artifact_id,
            },
            &job.created_at,
            observer,
        )?;
        let result_artifact_id = artifact_string(&result_commit.artifact, "artifactId", operation)?;
        artifact_ids.push(result_artifact_id.clone());
        let log_summary = json!({
            "message": "Renderer v1 已完成原子 Artifact 提交。",
            "warnings": [],
            "errors": [],
            "logArtifactId": result_artifact_id,
        });
        Ok(RenderCommitResultData {
            owner_project_id: options.expected_project_id,
            run_id: options.prepared.run_id,
            artifact_ids,
            video_artifact_id,
            result_artifact_id,
            result,
            log_summary,
        })
    }

    fn import_derived(
        &self,
        options: &CommitRenderOptions,
        input: DerivedArtifactData<'_>,
        created_at: &str,
        observer: &dyn ArtifactTransferObserver,
    ) -> Result<crate::ArtifactCommitResultData, RendererServiceError> {
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "render", "runId": options.prepared.run_id, "kind": input.kind, "mediaType": input.media_type,
            "evidenceRole": "non_evidence", "source": { "origin": "derived", "sourceArtifactIds": input.source_ids }, "provenance": input.provenance
        })).map_err(|_| contract_error(RendererOperation::CommitRender, "Render ArtifactDraft 未通过持久化契约。"))?;
        self.storage_service
            .import_artifact_file_idempotent_bounded_controlled(
                StoreArtifactFileOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                    source_path: input.source_path.to_owned(),
                    artifact: draft,
                },
                input.artifact_id,
                created_at,
                options.prepared.config.max_temporary_bytes,
                observer,
            )
            .map_err(|error| map_storage_error(error, RendererOperation::CommitRender))
    }

    fn read_verified_json(
        &self,
        project_path: &str,
        project_id: &str,
        artifact_id: &str,
        max_bytes: u64,
        operation: RendererOperation,
    ) -> Result<Value, RendererServiceError> {
        let verification = self
            .storage_service
            .verify_artifact(project_path, artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.status != ArtifactVerificationStatusData::Verified {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::InputHashMismatch,
                operation,
                "Artifact 内容哈希或字节数复验失败。",
            )
            .for_artifact(artifact_id));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(project_path, project_id, artifact_id, max_bytes)
            .map_err(|error| map_storage_error(error, operation))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| contract_error(operation, "Artifact 不是合法 JSON。"))
    }
}

fn build_snapshot(
    project_id: &str,
    timeline_input: &RendererTimelineInputData,
    timeline: &Value,
    scene_plan: &Value,
    scene_id: &str,
    operation: RendererOperation,
) -> Result<SceneSnapshotData, RendererServiceError> {
    let track = timeline
        .get("sceneTrack")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("sceneId").and_then(Value::as_str) == Some(scene_id))
        })
        .ok_or_else(|| {
            RendererServiceError::new(
                RendererServiceErrorCode::SceneNotFound,
                operation,
                "Timeline 中不存在目标场景。",
            )
        })?;
    let scene = scene_plan
        .get("scenes")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("sceneId").and_then(Value::as_str) == Some(scene_id))
        })
        .ok_or_else(|| {
            RendererServiceError::new(
                RendererServiceErrorCode::TraceabilityIncomplete,
                operation,
                "Scene Plan 中缺少目标场景追溯。",
            )
        })?;
    let start_ms = required_u64(track, "startMs", operation)?;
    let end_ms = required_u64(track, "endMs", operation)?;
    let title = required_string(scene, "title", operation)?;
    let narrative_role = required_string(scene, "narrativeRole", operation)?;
    let caption_cue_ids = string_array(scene, "cueIds", operation)?;
    let SceneTraceabilityData {
        provenance,
        claim_ids,
        evidence_refs,
    } = scene_traceability(scene, operation)?;
    let canvas: narracut_renderer::RenderCanvas = serde_json::from_value(
        timeline
            .get("canvas")
            .cloned()
            .ok_or_else(|| contract_error(operation, "Timeline 缺少 canvas。"))?,
    )
    .map_err(|_| contract_error(operation, "Timeline canvas 无效。"))?;
    let safe_area = timeline
        .get("safeArea")
        .cloned()
        .ok_or_else(|| contract_error(operation, "Timeline 缺少 safeArea。"))?;
    let snapshot_seed = hash_bytes(
        format!(
            "{}\0{}\0{}",
            project_id, timeline_input.content_hash, scene_id
        )
        .as_bytes(),
    );
    let snapshot_id = format!("snapshot_{}", &snapshot_seed["sha256:".len()..]);
    let html = scene_html(&SceneHtmlData {
        snapshot_id: &snapshot_id,
        scene_id,
        title: &title,
        narrative_role: &narrative_role,
        canvas,
        safe_area: &safe_area,
        claim_ids: &claim_ids,
        evidence_refs: &evidence_refs,
    });
    if html.len() > MAX_SNAPSHOT_BYTES
        || html.contains("<script")
        || html.contains("http://")
        || html.contains("https://")
        || html.contains("file:")
    {
        return Err(RendererServiceError::new(
            RendererServiceErrorCode::SnapshotTooLarge,
            operation,
            "Scene Snapshot 超出上限或包含禁止资源/脚本。",
        ));
    }
    let mut snapshot = SceneSnapshotData {
        snapshot_version: "1.0.0".to_owned(),
        snapshot_id,
        project_id: project_id.to_owned(),
        timeline_artifact_id: timeline_input.artifact_id.clone(),
        timeline_content_hash: timeline_input.content_hash.clone(),
        scene_id: scene_id.to_owned(),
        start_ms,
        end_ms,
        canvas,
        safe_area,
        title,
        narrative_role,
        caption_cue_ids,
        provenance,
        claim_ids,
        evidence_refs,
        csp: SNAPSHOT_CSP.to_owned(),
        resource_uris: Vec::new(),
        html,
        content_hash: String::new(),
    };
    snapshot.content_hash = hash_bytes(
        &serde_json::to_vec(&snapshot)
            .map_err(|_| contract_error(operation, "无法规范化 Scene Snapshot。"))?,
    );
    let message = json!({ "apiVersion": "1.0.0", "operation": "create_scene_snapshot", "ownerProjectId": project_id, "snapshot": snapshot });
    narracut_contracts::validate_renderer_message(&message)
        .map_err(|_| contract_error(operation, "Scene Snapshot 未通过 Renderer v1 契约。"))?;
    Ok(snapshot)
}

fn scene_traceability(
    scene: &Value,
    operation: RendererOperation,
) -> Result<SceneTraceabilityData, RendererServiceError> {
    let claim_ids = string_array(scene, "claimIds", operation)?;
    let evidence_refs = string_array(scene, "evidenceRefs", operation)?;
    let provenance = if let Some(items) = scene.get("provenance") {
        let items = items.as_array().ok_or_else(|| {
            RendererServiceError::new(
                RendererServiceErrorCode::TraceabilityIncomplete,
                operation,
                "场景 provenance 必须是 claimId/evidenceRef 对数组。",
            )
        })?;
        if items.len() > 1_024 {
            return Err(RendererServiceError::new(
                RendererServiceErrorCode::ResourceLimitExceeded,
                operation,
                "场景 provenance 超出 1024 对上限。",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut pairs = Vec::with_capacity(items.len());
        for item in items {
            let claim_id = required_string(item, "claimId", operation)?;
            let evidence_ref = required_string(item, "evidenceRef", operation)?;
            if claim_id.len() > 512 || evidence_ref.len() > 512 {
                return Err(RendererServiceError::new(
                    RendererServiceErrorCode::TraceabilityIncomplete,
                    operation,
                    "场景 provenance 字段超出 512 字符上限。",
                ));
            }
            if !seen.insert((claim_id.clone(), evidence_ref.clone())) {
                return Err(RendererServiceError::new(
                    RendererServiceErrorCode::TraceabilityIncomplete,
                    operation,
                    "场景 provenance 对必须唯一。",
                ));
            }
            pairs.push(ProvenanceReferenceData {
                claim_id,
                evidence_ref,
            });
        }
        pairs
    } else {
        match (claim_ids.as_slice(), evidence_refs.as_slice()) {
            ([], []) => Vec::new(),
            ([claim_id], [evidence_ref]) => vec![ProvenanceReferenceData {
                claim_id: claim_id.clone(),
                evidence_ref: evidence_ref.clone(),
            }],
            _ => {
                return Err(RendererServiceError::new(
                    RendererServiceErrorCode::TraceabilityIncomplete,
                    operation,
                    "多值追溯集合缺少权威 provenance 对，无法安全渲染。",
                ));
            }
        }
    };

    let mut seen_claims = BTreeSet::new();
    let mut seen_evidence = BTreeSet::new();
    let mut projected_claims = Vec::new();
    let mut projected_evidence = Vec::new();
    for pair in &provenance {
        if seen_claims.insert(pair.claim_id.clone()) {
            projected_claims.push(pair.claim_id.clone());
        }
        if seen_evidence.insert(pair.evidence_ref.clone()) {
            projected_evidence.push(pair.evidence_ref.clone());
        }
    }
    if projected_claims != claim_ids || projected_evidence != evidence_refs {
        return Err(RendererServiceError::new(
            RendererServiceErrorCode::TraceabilityIncomplete,
            operation,
            "claimIds/evidenceRefs 必须是 provenance 对的有序唯一投影。",
        ));
    }
    Ok(SceneTraceabilityData {
        provenance,
        claim_ids,
        evidence_refs,
    })
}

fn target_provenance(snapshots: &[SceneSnapshotData]) -> Vec<ProvenanceReferenceData> {
    let mut seen = BTreeSet::new();
    let mut provenance = Vec::new();
    for snapshot in snapshots {
        for pair in &snapshot.provenance {
            if seen.insert((pair.claim_id.clone(), pair.evidence_ref.clone())) {
                provenance.push(pair.clone());
            }
        }
    }
    provenance
}

fn stable_render_artifact_id(job_id: &str, role: &str) -> String {
    let digest = hash_bytes(format!("renderer-v1\0{job_id}\0{role}").as_bytes());
    format!("artifact_render_{}", &digest["sha256:".len()..])
}

struct SceneHtmlData<'a> {
    snapshot_id: &'a str,
    scene_id: &'a str,
    title: &'a str,
    narrative_role: &'a str,
    canvas: narracut_renderer::RenderCanvas,
    safe_area: &'a Value,
    claim_ids: &'a [String],
    evidence_refs: &'a [String],
}

fn scene_html(data: &SceneHtmlData<'_>) -> String {
    let title = escape_html(data.title);
    let role = escape_html(data.narrative_role);
    let claim_text = escape_html(&data.claim_ids.join(", "));
    let evidence_text = escape_html(&data.evidence_refs.join(", "));
    format!("<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{SNAPSHOT_CSP}\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>*{{box-sizing:border-box}}html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:#080c14;color:#f8fafc;font-family:system-ui,sans-serif}}main{{position:relative;width:{}px;height:{}px;background:linear-gradient(145deg,#101827,#071018);display:grid;place-items:center}}article{{position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;border:2px solid rgba(103,232,249,.45);padding:48px;display:flex;flex-direction:column;justify-content:center}}h1{{font-size:72px;margin:0 0 24px;line-height:1.1}}p{{font-size:30px;color:#a5f3fc;margin:0}}footer{{position:absolute;bottom:24px;left:48px;font-size:14px;color:#94a3b8}}</style></head><body><main data-snapshot-id=\"{}\" data-scene-id=\"{}\"><article><h1>{}</h1><p>{}</p></article><footer>claim: {} · evidence: {}</footer></main></body></html>", data.canvas.width, data.canvas.height, data.safe_area.get("x").and_then(Value::as_u64).unwrap_or(0), data.safe_area.get("y").and_then(Value::as_u64).unwrap_or(0), data.safe_area.get("width").and_then(Value::as_u64).unwrap_or(u64::from(data.canvas.width)), data.safe_area.get("height").and_then(Value::as_u64).unwrap_or(u64::from(data.canvas.height)), escape_html(data.snapshot_id), escape_html(data.scene_id), title, role, claim_text, evidence_text)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn approved_input(input: &RendererTimelineInputData) -> ApprovedArtifactInputData {
    ApprovedArtifactInputData {
        ref_id: format!("renderer_timeline_{}", input.artifact_id),
        kind: "timeline".to_owned(),
        artifact_id: input.artifact_id.clone(),
        source_run_id: input.run_id.clone(),
        review_record_id: input.review_record_id.clone(),
        content_hash: input.content_hash.clone(),
        claim_ids: input.claim_ids.clone(),
        evidence_refs: input.evidence_refs.clone(),
    }
}
fn workflow_input_ref(input: &RendererTimelineInputData) -> Value {
    json!({ "refId": format!("renderer_timeline_{}", input.artifact_id), "referenceType": "artifact", "kind": "timeline", "artifactId": input.artifact_id, "sourceRunId": input.run_id, "reviewRecordId": input.review_record_id, "contentHash": input.content_hash, "claimIds": input.claim_ids, "evidenceRefs": input.evidence_refs })
}

fn validate_target_scene_capacity(
    target: &RenderTargetData,
    timeline: &Value,
    operation: RendererOperation,
) -> Result<usize, RendererServiceError> {
    let scene_count = match target {
        RenderTargetData::Scene { .. } => 1,
        RenderTargetData::Timeline => timeline
            .get("sceneTrack")
            .and_then(Value::as_array)
            .ok_or_else(|| contract_error(operation, "Timeline is missing sceneTrack."))?
            .len(),
    };
    let artifact_count = scene_count.checked_add(RENDER_FIXED_ARTIFACTS);
    if scene_count == 0
        || scene_count > RENDERER_MAX_SCENES
        || artifact_count.is_none_or(|count| count > MAX_JOB_ARTIFACTS)
    {
        return Err(RendererServiceError::new(
            RendererServiceErrorCode::ResourceLimitExceeded,
            operation,
            format!(
                "Renderer accepts 1..={RENDERER_MAX_SCENES} scenes so the immutable outputs fit the Job/StageRun {MAX_JOB_ARTIFACTS} Artifact limit."
            ),
        ));
    }
    Ok(scene_count)
}

fn validate_request_shape(
    options: &EnqueueRenderOptions,
    operation: RendererOperation,
) -> Result<(), RendererServiceError> {
    if options.timeline_input.stage_id != "timeline"
        || options.run_id.len() < 5
        || !options.run_id.starts_with("run_")
        || options.idempotency_key.len() < 8
        || options.config.video_codec != "libx264"
        || options.config.audio_codec != "aac"
        || options.config.pixel_format != "yuv420p"
        || !matches!(
            options.config.preset.as_str(),
            "veryfast" | "faster" | "fast" | "medium"
        )
        || !(18..=35).contains(&options.config.crf)
        || options.config.max_duration_ms == 0
        || options.config.max_duration_ms > 86_400_000
        || options.config.timeout_ms < 1_000
        || options.config.timeout_ms > 7_200_000
    {
        return Err(RendererServiceError::new(
            RendererServiceErrorCode::InvalidRequest,
            operation,
            "Renderer 请求包含未授权配置或无效身份。",
        ));
    }
    Ok(())
}

fn render_config_from_timeline(
    timeline: &Value,
) -> Result<crate::RenderConfigData, RendererServiceError> {
    let canvas = serde_json::from_value(timeline.get("canvas").cloned().ok_or_else(|| {
        contract_error(
            RendererOperation::CreateSceneSnapshot,
            "Timeline 缺少 canvas。",
        )
    })?)
    .map_err(|_| {
        contract_error(
            RendererOperation::CreateSceneSnapshot,
            "Timeline canvas 无效。",
        )
    })?;
    Ok(crate::RenderConfigData {
        canvas,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        pixel_format: "yuv420p".to_owned(),
        preset: "fast".to_owned(),
        crf: 23,
        max_duration_ms: 86_400_000,
        max_temporary_bytes: 1024 * 1024 * 1024,
        timeout_ms: 600_000,
    })
}
fn default_config_from_timeline_placeholder() -> crate::RenderConfigData {
    crate::RenderConfigData {
        canvas: narracut_renderer::RenderCanvas {
            width: 320,
            height: 180,
            frame_rate_numerator: 30,
            frame_rate_denominator: 1,
        },
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        pixel_format: "yuv420p".to_owned(),
        preset: "fast".to_owned(),
        crf: 23,
        max_duration_ms: 86_400_000,
        max_temporary_bytes: 1024 * 1024 * 1024,
        timeout_ms: 600_000,
    }
}

fn manifest_entry(
    artifact: &Value,
    kind: &str,
    duration_ms: u64,
    width: u32,
    height: u32,
    has_audio: bool,
    scene_ids: Vec<String>,
) -> Value {
    json!({ "artifactId": artifact.get("artifactId").and_then(Value::as_str).unwrap_or_default(), "kind": kind, "uri": artifact.get("uri").and_then(Value::as_str).unwrap_or_default(), "contentHash": artifact.get("contentHash").and_then(Value::as_str).unwrap_or_default(), "byteLength": artifact.get("byteLength").and_then(Value::as_u64).unwrap_or_default(), "mediaType": artifact.get("mediaType").and_then(Value::as_str).unwrap_or_default(), "durationMs": duration_ms, "width": width, "height": height, "hasAudio": has_audio, "sceneIds": scene_ids })
}
fn renderer_identity_json(identity: &RendererIdentity) -> Value {
    json!({ "adapterId": identity.adapter_id, "adapterVersion": identity.adapter_version, "executableFileName": identity.executable_file_name, "executableHash": identity.executable_hash, "ffmpegVersion": identity.ffmpeg_version, "ffprobeFileName": identity.ffprobe_file_name, "ffprobeHash": identity.ffprobe_hash, "ffprobeVersion": identity.ffprobe_version, "capabilityHash": identity.capability_hash })
}

fn required_string(
    value: &Value,
    field: &str,
    operation: RendererOperation,
) -> Result<String, RendererServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 2048)
        .map(str::to_owned)
        .ok_or_else(|| contract_error(operation, format!("缺少字符串字段 {field}。")))
}
fn required_u64(
    value: &Value,
    field: &str,
    operation: RendererOperation,
) -> Result<u64, RendererServiceError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| contract_error(operation, format!("缺少整数宇段 {field}。")))
}
fn string_array(
    value: &Value,
    field: &str,
    operation: RendererOperation,
) -> Result<Vec<String>, RendererServiceError> {
    let values = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| contract_error(operation, format!("缺少数组字段 {field}。")))?;
    let result = values
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| contract_error(operation, format!("{field} 包含非字符串。")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if result.iter().collect::<BTreeSet<_>>().len() != result.len() {
        return Err(contract_error(operation, format!("{field} 包含重复值。")));
    }
    Ok(result)
}
fn artifact_string(
    value: &Value,
    field: &str,
    operation: RendererOperation,
) -> Result<String, RendererServiceError> {
    required_string(value, field, operation)
}
fn contract_error(
    operation: RendererOperation,
    message: impl Into<String>,
) -> RendererServiceError {
    RendererServiceError::new(
        RendererServiceErrorCode::ContractViolation,
        operation,
        message,
    )
}
fn io_error(operation: RendererOperation, message: impl Into<String>) -> RendererServiceError {
    RendererServiceError::new(RendererServiceErrorCode::Io, operation, message).retryable(true)
}

fn map_project_error(
    error: crate::ProjectServiceError,
    operation: RendererOperation,
) -> RendererServiceError {
    let code = match error.code {
        ProjectErrorCode::ProjectNotFound | ProjectErrorCode::MarkerMissing => {
            RendererServiceErrorCode::ProjectNotFound
        }
        ProjectErrorCode::IoError => RendererServiceErrorCode::Io,
        _ => RendererServiceErrorCode::InvalidRequest,
    };
    RendererServiceError::new(code, operation, error.message)
        .retryable(code == RendererServiceErrorCode::Io)
}
fn map_storage_error(
    error: crate::StorageServiceError,
    operation: RendererOperation,
) -> RendererServiceError {
    let code = match error.code {
        StorageErrorCode::ArtifactNotFound | StorageErrorCode::SourceNotFound => {
            RendererServiceErrorCode::ArtifactNotFound
        }
        StorageErrorCode::ContentCorrupt | StorageErrorCode::SourceChanged => {
            RendererServiceErrorCode::InputHashMismatch
        }
        StorageErrorCode::SourceTooLarge => RendererServiceErrorCode::ResourceLimitExceeded,
        StorageErrorCode::OperationCanceled => RendererServiceErrorCode::Canceled,
        StorageErrorCode::LeaseLost => RendererServiceErrorCode::JobConflict,
        StorageErrorCode::IoError | StorageErrorCode::IndexUnavailable => {
            RendererServiceErrorCode::Io
        }
        _ => RendererServiceErrorCode::InvalidRequest,
    };
    RendererServiceError::new(code, operation, error.message)
        .retryable(code == RendererServiceErrorCode::Io)
}
fn map_workflow_error(
    error: crate::WorkflowServiceError,
    operation: RendererOperation,
) -> RendererServiceError {
    let code = match error.code {
        WorkflowErrorCode::StageNotReady => RendererServiceErrorCode::InputStale,
        WorkflowErrorCode::ArtifactMismatch => RendererServiceErrorCode::InputHashMismatch,
        WorkflowErrorCode::ProjectNotFound => RendererServiceErrorCode::ProjectNotFound,
        WorkflowErrorCode::ProjectIdentityMismatch => {
            RendererServiceErrorCode::ProjectIdentityMismatch
        }
        WorkflowErrorCode::IoError => RendererServiceErrorCode::Io,
        _ => RendererServiceErrorCode::ReviewRequired,
    };
    RendererServiceError::new(code, operation, error.message)
        .retryable(code == RendererServiceErrorCode::Io)
}
fn map_job_error(
    error: crate::JobServiceError,
    operation: RendererOperation,
) -> RendererServiceError {
    RendererServiceError::new(
        RendererServiceErrorCode::JobConflict,
        operation,
        error.message,
    )
    .retryable(error.code == crate::JobErrorCode::IoError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderConfigData, RenderTargetData};

    fn timeline_input() -> RendererTimelineInputData {
        RendererTimelineInputData {
            stage_id: "timeline".into(),
            run_id: "run_timeline_001".into(),
            artifact_id: "artifact_timeline_001".into(),
            content_hash: format!("sha256:{}", "a".repeat(64)),
            review_record_id: "review_timeline_001".into(),
            claim_ids: vec!["claim_001".into()],
            evidence_refs: vec!["evidence_001".into()],
        }
    }

    #[test]
    fn scene_snapshot_is_deterministic_isolated_and_html_escaped() {
        let timeline = json!({
            "canvas": { "width": 1920, "height": 1080, "frameRateNumerator": 30, "frameRateDenominator": 1 },
            "safeArea": { "x": 100, "y": 80, "width": 1720, "height": 920 },
            "sceneTrack": [{ "sceneId": "scene_001", "startMs": 0, "endMs": 1500 }]
        });
        let scene_plan = json!({ "scenes": [{
            "sceneId": "scene_001", "title": "<unsafe & title>", "narrativeRole": "explain \"why\"",
            "cueIds": ["cue_001"], "claimIds": ["claim_001"], "evidenceRefs": ["evidence_001"]
        }] });
        let first = build_snapshot(
            "project_001",
            &timeline_input(),
            &timeline,
            &scene_plan,
            "scene_001",
            RendererOperation::CreateSceneSnapshot,
        )
        .expect("valid snapshot");
        let second = build_snapshot(
            "project_001",
            &timeline_input(),
            &timeline,
            &scene_plan,
            "scene_001",
            RendererOperation::CreateSceneSnapshot,
        )
        .expect("same snapshot");
        assert_eq!(first, second);
        assert!(first.html.contains("&lt;unsafe &amp; title&gt;"));
        assert!(!first.html.contains("<script"));
        assert!(!first.html.contains("http://"));
        assert_eq!(first.csp, SNAPSHOT_CSP);
        assert!(first.resource_uris.is_empty());
    }

    #[test]
    fn scene_snapshot_rejects_incomplete_claim_evidence_closure() {
        let timeline = json!({
            "canvas": { "width": 640, "height": 360, "frameRateNumerator": 30, "frameRateDenominator": 1 },
            "safeArea": { "x": 0, "y": 0, "width": 640, "height": 360 },
            "sceneTrack": [{ "sceneId": "scene_001", "startMs": 0, "endMs": 1000 }]
        });
        let scene_plan = json!({ "scenes": [{
            "sceneId": "scene_001", "title": "Title", "narrativeRole": "Role", "cueIds": ["cue_001"],
            "claimIds": ["claim_001"], "evidenceRefs": []
        }] });
        let error = build_snapshot(
            "project_001",
            &timeline_input(),
            &timeline,
            &scene_plan,
            "scene_001",
            RendererOperation::CreateSceneSnapshot,
        )
        .expect_err("traceability must fail closed");
        assert_eq!(error.code, RendererServiceErrorCode::TraceabilityIncomplete);
    }

    #[test]
    fn scene_snapshot_preserves_one_to_many_and_many_to_one_pairs() {
        let timeline = json!({
            "canvas": { "width": 640, "height": 360, "frameRateNumerator": 30, "frameRateDenominator": 1 },
            "safeArea": { "x": 0, "y": 0, "width": 640, "height": 360 },
            "sceneTrack": [{ "sceneId": "scene_001", "startMs": 0, "endMs": 1000 }]
        });
        let scene_plan = json!({ "scenes": [{
            "sceneId": "scene_001", "title": "Title", "narrativeRole": "Role", "cueIds": ["cue_001"],
            "provenance": [
                { "claimId": "claim_001", "evidenceRef": "evidence_001" },
                { "claimId": "claim_001", "evidenceRef": "evidence_002" },
                { "claimId": "claim_002", "evidenceRef": "evidence_002" }
            ],
            "claimIds": ["claim_001", "claim_002"],
            "evidenceRefs": ["evidence_001", "evidence_002"]
        }] });
        let snapshot = build_snapshot(
            "project_001",
            &timeline_input(),
            &timeline,
            &scene_plan,
            "scene_001",
            RendererOperation::CreateSceneSnapshot,
        )
        .expect("one-to-many and many-to-one provenance are legal");

        assert_eq!(snapshot.provenance.len(), 3);
        assert_eq!(snapshot.claim_ids, ["claim_001", "claim_002"]);
        assert_eq!(snapshot.evidence_refs, ["evidence_001", "evidence_002"]);
    }

    #[test]
    fn target_provenance_never_leaks_from_an_unselected_scene() {
        let snapshot = |scene_id: &str, claim_id: &str, evidence_ref: &str| SceneSnapshotData {
            snapshot_version: "1.0.0".into(),
            snapshot_id: format!("snapshot_{scene_id}"),
            project_id: "project_001".into(),
            timeline_artifact_id: "artifact_timeline_001".into(),
            timeline_content_hash: format!("sha256:{}", "a".repeat(64)),
            scene_id: scene_id.into(),
            start_ms: 0,
            end_ms: 1_000,
            canvas: narracut_renderer::RenderCanvas {
                width: 640,
                height: 360,
                frame_rate_numerator: 30,
                frame_rate_denominator: 1,
            },
            safe_area: json!({ "x": 0, "y": 0, "width": 640, "height": 360 }),
            title: "Title".into(),
            narrative_role: "Role".into(),
            caption_cue_ids: vec![],
            provenance: vec![ProvenanceReferenceData {
                claim_id: claim_id.into(),
                evidence_ref: evidence_ref.into(),
            }],
            claim_ids: vec![claim_id.into()],
            evidence_refs: vec![evidence_ref.into()],
            csp: SNAPSHOT_CSP.into(),
            resource_uris: vec![],
            html: "<!doctype html>".into(),
            content_hash: format!("sha256:{}", "b".repeat(64)),
        };
        let first = snapshot("scene_001", "claim_001", "evidence_001");
        let second = snapshot("scene_002", "claim_002", "evidence_002");

        assert_eq!(
            target_provenance(&[first]),
            vec![ProvenanceReferenceData {
                claim_id: "claim_001".into(),
                evidence_ref: "evidence_001".into(),
            }]
        );
        assert_eq!(target_provenance(&[second]).len(), 1);
    }

    #[test]
    fn render_request_rejects_unowned_codec_and_ffmpeg_surface() {
        let options = EnqueueRenderOptions {
            project_path: "project".into(),
            expected_project_id: "project_001".into(),
            run_id: "run_render_001".into(),
            timeline_input: timeline_input(),
            target: RenderTargetData::Timeline,
            config: RenderConfigData {
                canvas: narracut_renderer::RenderCanvas {
                    width: 640,
                    height: 360,
                    frame_rate_numerator: 30,
                    frame_rate_denominator: 1,
                },
                video_codec: "copy".into(),
                audio_codec: "aac".into(),
                pixel_format: "yuv420p".into(),
                preset: "fast".into(),
                crf: 23,
                max_duration_ms: 10_000,
                max_temporary_bytes: 64 * 1024 * 1024,
                timeout_ms: 60_000,
            },
            renderer_identity: None,
            idempotency_key: "renderer-idempotency-001".into(),
        };
        let error = validate_request_shape(&options, RendererOperation::EnqueueTimelineRender)
            .expect_err("codec surface is adapter owned");
        assert_eq!(error.code, RendererServiceErrorCode::InvalidRequest);
    }

    #[test]
    fn timeline_scene_capacity_matches_the_job_terminal_artifact_limit() {
        let timeline = |count: usize| {
            json!({
                "sceneTrack": (0..count)
                    .map(|index| json!({ "sceneId": format!("scene_{index:03}") }))
                    .collect::<Vec<_>>()
            })
        };

        let accepted = validate_target_scene_capacity(
            &RenderTargetData::Timeline,
            &timeline(RENDERER_MAX_SCENES),
            RendererOperation::EnqueueTimelineRender,
        )
        .expect("254 scenes must be accepted before Job creation");
        assert_eq!(accepted, 254);
        assert_eq!(
            accepted + RENDER_FIXED_ARTIFACTS,
            MAX_JOB_ARTIFACTS,
            "snapshots + video + log must fit Job v1"
        );

        let error = validate_target_scene_capacity(
            &RenderTargetData::Timeline,
            &timeline(RENDERER_MAX_SCENES + 1),
            RendererOperation::EnqueueTimelineRender,
        )
        .expect_err("255 scenes must be rejected before Job creation");
        assert_eq!(error.code, RendererServiceErrorCode::ResourceLimitExceeded);
    }
}
