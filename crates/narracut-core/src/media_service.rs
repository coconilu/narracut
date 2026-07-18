use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use narracut_contracts::{validate_media_document, ArtifactDraft, NARRACUT_MEDIA_SCHEMA_VERSION};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::media_types::system_media_clock;
use crate::{
    apply_scene_plan_edits, apply_timeline_edits, build_scene_plan_document,
    build_timeline_document, parse_pcm_wav_file, parse_srt_file, validate_scene_plan_semantics,
    validate_timeline_semantics, ApplyTimelineEditsOptions, ApprovedArtifactInputData,
    ArtifactVerificationStatusData, BuildScenePlanOptions, BuildTimelineOptions, ClaimJobOptions,
    ClaimStageJobRequestOptions, CompleteJobOptions, EnqueueStageJobOptions, FailJobOptions,
    GenerateScenePlanOptions, GenerateTimelineOptions, GetJobOptions, GetMediaDocumentOptions,
    ImportAudioOptions, ImportCaptionsOptions, JobClock, JobErrorCode, JobFailureData, JobService,
    JobServiceError, JobStatusData, MediaClock, MediaDocumentReadResultData, MediaErrorCode,
    MediaImportResultData, MediaOperation, MediaParseError, MediaParseErrorCode, MediaRightsData,
    MediaSaveResultData, MediaServiceError, ParsedSrt, ProjectErrorCode, ProjectService,
    ProjectServiceError, RecordJobArtifactOptions, RecordStageRunOptions, RetryPolicyData,
    SaveScenePlanOptions, SaveTimelineOptions, ScenePlanError, ScenePlanErrorCode,
    StorageErrorCode, StorageService, StorageServiceError, StoreArtifactFileOptions,
    TerminalRunStatusData, TimelineDomainError, TimelineDomainErrorCode,
    ValidateApprovedMediaInputsOptions, WorkflowErrorCode, WorkflowService, WorkflowServiceError,
};

const MAX_SOURCE_FILE_NAME_BYTES: usize = 255;
const MAX_AUDIO_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_AUDIO_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CAPTION_MAPPINGS: usize = 200_000;
const MAX_SCENE_PLAN_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TIMELINE_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MEDIA_COMMAND_API_VERSION: &str = "1.0.0";
const SCENE_PLAN_ALGORITHM_ID: &str = "narracut.scene-plan.caption-cue-grouping";
const SCENE_PLAN_ALGORITHM_VERSION: &str = "1.0.0";
const TIMELINE_ALGORITHM_ID: &str = "narracut.timeline.approved-media-assembly";
const TIMELINE_ALGORITHM_VERSION: &str = "1.0.0";

type CaptionBuildResult = (
    Vec<Value>,
    Vec<Value>,
    Vec<Value>,
    Vec<CaptionProvenancePair>,
);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CaptionProvenancePair {
    claim_id: String,
    evidence_ref: String,
}

#[derive(Debug)]
struct CaptionScriptSegment {
    start: usize,
    end: usize,
    provenance: Vec<CaptionProvenancePair>,
}

#[derive(Debug)]
struct CaptionScriptTraceability {
    normalized_narration: Vec<char>,
    segments: Vec<CaptionScriptSegment>,
}

struct ScenePlanSourceContext {
    captions_document: Value,
    audio_input: crate::FrozenArtifactInputData,
    audio_duration_ms: u64,
}

struct ScenePlanBaseContext {
    document: Value,
    content_hash: String,
    audio_duration_ms: u64,
    source_artifact_ids: Vec<String>,
    provenance: Vec<Value>,
}

struct TimelineSourceContext {
    audio_document: Value,
    captions_document: Value,
    scene_plan_document: Value,
    audio_duration_ms: u64,
}

struct TimelineBaseContext {
    document: Value,
    content_hash: String,
    source_artifact_ids: Vec<String>,
    provenance: Vec<Value>,
}

struct PreparedMediaSaveRun {
    job_id: String,
    created_at: String,
    lease_id: Option<String>,
}

struct PrepareMediaSaveRunOptions<'a> {
    project_path: &'a str,
    expected_project_id: &'a str,
    stage_id: &'a str,
    run_id: &'a str,
    base_artifact_id: &'a str,
    base_document: &'a Value,
    idempotency_key: &'a str,
    request_fingerprint: &'a str,
    operation: MediaOperation,
}

struct RecordMediaSaveRunOptions<'a> {
    project_path: &'a str,
    expected_project_id: &'a str,
    stage_id: &'a str,
    run_id: &'a str,
    job_id: &'a str,
    lease_id: Option<&'a str>,
    artifact_id: &'a str,
    change_summary: &'a str,
    operation: MediaOperation,
}

struct MediaJobClock {
    inner: Arc<dyn MediaClock>,
}

impl JobClock for MediaJobClock {
    fn now(&self) -> OffsetDateTime {
        self.inner.now()
    }
}

#[derive(Clone)]
pub struct MediaService {
    pub(crate) project_service: ProjectService,
    pub(crate) storage_service: StorageService,
    pub(crate) workflow_service: WorkflowService,
    job_service: JobService,
    pub(crate) clock: Arc<dyn MediaClock>,
    import_lock: Arc<Mutex<()>>,
}

impl MediaService {
    pub fn new(
        project_service: ProjectService,
        storage_service: StorageService,
        workflow_service: WorkflowService,
    ) -> Self {
        Self::with_clock(
            project_service,
            storage_service,
            workflow_service,
            system_media_clock(),
        )
    }

    pub fn with_clock(
        project_service: ProjectService,
        storage_service: StorageService,
        workflow_service: WorkflowService,
        clock: Arc<dyn MediaClock>,
    ) -> Self {
        let job_service = JobService::with_clock(
            project_service.clone(),
            storage_service.clone(),
            workflow_service.clone(),
            Arc::new(MediaJobClock {
                inner: clock.clone(),
            }),
        );
        Self {
            project_service,
            storage_service,
            workflow_service,
            job_service,
            clock,
            import_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn get_media_document(
        &self,
        options: GetMediaDocumentOptions,
    ) -> Result<MediaDocumentReadResultData, MediaServiceError> {
        let operation = MediaOperation::ReadMediaDocument;
        validate_get_media_document_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体文档读取请求声明的项目身份与实际项目不一致。",
            ));
        }

        let read = self
            .storage_service
            .get_artifact(&options.project_path, &options.artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id
            || read.artifact.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体 Artifact 不属于请求项目。",
            ));
        }
        let metadata_artifact_id = artifact_string(&read.artifact, "artifactId", operation)?;
        let run_id = artifact_string(&read.artifact, "runId", operation)?;
        let content_hash = artifact_string(&read.artifact, "contentHash", operation)?;
        if metadata_artifact_id != options.artifact_id
            || !valid_prefixed_id(&run_id, "run_", 160)
            || !is_sha256(&content_hash)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "媒体 Artifact 的身份、运行或内容哈希闭包无效。",
            ));
        }
        let kind = read
            .artifact
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let (stage_id, media_type, document_type, max_bytes) = match kind {
            "voice_audio" => (
                "audio",
                "application/vnd.narracut.audio+json",
                "audio_media",
                MAX_AUDIO_DOCUMENT_BYTES,
            ),
            "captions" => (
                "captions",
                "application/vnd.narracut.captions+json",
                "captions_media",
                MAX_AUDIO_SOURCE_BYTES,
            ),
            "scene_plan" => (
                "scene_plan",
                "application/vnd.narracut.scene-plan+json",
                "scene_plan",
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            ),
            "timeline" => (
                "timeline",
                "application/vnd.narracut.timeline+json",
                "timeline",
                MAX_TIMELINE_DOCUMENT_BYTES,
            ),
            _ => return Err(invalid_media_document_artifact(operation)),
        };
        if read.artifact.get("stageId").and_then(Value::as_str) != Some(stage_id)
            || read.artifact.get("mediaType").and_then(Value::as_str) != Some(media_type)
        {
            return Err(invalid_media_document_artifact(operation));
        }

        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &options.artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.owner_project_id != options.expected_project_id
            || verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "媒体 Artifact 实体未通过内容校验。",
            )
            .with_safe_context(
                Some(&options.expected_project_id),
                Some(stage_id),
                Some(&run_id),
                Some(&options.artifact_id),
            ));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.artifact_id,
                max_bytes,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "媒体 Artifact 不是合法 JSON 文档。",
            )
        })?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "媒体 Artifact 未通过 media v1 契约。",
            )
        })?;
        if document.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体文档属于其他项目。",
            ));
        }
        if document.get("documentType").and_then(Value::as_str) != Some(document_type)
            || document.get("runId").and_then(Value::as_str) != Some(run_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "媒体 Artifact kind、documentType 或 runId 未形成一致闭包。",
            ));
        }
        validate_media_document_read_semantics(
            &document,
            &read.artifact,
            &options.expected_project_id,
            operation,
        )?;

        Ok(MediaDocumentReadResultData {
            owner_project_id: options.expected_project_id,
            artifact_id: options.artifact_id,
            content_hash,
            document,
        })
    }

    pub fn import_audio(
        &self,
        options: ImportAudioOptions,
    ) -> Result<MediaImportResultData, MediaServiceError> {
        let operation = MediaOperation::ImportAudio;
        let _import_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "媒体导入互斥状态不可用。",
            )
        })?;
        let source_file_name = validate_audio_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体请求声明的项目身份与实际项目不一致。",
            ));
        }

        let request_fingerprint = audio_request_fingerprint(&options, &source_file_name)?;
        let receipt_id =
            stable_audio_receipt_id(&options.expected_project_id, &options.idempotency_key);
        if let Some(replay) =
            self.load_audio_replay_or_conflict(&options, &receipt_id, &request_fingerprint)?
        {
            return Ok(replay);
        }

        let parsed = parse_pcm_wav_file(Path::new(&options.source_path), options.limits)
            .map_err(|error| map_media_parse_error(error, operation))?;
        validate_expected_source_hash(
            options.expected_source_content_hash.as_deref(),
            &parsed.content_hash,
            operation,
        )?;

        let approved = self
            .workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "audio".to_owned(),
                inputs: vec![ApprovedArtifactInputData {
                    ref_id: "media_audio_script".to_owned(),
                    kind: "script".to_owned(),
                    artifact_id: options.script_input.artifact_id.clone(),
                    source_run_id: options.script_input.run_id.clone(),
                    review_record_id: options.script_input.review_record_id.clone(),
                    content_hash: options.script_input.content_hash.clone(),
                    claim_ids: options.script_input.claim_ids.clone(),
                    evidence_refs: options.script_input.evidence_refs.clone(),
                }],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        if approved.len() != 1
            || approved[0]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.script_input.artifact_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准输入闭包返回了不一致的 Script Artifact。",
            ));
        }
        self.storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.script_input.artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;

        let provenance = exact_artifact_provenance_union(
            approved.iter().map(|input| &input.artifact),
            operation,
        )?;
        let raw_draft = artifact_draft(
            json!({
                "stageId": "audio",
                "runId": options.run_id,
                "kind": "audio_source",
                "mediaType": "audio/wav",
                "evidenceRole": "expressive_material",
                "source": {
                    "origin": "imported",
                    "sourceUri": internal_source_uri(&parsed.content_hash, &source_file_name),
                    "author": options.rights.author,
                    "license": options.rights.license_id,
                    "attributionText": options.rights.attribution_text,
                    "authorizationRecordIds": [options.rights.license_id],
                },
                "provenance": provenance.clone(),
            }),
            operation,
        )?;
        let raw_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: options.source_path.clone(),
                artifact: raw_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let raw_artifact_id = artifact_string(&raw_commit.artifact, "artifactId", operation)?;
        if raw_commit
            .artifact
            .get("contentHash")
            .and_then(Value::as_str)
            != Some(parsed.content_hash.as_str())
            || raw_commit
                .artifact
                .get("byteLength")
                .and_then(Value::as_u64)
                != Some(parsed.byte_length)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::SourceChanged,
                operation,
                "媒体源在解析与不可变导入之间发生变化。",
            ));
        }

        let created_at = self.clock.now().format(&Rfc3339).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "无法生成 Audio 文档时间戳。",
            )
        })?;
        let document = json!({
            "schemaVersion": NARRACUT_MEDIA_SCHEMA_VERSION,
            "documentType": "audio_media",
            "mediaId": stable_media_id("audio", &request_fingerprint),
            "projectId": options.expected_project_id,
            "runId": options.run_id,
            "artifactUri": raw_commit.content_uri,
            "source": {
                "sourceFileName": source_file_name,
                "sourceContentHash": parsed.content_hash,
                "byteLength": parsed.byte_length,
            },
            "rights": options.rights,
            "durationMs": parsed.duration_ms,
            "sampleRateHz": parsed.sample_rate,
            "bitsPerSample": parsed.bits_per_sample,
            "channels": parsed.channels,
            "blockAlign": parsed.block_align,
            "byteRate": parsed.byte_rate,
            "dataBytes": parsed.data_byte_length,
            "inputRefs": [frozen_input_document(&options.expected_project_id, &options.script_input)],
            "configSnapshot": options.config_snapshot,
            "createdAt": created_at,
        });
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "Audio 文档未通过 media v1 契约。",
            )
        })?;

        let mut document_file = NamedTempFile::new().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::Io,
                operation,
                "无法创建 Audio 文档临时文件。",
            )
        })?;
        serde_json::to_writer(&mut document_file, &document).map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法序列化 Audio 文档。")
        })?;
        document_file.write_all(b"\n").map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法写入 Audio 文档。")
        })?;
        document_file.as_file().sync_all().map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法同步 Audio 文档。")
        })?;
        let derived_draft = artifact_draft(
            json!({
                "stageId": "audio",
                "runId": options.run_id,
                "kind": "voice_audio",
                "mediaType": "application/vnd.narracut.audio+json",
                "evidenceRole": "non_evidence",
                "source": {
                    "origin": "derived",
                    "sourceArtifactIds": [raw_artifact_id, options.script_input.artifact_id],
                },
                "provenance": provenance,
            }),
            operation,
        )?;
        let document_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: document_file.path().to_string_lossy().into_owned(),
                artifact: derived_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
        let content_hash = artifact_string(&document_commit.artifact, "contentHash", operation)?;
        let persisted_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_AUDIO_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Audio 文档不是合法 JSON。",
            )
        })?;
        validate_media_document(&persisted).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Audio 文档未通过 media v1 契约。",
            )
        })?;
        if persisted != document {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "持久化 Audio 文档与写入前文档不一致。",
            ));
        }

        let receipt = json!({
            "schemaVersion": 1,
            "documentType": "media_import_receipt",
            "receiptId": receipt_id,
            "projectId": options.expected_project_id,
            "operation": "import_audio",
            "requestFingerprint": request_fingerprint,
            "runId": options.run_id,
            "rawArtifactId": raw_artifact_id,
            "artifactId": artifact_id,
            "contentHash": content_hash,
        });
        let (_, created) = self
            .storage_service
            .commit_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                &receipt_id,
                &receipt,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        if !created {
            return self
                .load_audio_replay_or_conflict(&options, &receipt_id, &request_fingerprint)?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Audio receipt 并发提交后无法重放。",
                    )
                });
        }

        Ok(MediaImportResultData {
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            raw_artifact_id,
            artifact_id,
            content_hash,
            document,
            idempotent_replay: false,
        })
    }

    pub fn generate_timeline(
        &self,
        options: GenerateTimelineOptions,
    ) -> Result<MediaSaveResultData, MediaServiceError> {
        let operation = MediaOperation::GenerateTimeline;
        let _import_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "Timeline 生成互斥状态不可用。",
            )
        })?;
        validate_generate_timeline_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "Timeline 请求声明的项目身份与实际项目不一致。",
            ));
        }

        let request_fingerprint = timeline_request_fingerprint(&options)?;
        let receipt_id =
            stable_timeline_receipt_id(&options.expected_project_id, &options.idempotency_key);
        let receipt_exists =
            self.timeline_receipt_exists_or_conflict(&options, &receipt_id, &request_fingerprint)?;
        let approved = self
            .workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "timeline".to_owned(),
                inputs: vec![
                    approved_input(&options.audio_input, "media_timeline_audio", "voice_audio"),
                    approved_input(
                        &options.captions_input,
                        "media_timeline_captions",
                        "captions",
                    ),
                    approved_input(
                        &options.scene_plan_input,
                        "media_timeline_scene_plan",
                        "scene_plan",
                    ),
                ],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        if approved.len() != 3
            || approved[0]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.audio_input.artifact_id.as_str())
            || approved[1]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.captions_input.artifact_id.as_str())
            || approved[2]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.scene_plan_input.artifact_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准输入闭包返回了不一致的 Audio/Captions/Scene Plan Artifact。",
            ));
        }
        let context = self.load_timeline_source_context(&options)?;
        let source_artifact_ids = vec![
            options.audio_input.artifact_id.clone(),
            options.captions_input.artifact_id.clone(),
            options.scene_plan_input.artifact_id.clone(),
        ];
        if receipt_exists {
            return self
                .load_timeline_replay_or_conflict(
                    &options,
                    &context,
                    &receipt_id,
                    &request_fingerprint,
                    &source_artifact_ids,
                )?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Timeline receipt 存在但无法重放。",
                    )
                });
        }

        let created_at = self.clock.now().format(&Rfc3339).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "无法生成 Timeline 文档时间戳。",
            )
        })?;
        let document = build_timeline_document(BuildTimelineOptions {
            audio_document: context.audio_document.clone(),
            captions_document: context.captions_document.clone(),
            scene_plan_document: context.scene_plan_document.clone(),
            audio_input: options.audio_input.clone(),
            captions_input: options.captions_input.clone(),
            scene_plan_input: options.scene_plan_input.clone(),
            canvas: options.canvas,
            safe_area: options.safe_area,
            config_snapshot: timeline_config_snapshot(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            stable_seed: request_fingerprint.clone(),
            created_at,
        })
        .map_err(|error| map_timeline_error(error, operation))?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "生成的 TimelineDocument 未通过 media v1 契约。",
            )
        })?;
        validate_timeline_semantics(&document)
            .map_err(|error| map_timeline_error(error, operation))?;
        let changed_scene_ids = timeline_changed_ids(&document, operation)?;

        let mut document_file = NamedTempFile::new().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::Io,
                operation,
                "无法创建 Timeline 文档临时文件。",
            )
        })?;
        serde_json::to_writer(&mut document_file, &document).map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法序列化 Timeline 文档。")
        })?;
        document_file.write_all(b"\n").map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法写入 Timeline 文档。")
        })?;
        document_file.as_file().sync_all().map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法同步 Timeline 文档。")
        })?;
        let provenance = exact_artifact_provenance_union(
            approved.iter().map(|input| &input.artifact),
            operation,
        )?;
        let derived_draft = artifact_draft(
            json!({
                "stageId": "timeline",
                "runId": options.run_id,
                "kind": "timeline",
                "mediaType": "application/vnd.narracut.timeline+json",
                "evidenceRole": "non_evidence",
                "source": {
                    "origin": "derived",
                    "sourceArtifactIds": source_artifact_ids,
                },
                "provenance": provenance,
            }),
            operation,
        )?;
        let document_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: document_file.path().to_string_lossy().into_owned(),
                artifact: derived_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
        let content_hash = artifact_string(&document_commit.artifact, "contentHash", operation)?;
        let persisted_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_TIMELINE_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Timeline 文档不是合法 JSON。",
            )
        })?;
        validate_media_document(&persisted).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Timeline 文档未通过 media v1 契约。",
            )
        })?;
        validate_timeline_semantics(&persisted)
            .map_err(|error| map_timeline_error(error, operation))?;
        if persisted != document {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "持久化 Timeline 文档与写入前文档不一致。",
            ));
        }

        let receipt = json!({
            "schemaVersion": 1,
            "documentType": "media_import_receipt",
            "receiptId": receipt_id,
            "projectId": options.expected_project_id,
            "operation": "generate_timeline",
            "requestFingerprint": request_fingerprint,
            "runId": options.run_id,
            "artifactId": artifact_id,
            "contentHash": content_hash,
        });
        let (_, created) = self
            .storage_service
            .commit_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                &receipt_id,
                &receipt,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        if !created {
            return self
                .load_timeline_replay_or_conflict(
                    &options,
                    &context,
                    &receipt_id,
                    &request_fingerprint,
                    &source_artifact_ids,
                )?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Timeline receipt 并发提交后无法重放。",
                    )
                });
        }

        Ok(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "generate_timeline".to_owned(),
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            artifact_id,
            changed_scene_ids,
            stale_because_stage_ids: vec!["timeline".to_owned()],
            idempotent_replay: false,
        })
    }

    pub fn generate_scene_plan(
        &self,
        options: GenerateScenePlanOptions,
    ) -> Result<MediaSaveResultData, MediaServiceError> {
        let operation = MediaOperation::GenerateScenePlan;
        let _import_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "Scene Plan 生成互斥状态不可用。",
            )
        })?;
        validate_generate_scene_plan_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "Scene Plan 请求声明的项目身份与实际项目不一致。",
            ));
        }

        let request_fingerprint = scene_plan_request_fingerprint(&options)?;
        let receipt_id =
            stable_scene_plan_receipt_id(&options.expected_project_id, &options.idempotency_key);
        let receipt_exists = self.scene_plan_receipt_exists_or_conflict(
            &options,
            &receipt_id,
            &request_fingerprint,
        )?;

        let approved = self
            .workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "scene_plan".to_owned(),
                inputs: vec![
                    approved_input(&options.research_input, "media_scene_research", "claim_set"),
                    approved_input(&options.script_input, "media_scene_script", "script"),
                    approved_input(&options.captions_input, "media_scene_captions", "captions"),
                ],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        if approved.len() != 3
            || approved[0]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.research_input.artifact_id.as_str())
            || approved[1]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.script_input.artifact_id.as_str())
            || approved[2]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.captions_input.artifact_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准输入闭包返回了不一致的 Research/Script/Captions Artifact。",
            ));
        }
        let context = self.load_scene_plan_source_context(&options)?;
        let source_artifact_ids = vec![
            options.research_input.artifact_id.clone(),
            options.script_input.artifact_id.clone(),
            options.captions_input.artifact_id.clone(),
            context.audio_input.artifact_id.clone(),
        ];
        if receipt_exists {
            return self
                .load_scene_plan_replay_or_conflict(
                    &options,
                    &receipt_id,
                    &request_fingerprint,
                    context.audio_duration_ms,
                    &source_artifact_ids,
                )?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Scene Plan receipt 存在但无法重放。",
                    )
                });
        }

        let created_at = self.clock.now().format(&Rfc3339).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "无法生成 Scene Plan 文档时间戳。",
            )
        })?;
        let config_snapshot = scene_plan_config_snapshot();
        let input_refs = vec![
            frozen_input_document(&options.expected_project_id, &options.research_input),
            frozen_input_document(&options.expected_project_id, &options.script_input),
            frozen_input_document(&options.expected_project_id, &options.captions_input),
        ];
        let document = build_scene_plan_document(BuildScenePlanOptions {
            captions_document: context.captions_document,
            audio_duration_ms: context.audio_duration_ms,
            input_refs,
            config_snapshot,
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            stable_seed: request_fingerprint.clone(),
            created_at,
        })
        .map_err(|error| map_scene_plan_error(error, operation))?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "生成的 ScenePlanDocument 未通过 media v1 契约。",
            )
        })?;
        validate_scene_plan_semantics(&document, context.audio_duration_ms)
            .map_err(|error| map_scene_plan_error(error, operation))?;
        let changed_scene_ids = scene_plan_changed_ids(&document, operation)?;

        let mut document_file = NamedTempFile::new().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::Io,
                operation,
                "无法创建 Scene Plan 文档临时文件。",
            )
        })?;
        serde_json::to_writer(&mut document_file, &document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::Io,
                operation,
                "无法序列化 Scene Plan 文档。",
            )
        })?;
        document_file.write_all(b"\n").map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法写入 Scene Plan 文档。")
        })?;
        document_file.as_file().sync_all().map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法同步 Scene Plan 文档。")
        })?;

        let provenance = exact_artifact_provenance_union(
            approved.iter().map(|input| &input.artifact),
            operation,
        )?;
        let derived_draft = artifact_draft(
            json!({
                "stageId": "scene_plan",
                "runId": options.run_id,
                "kind": "scene_plan",
                "mediaType": "application/vnd.narracut.scene-plan+json",
                "evidenceRole": "non_evidence",
                "source": {
                    "origin": "derived",
                    "sourceArtifactIds": source_artifact_ids,
                },
                "provenance": provenance,
            }),
            operation,
        )?;
        let document_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: document_file.path().to_string_lossy().into_owned(),
                artifact: derived_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
        let content_hash = artifact_string(&document_commit.artifact, "contentHash", operation)?;
        let persisted_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Scene Plan 文档不是合法 JSON。",
            )
        })?;
        validate_media_document(&persisted).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Scene Plan 文档未通过 media v1 契约。",
            )
        })?;
        validate_scene_plan_semantics(&persisted, context.audio_duration_ms)
            .map_err(|error| map_scene_plan_error(error, operation))?;
        if persisted != document {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "持久化 Scene Plan 文档与写入前文档不一致。",
            ));
        }

        let receipt = json!({
            "schemaVersion": 1,
            "documentType": "media_import_receipt",
            "receiptId": receipt_id,
            "projectId": options.expected_project_id,
            "operation": "generate_scene_plan",
            "requestFingerprint": request_fingerprint,
            "runId": options.run_id,
            "artifactId": artifact_id,
            "contentHash": content_hash,
        });
        let (_, created) = self
            .storage_service
            .commit_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                &receipt_id,
                &receipt,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        if !created {
            return self
                .load_scene_plan_replay_or_conflict(
                    &options,
                    &receipt_id,
                    &request_fingerprint,
                    context.audio_duration_ms,
                    &source_artifact_ids,
                )?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Scene Plan receipt 并发提交后无法重放。",
                    )
                });
        }

        Ok(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "generate_scene_plan".to_owned(),
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            artifact_id,
            changed_scene_ids,
            stale_because_stage_ids: vec!["scene_plan".to_owned()],
            idempotent_replay: false,
        })
    }

    fn prepare_media_save_run(
        &self,
        options: PrepareMediaSaveRunOptions<'_>,
    ) -> Result<PreparedMediaSaveRun, MediaServiceError> {
        let inputs = match options.stage_id {
            "scene_plan" => scene_plan_frozen_inputs(
                options.base_document,
                options.expected_project_id,
                options.operation,
            )?,
            "timeline" => timeline_frozen_inputs(
                options.base_document,
                options.expected_project_id,
                options.operation,
            )?,
            _ => {
                return Err(MediaServiceError::new(
                    MediaErrorCode::InvalidRequest,
                    options.operation,
                    "媒体编辑保存只支持 scene_plan 或 timeline 阶段。",
                ))
            }
        };
        let mut input_refs = Vec::with_capacity(inputs.len() + 1);
        let base_read = self
            .storage_service
            .get_artifact(options.project_path, options.base_artifact_id)
            .map_err(|error| map_storage_error(error, options.operation))?;
        let base_kind = artifact_string(&base_read.artifact, "kind", options.operation)?;
        let base_run_id = artifact_string(&base_read.artifact, "runId", options.operation)?;
        let _base_content_hash =
            artifact_string(&base_read.artifact, "contentHash", options.operation)?;
        if base_read.owner_project_id != options.expected_project_id
            || !base_read.content_available
            || base_read.artifact.get("stageId").and_then(Value::as_str) != Some(options.stage_id)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                options.operation,
                "媒体编辑基础 Artifact 无法冻结到执行快照。",
            )
            .with_safe_context(
                Some(options.expected_project_id),
                Some(options.stage_id),
                Some(&base_run_id),
                Some(options.base_artifact_id),
            ));
        }
        let base_metadata_relative =
            format!("artifacts/metadata/{}.json", options.base_artifact_id);
        let base_metadata_path = Path::new(options.project_path).join(&base_metadata_relative);
        let base_metadata_bytes = fs::read(&base_metadata_path).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                options.operation,
                "无法读取媒体编辑基础 Artifact 元数据。",
            )
            .with_safe_context(
                Some(options.expected_project_id),
                Some(options.stage_id),
                Some(&base_run_id),
                Some(options.base_artifact_id),
            )
        })?;
        if base_metadata_bytes.len() as u64 > MAX_TIMELINE_DOCUMENT_BYTES {
            return Err(MediaServiceError::new(
                MediaErrorCode::ResourceLimitExceeded,
                options.operation,
                "媒体编辑基础 Artifact 元数据超过同步读取上限。",
            ));
        }
        let base_metadata_hash = format!(
            "sha256:{}",
            lowercase_hex(&Sha256::digest(&base_metadata_bytes))
        );
        let (base_claim_ids, base_evidence_refs) =
            artifact_traceability_sets(&base_read.artifact, options.operation)?;
        input_refs.push(json!({
            "refId": format!("ref_save_{}_base", options.stage_id),
            "referenceType": "project_document",
            "kind": base_kind,
            "contentHash": base_metadata_hash,
            "uri": format!("project://{base_metadata_relative}"),
            "claimIds": base_claim_ids,
            "evidenceRefs": base_evidence_refs,
        }));
        for (index, input) in inputs.into_iter().enumerate() {
            let read = self
                .storage_service
                .get_artifact(options.project_path, &input.artifact_id)
                .map_err(|error| map_storage_error(error, options.operation))?;
            let kind = artifact_string(&read.artifact, "kind", options.operation)?;
            if read.owner_project_id != options.expected_project_id
                || !read.content_available
                || read.artifact.get("stageId").and_then(Value::as_str)
                    != Some(input.stage_id.as_str())
                || read.artifact.get("runId").and_then(Value::as_str) != Some(input.run_id.as_str())
                || read.artifact.get("contentHash").and_then(Value::as_str)
                    != Some(input.content_hash.as_str())
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    options.operation,
                    "媒体编辑保存输入无法转换为已审核 Workflow Artifact 引用。",
                )
                .with_safe_context(
                    Some(options.expected_project_id),
                    Some(options.stage_id),
                    Some(options.run_id),
                    Some(&input.artifact_id),
                ));
            }
            input_refs.push(json!({
                "refId": format!("ref_save_{}_{index}", options.stage_id),
                "referenceType": "artifact",
                "kind": kind,
                "contentHash": input.content_hash,
                "artifactId": input.artifact_id,
                "sourceRunId": input.run_id,
                "reviewRecordId": input.review_record_id,
                "claimIds": input.claim_ids,
                "evidenceRefs": input.evidence_refs,
            }));
        }
        let executor = json!({
            "providerId": "narracut_media_editor",
            "providerVersion": "1.1.0",
            "executionMode": "local",
        });
        let job_idempotency_key =
            media_save_job_idempotency_key(options.stage_id, options.idempotency_key);
        let job_request = json!({
            "schemaVersion": 1,
            "documentType": "media_save_job_request",
            "operation": match options.operation {
                MediaOperation::SaveScenePlan => "save_scene_plan",
                MediaOperation::SaveTimeline => "save_timeline",
                _ => unreachable!("media save preparation only supports edit operations"),
            },
            "requestFingerprint": options.request_fingerprint,
        });
        self.job_service
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.to_owned(),
                expected_project_id: options.expected_project_id.to_owned(),
                idempotency_key: job_idempotency_key.clone(),
                request: job_request.clone(),
            })
            .map_err(|error| map_job_error(error, options.operation))?;
        let job_snapshot = self
            .job_service
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: options.project_path.to_owned(),
                    expected_project_id: options.expected_project_id.to_owned(),
                    stage_id: options.stage_id.to_owned(),
                    run_id: options.run_id.to_owned(),
                    input_refs,
                    executor,
                    idempotency_key: job_idempotency_key,
                    retry_policy: RetryPolicyData {
                        max_attempts: 3,
                        initial_backoff_ms: 0,
                        backoff_multiplier: 2,
                        max_backoff_ms: 0,
                    },
                },
                job_request,
            )
            .map_err(|error| map_job_error(error, options.operation))?;
        let job_id = artifact_string(&job_snapshot.job, "jobId", options.operation)?;
        let execution_snapshot = self
            .workflow_service
            .get_stage_execution_snapshot(
                options.project_path,
                options.expected_project_id,
                options.stage_id,
                options.run_id,
                &job_id,
            )
            .map_err(|error| map_workflow_error(error, options.operation))?;
        let created_at = artifact_string(&execution_snapshot, "createdAt", options.operation)?;
        let lease_id = match job_snapshot.status {
            JobStatusData::Queued | JobStatusData::Retrying => self
                .job_service
                .claim_job(ClaimJobOptions {
                    project_path: options.project_path.to_owned(),
                    expected_project_id: options.expected_project_id.to_owned(),
                    job_id: job_id.clone(),
                    worker_id: "narracut_media_editor".to_owned(),
                    lease_duration_ms: 5 * 60 * 1_000,
                })
                .map_err(|error| map_job_error(error, options.operation))?
                .and_then(|snapshot| snapshot.lease.map(|lease| lease.lease_id))
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::StorageUnavailable,
                        options.operation,
                        "媒体编辑 Job 暂时无法领取，请稍后按同一幂等键重试。",
                    )
                })?
                .into(),
            JobStatusData::Running => job_snapshot
                .lease
                .filter(|lease| lease.worker_id == "narracut_media_editor")
                .map(|lease| lease.lease_id),
            JobStatusData::Succeeded => None,
            JobStatusData::Failed | JobStatusData::Canceled => {
                return Err(MediaServiceError::new(
                    MediaErrorCode::StorageUnavailable,
                    options.operation,
                    "媒体编辑 Job 已处于不可重放的终态。",
                ))
            }
        };
        if job_snapshot.status == JobStatusData::Running && lease_id.is_none() {
            return Err(MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                options.operation,
                "媒体编辑 Job 正由其他 worker 执行，请稍后按同一幂等键重试。",
            ));
        }
        Ok(PreparedMediaSaveRun {
            job_id,
            created_at,
            lease_id,
        })
    }

    fn record_media_save_run(
        &self,
        options: RecordMediaSaveRunOptions<'_>,
    ) -> Result<Vec<String>, MediaServiceError> {
        let log_summary = json!({
            "message": format!(
                "已保存 {} 可审核编辑版本：{}",
                options.stage_id, options.change_summary
            ),
            "warnings": [],
            "errors": [],
        });
        if let Some(lease_id) = options.lease_id {
            self.job_service
                .record_job_artifact(RecordJobArtifactOptions {
                    project_path: options.project_path.to_owned(),
                    expected_project_id: options.expected_project_id.to_owned(),
                    job_id: options.job_id.to_owned(),
                    lease_id: lease_id.to_owned(),
                    artifact_id: options.artifact_id.to_owned(),
                })
                .map_err(|error| map_job_error(error, options.operation))?;
            self.job_service
                .complete_job(CompleteJobOptions {
                    project_path: options.project_path.to_owned(),
                    expected_project_id: options.expected_project_id.to_owned(),
                    job_id: options.job_id.to_owned(),
                    lease_id: lease_id.to_owned(),
                    artifact_ids: vec![options.artifact_id.to_owned()],
                    log_summary: log_summary.clone(),
                })
                .map_err(|error| map_job_error(error, options.operation))?;
        } else {
            let snapshot = self
                .job_service
                .get_job(GetJobOptions {
                    project_path: options.project_path.to_owned(),
                    expected_project_id: options.expected_project_id.to_owned(),
                    job_id: options.job_id.to_owned(),
                })
                .map_err(|error| map_job_error(error, options.operation))?;
            if snapshot.status != JobStatusData::Succeeded
                || !snapshot
                    .artifact_ids
                    .iter()
                    .any(|artifact_id| artifact_id == options.artifact_id)
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ArtifactVerificationFailed,
                    options.operation,
                    "媒体编辑重放未命中已完成 Job 的不可变产物。",
                ));
            }
        }
        let committed = self
            .workflow_service
            .record_stage_run(RecordStageRunOptions {
                project_path: options.project_path.to_owned(),
                expected_project_id: options.expected_project_id.to_owned(),
                stage_id: options.stage_id.to_owned(),
                run_id: options.run_id.to_owned(),
                status: TerminalRunStatusData::Succeeded,
                job_id: options.job_id.to_owned(),
                artifact_ids: vec![options.artifact_id.to_owned()],
                log_summary,
            })
            .map_err(|error| map_workflow_error(error, options.operation))?;
        Ok(committed.stage_state.stale_because_stage_ids)
    }

    fn fail_media_save_run(
        &self,
        project_path: &str,
        expected_project_id: &str,
        prepared: &PreparedMediaSaveRun,
        error: &MediaServiceError,
    ) {
        let Some(lease_id) = prepared.lease_id.as_deref() else {
            return;
        };
        let code = format!("media_{:?}", error.code).to_ascii_lowercase();
        let _ = self.job_service.fail_job(FailJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: expected_project_id.to_owned(),
            job_id: prepared.job_id.clone(),
            lease_id: lease_id.to_owned(),
            error: JobFailureData {
                code: code.clone(),
                message: error.message.clone(),
                retryable: media_save_error_is_retryable(error.code),
                details: Map::new(),
            },
            log_summary: json!({
                "message": "媒体编辑保存未完成。",
                "warnings": [],
                "errors": [code],
            }),
        });
    }

    pub fn save_timeline(
        &self,
        options: SaveTimelineOptions,
    ) -> Result<MediaSaveResultData, MediaServiceError> {
        let operation = MediaOperation::SaveTimeline;
        let _save_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "Timeline 保存互斥状态不可用。",
            )
        })?;
        validate_save_timeline_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "Timeline 保存请求声明的项目身份与实际项目不一致。",
            ));
        }
        let context = self.load_timeline_base_context(&options)?;
        let request_fingerprint =
            timeline_save_request_fingerprint(&options, &context.content_hash)?;
        let receipt_id =
            stable_timeline_save_receipt_id(&options.expected_project_id, &options.idempotency_key);
        let receipt_exists = self.timeline_save_receipt_exists_or_conflict(
            &options,
            &receipt_id,
            &request_fingerprint,
            &context.content_hash,
        )?;
        let timeline_id = stable_media_id("timeline_edit", &request_fingerprint);
        if !receipt_exists {
            apply_timeline_edits(ApplyTimelineEditsOptions {
                base_timeline_document: context.document.clone(),
                edits: options.edits.clone(),
                change_summary: options.change_summary.clone(),
                new_run_id: options.run_id.clone(),
                new_timeline_id: timeline_id.clone(),
                created_at: "1970-01-01T00:00:00Z".to_owned(),
                base_artifact_id: options.base_artifact_id.clone(),
            })
            .map_err(|error| map_timeline_error(error, operation))?;
        }
        let prepared = self.prepare_media_save_run(PrepareMediaSaveRunOptions {
            project_path: &options.project_path,
            expected_project_id: &options.expected_project_id,
            stage_id: "timeline",
            run_id: &options.run_id,
            base_artifact_id: &options.base_artifact_id,
            base_document: &context.document,
            idempotency_key: &options.idempotency_key,
            request_fingerprint: &request_fingerprint,
            operation,
        })?;
        let result = (|| -> Result<MediaSaveResultData, MediaServiceError> {
            if receipt_exists {
                let mut replay = self
                    .load_timeline_save_replay_or_conflict(
                        &options,
                        &context,
                        &receipt_id,
                        &request_fingerprint,
                    )?
                    .ok_or_else(|| {
                        MediaServiceError::new(
                            MediaErrorCode::IdempotencyConflict,
                            operation,
                            "Timeline 保存 receipt 存在但无法重放。",
                        )
                    })?;
                replay.stale_because_stage_ids =
                    self.record_media_save_run(RecordMediaSaveRunOptions {
                        project_path: &options.project_path,
                        expected_project_id: &options.expected_project_id,
                        stage_id: "timeline",
                        run_id: &options.run_id,
                        job_id: &prepared.job_id,
                        lease_id: prepared.lease_id.as_deref(),
                        artifact_id: &replay.artifact_id,
                        change_summary: &options.change_summary,
                        operation,
                    })?;
                return Ok(replay);
            }

            let document = apply_timeline_edits(ApplyTimelineEditsOptions {
                base_timeline_document: context.document.clone(),
                edits: options.edits.clone(),
                change_summary: options.change_summary.clone(),
                new_run_id: options.run_id.clone(),
                new_timeline_id: timeline_id,
                created_at: prepared.created_at.clone(),
                base_artifact_id: options.base_artifact_id.clone(),
            })
            .map_err(|error| map_timeline_error(error, operation))?;
            validate_media_document(&document).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "编辑后的 TimelineDocument 未通过 media v1 契约。",
                )
            })?;
            validate_timeline_semantics(&document)
                .map_err(|error| map_timeline_error(error, operation))?;
            let changed_scene_ids = scene_plan_changed_ids(&document, operation)?;

            let mut document_file = NamedTempFile::new().map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法创建 Timeline 保存临时文件。",
                )
            })?;
            serde_json::to_writer(&mut document_file, &document).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法序列化编辑后的 Timeline。",
                )
            })?;
            document_file.write_all(b"\n").map_err(|_| {
                MediaServiceError::new(MediaErrorCode::Io, operation, "无法写入编辑后的 Timeline。")
            })?;
            document_file.as_file().sync_all().map_err(|_| {
                MediaServiceError::new(MediaErrorCode::Io, operation, "无法同步编辑后的 Timeline。")
            })?;
            let derived_draft = artifact_draft(
                json!({
                    "stageId": "timeline",
                    "runId": options.run_id,
                    "kind": "timeline",
                    "mediaType": "application/vnd.narracut.timeline+json",
                    "evidenceRole": "non_evidence",
                    "source": {
                        "origin": "derived",
                        "sourceArtifactIds": context.source_artifact_ids,
                    },
                    "provenance": context.provenance,
                }),
                operation,
            )?;
            let document_commit = self
                .storage_service
                .import_artifact_file_idempotent(
                    StoreArtifactFileOptions {
                        project_path: options.project_path.clone(),
                        expected_project_id: options.expected_project_id.clone(),
                        source_path: document_file.path().to_string_lossy().into_owned(),
                        artifact: derived_draft,
                    },
                    &stable_media_artifact_id(&request_fingerprint),
                    &prepared.created_at,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
            let content_hash =
                artifact_string(&document_commit.artifact, "contentHash", operation)?;
            let persisted_bytes = self
                .storage_service
                .read_artifact_content_bounded(
                    &options.project_path,
                    &options.expected_project_id,
                    &artifact_id,
                    MAX_TIMELINE_DOCUMENT_BYTES,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "持久化 Timeline 编辑结果不是合法 JSON。",
                )
            })?;
            validate_media_document(&persisted).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "持久化 Timeline 编辑结果未通过 media v1 契约。",
                )
            })?;
            validate_timeline_semantics(&persisted)
                .map_err(|error| map_timeline_error(error, operation))?;
            if persisted != document {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ArtifactVerificationFailed,
                    operation,
                    "持久化 Timeline 编辑结果与写入前文档不一致。",
                ));
            }

            let receipt = json!({
                "schemaVersion": 1,
                "documentType": "media_import_receipt",
                "receiptId": receipt_id,
                "projectId": options.expected_project_id,
                "operation": "save_timeline",
                "requestFingerprint": request_fingerprint,
                "runId": options.run_id,
                "baseArtifactId": options.base_artifact_id,
                "baseContentHash": context.content_hash,
                "artifactId": artifact_id,
                "contentHash": content_hash,
            });
            let (_, created) = self
                .storage_service
                .commit_media_receipt(
                    &options.project_path,
                    &options.expected_project_id,
                    &receipt_id,
                    &receipt,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            if !created {
                let mut replay = self
                    .load_timeline_save_replay_or_conflict(
                        &options,
                        &context,
                        &receipt_id,
                        &request_fingerprint,
                    )?
                    .ok_or_else(|| {
                        MediaServiceError::new(
                            MediaErrorCode::IdempotencyConflict,
                            operation,
                            "Timeline 保存 receipt 并发提交后无法重放。",
                        )
                    })?;
                replay.stale_because_stage_ids =
                    self.record_media_save_run(RecordMediaSaveRunOptions {
                        project_path: &options.project_path,
                        expected_project_id: &options.expected_project_id,
                        stage_id: "timeline",
                        run_id: &options.run_id,
                        job_id: &prepared.job_id,
                        lease_id: prepared.lease_id.as_deref(),
                        artifact_id: &replay.artifact_id,
                        change_summary: &options.change_summary,
                        operation,
                    })?;
                return Ok(replay);
            }

            let stale_because_stage_ids =
                self.record_media_save_run(RecordMediaSaveRunOptions {
                    project_path: &options.project_path,
                    expected_project_id: &options.expected_project_id,
                    stage_id: "timeline",
                    run_id: &options.run_id,
                    job_id: &prepared.job_id,
                    lease_id: prepared.lease_id.as_deref(),
                    artifact_id: &artifact_id,
                    change_summary: &options.change_summary,
                    operation,
                })?;

            Ok(MediaSaveResultData {
                api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
                operation: "save_timeline".to_owned(),
                owner_project_id: options.expected_project_id.clone(),
                run_id: options.run_id.clone(),
                artifact_id,
                changed_scene_ids,
                stale_because_stage_ids,
                idempotent_replay: false,
            })
        })();
        if let Err(error) = &result {
            self.fail_media_save_run(
                &options.project_path,
                &options.expected_project_id,
                &prepared,
                error,
            );
        }
        result
    }

    pub fn save_scene_plan(
        &self,
        options: SaveScenePlanOptions,
    ) -> Result<MediaSaveResultData, MediaServiceError> {
        let operation = MediaOperation::SaveScenePlan;
        let _save_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "Scene Plan 保存互斥状态不可用。",
            )
        })?;
        validate_save_scene_plan_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "Scene Plan 保存请求声明的项目身份与实际项目不一致。",
            ));
        }
        let context = self.load_scene_plan_base_context(&options)?;
        let request_fingerprint =
            scene_plan_save_request_fingerprint(&options, &context.content_hash)?;
        let receipt_id = stable_scene_plan_save_receipt_id(
            &options.expected_project_id,
            &options.idempotency_key,
        );
        let receipt_exists = self.scene_plan_save_receipt_exists_or_conflict(
            &options,
            &receipt_id,
            &request_fingerprint,
            &context.content_hash,
        )?;
        let scene_plan_id = stable_media_id("sceneplan_edit", &request_fingerprint);
        if !receipt_exists {
            apply_scene_plan_edits(
                &context.document,
                &options.edits,
                &options.change_summary,
                &options.run_id,
                &scene_plan_id,
                "1970-01-01T00:00:00Z",
                &options.base_artifact_id,
            )
            .map_err(|error| map_scene_plan_error(error, operation))?;
        }
        let prepared = self.prepare_media_save_run(PrepareMediaSaveRunOptions {
            project_path: &options.project_path,
            expected_project_id: &options.expected_project_id,
            stage_id: "scene_plan",
            run_id: &options.run_id,
            base_artifact_id: &options.base_artifact_id,
            base_document: &context.document,
            idempotency_key: &options.idempotency_key,
            request_fingerprint: &request_fingerprint,
            operation,
        })?;
        let result = (|| -> Result<MediaSaveResultData, MediaServiceError> {
            if receipt_exists {
                let mut replay = self
                    .load_scene_plan_save_replay_or_conflict(
                        &options,
                        &context,
                        &receipt_id,
                        &request_fingerprint,
                    )?
                    .ok_or_else(|| {
                        MediaServiceError::new(
                            MediaErrorCode::IdempotencyConflict,
                            operation,
                            "Scene Plan 保存 receipt 存在但无法重放。",
                        )
                    })?;
                replay.stale_because_stage_ids =
                    self.record_media_save_run(RecordMediaSaveRunOptions {
                        project_path: &options.project_path,
                        expected_project_id: &options.expected_project_id,
                        stage_id: "scene_plan",
                        run_id: &options.run_id,
                        job_id: &prepared.job_id,
                        lease_id: prepared.lease_id.as_deref(),
                        artifact_id: &replay.artifact_id,
                        change_summary: &options.change_summary,
                        operation,
                    })?;
                return Ok(replay);
            }

            let document = apply_scene_plan_edits(
                &context.document,
                &options.edits,
                &options.change_summary,
                &options.run_id,
                &scene_plan_id,
                &prepared.created_at,
                &options.base_artifact_id,
            )
            .map_err(|error| map_scene_plan_error(error, operation))?;
            validate_media_document(&document).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "编辑后的 ScenePlanDocument 未通过 media v1 契约。",
                )
            })?;
            validate_scene_plan_semantics(&document, context.audio_duration_ms)
                .map_err(|error| map_scene_plan_error(error, operation))?;
            let changed_scene_ids = scene_plan_changed_ids(&document, operation)?;

            let mut document_file = NamedTempFile::new().map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法创建 Scene Plan 保存临时文件。",
                )
            })?;
            serde_json::to_writer(&mut document_file, &document).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法序列化编辑后的 Scene Plan。",
                )
            })?;
            document_file.write_all(b"\n").map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法写入编辑后的 Scene Plan。",
                )
            })?;
            document_file.as_file().sync_all().map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::Io,
                    operation,
                    "无法同步编辑后的 Scene Plan。",
                )
            })?;
            let derived_draft = artifact_draft(
                json!({
                    "stageId": "scene_plan",
                    "runId": options.run_id,
                    "kind": "scene_plan",
                    "mediaType": "application/vnd.narracut.scene-plan+json",
                    "evidenceRole": "non_evidence",
                    "source": {
                        "origin": "derived",
                        "sourceArtifactIds": context.source_artifact_ids.clone(),
                    },
                    "provenance": context.provenance,
                }),
                operation,
            )?;
            let document_commit = self
                .storage_service
                .import_artifact_file_idempotent(
                    StoreArtifactFileOptions {
                        project_path: options.project_path.clone(),
                        expected_project_id: options.expected_project_id.clone(),
                        source_path: document_file.path().to_string_lossy().into_owned(),
                        artifact: derived_draft,
                    },
                    &stable_media_artifact_id(&request_fingerprint),
                    &prepared.created_at,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
            let content_hash =
                artifact_string(&document_commit.artifact, "contentHash", operation)?;
            let persisted_bytes = self
                .storage_service
                .read_artifact_content_bounded(
                    &options.project_path,
                    &options.expected_project_id,
                    &artifact_id,
                    MAX_SCENE_PLAN_DOCUMENT_BYTES,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "持久化 Scene Plan 编辑结果不是合法 JSON。",
                )
            })?;
            validate_media_document(&persisted).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "持久化 Scene Plan 编辑结果未通过 media v1 契约。",
                )
            })?;
            validate_scene_plan_semantics(&persisted, context.audio_duration_ms)
                .map_err(|error| map_scene_plan_error(error, operation))?;
            if persisted != document {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ArtifactVerificationFailed,
                    operation,
                    "持久化 Scene Plan 编辑结果与写入前文档不一致。",
                ));
            }

            let receipt = json!({
                "schemaVersion": 1,
                "documentType": "media_import_receipt",
                "receiptId": receipt_id,
                "projectId": options.expected_project_id,
                "operation": "save_scene_plan",
                "requestFingerprint": request_fingerprint,
                "runId": options.run_id,
                "baseArtifactId": options.base_artifact_id,
                "baseContentHash": context.content_hash,
                "artifactId": artifact_id,
                "contentHash": content_hash,
            });
            let (_, created) = self
                .storage_service
                .commit_media_receipt(
                    &options.project_path,
                    &options.expected_project_id,
                    &receipt_id,
                    &receipt,
                )
                .map_err(|error| map_storage_error(error, operation))?;
            if !created {
                let mut replay = self
                    .load_scene_plan_save_replay_or_conflict(
                        &options,
                        &context,
                        &receipt_id,
                        &request_fingerprint,
                    )?
                    .ok_or_else(|| {
                        MediaServiceError::new(
                            MediaErrorCode::IdempotencyConflict,
                            operation,
                            "Scene Plan 保存 receipt 并发提交后无法重放。",
                        )
                    })?;
                replay.stale_because_stage_ids =
                    self.record_media_save_run(RecordMediaSaveRunOptions {
                        project_path: &options.project_path,
                        expected_project_id: &options.expected_project_id,
                        stage_id: "scene_plan",
                        run_id: &options.run_id,
                        job_id: &prepared.job_id,
                        lease_id: prepared.lease_id.as_deref(),
                        artifact_id: &replay.artifact_id,
                        change_summary: &options.change_summary,
                        operation,
                    })?;
                return Ok(replay);
            }

            let stale_because_stage_ids =
                self.record_media_save_run(RecordMediaSaveRunOptions {
                    project_path: &options.project_path,
                    expected_project_id: &options.expected_project_id,
                    stage_id: "scene_plan",
                    run_id: &options.run_id,
                    job_id: &prepared.job_id,
                    lease_id: prepared.lease_id.as_deref(),
                    artifact_id: &artifact_id,
                    change_summary: &options.change_summary,
                    operation,
                })?;

            Ok(MediaSaveResultData {
                api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
                operation: "save_scene_plan".to_owned(),
                owner_project_id: options.expected_project_id.clone(),
                run_id: options.run_id.clone(),
                artifact_id,
                changed_scene_ids,
                stale_because_stage_ids,
                idempotent_replay: false,
            })
        })();
        if let Err(error) = &result {
            self.fail_media_save_run(
                &options.project_path,
                &options.expected_project_id,
                &prepared,
                error,
            );
        }
        result
    }

    pub fn import_captions(
        &self,
        options: ImportCaptionsOptions,
    ) -> Result<MediaImportResultData, MediaServiceError> {
        let operation = MediaOperation::ImportCaptions;
        let _import_guard = self.import_lock.lock().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::StorageUnavailable,
                operation,
                "媒体导入互斥状态不可用。",
            )
        })?;
        let source_file_name = validate_captions_options(&options)?;
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(|error| map_project_error(error, operation))?;
        if descriptor.project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体请求声明的项目身份与实际项目不一致。",
            ));
        }

        let parsed = parse_srt_file(
            Path::new(&options.source_path),
            options.audio_duration_ms,
            options.limits,
        )
        .map_err(|error| map_media_parse_error(error, operation))?;
        validate_expected_source_hash(
            options.expected_source_content_hash.as_deref(),
            &parsed.content_hash,
            operation,
        )?;

        let request_fingerprint =
            captions_request_fingerprint(&options, &source_file_name, &parsed.content_hash)?;
        let receipt_id =
            stable_captions_receipt_id(&options.expected_project_id, &options.idempotency_key);
        if let Some(replay) =
            self.load_captions_replay_or_conflict(&options, &receipt_id, &request_fingerprint)?
        {
            return Ok(replay);
        }

        let approved = self
            .workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "captions".to_owned(),
                inputs: vec![
                    approved_input(&options.script_input, "media_captions_script", "script"),
                    approved_input(&options.audio_input, "media_captions_audio", "voice_audio"),
                ],
            })
            .map_err(|error| map_workflow_error(error, operation))?;
        if approved.len() != 2
            || approved[0]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.script_input.artifact_id.as_str())
            || approved[1]
                .artifact
                .get("artifactId")
                .and_then(Value::as_str)
                != Some(options.audio_input.artifact_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准输入闭包返回了不一致的 Script/Audio Artifact。",
            ));
        }
        let script_document_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.script_input.artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let script_traceability =
            parse_caption_script_traceability(&script_document_bytes, &approved[0].artifact)?;
        let audio_document_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.audio_input.artifact_id,
                MAX_AUDIO_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let audio_document: Value =
            serde_json::from_slice(&audio_document_bytes).map_err(|_| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "批准 Audio Artifact 不是 AudioMediaDocument JSON。",
                )
            })?;
        validate_media_document(&audio_document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准 Audio Artifact 未通过 media v1 契约。",
            )
        })?;
        if audio_document.get("documentType").and_then(Value::as_str) != Some("audio_media")
            || audio_document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || audio_document.get("runId").and_then(Value::as_str)
                != Some(options.audio_input.run_id.as_str())
            || audio_document.get("durationMs").and_then(Value::as_u64)
                != Some(options.audio_duration_ms)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准 AudioMediaDocument 的项目、运行或时长与冻结引用不一致。",
            ));
        }

        let (cues, mappings, diagnostics, caption_provenance) =
            build_caption_cues_and_mappings(&parsed, &script_traceability)?;
        let raw_draft = artifact_draft(
            json!({
                "stageId": "captions",
                "runId": options.run_id,
                "kind": "captions_source",
                "mediaType": "application/x-subrip",
                "evidenceRole": "expressive_material",
                "source": {
                    "origin": "imported",
                    "sourceUri": internal_source_uri(&parsed.content_hash, &source_file_name),
                    "author": options.rights.author,
                    "license": options.rights.license_id,
                    "attributionText": options.rights.attribution_text,
                    "authorizationRecordIds": [options.rights.license_id],
                },
                "provenance": caption_provenance_values(&caption_provenance),
            }),
            operation,
        )?;
        let raw_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: options.source_path.clone(),
                artifact: raw_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let raw_artifact_id = artifact_string(&raw_commit.artifact, "artifactId", operation)?;
        if raw_commit
            .artifact
            .get("contentHash")
            .and_then(Value::as_str)
            != Some(parsed.content_hash.as_str())
            || raw_commit
                .artifact
                .get("byteLength")
                .and_then(Value::as_u64)
                != Some(parsed.byte_length)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::SourceChanged,
                operation,
                "字幕源在解析与不可变导入之间发生变化。",
            ));
        }

        let created_at = self.clock.now().format(&Rfc3339).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "无法生成 Captions 文档时间戳。",
            )
        })?;
        let document = json!({
            "schemaVersion": NARRACUT_MEDIA_SCHEMA_VERSION,
            "documentType": "captions_media",
            "captionsId": stable_media_id("captions", &request_fingerprint),
            "projectId": options.expected_project_id,
            "runId": options.run_id,
            "rawArtifactId": raw_artifact_id,
            "rawContentHash": parsed.content_hash,
            "source": {
                "sourceFileName": source_file_name,
                "sourceContentHash": parsed.content_hash,
                "byteLength": parsed.byte_length,
            },
            "audioInput": frozen_input_document(&options.expected_project_id, &options.audio_input),
            "cues": cues,
            "mappings": mappings,
            "diagnostics": diagnostics,
            "inputRefs": [
                frozen_input_document(&options.expected_project_id, &options.script_input),
                frozen_input_document(&options.expected_project_id, &options.audio_input),
            ],
            "configSnapshot": options.config_snapshot,
            "createdAt": created_at,
        });
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "Captions 文档未通过 media v1 契约。",
            )
        })?;

        let mut document_file = NamedTempFile::new().map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::Io,
                operation,
                "无法创建 Captions 文档临时文件。",
            )
        })?;
        serde_json::to_writer(&mut document_file, &document).map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法序列化 Captions 文档。")
        })?;
        document_file.write_all(b"\n").map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法写入 Captions 文档。")
        })?;
        document_file.as_file().sync_all().map_err(|_| {
            MediaServiceError::new(MediaErrorCode::Io, operation, "无法同步 Captions 文档。")
        })?;
        let derived_draft = artifact_draft(
            json!({
                "stageId": "captions",
                "runId": options.run_id,
                "kind": "captions",
                "mediaType": "application/vnd.narracut.captions+json",
                "evidenceRole": "non_evidence",
                "source": {
                    "origin": "derived",
                    "sourceArtifactIds": [
                        raw_artifact_id,
                        options.script_input.artifact_id,
                        options.audio_input.artifact_id,
                    ],
                },
                "provenance": caption_provenance_values(&caption_provenance),
            }),
            operation,
        )?;
        let document_commit = self
            .storage_service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: document_file.path().to_string_lossy().into_owned(),
                artifact: derived_draft,
            })
            .map_err(|error| map_storage_error(error, operation))?;
        let artifact_id = artifact_string(&document_commit.artifact, "artifactId", operation)?;
        let content_hash = artifact_string(&document_commit.artifact, "contentHash", operation)?;
        let persisted_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let persisted: Value = serde_json::from_slice(&persisted_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Captions 文档不是合法 JSON。",
            )
        })?;
        validate_media_document(&persisted).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "持久化 Captions 文档未通过 media v1 契约。",
            )
        })?;
        if persisted != document {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "持久化 Captions 文档与写入前文档不一致。",
            ));
        }

        let receipt = json!({
            "schemaVersion": 1,
            "documentType": "media_import_receipt",
            "receiptId": receipt_id,
            "projectId": options.expected_project_id,
            "operation": "import_captions",
            "requestFingerprint": request_fingerprint,
            "runId": options.run_id,
            "rawArtifactId": raw_artifact_id,
            "artifactId": artifact_id,
            "contentHash": content_hash,
        });
        let (_, created) = self
            .storage_service
            .commit_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                &receipt_id,
                &receipt,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        if !created {
            return self
                .load_captions_replay_or_conflict(&options, &receipt_id, &request_fingerprint)?
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::IdempotencyConflict,
                        operation,
                        "Captions receipt 并发提交后无法重放。",
                    )
                });
        }

        Ok(MediaImportResultData {
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            raw_artifact_id,
            artifact_id,
            content_hash,
            document,
            idempotent_replay: false,
        })
    }

    fn timeline_receipt_exists_or_conflict(
        &self,
        options: &GenerateTimelineOptions,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<bool, MediaServiceError> {
        let operation = MediaOperation::GenerateTimeline;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(false);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("generate_timeline")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Timeline 生成语义。",
            ));
        }
        Ok(true)
    }

    fn load_timeline_source_context(
        &self,
        options: &GenerateTimelineOptions,
    ) -> Result<TimelineSourceContext, MediaServiceError> {
        let operation = MediaOperation::GenerateTimeline;
        let inputs = [
            (&options.audio_input, "audio", "voice_audio"),
            (&options.captions_input, "captions", "captions"),
            (&options.scene_plan_input, "scene_plan", "scene_plan"),
        ];
        let mut artifact_metadata = Vec::with_capacity(inputs.len());
        for (input, stage_id, kind) in inputs {
            let read = self
                .storage_service
                .get_artifact(&options.project_path, &input.artifact_id)
                .map_err(|error| map_storage_error(error, operation))?;
            if read.owner_project_id != options.expected_project_id
                || read.artifact.get("projectId").and_then(Value::as_str)
                    != Some(options.expected_project_id.as_str())
                || read.artifact.get("artifactId").and_then(Value::as_str)
                    != Some(input.artifact_id.as_str())
                || read.artifact.get("stageId").and_then(Value::as_str) != Some(stage_id)
                || read.artifact.get("runId").and_then(Value::as_str) != Some(input.run_id.as_str())
                || read.artifact.get("kind").and_then(Value::as_str) != Some(kind)
                || read.artifact.get("contentHash").and_then(Value::as_str)
                    != Some(input.content_hash.as_str())
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "Timeline 冻结输入的 Artifact 元数据闭包不一致。",
                )
                .with_safe_context(
                    Some(&options.expected_project_id),
                    Some(stage_id),
                    Some(&input.run_id),
                    Some(&input.artifact_id),
                ));
            }
            let verification = self
                .storage_service
                .verify_artifact(&options.project_path, &input.artifact_id)
                .map_err(|error| map_storage_error(error, operation))?;
            if verification.owner_project_id != options.expected_project_id
                || verification.status != ArtifactVerificationStatusData::Verified
                || verification.expected_content_hash != input.content_hash
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ArtifactVerificationFailed,
                    operation,
                    "Timeline 冻结输入实体未通过内容校验。",
                )
                .with_safe_context(
                    Some(&options.expected_project_id),
                    Some(stage_id),
                    Some(&input.run_id),
                    Some(&input.artifact_id),
                ));
            }
            artifact_metadata.push(read.artifact);
        }

        let audio_document = self.read_timeline_input_document(
            options,
            &options.audio_input,
            MAX_AUDIO_DOCUMENT_BYTES,
            "AudioMediaDocument",
        )?;
        let captions_document = self.read_timeline_input_document(
            options,
            &options.captions_input,
            MAX_SCENE_PLAN_DOCUMENT_BYTES,
            "CaptionsMediaDocument",
        )?;
        let scene_plan_document = self.read_timeline_input_document(
            options,
            &options.scene_plan_input,
            MAX_SCENE_PLAN_DOCUMENT_BYTES,
            "ScenePlanDocument",
        )?;
        if audio_document.get("documentType").and_then(Value::as_str) != Some("audio_media")
            || captions_document
                .get("documentType")
                .and_then(Value::as_str)
                != Some("captions_media")
            || scene_plan_document
                .get("documentType")
                .and_then(Value::as_str)
                != Some("scene_plan")
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Timeline 输入文档类型与批准阶段不一致。",
            ));
        }
        for (document, input) in [
            (&audio_document, &options.audio_input),
            (&captions_document, &options.captions_input),
            (&scene_plan_document, &options.scene_plan_input),
        ] {
            if document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
                || document.get("runId").and_then(Value::as_str) != Some(input.run_id.as_str())
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "Timeline 输入文档的项目或运行身份与冻结引用不一致。",
                ));
            }
        }
        for (document, metadata) in [
            (&audio_document, &artifact_metadata[0]),
            (&captions_document, &artifact_metadata[1]),
            (&scene_plan_document, &artifact_metadata[2]),
        ] {
            let lineage_ids =
                document_input_artifact_ids(document, &options.expected_project_id, operation)?;
            if lineage_ids.is_empty() || !artifact_source_ids_contain(metadata, &lineage_ids) {
                return Err(MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "Timeline 输入 Artifact 来源与文档 inputRefs 不闭包。",
                ));
            }
        }

        let expected_audio =
            frozen_input_document(&options.expected_project_id, &options.audio_input);
        if captions_document.get("audioInput") != Some(&expected_audio)
            || !document_has_exact_frozen_input(&captions_document, &expected_audio)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "CaptionsMediaDocument 未精确闭包 Timeline 批准 Audio 输入。",
            ));
        }
        let expected_captions =
            frozen_input_document(&options.expected_project_id, &options.captions_input);
        if !document_has_exact_frozen_input(&scene_plan_document, &expected_captions) {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "ScenePlanDocument 未精确闭包 Timeline 批准 Captions 输入。",
            ));
        }
        let audio_duration_ms = audio_document
            .get("durationMs")
            .and_then(Value::as_u64)
            .filter(|duration| *duration > 0 && *duration <= 86_400_000)
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "Timeline AudioMediaDocument 时长无效。",
                )
            })?;
        validate_scene_plan_semantics(&scene_plan_document, audio_duration_ms)
            .map_err(|error| map_scene_plan_error(error, operation))?;

        Ok(TimelineSourceContext {
            audio_document,
            captions_document,
            scene_plan_document,
            audio_duration_ms,
        })
    }

    fn read_timeline_input_document(
        &self,
        options: &GenerateTimelineOptions,
        input: &crate::FrozenArtifactInputData,
        max_bytes: u64,
        label: &str,
    ) -> Result<Value, MediaServiceError> {
        let operation = MediaOperation::GenerateTimeline;
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &input.artifact_id,
                max_bytes,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                format!("批准 {label} Artifact 不是 JSON 文档。"),
            )
        })?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                format!("批准 {label} Artifact 未通过 media v1 契约。"),
            )
        })?;
        Ok(document)
    }

    fn load_timeline_replay_or_conflict(
        &self,
        options: &GenerateTimelineOptions,
        context: &TimelineSourceContext,
        receipt_id: &str,
        request_fingerprint: &str,
        source_artifact_ids: &[String],
    ) -> Result<Option<MediaSaveResultData>, MediaServiceError> {
        let operation = MediaOperation::GenerateTimeline;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("generate_timeline")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Timeline 生成语义。",
            ));
        }
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id
            || read.artifact.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("timeline")
            || read.artifact.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || read.artifact.get("kind").and_then(Value::as_str) != Some("timeline")
            || read.artifact.get("contentHash").and_then(Value::as_str)
                != Some(content_hash.as_str())
            || !artifact_source_ids_match(&read.artifact, source_artifact_ids)
        {
            return Err(invalid_replay_artifact(operation));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.owner_project_id != options.expected_project_id
            || verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(invalid_replay_artifact(operation));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_TIMELINE_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        validate_timeline_semantics(&document).map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("timeline")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("configSnapshot") != Some(&timeline_config_snapshot())
        {
            return Err(invalid_replay_artifact(operation));
        }
        let created_at = document
            .get("createdAt")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        let expected = build_timeline_document(BuildTimelineOptions {
            audio_document: context.audio_document.clone(),
            captions_document: context.captions_document.clone(),
            scene_plan_document: context.scene_plan_document.clone(),
            audio_input: options.audio_input.clone(),
            captions_input: options.captions_input.clone(),
            scene_plan_input: options.scene_plan_input.clone(),
            canvas: options.canvas,
            safe_area: options.safe_area,
            config_snapshot: timeline_config_snapshot(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            stable_seed: request_fingerprint.to_owned(),
            created_at: created_at.to_owned(),
        })
        .map_err(|_| invalid_replay_artifact(operation))?;
        if expected != document
            || document.get("durationMs").and_then(Value::as_u64) != Some(context.audio_duration_ms)
        {
            return Err(invalid_replay_artifact(operation));
        }
        Ok(Some(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "generate_timeline".to_owned(),
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            artifact_id,
            changed_scene_ids: timeline_changed_ids(&document, operation)?,
            stale_because_stage_ids: vec!["timeline".to_owned()],
            idempotent_replay: true,
        }))
    }

    fn timeline_save_receipt_exists_or_conflict(
        &self,
        options: &SaveTimelineOptions,
        receipt_id: &str,
        request_fingerprint: &str,
        base_content_hash: &str,
    ) -> Result<bool, MediaServiceError> {
        let operation = MediaOperation::SaveTimeline;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(false);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("save_timeline")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("baseArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || receipt.get("baseContentHash").and_then(Value::as_str) != Some(base_content_hash)
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Timeline 保存语义。",
            ));
        }
        Ok(true)
    }

    fn load_timeline_base_context(
        &self,
        options: &SaveTimelineOptions,
    ) -> Result<TimelineBaseContext, MediaServiceError> {
        let operation = MediaOperation::SaveTimeline;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &options.base_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 Timeline Artifact 属于其他项目。",
            ));
        }
        let base_run_id = read
            .artifact
            .get("runId")
            .and_then(Value::as_str)
            .filter(|value| valid_prefixed_id(value, "run_", 160))
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "基础 Timeline Artifact 缺少合法 runId。",
                )
            })?;
        let content_hash = read
            .artifact
            .get("contentHash")
            .and_then(Value::as_str)
            .filter(|value| is_sha256(value))
            .map(str::to_owned)
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "基础 Timeline Artifact 缺少合法 contentHash。",
                )
            })?;
        if read.artifact.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
            || read.artifact.get("artifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("timeline")
            || read.artifact.get("kind").and_then(Value::as_str) != Some("timeline")
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Artifact 不是当前项目中的 Timeline 产物。",
            ));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &options.base_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.owner_project_id != options.expected_project_id
            || verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "基础 Timeline Artifact 实体未通过内容校验。",
            )
            .with_safe_context(
                Some(&options.expected_project_id),
                Some("timeline"),
                Some(base_run_id),
                Some(&options.base_artifact_id),
            ));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.base_artifact_id,
                MAX_TIMELINE_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Timeline Artifact 不是 JSON 文档。",
            )
        })?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Timeline Artifact 未通过 media v1 契约。",
            )
        })?;
        if document.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 TimelineDocument 属于其他项目。",
            ));
        }
        if document.get("documentType").and_then(Value::as_str) != Some("timeline")
            || document.get("runId").and_then(Value::as_str) != Some(base_run_id)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 TimelineDocument 与 Artifact 运行身份不闭包。",
            ));
        }
        validate_timeline_semantics(&document)
            .map_err(|error| map_timeline_error(error, operation))?;
        let inputs = timeline_frozen_inputs(&document, &options.expected_project_id, operation)?;
        let input_artifact_ids = inputs
            .iter()
            .map(|input| input.artifact_id.clone())
            .collect::<Vec<_>>();
        if !artifact_source_ids_contain(&read.artifact, &input_artifact_ids) {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Timeline Artifact 来源与文档 inputRefs 不闭包。",
            ));
        }
        let provenance = exact_artifact_provenance_union([&read.artifact], operation)?;
        let mut source_artifact_ids = vec![options.base_artifact_id.clone()];
        source_artifact_ids.extend(input_artifact_ids);
        Ok(TimelineBaseContext {
            document,
            content_hash,
            source_artifact_ids,
            provenance,
        })
    }

    fn load_timeline_save_replay_or_conflict(
        &self,
        options: &SaveTimelineOptions,
        context: &TimelineBaseContext,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<MediaSaveResultData>, MediaServiceError> {
        let operation = MediaOperation::SaveTimeline;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("save_timeline")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("baseArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || receipt.get("baseContentHash").and_then(Value::as_str)
                != Some(context.content_hash.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Timeline 保存语义。",
            ));
        }
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id
            || read.artifact.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("timeline")
            || read.artifact.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || read.artifact.get("kind").and_then(Value::as_str) != Some("timeline")
            || read.artifact.get("contentHash").and_then(Value::as_str)
                != Some(content_hash.as_str())
            || !artifact_source_ids_match(&read.artifact, &context.source_artifact_ids)
        {
            return Err(invalid_replay_artifact(operation));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.owner_project_id != options.expected_project_id
            || verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(invalid_replay_artifact(operation));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_TIMELINE_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        validate_timeline_semantics(&document).map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("timeline")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("supersedesArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || document.get("inputRefs") != context.document.get("inputRefs")
            || document.get("configSnapshot") != context.document.get("configSnapshot")
        {
            return Err(invalid_replay_artifact(operation));
        }
        let timeline_id = document
            .get("timelineId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        if timeline_id != stable_media_id("timeline_edit", request_fingerprint) {
            return Err(invalid_replay_artifact(operation));
        }
        let created_at = document
            .get("createdAt")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        let expected = apply_timeline_edits(ApplyTimelineEditsOptions {
            base_timeline_document: context.document.clone(),
            edits: options.edits.clone(),
            change_summary: options.change_summary.clone(),
            new_run_id: options.run_id.clone(),
            new_timeline_id: timeline_id.to_owned(),
            created_at: created_at.to_owned(),
            base_artifact_id: options.base_artifact_id.clone(),
        })
        .map_err(|_| invalid_replay_artifact(operation))?;
        if expected != document {
            return Err(invalid_replay_artifact(operation));
        }
        Ok(Some(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "save_timeline".to_owned(),
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            artifact_id,
            changed_scene_ids: scene_plan_changed_ids(&document, operation)?,
            stale_because_stage_ids: vec!["timeline".to_owned()],
            idempotent_replay: true,
        }))
    }

    fn scene_plan_receipt_exists_or_conflict(
        &self,
        options: &GenerateScenePlanOptions,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<bool, MediaServiceError> {
        let operation = MediaOperation::GenerateScenePlan;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(false);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("generate_scene_plan")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Scene Plan 生成语义。",
            ));
        }
        Ok(true)
    }

    fn load_scene_plan_source_context(
        &self,
        options: &GenerateScenePlanOptions,
    ) -> Result<ScenePlanSourceContext, MediaServiceError> {
        let operation = MediaOperation::GenerateScenePlan;
        for input in [
            &options.research_input,
            &options.script_input,
            &options.captions_input,
        ] {
            let verification = self
                .storage_service
                .verify_artifact(&options.project_path, &input.artifact_id)
                .map_err(|error| map_storage_error(error, operation))?;
            if verification.owner_project_id != options.expected_project_id
                || verification.status != ArtifactVerificationStatusData::Verified
                || verification.expected_content_hash != input.content_hash
            {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ArtifactVerificationFailed,
                    operation,
                    "Scene Plan 冻结输入实体未通过内容校验。",
                )
                .with_safe_context(
                    Some(&options.expected_project_id),
                    Some(&input.stage_id),
                    Some(&input.run_id),
                    Some(&input.artifact_id),
                ));
            }
        }

        let captions_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.captions_input.artifact_id,
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let captions_document: Value = serde_json::from_slice(&captions_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准 Captions Artifact 不是 CaptionsMediaDocument JSON。",
            )
        })?;
        validate_media_document(&captions_document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "批准 Captions Artifact 未通过 media v1 契约。",
            )
        })?;
        if captions_document
            .get("documentType")
            .and_then(Value::as_str)
            != Some("captions_media")
            || captions_document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || captions_document.get("runId").and_then(Value::as_str)
                != Some(options.captions_input.run_id.as_str())
            || !document_has_frozen_input(&captions_document, &options.script_input)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "CaptionsMediaDocument 的项目、运行或 Script 闭包与冻结输入不一致。",
            ));
        }

        let audio_value = captions_document.get("audioInput").ok_or_else(|| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "CaptionsMediaDocument 缺少 Audio 冻结输入。",
            )
        })?;
        if audio_value.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "Captions Audio 输入属于其他项目。",
            ));
        }
        let audio_input = frozen_input_from_document(audio_value, operation)?;
        validate_frozen_audio_input(&audio_input, operation)?;
        if !document_has_frozen_input(&captions_document, &audio_input) {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Captions audioInput 与 inputRefs 不一致。",
            ));
        }

        let audio_read = self
            .storage_service
            .get_artifact(&options.project_path, &audio_input.artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if audio_read.owner_project_id != options.expected_project_id
            || audio_read.artifact.get("stageId").and_then(Value::as_str) != Some("audio")
            || audio_read.artifact.get("runId").and_then(Value::as_str)
                != Some(audio_input.run_id.as_str())
            || audio_read.artifact.get("kind").and_then(Value::as_str) != Some("voice_audio")
            || audio_read
                .artifact
                .get("contentHash")
                .and_then(Value::as_str)
                != Some(audio_input.content_hash.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Captions Audio Artifact 元数据闭包不一致。",
            ));
        }
        let audio_verification = self
            .storage_service
            .verify_artifact(&options.project_path, &audio_input.artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if audio_verification.status != ArtifactVerificationStatusData::Verified
            || audio_verification.expected_content_hash != audio_input.content_hash
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "Captions Audio Artifact 实体未通过内容校验。",
            )
            .with_safe_context(
                Some(&options.expected_project_id),
                Some("audio"),
                Some(&audio_input.run_id),
                Some(&audio_input.artifact_id),
            ));
        }
        let audio_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &audio_input.artifact_id,
                MAX_AUDIO_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let audio_document: Value = serde_json::from_slice(&audio_bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Captions Audio Artifact 不是 AudioMediaDocument JSON。",
            )
        })?;
        validate_media_document(&audio_document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Captions Audio Artifact 未通过 media v1 契约。",
            )
        })?;
        let audio_duration_ms = audio_document
            .get("durationMs")
            .and_then(Value::as_u64)
            .filter(|duration| *duration > 0 && *duration <= 86_400_000)
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "AudioMediaDocument 时长无效。",
                )
            })?;
        if audio_document.get("documentType").and_then(Value::as_str) != Some("audio_media")
            || audio_document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || audio_document.get("runId").and_then(Value::as_str)
                != Some(audio_input.run_id.as_str())
            || !document_has_frozen_input(&audio_document, &options.script_input)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "AudioMediaDocument 的项目、运行或 Script 闭包不一致。",
            ));
        }

        Ok(ScenePlanSourceContext {
            captions_document,
            audio_input,
            audio_duration_ms,
        })
    }

    fn load_scene_plan_replay_or_conflict(
        &self,
        options: &GenerateScenePlanOptions,
        receipt_id: &str,
        request_fingerprint: &str,
        audio_duration_ms: u64,
        source_artifact_ids: &[String],
    ) -> Result<Option<MediaSaveResultData>, MediaServiceError> {
        let operation = MediaOperation::GenerateScenePlan;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("generate_scene_plan")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Scene Plan 生成语义。",
            ));
        }
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("scene_plan")
            || read.artifact.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || read.artifact.get("kind").and_then(Value::as_str) != Some("scene_plan")
            || read.artifact.get("contentHash").and_then(Value::as_str)
                != Some(content_hash.as_str())
            || !artifact_source_ids_match(&read.artifact, source_artifact_ids)
        {
            return Err(invalid_replay_artifact(operation));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(invalid_replay_artifact(operation));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        validate_scene_plan_semantics(&document, audio_duration_ms)
            .map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("scene_plan")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("configSnapshot") != Some(&scene_plan_config_snapshot())
            || !document_has_frozen_input(&document, &options.research_input)
            || !document_has_frozen_input(&document, &options.script_input)
            || !document_has_frozen_input(&document, &options.captions_input)
        {
            return Err(invalid_replay_artifact(operation));
        }
        Ok(Some(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "generate_scene_plan".to_owned(),
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            artifact_id,
            changed_scene_ids: scene_plan_changed_ids(&document, operation)?,
            stale_because_stage_ids: vec!["scene_plan".to_owned()],
            idempotent_replay: true,
        }))
    }

    fn scene_plan_save_receipt_exists_or_conflict(
        &self,
        options: &SaveScenePlanOptions,
        receipt_id: &str,
        request_fingerprint: &str,
        base_content_hash: &str,
    ) -> Result<bool, MediaServiceError> {
        let operation = MediaOperation::SaveScenePlan;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(false);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("save_scene_plan")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("baseArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || receipt.get("baseContentHash").and_then(Value::as_str) != Some(base_content_hash)
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Scene Plan 保存语义。",
            ));
        }
        Ok(true)
    }

    fn load_scene_plan_base_context(
        &self,
        options: &SaveScenePlanOptions,
    ) -> Result<ScenePlanBaseContext, MediaServiceError> {
        let operation = MediaOperation::SaveScenePlan;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &options.base_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 Scene Plan Artifact 属于其他项目。",
            ));
        }
        let base_run_id = read
            .artifact
            .get("runId")
            .and_then(Value::as_str)
            .filter(|value| valid_prefixed_id(value, "run_", 160))
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "基础 Scene Plan Artifact 缺少合法 runId。",
                )
            })?;
        let content_hash = read
            .artifact
            .get("contentHash")
            .and_then(Value::as_str)
            .filter(|value| is_sha256(value))
            .map(str::to_owned)
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "基础 Scene Plan Artifact 缺少合法 contentHash。",
                )
            })?;
        if read.artifact.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
            || read.artifact.get("artifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("scene_plan")
            || read.artifact.get("kind").and_then(Value::as_str) != Some("scene_plan")
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Artifact 不是当前项目中的 Scene Plan 产物。",
            ));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &options.base_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.status != ArtifactVerificationStatusData::Verified
            || verification.owner_project_id != options.expected_project_id
            || verification.expected_content_hash != content_hash
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ArtifactVerificationFailed,
                operation,
                "基础 Scene Plan Artifact 实体未通过内容校验。",
            )
            .with_safe_context(
                Some(&options.expected_project_id),
                Some("scene_plan"),
                Some(base_run_id),
                Some(&options.base_artifact_id),
            ));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.base_artifact_id,
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Scene Plan Artifact 不是 JSON 文档。",
            )
        })?;
        validate_media_document(&document).map_err(|_| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Scene Plan Artifact 未通过 media v1 契约。",
            )
        })?;
        if document.get("projectId").and_then(Value::as_str)
            != Some(options.expected_project_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 ScenePlanDocument 属于其他项目。",
            ));
        }
        if document.get("documentType").and_then(Value::as_str) != Some("scene_plan")
            || document.get("runId").and_then(Value::as_str) != Some(base_run_id)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 ScenePlanDocument 与 Artifact 运行身份不闭包。",
            ));
        }

        let all_inputs =
            scene_plan_frozen_inputs(&document, &options.expected_project_id, operation)?;
        let research_input = required_scene_plan_input(&all_inputs, "research", operation)?;
        let script_input = required_scene_plan_input(&all_inputs, "script", operation)?;
        let captions_input = required_scene_plan_input(&all_inputs, "captions", operation)?;
        let all_source_ids = all_inputs
            .iter()
            .map(|input| input.artifact_id.clone())
            .collect::<Vec<_>>();
        if !artifact_source_ids_contain(&read.artifact, &all_source_ids) {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Scene Plan Artifact 来源与文档 inputRefs 不闭包。",
            ));
        }
        let source_context = self.load_scene_plan_source_context(&GenerateScenePlanOptions {
            project_path: options.project_path.clone(),
            expected_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            research_input: research_input.clone(),
            script_input: script_input.clone(),
            captions_input: captions_input.clone(),
            idempotency_key: options.idempotency_key.clone(),
        })?;
        validate_scene_plan_semantics(&document, source_context.audio_duration_ms)
            .map_err(|error| map_scene_plan_error(error, operation))?;
        let provenance = exact_artifact_provenance_union([&read.artifact], operation)?;
        let mut source_artifact_ids = vec![options.base_artifact_id.clone()];
        source_artifact_ids.extend(all_source_ids);

        Ok(ScenePlanBaseContext {
            document,
            content_hash,
            audio_duration_ms: source_context.audio_duration_ms,
            source_artifact_ids,
            provenance,
        })
    }

    fn load_scene_plan_save_replay_or_conflict(
        &self,
        options: &SaveScenePlanOptions,
        context: &ScenePlanBaseContext,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<MediaSaveResultData>, MediaServiceError> {
        let operation = MediaOperation::SaveScenePlan;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("save_scene_plan")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("baseArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || receipt.get("baseContentHash").and_then(Value::as_str)
                != Some(context.content_hash.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Scene Plan 保存语义。",
            ));
        }
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if read.owner_project_id != options.expected_project_id
            || read.artifact.get("stageId").and_then(Value::as_str) != Some("scene_plan")
            || read.artifact.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || read.artifact.get("kind").and_then(Value::as_str) != Some("scene_plan")
            || read.artifact.get("contentHash").and_then(Value::as_str)
                != Some(content_hash.as_str())
            || !artifact_source_ids_match(&read.artifact, &context.source_artifact_ids)
        {
            return Err(invalid_replay_artifact(operation));
        }
        let verification = self
            .storage_service
            .verify_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if verification.status != ArtifactVerificationStatusData::Verified
            || verification.expected_content_hash != content_hash
        {
            return Err(invalid_replay_artifact(operation));
        }
        let bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_SCENE_PLAN_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        validate_scene_plan_semantics(&document, context.audio_duration_ms)
            .map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("scene_plan")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("supersedesArtifactId").and_then(Value::as_str)
                != Some(options.base_artifact_id.as_str())
            || document.get("inputRefs") != context.document.get("inputRefs")
            || document.get("configSnapshot") != context.document.get("configSnapshot")
        {
            return Err(invalid_replay_artifact(operation));
        }
        let scene_plan_id = document
            .get("scenePlanId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        if scene_plan_id != stable_media_id("sceneplan_edit", request_fingerprint) {
            return Err(invalid_replay_artifact(operation));
        }
        let created_at = document
            .get("createdAt")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        let expected = apply_scene_plan_edits(
            &context.document,
            &options.edits,
            &options.change_summary,
            &options.run_id,
            scene_plan_id,
            created_at,
            &options.base_artifact_id,
        )
        .map_err(|_| invalid_replay_artifact(operation))?;
        if expected != document {
            return Err(invalid_replay_artifact(operation));
        }
        Ok(Some(MediaSaveResultData {
            api_version: MEDIA_COMMAND_API_VERSION.to_owned(),
            operation: "save_scene_plan".to_owned(),
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            artifact_id,
            changed_scene_ids: scene_plan_changed_ids(&document, operation)?,
            stale_because_stage_ids: vec!["scene_plan".to_owned()],
            idempotent_replay: true,
        }))
    }

    pub(crate) fn load_captions_replay_or_conflict(
        &self,
        options: &ImportCaptionsOptions,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<MediaImportResultData>, MediaServiceError> {
        let operation = MediaOperation::ImportCaptions;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("import_captions")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || receipt.get("requestFingerprint").and_then(Value::as_str)
                != Some(request_fingerprint)
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Captions 导入语义。",
            ));
        }
        let raw_artifact_id = receipt_string(&receipt, "rawArtifactId", operation)?;
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let raw_read = self
            .storage_service
            .get_artifact(&options.project_path, &raw_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        let document_read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if raw_read.owner_project_id != options.expected_project_id
            || document_read.owner_project_id != options.expected_project_id
            || raw_read.artifact.get("stageId").and_then(Value::as_str) != Some("captions")
            || raw_read.artifact.get("runId").and_then(Value::as_str)
                != Some(options.run_id.as_str())
            || document_read
                .artifact
                .get("stageId")
                .and_then(Value::as_str)
                != Some("captions")
            || document_read.artifact.get("runId").and_then(Value::as_str)
                != Some(options.run_id.as_str())
            || document_read.artifact.get("kind").and_then(Value::as_str) != Some("captions")
            || document_read
                .artifact
                .get("contentHash")
                .and_then(Value::as_str)
                != Some(content_hash.as_str())
        {
            return Err(invalid_replay_artifact(operation));
        }
        self.storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &raw_artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&document_bytes)
            .map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("captions_media")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("rawArtifactId").and_then(Value::as_str)
                != Some(raw_artifact_id.as_str())
            || document.get("rawContentHash").and_then(Value::as_str)
                != raw_read.artifact.get("contentHash").and_then(Value::as_str)
            || document
                .pointer("/audioInput/artifactId")
                .and_then(Value::as_str)
                != Some(options.audio_input.artifact_id.as_str())
            || document
                .pointer("/audioInput/contentHash")
                .and_then(Value::as_str)
                != Some(options.audio_input.content_hash.as_str())
        {
            return Err(invalid_replay_artifact(operation));
        }
        Ok(Some(MediaImportResultData {
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            raw_artifact_id,
            artifact_id,
            content_hash,
            document,
            idempotent_replay: true,
        }))
    }

    pub(crate) fn load_audio_replay_or_conflict(
        &self,
        options: &ImportAudioOptions,
        receipt_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<MediaImportResultData>, MediaServiceError> {
        let operation = MediaOperation::ImportAudio;
        let Some(receipt) = self
            .storage_service
            .read_media_receipt(
                &options.project_path,
                &options.expected_project_id,
                receipt_id,
            )
            .map_err(|error| map_storage_error(error, operation))?
        else {
            return Ok(None);
        };
        if receipt.get("operation").and_then(Value::as_str) != Some("import_audio")
            || receipt.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Audio 导入语义。",
            ));
        }
        if receipt.get("requestFingerprint").and_then(Value::as_str) != Some(request_fingerprint) {
            return Err(MediaServiceError::new(
                MediaErrorCode::IdempotencyConflict,
                operation,
                "幂等键已绑定到不同的 Audio 请求指纹。",
            ));
        }

        let raw_artifact_id = receipt_string(&receipt, "rawArtifactId", operation)?;
        let artifact_id = receipt_string(&receipt, "artifactId", operation)?;
        let content_hash = receipt_string(&receipt, "contentHash", operation)?;
        let raw_read = self
            .storage_service
            .get_artifact(&options.project_path, &raw_artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        let document_read = self
            .storage_service
            .get_artifact(&options.project_path, &artifact_id)
            .map_err(|error| map_storage_error(error, operation))?;
        if raw_read.owner_project_id != options.expected_project_id
            || document_read.owner_project_id != options.expected_project_id
            || document_read
                .artifact
                .get("contentHash")
                .and_then(Value::as_str)
                != Some(content_hash.as_str())
            || document_read
                .artifact
                .get("stageId")
                .and_then(Value::as_str)
                != Some("audio")
            || document_read.artifact.get("runId").and_then(Value::as_str)
                != Some(options.run_id.as_str())
            || document_read.artifact.get("kind").and_then(Value::as_str) != Some("voice_audio")
            || raw_read.artifact.get("stageId").and_then(Value::as_str) != Some("audio")
            || raw_read.artifact.get("runId").and_then(Value::as_str)
                != Some(options.run_id.as_str())
        {
            return Err(invalid_replay_artifact(operation));
        }

        self.storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &raw_artifact_id,
                MAX_AUDIO_SOURCE_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &artifact_id,
                MAX_AUDIO_DOCUMENT_BYTES,
            )
            .map_err(|error| map_storage_error(error, operation))?;
        let document: Value = serde_json::from_slice(&document_bytes)
            .map_err(|_| invalid_replay_artifact(operation))?;
        validate_media_document(&document).map_err(|_| invalid_replay_artifact(operation))?;
        if document.get("documentType").and_then(Value::as_str) != Some("audio_media")
            || document.get("projectId").and_then(Value::as_str)
                != Some(options.expected_project_id.as_str())
            || document.get("runId").and_then(Value::as_str) != Some(options.run_id.as_str())
            || document.get("artifactUri").and_then(Value::as_str)
                != Some(raw_read.content_uri.as_str())
            || document
                .pointer("/source/sourceContentHash")
                .and_then(Value::as_str)
                != raw_read.artifact.get("contentHash").and_then(Value::as_str)
        {
            return Err(invalid_replay_artifact(operation));
        }

        Ok(Some(MediaImportResultData {
            owner_project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            raw_artifact_id,
            artifact_id,
            content_hash,
            document,
            idempotent_replay: true,
        }))
    }
}

fn validate_get_media_document_options(
    options: &GetMediaDocumentOptions,
) -> Result<(), MediaServiceError> {
    let operation = MediaOperation::ReadMediaDocument;
    if !valid_portable_id(&options.expected_project_id, 160)
        || !valid_prefixed_id(&options.artifact_id, "artifact_", 160)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "媒体文档读取请求的项目或 Artifact 标识无效。",
        ));
    }
    Ok(())
}

fn invalid_media_document_artifact(operation: MediaOperation) -> MediaServiceError {
    MediaServiceError::new(
        MediaErrorCode::InputReferenceMismatch,
        operation,
        "Artifact 不是受支持且身份闭包完整的媒体文档产物。",
    )
}

fn validate_media_document_read_semantics(
    document: &Value,
    artifact: &Value,
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    let inputs = media_document_frozen_inputs(document, expected_project_id, operation)?;
    let input_artifact_ids = inputs
        .iter()
        .map(|input| input.artifact_id.clone())
        .collect::<Vec<_>>();
    if !artifact_source_ids_contain(artifact, &input_artifact_ids) {
        return Err(MediaServiceError::new(
            MediaErrorCode::InputReferenceMismatch,
            operation,
            "媒体 Artifact 来源未闭包覆盖文档 inputRefs。",
        ));
    }
    match document.get("documentType").and_then(Value::as_str) {
        Some("audio_media") => validate_audio_document_read_semantics(
            document,
            artifact,
            &inputs,
            expected_project_id,
            operation,
        ),
        Some("captions_media") => validate_captions_document_read_semantics(
            document,
            artifact,
            &inputs,
            expected_project_id,
            operation,
        ),
        Some("scene_plan") => {
            validate_exact_input_stages(&inputs, &["research", "script", "captions"], operation)?;
            let duration_ms = document
                .get("scenes")
                .and_then(Value::as_array)
                .and_then(|scenes| scenes.last())
                .and_then(|scene| scene.get("suggestedEndMs"))
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid_media_document_artifact(operation))?;
            validate_scene_plan_semantics(document, duration_ms)
                .map_err(|error| map_scene_plan_error(error, operation))
        }
        Some("timeline") => validate_timeline_semantics(document)
            .map_err(|error| map_timeline_error(error, operation)),
        _ => Err(invalid_media_document_artifact(operation)),
    }
}

fn media_document_frozen_inputs(
    document: &Value,
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<Vec<crate::FrozenArtifactInputData>, MediaServiceError> {
    let values = document
        .get("inputRefs")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 32)
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let mut inputs = Vec::with_capacity(values.len());
    let mut artifact_ids = BTreeSet::new();
    for value in values {
        if value.get("projectId").and_then(Value::as_str) != Some(expected_project_id) {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "媒体文档 inputRefs 包含其他项目引用。",
            ));
        }
        let input = frozen_input_from_document(value, operation)?;
        if !valid_portable_id(&input.stage_id, 160)
            || !valid_prefixed_id(&input.run_id, "run_", 160)
            || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
            || !is_sha256(&input.content_hash)
            || !valid_portable_id(&input.review_record_id, 160)
            || !valid_string_set(&input.claim_ids)
            || !valid_string_set(&input.evidence_refs)
            || !artifact_ids.insert(input.artifact_id.clone())
        {
            return Err(invalid_media_document_artifact(operation));
        }
        inputs.push(input);
    }
    Ok(inputs)
}

fn validate_exact_input_stages(
    inputs: &[crate::FrozenArtifactInputData],
    expected: &[&str],
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    let actual = inputs
        .iter()
        .map(|input| input.stage_id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_set = expected.iter().copied().collect::<BTreeSet<_>>();
    if inputs.len() != expected.len() || actual != expected_set {
        return Err(invalid_media_document_artifact(operation));
    }
    Ok(())
}

fn validate_audio_document_read_semantics(
    document: &Value,
    artifact: &Value,
    inputs: &[crate::FrozenArtifactInputData],
    _expected_project_id: &str,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    validate_exact_input_stages(inputs, &["script"], operation)?;
    if artifact.pointer("/source/origin").and_then(Value::as_str) != Some("derived") {
        return Err(invalid_media_document_artifact(operation));
    }
    let sample_rate = document["sampleRateHz"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let bits_per_sample = document["bitsPerSample"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let channels = document["channels"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let block_align = document["blockAlign"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let byte_rate = document["byteRate"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let data_bytes = document["dataBytes"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let duration_ms = document["durationMs"]
        .as_u64()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let expected_block_align = channels
        .checked_mul(bits_per_sample)
        .and_then(|value| value.checked_div(8));
    let expected_byte_rate = sample_rate.checked_mul(block_align);
    let expected_duration = data_bytes
        .checked_div(block_align.max(1))
        .and_then(|frames| frames.checked_mul(1_000))
        .and_then(|milliseconds| milliseconds.checked_div(sample_rate.max(1)));
    let source_hash = document
        .pointer("/source/sourceContentHash")
        .and_then(Value::as_str)
        .filter(|value| is_sha256(value))
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let digest = source_hash
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let expected_uri = format!("artifacts/objects/sha256/{}/{}", &digest[..2], digest);
    if expected_block_align != Some(block_align)
        || expected_byte_rate != Some(byte_rate)
        || data_bytes == 0
        || !data_bytes.is_multiple_of(block_align.max(1))
        || expected_duration != Some(duration_ms)
        || document.get("artifactUri").and_then(Value::as_str) != Some(expected_uri.as_str())
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::ContractViolation,
            operation,
            "AudioMediaDocument 的 PCM 参数、时长或内部 URI 语义无效。",
        ));
    }
    Ok(())
}

fn validate_captions_document_read_semantics(
    document: &Value,
    artifact: &Value,
    inputs: &[crate::FrozenArtifactInputData],
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    validate_exact_input_stages(inputs, &["script", "audio"], operation)?;
    let audio_input = inputs
        .iter()
        .find(|input| input.stage_id == "audio")
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    if document.get("audioInput") != Some(&frozen_input_document(expected_project_id, audio_input))
        || document.get("rawContentHash") != document.pointer("/source/sourceContentHash")
    {
        return Err(invalid_media_document_artifact(operation));
    }
    let raw_artifact_id = document
        .get("rawArtifactId")
        .and_then(Value::as_str)
        .filter(|value| valid_prefixed_id(value, "artifact_", 160))
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let mut expected_sources = vec![raw_artifact_id.to_owned()];
    expected_sources.extend(inputs.iter().map(|input| input.artifact_id.clone()));
    if !artifact_source_ids_contain(artifact, &expected_sources) {
        return Err(invalid_media_document_artifact(operation));
    }

    let cues = document["cues"]
        .as_array()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let mut cue_ranges = std::collections::BTreeMap::new();
    let mut previous_end = 0_u64;
    for (index, cue) in cues.iter().enumerate() {
        let cue_id = cue["cueId"]
            .as_str()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let start = cue["startMs"]
            .as_u64()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let end = cue["endMs"]
            .as_u64()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let text = cue["text"]
            .as_str()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        if cue["sourceIndex"].as_u64() != Some(index as u64 + 1)
            || start < previous_end
            || start >= end
            || cue_ranges
                .insert(cue_id.to_owned(), (start, end, text.to_owned()))
                .is_some()
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "Captions cue 必须连续编号、唯一、有序且不重叠。",
            ));
        }
        previous_end = end;
    }

    let mappings = document["mappings"]
        .as_array()
        .ok_or_else(|| invalid_media_document_artifact(operation))?;
    let mut mapping_ids = BTreeSet::new();
    let mut ranges = std::collections::BTreeMap::<(String, String), Vec<(u64, u64)>>::new();
    let mut cue_mapping_count = std::collections::BTreeMap::<String, usize>::new();
    for mapping in mappings {
        let mapping_id = mapping["mappingId"]
            .as_str()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let cue_id = mapping["sourceCueId"]
            .as_str()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let level = mapping["level"]
            .as_str()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let start = mapping["startMs"]
            .as_u64()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let end = mapping["endMs"]
            .as_u64()
            .ok_or_else(|| invalid_media_document_artifact(operation))?;
        let Some((cue_start, cue_end, cue_text)) = cue_ranges.get(cue_id) else {
            return Err(invalid_media_document_artifact(operation));
        };
        let timing_pair_valid = match level {
            "cue" => {
                mapping["timingPrecision"].as_str() == Some("cue_exact")
                    && mapping["timingBasis"].as_str() == Some("srt_cue")
                    && start == *cue_start
                    && end == *cue_end
                    && mapping["text"].as_str() == Some(cue_text.as_str())
            }
            "sentence" => {
                mapping["timingPrecision"].as_str() == Some("estimated")
                    && mapping["timingBasis"].as_str() == Some("sentence_interpolation")
            }
            "word" => {
                mapping["timingPrecision"].as_str() == Some("estimated")
                    && mapping["timingBasis"].as_str() == Some("word_interpolation")
            }
            _ => false,
        };
        if !mapping_ids.insert(mapping_id)
            || start < *cue_start
            || end > *cue_end
            || start >= end
            || !timing_pair_valid
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::ContractViolation,
                operation,
                "Captions timing mapping 未闭包于来源 cue 或 timing 语义不一致。",
            ));
        }
        if level == "cue" {
            *cue_mapping_count.entry(cue_id.to_owned()).or_default() += 1;
        } else {
            ranges
                .entry((cue_id.to_owned(), level.to_owned()))
                .or_default()
                .push((start, end));
        }
    }
    for (cue_id, (cue_start, cue_end, _)) in &cue_ranges {
        if cue_mapping_count.get(cue_id) != Some(&1) {
            return Err(invalid_media_document_artifact(operation));
        }
        for level in ["sentence", "word"] {
            let Some(level_ranges) = ranges.get(&(cue_id.clone(), level.to_owned())) else {
                return Err(invalid_media_document_artifact(operation));
            };
            let mut expected_start = *cue_start;
            for (start, end) in level_ranges {
                if *start != expected_start {
                    return Err(MediaServiceError::new(
                        MediaErrorCode::ContractViolation,
                        operation,
                        "Captions 子级 timing mapping 必须无洞、无重叠地覆盖 cue。",
                    ));
                }
                expected_start = *end;
            }
            if expected_start != *cue_end {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ContractViolation,
                    operation,
                    "Captions 子级 timing mapping 未完整覆盖 cue。",
                ));
            }
        }
    }
    Ok(())
}

fn validate_generate_timeline_options(
    options: &GenerateTimelineOptions,
) -> Result<(), MediaServiceError> {
    let operation = MediaOperation::GenerateTimeline;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || !valid_idempotency_key(&options.idempotency_key)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Timeline 请求标识或幂等键无效。",
        ));
    }
    for (input, stage_id) in [
        (&options.audio_input, "audio"),
        (&options.captions_input, "captions"),
        (&options.scene_plan_input, "scene_plan"),
    ] {
        validate_frozen_stage_input(input, stage_id, operation)?;
    }
    let unique_artifacts = [
        &options.audio_input.artifact_id,
        &options.captions_input.artifact_id,
        &options.scene_plan_input.artifact_id,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let canvas = options.canvas;
    let safe_area = options.safe_area;
    let frame_rate_valid = canvas.frame_rate_numerator > 0
        && canvas.frame_rate_numerator <= 240_000
        && canvas.frame_rate_denominator > 0
        && canvas.frame_rate_denominator <= 10_000
        && u64::from(canvas.frame_rate_numerator) >= u64::from(canvas.frame_rate_denominator)
        && u64::from(canvas.frame_rate_numerator)
            <= 240_u64.saturating_mul(u64::from(canvas.frame_rate_denominator));
    let safe_right = safe_area.x.checked_add(safe_area.width);
    let safe_bottom = safe_area.y.checked_add(safe_area.height);
    if unique_artifacts.len() != 3
        || !(16..=16_384).contains(&canvas.width)
        || !(16..=16_384).contains(&canvas.height)
        || !frame_rate_valid
        || safe_area.width == 0
        || safe_area.height == 0
        || safe_area.x > 16_384
        || safe_area.y > 16_384
        || safe_area.width > 16_384
        || safe_area.height > 16_384
        || safe_right.is_none_or(|right| right > canvas.width)
        || safe_bottom.is_none_or(|bottom| bottom > canvas.height)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Timeline 冻结输入、canvas 或 safeArea 无效。",
        ));
    }
    Ok(())
}

fn validate_generate_scene_plan_options(
    options: &GenerateScenePlanOptions,
) -> Result<(), MediaServiceError> {
    let operation = MediaOperation::GenerateScenePlan;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || !valid_idempotency_key(&options.idempotency_key)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Scene Plan 请求标识或幂等键无效。",
        ));
    }
    for (input, stage_id) in [
        (&options.research_input, "research"),
        (&options.script_input, "script"),
        (&options.captions_input, "captions"),
    ] {
        validate_frozen_stage_input(input, stage_id, operation)?;
    }
    let unique_artifacts = [
        &options.research_input.artifact_id,
        &options.script_input.artifact_id,
        &options.captions_input.artifact_id,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if unique_artifacts.len() != 3 {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Scene Plan 的 Research/Script/Captions 必须引用不同 Artifact。",
        ));
    }
    Ok(())
}

fn validate_save_scene_plan_options(
    options: &SaveScenePlanOptions,
) -> Result<(), MediaServiceError> {
    let operation = MediaOperation::SaveScenePlan;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || !valid_prefixed_id(&options.base_artifact_id, "artifact_", 160)
        || options.edits.is_empty()
        || options.edits.len() > 1_000
        || !valid_bounded_text(&options.change_summary, 2_048, false)
        || !valid_idempotency_key(&options.idempotency_key)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Scene Plan 保存请求无效或超过资源上限。",
        ));
    }
    Ok(())
}

fn validate_save_timeline_options(options: &SaveTimelineOptions) -> Result<(), MediaServiceError> {
    let operation = MediaOperation::SaveTimeline;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || !valid_prefixed_id(&options.base_artifact_id, "artifact_", 160)
        || options.edits.is_empty()
        || options.edits.len() > 1_000
        || !valid_bounded_text(&options.change_summary, 2_048, false)
        || !valid_idempotency_key(&options.idempotency_key)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Timeline 保存请求无效或超过资源上限。",
        ));
    }
    Ok(())
}

fn validate_frozen_stage_input(
    input: &crate::FrozenArtifactInputData,
    expected_stage_id: &str,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if input.stage_id != expected_stage_id
        || !valid_prefixed_id(&input.run_id, "run_", 160)
        || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
        || !valid_portable_id(&input.review_record_id, 160)
        || !is_sha256(&input.content_hash)
        || !valid_string_set(&input.claim_ids)
        || !valid_string_set(&input.evidence_refs)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            format!("{expected_stage_id} 冻结输入引用格式无效。"),
        ));
    }
    Ok(())
}

fn valid_idempotency_key(value: &str) -> bool {
    (8..=256).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn timeline_request_fingerprint(
    options: &GenerateTimelineOptions,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::GenerateTimeline;
    let semantic_request = json!({
        "version": 1,
        "operation": "generate_timeline",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "audioInput": options.audio_input,
        "captionsInput": options.captions_input,
        "scenePlanInput": options.scene_plan_input,
        "canvas": options.canvas,
        "safeArea": options.safe_area,
        "configSnapshot": timeline_config_snapshot(),
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Timeline 请求无法生成规范化语义指纹。",
        )
    })
}

fn scene_plan_request_fingerprint(
    options: &GenerateScenePlanOptions,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::GenerateScenePlan;
    let semantic_request = json!({
        "version": 1,
        "operation": "generate_scene_plan",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "researchInput": options.research_input,
        "scriptInput": options.script_input,
        "captionsInput": options.captions_input,
        "configSnapshot": scene_plan_config_snapshot(),
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Scene Plan 请求无法生成规范化语义指纹。",
        )
    })
}

fn scene_plan_save_request_fingerprint(
    options: &SaveScenePlanOptions,
    base_content_hash: &str,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::SaveScenePlan;
    let semantic_request = json!({
        "version": 1,
        "operation": "save_scene_plan",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "baseArtifactId": options.base_artifact_id,
        "baseContentHash": base_content_hash,
        "edits": options.edits,
        "changeSummary": options.change_summary,
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Scene Plan 保存请求无法生成规范化语义指纹。",
        )
    })
}

fn timeline_save_request_fingerprint(
    options: &SaveTimelineOptions,
    base_content_hash: &str,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::SaveTimeline;
    let semantic_request = json!({
        "version": 1,
        "operation": "save_timeline",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "baseArtifactId": options.base_artifact_id,
        "baseContentHash": base_content_hash,
        "edits": options.edits,
        "changeSummary": options.change_summary,
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Timeline 保存请求无法生成规范化语义指纹。",
        )
    })
}

fn scene_plan_config_snapshot() -> Value {
    json!({
        "algorithmId": SCENE_PLAN_ALGORITHM_ID,
        "algorithmVersion": SCENE_PLAN_ALGORITHM_VERSION,
        "cueGrouping": {"maxCuesPerScene": 3},
        "silenceBoundaryPolicy": "midpoint_between_adjacent_scene_cues",
        "titlePolicy": "first_cue_safe_excerpt_v1",
        "traceabilityPolicy": "ordered_union_from_assigned_cues_v1",
    })
}

fn timeline_config_snapshot() -> Value {
    json!({
        "algorithmId": TIMELINE_ALGORITHM_ID,
        "algorithmVersion": TIMELINE_ALGORITHM_VERSION,
        "trackProjectionPolicy": "approved_structured_references_only_v1",
        "sceneBoundaryPolicy": "scene_plan_exact_projection_v1",
        "captionReferencePolicy": "complete_ordered_cue_ids_v1",
    })
}

fn stable_timeline_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0generate_timeline\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn stable_scene_plan_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0generate_scene_plan\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn stable_scene_plan_save_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0save_scene_plan\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn stable_timeline_save_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0save_timeline\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn timeline_frozen_inputs(
    document: &Value,
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<Vec<crate::FrozenArtifactInputData>, MediaServiceError> {
    let values = document
        .get("inputRefs")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 3)
        .ok_or_else(|| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Timeline 必须包含且只包含 audio、captions、scene_plan 三个冻结输入。",
            )
        })?;
    let mut inputs = Vec::with_capacity(values.len());
    let mut artifact_ids = BTreeSet::new();
    let mut stage_ids = BTreeSet::new();
    for value in values {
        if value.get("projectId").and_then(Value::as_str) != Some(expected_project_id) {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 Timeline inputRefs 包含其他项目引用。",
            ));
        }
        let input = frozen_input_from_document(value, operation)?;
        if !matches!(input.stage_id.as_str(), "audio" | "captions" | "scene_plan")
            || !valid_prefixed_id(&input.run_id, "run_", 160)
            || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
            || !is_sha256(&input.content_hash)
            || !valid_portable_id(&input.review_record_id, 160)
            || !valid_string_set(&input.claim_ids)
            || !valid_string_set(&input.evidence_refs)
            || !artifact_ids.insert(input.artifact_id.clone())
            || !stage_ids.insert(input.stage_id.clone())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Timeline inputRefs 的阶段、身份、哈希或追溯字段无效。",
            ));
        }
        inputs.push(input);
    }
    let expected_stages = ["audio", "captions", "scene_plan"]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if stage_ids != expected_stages {
        return Err(MediaServiceError::new(
            MediaErrorCode::InputReferenceMismatch,
            operation,
            "基础 Timeline inputRefs 的阶段集合不完整。",
        ));
    }
    Ok(inputs)
}

fn scene_plan_frozen_inputs(
    document: &Value,
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<Vec<crate::FrozenArtifactInputData>, MediaServiceError> {
    let values = document
        .get("inputRefs")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 32)
        .ok_or_else(|| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Scene Plan inputRefs 缺失或超过上限。",
            )
        })?;
    let mut inputs = Vec::with_capacity(values.len());
    let mut artifact_ids = BTreeSet::new();
    for value in values {
        if value.get("projectId").and_then(Value::as_str) != Some(expected_project_id) {
            return Err(MediaServiceError::new(
                MediaErrorCode::CrossProjectReference,
                operation,
                "基础 Scene Plan inputRefs 包含其他项目引用。",
            ));
        }
        let input = frozen_input_from_document(value, operation)?;
        if !valid_portable_id(&input.stage_id, 160)
            || !valid_prefixed_id(&input.run_id, "run_", 160)
            || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
            || !is_sha256(&input.content_hash)
            || !valid_portable_id(&input.review_record_id, 160)
            || !valid_string_set(&input.claim_ids)
            || !valid_string_set(&input.evidence_refs)
            || !artifact_ids.insert(input.artifact_id.clone())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "基础 Scene Plan inputRefs 身份、哈希或追溯字段无效。",
            ));
        }
        inputs.push(input);
    }
    Ok(inputs)
}

fn required_scene_plan_input(
    inputs: &[crate::FrozenArtifactInputData],
    stage_id: &str,
    operation: MediaOperation,
) -> Result<crate::FrozenArtifactInputData, MediaServiceError> {
    let mut matches = inputs.iter().filter(|input| input.stage_id == stage_id);
    let input = matches.next().cloned().ok_or_else(|| {
        MediaServiceError::new(
            MediaErrorCode::InputReferenceMismatch,
            operation,
            format!("基础 Scene Plan 缺少 {stage_id} 冻结输入。"),
        )
    })?;
    if matches.next().is_some() {
        return Err(MediaServiceError::new(
            MediaErrorCode::InputReferenceMismatch,
            operation,
            format!("基础 Scene Plan 包含重复 {stage_id} 冻结输入。"),
        ));
    }
    Ok(input)
}

fn frozen_input_from_document(
    value: &Value,
    operation: MediaOperation,
) -> Result<crate::FrozenArtifactInputData, MediaServiceError> {
    serde_json::from_value(value.clone()).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InputReferenceMismatch,
            operation,
            "媒体文档中的冻结输入格式无效。",
        )
    })
}

fn document_has_frozen_input(document: &Value, input: &crate::FrozenArtifactInputData) -> bool {
    document
        .get("inputRefs")
        .and_then(Value::as_array)
        .is_some_and(|references| {
            references.iter().any(|reference| {
                reference.get("stageId").and_then(Value::as_str) == Some(input.stage_id.as_str())
                    && reference.get("runId").and_then(Value::as_str) == Some(input.run_id.as_str())
                    && reference.get("artifactId").and_then(Value::as_str)
                        == Some(input.artifact_id.as_str())
                    && reference.get("contentHash").and_then(Value::as_str)
                        == Some(input.content_hash.as_str())
                    && reference.get("reviewRecordId").and_then(Value::as_str)
                        == Some(input.review_record_id.as_str())
                    && reference.get("claimIds") == Some(&json!(input.claim_ids))
                    && reference.get("evidenceRefs") == Some(&json!(input.evidence_refs))
            })
        })
}

fn document_has_exact_frozen_input(document: &Value, expected: &Value) -> bool {
    document
        .get("inputRefs")
        .and_then(Value::as_array)
        .is_some_and(|references| references.iter().any(|reference| reference == expected))
}

fn document_input_artifact_ids(
    document: &Value,
    expected_project_id: &str,
    operation: MediaOperation,
) -> Result<Vec<String>, MediaServiceError> {
    let references = document
        .get("inputRefs")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 32)
        .ok_or_else(|| {
            MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Timeline 输入文档缺少有界 inputRefs。",
            )
        })?;
    let mut ids = Vec::with_capacity(references.len());
    let mut unique = BTreeSet::new();
    for reference in references {
        let artifact_id = reference
            .get("artifactId")
            .and_then(Value::as_str)
            .filter(|value| valid_prefixed_id(value, "artifact_", 160))
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "Timeline 输入文档包含无效 Artifact 引用。",
                )
            })?;
        if reference.get("projectId").and_then(Value::as_str) != Some(expected_project_id)
            || !unique.insert(artifact_id.to_owned())
        {
            return Err(MediaServiceError::new(
                MediaErrorCode::InputReferenceMismatch,
                operation,
                "Timeline 输入文档包含跨项目或重复 Artifact 引用。",
            ));
        }
        ids.push(artifact_id.to_owned());
    }
    Ok(ids)
}

fn artifact_source_ids_match(artifact: &Value, expected: &[String]) -> bool {
    artifact
        .pointer("/source/sourceArtifactIds")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.len() == expected.len()
                && values
                    .iter()
                    .zip(expected)
                    .all(|(value, expected)| value.as_str() == Some(expected.as_str()))
        })
}

fn artifact_source_ids_contain(artifact: &Value, expected: &[String]) -> bool {
    artifact
        .pointer("/source/sourceArtifactIds")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            expected.iter().all(|expected_id| {
                values
                    .iter()
                    .any(|value| value.as_str() == Some(expected_id.as_str()))
            })
        })
}

fn scene_plan_changed_ids(
    document: &Value,
    operation: MediaOperation,
) -> Result<Vec<String>, MediaServiceError> {
    let values = document
        .pointer("/changeSummary/changedSceneIds")
        .and_then(Value::as_array)
        .filter(|values| values.len() <= 10_000)
        .ok_or_else(|| invalid_replay_artifact(operation))?;
    let mut ids = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for value in values {
        let id = value
            .as_str()
            .filter(|id| valid_portable_id(id, 160))
            .ok_or_else(|| invalid_replay_artifact(operation))?;
        if !seen.insert(id.to_owned()) {
            return Err(invalid_replay_artifact(operation));
        }
        ids.push(id.to_owned());
    }
    if !ids.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(invalid_replay_artifact(operation));
    }
    Ok(ids)
}

fn timeline_changed_ids(
    document: &Value,
    operation: MediaOperation,
) -> Result<Vec<String>, MediaServiceError> {
    let changed = scene_plan_changed_ids(document, operation)?;
    let scenes = document
        .get("sceneTrack")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 10_000)
        .ok_or_else(|| invalid_replay_artifact(operation))?;
    let mut expected = scenes
        .iter()
        .map(|scene| {
            scene
                .get("sceneId")
                .and_then(Value::as_str)
                .filter(|value| valid_portable_id(value, 160))
                .map(str::to_owned)
                .ok_or_else(|| invalid_replay_artifact(operation))
        })
        .collect::<Result<Vec<_>, _>>()?;
    expected.sort();
    expected.dedup();
    if changed != expected {
        return Err(invalid_replay_artifact(operation));
    }
    Ok(changed)
}

fn map_scene_plan_error(error: ScenePlanError, operation: MediaOperation) -> MediaServiceError {
    let code = match error.code {
        ScenePlanErrorCode::InvalidRequest | ScenePlanErrorCode::InvalidEdit => {
            MediaErrorCode::InvalidRequest
        }
        ScenePlanErrorCode::InvalidCaptions => MediaErrorCode::InputReferenceMismatch,
        ScenePlanErrorCode::InvalidScenePlan | ScenePlanErrorCode::ContractViolation => {
            MediaErrorCode::ContractViolation
        }
        ScenePlanErrorCode::ResourceLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
    };
    MediaServiceError::new(code, operation, error.message)
}

fn map_timeline_error(error: TimelineDomainError, operation: MediaOperation) -> MediaServiceError {
    let code = match error.code {
        TimelineDomainErrorCode::InvalidRequest | TimelineDomainErrorCode::InvalidEdit => {
            MediaErrorCode::InvalidRequest
        }
        TimelineDomainErrorCode::InvalidInput => MediaErrorCode::InputReferenceMismatch,
        TimelineDomainErrorCode::InvalidTimeline | TimelineDomainErrorCode::ContractViolation => {
            MediaErrorCode::ContractViolation
        }
        TimelineDomainErrorCode::ResourceLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
    };
    MediaServiceError::new(code, operation, error.message)
}

pub(crate) fn audio_request_fingerprint(
    options: &ImportAudioOptions,
    source_file_name: &str,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::ImportAudio;
    let semantic_request = json!({
        "version": 1,
        "operation": "import_audio",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "source": {
            "sourceFileName": source_file_name,
            "expectedSourceContentHash": options.expected_source_content_hash,
        },
        "scriptInput": options.script_input,
        "rights": options.rights,
        "limits": {
            "maxBytes": options.limits.max_bytes,
        },
        "configSnapshot": options.config_snapshot,
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Audio 请求无法生成规范化语义指纹。",
        )
    })
}

fn captions_request_fingerprint(
    options: &ImportCaptionsOptions,
    source_file_name: &str,
    source_content_hash: &str,
) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::ImportCaptions;
    let semantic_request = json!({
        "version": 1,
        "operation": "import_captions",
        "projectId": options.expected_project_id,
        "runId": options.run_id,
        "source": {
            "sourceFileName": source_file_name,
            "sourceContentHash": source_content_hash,
            "expectedSourceContentHash": options.expected_source_content_hash,
        },
        "scriptInput": options.script_input,
        "audioInput": options.audio_input,
        "audioDurationMs": options.audio_duration_ms,
        "rights": options.rights,
        "limits": {
            "maxBytes": options.limits.max_bytes,
            "maxCueCount": options.limits.max_cue_count,
            "maxCueTextBytes": options.limits.max_cue_text_bytes,
        },
        "configSnapshot": options.config_snapshot,
    });
    hash_canonical_json(&semantic_request).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Captions 请求无法生成规范化语义指纹。",
        )
    })
}

fn validate_audio_options(options: &ImportAudioOptions) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::ImportAudio;
    validate_rights(&options.rights, operation)?;
    if !Path::new(&options.source_path).is_absolute() {
        return Err(invalid_source_name(operation));
    }
    let source_file_name = safe_source_basename(Path::new(&options.source_path), operation)?;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || options.idempotency_key.is_empty()
        || options.idempotency_key.chars().count() > 256
        || options.idempotency_key.chars().any(char::is_control)
        || !options.config_snapshot.is_object()
        || options.limits.max_bytes == 0
        || options.limits.max_bytes > MAX_AUDIO_SOURCE_BYTES
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Audio 请求标识、配置或资源上限无效。",
        ));
    }
    if let Some(expected) = options.expected_source_content_hash.as_deref() {
        validate_expected_source_hash(Some(expected), expected, operation)?;
    }
    validate_frozen_script_input(&options.script_input, operation)?;
    Ok(source_file_name)
}

fn validate_captions_options(options: &ImportCaptionsOptions) -> Result<String, MediaServiceError> {
    let operation = MediaOperation::ImportCaptions;
    validate_rights(&options.rights, operation)?;
    if !Path::new(&options.source_path).is_absolute() {
        return Err(invalid_source_name(operation));
    }
    let source_file_name = safe_source_basename(Path::new(&options.source_path), operation)?;
    if !valid_prefixed_id(&options.run_id, "run_", 160)
        || !valid_portable_id(&options.expected_project_id, 160)
        || options.idempotency_key.is_empty()
        || options.idempotency_key.chars().count() > 256
        || options.idempotency_key.chars().any(char::is_control)
        || !options.config_snapshot.is_object()
        || options.audio_duration_ms == 0
        || options.audio_duration_ms > 86_400_000
        || options.limits.max_bytes == 0
        || options.limits.max_bytes > MAX_AUDIO_SOURCE_BYTES
        || options.limits.max_cue_count == 0
        || options.limits.max_cue_count > 10_000
        || options.limits.max_cue_text_bytes == 0
        || options.limits.max_cue_text_bytes > 8_000
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Captions 请求标识、配置、时长或资源上限无效。",
        ));
    }
    if let Some(expected) = options.expected_source_content_hash.as_deref() {
        validate_expected_source_hash(Some(expected), expected, operation)?;
    }
    validate_frozen_script_input(&options.script_input, operation)?;
    validate_frozen_audio_input(&options.audio_input, operation)?;
    if options.script_input.artifact_id == options.audio_input.artifact_id {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Captions 的 Script 与 Audio 输入必须引用不同 Artifact。",
        ));
    }
    Ok(source_file_name)
}

fn validate_frozen_script_input(
    input: &crate::FrozenArtifactInputData,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if input.stage_id != "script"
        || !valid_prefixed_id(&input.run_id, "run_", 160)
        || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
        || !valid_portable_id(&input.review_record_id, 160)
        || !is_sha256(&input.content_hash)
        || !valid_string_set(&input.claim_ids)
        || !valid_string_set(&input.evidence_refs)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Script 冻结输入引用格式无效。",
        ));
    }
    Ok(())
}

fn validate_frozen_audio_input(
    input: &crate::FrozenArtifactInputData,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if input.stage_id != "audio"
        || !valid_prefixed_id(&input.run_id, "run_", 160)
        || !valid_prefixed_id(&input.artifact_id, "artifact_", 160)
        || !valid_portable_id(&input.review_record_id, 160)
        || !is_sha256(&input.content_hash)
        || !valid_string_set(&input.claim_ids)
        || !valid_string_set(&input.evidence_refs)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "Audio 冻结输入引用格式无效。",
        ));
    }
    Ok(())
}

fn approved_input(
    input: &crate::FrozenArtifactInputData,
    ref_id: &str,
    kind: &str,
) -> ApprovedArtifactInputData {
    ApprovedArtifactInputData {
        ref_id: ref_id.to_owned(),
        kind: kind.to_owned(),
        artifact_id: input.artifact_id.clone(),
        source_run_id: input.run_id.clone(),
        review_record_id: input.review_record_id.clone(),
        content_hash: input.content_hash.clone(),
        claim_ids: input.claim_ids.clone(),
        evidence_refs: input.evidence_refs.clone(),
    }
}

fn valid_string_set(values: &[String]) -> bool {
    values.len() <= 1_024
        && values
            .iter()
            .all(|value| valid_bounded_text(value, 512, false))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_prefixed_id(value: &str, prefix: &str, max_bytes: usize) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty() && valid_portable_id(suffix, max_bytes - prefix.len())
    })
}

fn valid_portable_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn exact_artifact_provenance_union<'a>(
    artifacts: impl IntoIterator<Item = &'a Value>,
    operation: MediaOperation,
) -> Result<Vec<Value>, MediaServiceError> {
    let mut pairs = BTreeSet::new();
    for artifact in artifacts {
        let provenance = artifact
            .get("provenance")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                MediaServiceError::new(
                    MediaErrorCode::InputReferenceMismatch,
                    operation,
                    "批准 Artifact 缺少可验证的 provenance 配对。",
                )
            })?;
        for reference in provenance {
            let claim_id = reference
                .get("claimId")
                .and_then(Value::as_str)
                .filter(|value| valid_bounded_text(value, 512, false))
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::InputReferenceMismatch,
                        operation,
                        "批准 Artifact 包含无效的 claim provenance。",
                    )
                })?;
            let evidence_ref = reference
                .get("evidenceRef")
                .and_then(Value::as_str)
                .filter(|value| valid_bounded_text(value, 512, false))
                .ok_or_else(|| {
                    MediaServiceError::new(
                        MediaErrorCode::InputReferenceMismatch,
                        operation,
                        "批准 Artifact 包含无效的 evidence provenance。",
                    )
                })?;
            pairs.insert((claim_id.to_owned(), evidence_ref.to_owned()));
            if pairs.len() > 4_096 {
                return Err(MediaServiceError::new(
                    MediaErrorCode::ResourceLimitExceeded,
                    operation,
                    "批准 Artifact provenance 超过安全上限。",
                ));
            }
        }
    }
    Ok(pairs
        .into_iter()
        .map(|(claim_id, evidence_ref)| json!({"claimId": claim_id, "evidenceRef": evidence_ref}))
        .collect())
}

fn artifact_traceability_sets(
    artifact: &Value,
    operation: MediaOperation,
) -> Result<(Vec<String>, Vec<String>), MediaServiceError> {
    let provenance = exact_artifact_provenance_union([artifact], operation)?;
    let mut claim_ids = BTreeSet::new();
    let mut evidence_refs = BTreeSet::new();
    for reference in provenance {
        claim_ids.insert(artifact_string(&reference, "claimId", operation)?);
        evidence_refs.insert(artifact_string(&reference, "evidenceRef", operation)?);
    }
    Ok((
        claim_ids.into_iter().collect(),
        evidence_refs.into_iter().collect(),
    ))
}

fn media_save_job_idempotency_key(stage_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-save-job:v1\0");
    hasher.update(stage_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(idempotency_key.as_bytes());
    format!("media_save_{}", lowercase_hex(&hasher.finalize()))
}

fn media_save_error_is_retryable(code: MediaErrorCode) -> bool {
    matches!(
        code,
        MediaErrorCode::Io | MediaErrorCode::StorageUnavailable
    )
}

fn frozen_input_document(project_id: &str, input: &crate::FrozenArtifactInputData) -> Value {
    json!({
        "projectId": project_id,
        "stageId": input.stage_id,
        "runId": input.run_id,
        "artifactId": input.artifact_id,
        "contentHash": input.content_hash,
        "reviewRecordId": input.review_record_id,
        "claimIds": input.claim_ids,
        "evidenceRefs": input.evidence_refs,
    })
}

fn artifact_draft(
    value: Value,
    operation: MediaOperation,
) -> Result<ArtifactDraft, MediaServiceError> {
    serde_json::from_value(value).map_err(|_| {
        MediaServiceError::new(
            MediaErrorCode::ContractViolation,
            operation,
            "媒体 Artifact 草稿未通过 v1 契约。",
        )
    })
}

fn artifact_string(
    artifact: &Value,
    field: &str,
    operation: MediaOperation,
) -> Result<String, MediaServiceError> {
    artifact
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .map(str::to_owned)
        .ok_or_else(|| invalid_replay_artifact(operation))
}

fn internal_source_uri(content_hash: &str, source_file_name: &str) -> String {
    format!(
        "narracut:sha256/{}/{}",
        &content_hash["sha256:".len()..],
        percent_encode_uri_segment(source_file_name)
    )
}

fn percent_encode_uri_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

fn stable_media_id(prefix: &str, fingerprint: &str) -> String {
    format!("{prefix}_{}", &fingerprint["sha256:".len()..])
}

fn stable_media_artifact_id(fingerprint: &str) -> String {
    format!(
        "artifact_{}",
        &fingerprint["sha256:".len().."sha256:".len() + 32]
    )
}

fn parse_caption_script_traceability(
    content: &[u8],
    artifact: &Value,
) -> Result<CaptionScriptTraceability, MediaServiceError> {
    let operation = MediaOperation::ImportCaptions;
    let artifact_provenance = match artifact.get("provenance") {
        Some(provenance) => parse_caption_provenance_array(
            Some(provenance),
            "Script Artifact provenance",
            operation,
        )?,
        None => Vec::new(),
    };
    let document: Value = match serde_json::from_slice(content) {
        Ok(document) => document,
        Err(_) if artifact_provenance.is_empty() => {
            return Ok(CaptionScriptTraceability {
                normalized_narration: Vec::new(),
                segments: Vec::new(),
            });
        }
        Err(_) => return Err(invalid_caption_script_traceability()),
    };
    if document.get("schemaVersion").and_then(Value::as_str) != Some("narracut.script/v1") {
        if artifact_provenance.is_empty() {
            return Ok(CaptionScriptTraceability {
                normalized_narration: Vec::new(),
                segments: Vec::new(),
            });
        }
        return Err(invalid_caption_script_traceability());
    }
    serde_json::from_value::<narracut_contracts::StructuredScriptOutput>(document.clone())
        .map_err(|_| invalid_caption_script_traceability())?;

    let source_segments = document
        .get("segments")
        .and_then(Value::as_array)
        .filter(|segments| !segments.is_empty() && segments.len() <= 128)
        .ok_or_else(invalid_caption_script_traceability)?;
    let artifact_pairs = artifact_provenance.iter().cloned().collect::<BTreeSet<_>>();
    let mut used_pairs = BTreeSet::new();
    let mut normalized_narration = Vec::new();
    let mut segments = Vec::with_capacity(source_segments.len());
    for (index, segment) in source_segments.iter().enumerate() {
        if segment.get("order").and_then(Value::as_u64) != Some(index as u64)
            || segment
                .get("segmentId")
                .and_then(Value::as_str)
                .is_none_or(|id| !valid_prefixed_id(id, "segment_", 160))
        {
            return Err(invalid_caption_script_traceability());
        }
        let narration = segment
            .get("narration")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty() && text.chars().count() <= 8_000)
            .ok_or_else(invalid_caption_script_traceability)?;
        let normalized = normalize_caption_trace_text(narration);
        if normalized.is_empty() {
            return Err(invalid_caption_script_traceability());
        }
        let provenance = parse_caption_provenance_array(
            segment.get("provenance"),
            "Script segment provenance",
            operation,
        )?;
        if provenance.is_empty()
            || provenance.len() > 128
            || provenance.iter().any(|pair| !artifact_pairs.contains(pair))
        {
            return Err(invalid_caption_script_traceability());
        }
        used_pairs.extend(provenance.iter().cloned());
        let start = normalized_narration.len();
        normalized_narration.extend(normalized);
        segments.push(CaptionScriptSegment {
            start,
            end: normalized_narration.len(),
            provenance,
        });
    }
    if used_pairs != artifact_pairs {
        return Err(invalid_caption_script_traceability());
    }
    Ok(CaptionScriptTraceability {
        normalized_narration,
        segments,
    })
}

fn parse_caption_provenance_array(
    value: Option<&Value>,
    _label: &str,
    operation: MediaOperation,
) -> Result<Vec<CaptionProvenancePair>, MediaServiceError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(invalid_caption_script_traceability)?;
    if values.len() > 4_096 {
        return Err(MediaServiceError::new(
            MediaErrorCode::ResourceLimitExceeded,
            operation,
            "Script provenance 超过安全上限。",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::with_capacity(values.len());
    for item in values {
        let claim_id = item
            .get("claimId")
            .and_then(Value::as_str)
            .filter(|value| valid_bounded_text(value, 512, false))
            .ok_or_else(invalid_caption_script_traceability)?;
        let evidence_ref = item
            .get("evidenceRef")
            .and_then(Value::as_str)
            .filter(|value| valid_bounded_text(value, 512, false))
            .ok_or_else(invalid_caption_script_traceability)?;
        let pair = CaptionProvenancePair {
            claim_id: claim_id.to_owned(),
            evidence_ref: evidence_ref.to_owned(),
        };
        if !seen.insert(pair.clone()) {
            return Err(invalid_caption_script_traceability());
        }
        pairs.push(pair);
    }
    Ok(pairs)
}

fn invalid_caption_script_traceability() -> MediaServiceError {
    MediaServiceError::new(
        MediaErrorCode::InputReferenceMismatch,
        MediaOperation::ImportCaptions,
        "批准 Script Artifact 的结构化 provenance 无法验证。",
    )
}

fn normalize_caption_trace_text(value: &str) -> Vec<char> {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn cue_provenance(
    traceability: &CaptionScriptTraceability,
    cue: &crate::ParsedCaptionCue,
    cursor: &mut usize,
) -> Result<Vec<CaptionProvenancePair>, MediaServiceError> {
    if traceability.segments.is_empty() {
        return Ok(Vec::new());
    }
    let needle = normalize_caption_trace_text(&cue.text);
    let mut match_start = None;
    for start in *cursor
        ..=traceability
            .normalized_narration
            .len()
            .saturating_sub(needle.len())
    {
        if traceability.normalized_narration[start..].starts_with(&needle) {
            if match_start.is_some() {
                return Err(unmappable_caption_cue(cue));
            }
            match_start = Some(start);
        }
    }
    let start = match_start.ok_or_else(|| unmappable_caption_cue(cue))?;
    let end = start + needle.len();
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for segment in &traceability.segments {
        if segment.start < end && segment.end > start {
            for pair in &segment.provenance {
                if seen.insert(pair.clone()) {
                    pairs.push(pair.clone());
                }
            }
        }
    }
    if pairs.is_empty() {
        return Err(unmappable_caption_cue(cue));
    }
    *cursor = end;
    Ok(pairs)
}

fn unmappable_caption_cue(cue: &crate::ParsedCaptionCue) -> MediaServiceError {
    MediaServiceError::new(
        MediaErrorCode::InputReferenceMismatch,
        MediaOperation::ImportCaptions,
        format!(
            "captions_provenance_unmappable sourceIndex={} cueId={}",
            cue.source_index, cue.cue_id
        ),
    )
}

fn caption_provenance_values(pairs: &[CaptionProvenancePair]) -> Vec<Value> {
    pairs
        .iter()
        .map(|pair| {
            json!({
                "claimId": pair.claim_id,
                "evidenceRef": pair.evidence_ref,
            })
        })
        .collect()
}

fn caption_provenance_projection(pairs: &[CaptionProvenancePair]) -> (Vec<String>, Vec<String>) {
    let mut seen_claims = BTreeSet::new();
    let mut seen_evidence = BTreeSet::new();
    let mut claims = Vec::new();
    let mut evidence = Vec::new();
    for pair in pairs {
        if seen_claims.insert(pair.claim_id.clone()) {
            claims.push(pair.claim_id.clone());
        }
        if seen_evidence.insert(pair.evidence_ref.clone()) {
            evidence.push(pair.evidence_ref.clone());
        }
    }
    (claims, evidence)
}

fn build_caption_cues_and_mappings(
    parsed: &ParsedSrt,
    script_traceability: &CaptionScriptTraceability,
) -> Result<CaptionBuildResult, MediaServiceError> {
    let operation = MediaOperation::ImportCaptions;
    let mut cues = Vec::with_capacity(parsed.cues.len());
    let mut mappings = Vec::new();
    let mut all_provenance = Vec::new();
    let mut seen_provenance = BTreeSet::new();
    let mut script_cursor = 0;
    for cue in &parsed.cues {
        let provenance = cue_provenance(script_traceability, cue, &mut script_cursor)?;
        let (claim_ids, evidence_refs) = caption_provenance_projection(&provenance);
        for pair in &provenance {
            if seen_provenance.insert(pair.clone()) {
                all_provenance.push(pair.clone());
            }
        }
        cues.push(json!({
            "cueId": cue.cue_id,
            "sourceIndex": cue.source_index,
            "startMs": cue.start_ms,
            "endMs": cue.end_ms,
            "text": cue.text,
            "provenance": caption_provenance_values(&provenance),
            "claimIds": claim_ids,
            "evidenceRefs": evidence_refs,
        }));
        push_mapping(
            &mut mappings,
            json!({
                "mappingId": stable_mapping_id(&cue.cue_id, "cue", 0, &cue.text),
                "level": "cue",
                "sourceCueId": cue.cue_id,
                "startMs": cue.start_ms,
                "endMs": cue.end_ms,
                "text": cue.text,
                "timingPrecision": "cue_exact",
                "timingBasis": "srt_cue",
            }),
            operation,
        )?;

        let sentences = split_sentences(&cue.text);
        let sentence_ranges = partition_time_range(
            cue.start_ms,
            cue.end_ms,
            &sentences
                .iter()
                .map(|sentence| text_weight(sentence))
                .collect::<Vec<_>>(),
            operation,
        )?;
        for (index, (sentence, (start_ms, end_ms))) in
            sentences.iter().zip(sentence_ranges).enumerate()
        {
            push_mapping(
                &mut mappings,
                json!({
                    "mappingId": stable_mapping_id(&cue.cue_id, "sentence", index, sentence),
                    "level": "sentence",
                    "sourceCueId": cue.cue_id,
                    "startMs": start_ms,
                    "endMs": end_ms,
                    "text": sentence,
                    "timingPrecision": "estimated",
                    "timingBasis": "sentence_interpolation",
                }),
                operation,
            )?;
        }

        let words = tokenize_words(&cue.text);
        let word_ranges = partition_time_range(
            cue.start_ms,
            cue.end_ms,
            &words
                .iter()
                .map(|word| text_weight(word))
                .collect::<Vec<_>>(),
            operation,
        )?;
        for (index, (word, (start_ms, end_ms))) in words.iter().zip(word_ranges).enumerate() {
            push_mapping(
                &mut mappings,
                json!({
                    "mappingId": stable_mapping_id(&cue.cue_id, "word", index, word),
                    "level": "word",
                    "sourceCueId": cue.cue_id,
                    "startMs": start_ms,
                    "endMs": end_ms,
                    "text": word,
                    "timingPrecision": "estimated",
                    "timingBasis": "word_interpolation",
                }),
                operation,
            )?;
        }
    }
    let diagnostics = vec![json!({
        "code": "captions_estimated_subcue_timing",
        "severity": "info",
        "message": "Sentence and word timing is deterministically interpolated inside each exact SRT cue.",
        "blocking": false,
    })];
    Ok((cues, mappings, diagnostics, all_provenance))
}

fn push_mapping(
    mappings: &mut Vec<Value>,
    mapping: Value,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if mappings.len() >= MAX_CAPTION_MAPPINGS {
        return Err(MediaServiceError::new(
            MediaErrorCode::ResourceLimitExceeded,
            operation,
            "字幕 timing mappings 超过 200000 条契约上限。",
        ));
    }
    mappings.push(mapping);
    Ok(())
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if matches!(
            character,
            '。' | '！' | '？' | '!' | '?' | '.' | '；' | ';' | '\n'
        ) {
            let sentence = current.trim().to_owned();
            if !sentence.is_empty() {
                sentences.push(sentence);
            }
            current.clear();
        }
    }
    let trailing = current.trim().to_owned();
    if !trailing.is_empty() {
        sentences.push(trailing);
    }
    if sentences.is_empty() {
        sentences.push(text.to_owned());
    }
    sentences
}

/// 连续 ASCII 字母数字（含单词内撇号/下划线）为一个词；非 ASCII 非空白字符逐字成词；
/// 标点附着前一个 token，确保中英文混排规则稳定且不依赖 locale。
fn tokenize_words(text: &str) -> Vec<String> {
    let mut tokens = Vec::<String>::new();
    let mut ascii = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '\'' | '_') {
            ascii.push(character);
            continue;
        }
        if !ascii.is_empty() {
            tokens.push(std::mem::take(&mut ascii));
        }
        if character.is_whitespace() {
            continue;
        }
        if character.is_ascii_punctuation() || is_cjk_punctuation(character) {
            if let Some(previous) = tokens.last_mut() {
                previous.push(character);
            } else {
                tokens.push(character.to_string());
            }
        } else {
            tokens.push(character.to_string());
        }
    }
    if !ascii.is_empty() {
        tokens.push(ascii);
    }
    if tokens.is_empty() {
        tokens.push(text.to_owned());
    }
    tokens
}

fn is_cjk_punctuation(character: char) -> bool {
    matches!(
        character,
        '，' | '。' | '！' | '？' | '；' | '：' | '、' | '“' | '”' | '‘' | '’' | '（' | '）'
    )
}

fn text_weight(text: &str) -> u64 {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .count()
        .max(1) as u64
}

fn partition_time_range(
    start_ms: u64,
    end_ms: u64,
    weights: &[u64],
    operation: MediaOperation,
) -> Result<Vec<(u64, u64)>, MediaServiceError> {
    let duration = end_ms.checked_sub(start_ms).ok_or_else(|| {
        MediaServiceError::new(
            MediaErrorCode::InvalidMedia,
            operation,
            "字幕 cue 时间范围无效。",
        )
    })?;
    if weights.is_empty() || duration < weights.len() as u64 {
        return Err(MediaServiceError::new(
            MediaErrorCode::ResourceLimitExceeded,
            operation,
            "字幕 cue 时长不足以生成非零估算 timing mappings。",
        ));
    }
    let total_weight = weights.iter().try_fold(0_u64, |total, weight| {
        total.checked_add(*weight).filter(|value| *value > 0)
    });
    let total_weight = total_weight.ok_or_else(|| {
        MediaServiceError::new(
            MediaErrorCode::ResourceLimitExceeded,
            operation,
            "字幕 mapping 权重溢出。",
        )
    })?;
    let distributable = duration - weights.len() as u64;
    let mut cumulative_weight = 0_u64;
    let mut previous = start_ms;
    let mut ranges = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        cumulative_weight += *weight;
        let distributed = ((u128::from(distributable) * u128::from(cumulative_weight))
            / u128::from(total_weight)) as u64;
        let boundary = if index + 1 == weights.len() {
            end_ms
        } else {
            start_ms + index as u64 + 1 + distributed
        };
        ranges.push((previous, boundary));
        previous = boundary;
    }
    Ok(ranges)
}

fn stable_mapping_id(cue_id: &str, level: &str, index: usize, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:caption-mapping:v1\0");
    hasher.update(cue_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(level.as_bytes());
    hasher.update((index as u64).to_le_bytes());
    hasher.update(text.as_bytes());
    format!("map_{}", lowercase_hex(&hasher.finalize()))
}

pub(crate) fn stable_audio_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0import_audio\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn stable_captions_receipt_id(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0import_captions\0");
    hasher.update(idempotency_key.as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn hash_canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{}", lowercase_hex(&digest)))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

fn receipt_string(
    receipt: &Value,
    field: &str,
    operation: MediaOperation,
) -> Result<String, MediaServiceError> {
    receipt
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .map(str::to_owned)
        .ok_or_else(|| invalid_replay_artifact(operation))
}

fn invalid_replay_artifact(operation: MediaOperation) -> MediaServiceError {
    MediaServiceError::new(
        MediaErrorCode::ArtifactVerificationFailed,
        operation,
        "媒体幂等 receipt 指向的不可变 Artifact 无法通过校验。",
    )
}

pub(crate) fn validate_rights(
    rights: &MediaRightsData,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if rights.voice_authorization != "not_voice_clone" {
        return Err(MediaServiceError::new(
            MediaErrorCode::VoiceCloneNotAllowed,
            operation,
            "当前媒体导入不允许声音克隆授权。",
        ));
    }
    if !matches!(rights.ownership.as_str(), "self_recorded" | "licensed")
        || !valid_bounded_text(&rights.author, 256, false)
        || !valid_bounded_text(&rights.rights_statement, 2_048, false)
        || !valid_bounded_text(&rights.license_id, 256, false)
        || !valid_bounded_text(&rights.attribution_text, 2_048, true)
    {
        return Err(MediaServiceError::new(
            MediaErrorCode::RightsRequired,
            operation,
            "媒体来源必须包含完整且有界的权利声明。",
        ));
    }
    Ok(())
}

pub(crate) fn safe_source_basename(
    source_path: &Path,
    operation: MediaOperation,
) -> Result<String, MediaServiceError> {
    let file_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid_source_name(operation))?;
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.len() > MAX_SOURCE_FILE_NAME_BYTES
        || file_name.ends_with(['.', ' '])
        || file_name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        || is_windows_reserved_basename(file_name)
    {
        return Err(invalid_source_name(operation));
    }
    Ok(file_name.to_owned())
}

pub(crate) fn validate_expected_source_hash(
    expected: Option<&str>,
    actual: &str,
    operation: MediaOperation,
) -> Result<(), MediaServiceError> {
    if !is_sha256(actual) || expected.is_some_and(|value| !is_sha256(value)) {
        return Err(MediaServiceError::new(
            MediaErrorCode::InvalidRequest,
            operation,
            "源内容哈希必须使用 sha256:<64位小写十六进制>。",
        ));
    }
    if expected.is_some_and(|value| value != actual) {
        return Err(MediaServiceError::new(
            MediaErrorCode::SourceHashMismatch,
            operation,
            "媒体源内容哈希与调用方冻结值不一致。",
        ));
    }
    Ok(())
}

pub(crate) fn map_media_parse_error(
    error: MediaParseError,
    operation: MediaOperation,
) -> MediaServiceError {
    let code = match error.code {
        MediaParseErrorCode::ResourceLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
        MediaParseErrorCode::Io => MediaErrorCode::Io,
        MediaParseErrorCode::InvalidWav
        | MediaParseErrorCode::InvalidUtf8
        | MediaParseErrorCode::InvalidSrt
        | MediaParseErrorCode::Unsupported => MediaErrorCode::InvalidMedia,
    };
    MediaServiceError::new(code, operation, "媒体源未通过有界格式校验。")
}

pub(crate) fn map_storage_error(
    error: StorageServiceError,
    operation: MediaOperation,
) -> MediaServiceError {
    let code = match error.code {
        StorageErrorCode::ProjectIdentityMismatch => MediaErrorCode::CrossProjectReference,
        StorageErrorCode::SourceChanged => MediaErrorCode::SourceChanged,
        StorageErrorCode::SourceTooLarge
        | StorageErrorCode::ArtifactTooLarge
        | StorageErrorCode::ScanLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
        StorageErrorCode::ArtifactNotFound | StorageErrorCode::ContentCorrupt => {
            MediaErrorCode::ArtifactVerificationFailed
        }
        StorageErrorCode::ArtifactConflict => MediaErrorCode::IdempotencyConflict,
        StorageErrorCode::InvalidRequest
        | StorageErrorCode::InvalidPath
        | StorageErrorCode::PathContainsSymlink
        | StorageErrorCode::InvalidArtifact => MediaErrorCode::InvalidRequest,
        StorageErrorCode::SourceNotFound | StorageErrorCode::IoError => MediaErrorCode::Io,
        StorageErrorCode::ProjectNotFound
        | StorageErrorCode::InvalidProject
        | StorageErrorCode::MigrationRequired
        | StorageErrorCode::UnsupportedNewerVersion
        | StorageErrorCode::IndexUnavailable
        | StorageErrorCode::IndexMigrationFailed
        | StorageErrorCode::CacheCleanupFailed
        | StorageErrorCode::InternalContractError => MediaErrorCode::StorageUnavailable,
    };
    MediaServiceError::new(code, operation, "媒体 Artifact 操作失败。").with_safe_context(
        None,
        None,
        None,
        error.artifact_id.as_deref(),
    )
}

pub(crate) fn map_workflow_error(
    error: WorkflowServiceError,
    operation: MediaOperation,
) -> MediaServiceError {
    let code = match error.code {
        WorkflowErrorCode::ProjectIdentityMismatch => MediaErrorCode::CrossProjectReference,
        WorkflowErrorCode::StageNotReady
        | WorkflowErrorCode::ReviewConflict
        | WorkflowErrorCode::RunNotFound => MediaErrorCode::InputNotApproved,
        WorkflowErrorCode::ArtifactMismatch
        | WorkflowErrorCode::RunConflict
        | WorkflowErrorCode::ImmutableConflict => MediaErrorCode::InputReferenceMismatch,
        WorkflowErrorCode::InvalidRequest
        | WorkflowErrorCode::InvalidPath
        | WorkflowErrorCode::PathContainsSymlink
        | WorkflowErrorCode::StageNotFound => MediaErrorCode::InvalidRequest,
        WorkflowErrorCode::ScanLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
        WorkflowErrorCode::ProjectNotFound
        | WorkflowErrorCode::InvalidProject
        | WorkflowErrorCode::MigrationRequired
        | WorkflowErrorCode::UnsupportedNewerVersion
        | WorkflowErrorCode::WorkflowNotInitialized
        | WorkflowErrorCode::UnsupportedWorkflow
        | WorkflowErrorCode::InvalidStageGraph
        | WorkflowErrorCode::ConfigConflict
        | WorkflowErrorCode::IoError
        | WorkflowErrorCode::InternalContractError => MediaErrorCode::StorageUnavailable,
    };
    MediaServiceError::new(code, operation, "冻结输入未通过当前批准闭包校验。").with_safe_context(
        None,
        error.stage_id.as_deref(),
        error.run_id.as_deref(),
        None,
    )
}

fn map_job_error(error: JobServiceError, operation: MediaOperation) -> MediaServiceError {
    let code = match error.code {
        JobErrorCode::ProjectIdentityMismatch => MediaErrorCode::CrossProjectReference,
        JobErrorCode::IdempotencyConflict | JobErrorCode::EventConflict => {
            MediaErrorCode::IdempotencyConflict
        }
        JobErrorCode::StageNotReady | JobErrorCode::WorkflowNotInitialized => {
            MediaErrorCode::InputNotApproved
        }
        JobErrorCode::ScanLimitExceeded => MediaErrorCode::ResourceLimitExceeded,
        JobErrorCode::InvalidRequest
        | JobErrorCode::InvalidPath
        | JobErrorCode::PathContainsSymlink => MediaErrorCode::InvalidRequest,
        JobErrorCode::JobNotFound
        | JobErrorCode::InvalidTransition
        | JobErrorCode::LeaseConflict
        | JobErrorCode::LeaseExpired
        | JobErrorCode::ProjectNotFound
        | JobErrorCode::InvalidProject
        | JobErrorCode::MigrationRequired
        | JobErrorCode::UnsupportedNewerVersion
        | JobErrorCode::IoError
        | JobErrorCode::InternalContractError => MediaErrorCode::StorageUnavailable,
    };
    MediaServiceError::new(
        code,
        operation,
        format!(
            "媒体编辑 Job 生命周期操作失败（{}）：{}",
            error.code.as_str(),
            error.message
        ),
    )
    .with_safe_context(
        None,
        error.stage_id.as_deref(),
        error.run_id.as_deref(),
        None,
    )
}

fn valid_bounded_text(value: &str, max_chars: usize, allow_empty: bool) -> bool {
    let character_count = value.chars().count();
    (allow_empty || character_count > 0)
        && character_count <= max_chars
        && !value.chars().any(char::is_control)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_windows_reserved_basename(file_name: &str) -> bool {
    let stem = file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn invalid_source_name(operation: MediaOperation) -> MediaServiceError {
    MediaServiceError::new(
        MediaErrorCode::InvalidSourceName,
        operation,
        "媒体源文件名不满足可移植安全规则。",
    )
}

fn map_project_error(error: ProjectServiceError, operation: MediaOperation) -> MediaServiceError {
    let code = match error.code {
        ProjectErrorCode::InvalidRequest
        | ProjectErrorCode::InvalidName
        | ProjectErrorCode::InvalidPath
        | ProjectErrorCode::PathContainsSymlink => MediaErrorCode::InvalidRequest,
        ProjectErrorCode::CopyTooLarge => MediaErrorCode::ResourceLimitExceeded,
        ProjectErrorCode::ProjectNotFound
        | ProjectErrorCode::MarkerMissing
        | ProjectErrorCode::MarkerTooLarge
        | ProjectErrorCode::InvalidProject
        | ProjectErrorCode::MigrationRequired
        | ProjectErrorCode::MigrationConflict
        | ProjectErrorCode::UnsupportedNewerVersion
        | ProjectErrorCode::MigrationFailed
        | ProjectErrorCode::DestinationExists
        | ProjectErrorCode::IoError
        | ProjectErrorCode::TrashFailed
        | ProjectErrorCode::InternalContractError => MediaErrorCode::StorageUnavailable,
    };
    MediaServiceError::new(code, operation, "无法打开 Audio 请求所属项目。")
}

#[cfg(test)]
mod caption_traceability_tests {
    use serde_json::{json, Value};

    use super::{
        build_caption_cues_and_mappings, parse_caption_script_traceability, MediaErrorCode,
    };
    use crate::{ParsedCaptionCue, ParsedSrt};

    #[test]
    fn cue_mapping_preserves_exact_pairs_without_cartesian_products() {
        let artifact = script_artifact(json!([
            {"claimId":"claim_1","evidenceRef":"evidence_1"},
            {"claimId":"claim_1","evidenceRef":"evidence_2"},
            {"claimId":"claim_2","evidenceRef":"evidence_1"}
        ]));
        let script = json!({
            "schemaVersion": "narracut.script/v1",
            "title": "Trace fixture",
            "language": "en",
            "summary": "Trace fixture",
            "estimatedDurationSeconds": 1,
            "segments": [
                {
                    "segmentId": "segment_one",
                    "order": 0,
                    "title": "One",
                    "narration": "Alpha cue.",
                    "provenance": [
                        {"claimId":"claim_1","evidenceRef":"evidence_1"},
                        {"claimId":"claim_1","evidenceRef":"evidence_2"}
                    ]
                },
                {
                    "segmentId": "segment_two",
                    "order": 1,
                    "title": "Two",
                    "narration": "Beta cue.",
                    "provenance": [
                        {"claimId":"claim_2","evidenceRef":"evidence_1"}
                    ]
                }
            ]
        });
        let traceability = parse_caption_script_traceability(
            &serde_json::to_vec(&script).expect("script"),
            &artifact,
        )
        .expect("parse traceability");
        let parsed = parsed_srt(&["Alpha cue.", "Beta cue."]);
        let (cues, _, _, all_pairs) =
            build_caption_cues_and_mappings(&parsed, &traceability).expect("map cues");

        assert_eq!(
            cues[0]["provenance"],
            json!([
                {"claimId":"claim_1","evidenceRef":"evidence_1"},
                {"claimId":"claim_1","evidenceRef":"evidence_2"}
            ])
        );
        assert_eq!(
            cues[1]["provenance"],
            json!([{"claimId":"claim_2","evidenceRef":"evidence_1"}])
        );
        assert_eq!(all_pairs.len(), 3);
        assert!(!cues.iter().any(|cue| {
            cue["provenance"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|pair| pair["claimId"] == "claim_2" && pair["evidenceRef"] == "evidence_2")
        }));
    }

    #[test]
    fn cue_can_span_segments_and_unmappable_errors_are_redacted() {
        let artifact = script_artifact(json!([
            {"claimId":"claim_a","evidenceRef":"evidence_a"},
            {"claimId":"claim_b","evidenceRef":"evidence_b"}
        ]));
        let script = json!({
            "schemaVersion": "narracut.script/v1",
            "title": "Span fixture",
            "language": "en",
            "summary": "Span fixture",
            "estimatedDurationSeconds": 1,
            "segments": [
                {"segmentId":"segment_a","order":0,"title":"A","narration":"Alpha ","provenance":[{"claimId":"claim_a","evidenceRef":"evidence_a"}]},
                {"segmentId":"segment_b","order":1,"title":"B","narration":" Beta","provenance":[{"claimId":"claim_b","evidenceRef":"evidence_b"}]}
            ]
        });
        let traceability = parse_caption_script_traceability(
            &serde_json::to_vec(&script).expect("script"),
            &artifact,
        )
        .expect("parse traceability");
        let (cues, _, _, _) =
            build_caption_cues_and_mappings(&parsed_srt(&["Alpha\u{3000}Beta"]), &traceability)
                .expect("map spanning cue");
        assert_eq!(cues[0]["provenance"].as_array().map(Vec::len), Some(2));

        let secret = "PRIVATE_CAPTION_TEXT";
        let error = build_caption_cues_and_mappings(&parsed_srt(&[secret]), &traceability)
            .expect_err("unmappable cue");
        assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
        assert!(error.message.contains("sourceIndex=1"));
        assert!(error.message.contains("cueId=cue_1"));
        assert!(!error.message.contains(secret));
    }

    #[test]
    fn legacy_script_is_only_allowed_without_factual_provenance() {
        let legacy = br#"{"segments":[{"text":"legacy"}]}"#;
        let empty = parse_caption_script_traceability(legacy, &script_artifact(json!([])))
            .expect("non-factual legacy script");
        let (cues, _, _, all_pairs) =
            build_caption_cues_and_mappings(&parsed_srt(&["anything"]), &empty)
                .expect("empty traceability");
        assert_eq!(cues[0]["provenance"], json!([]));
        assert!(all_pairs.is_empty());

        let error = parse_caption_script_traceability(
            legacy,
            &script_artifact(json!([{"claimId":"claim_1","evidenceRef":"evidence_1"}])),
        )
        .expect_err("factual legacy script must block");
        assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
    }

    fn script_artifact(provenance: Value) -> Value {
        json!({"provenance": provenance})
    }

    fn parsed_srt(texts: &[&str]) -> ParsedSrt {
        ParsedSrt {
            content_hash: format!("sha256:{}", "a".repeat(64)),
            byte_length: 1,
            cues: texts
                .iter()
                .enumerate()
                .map(|(index, text)| ParsedCaptionCue {
                    cue_id: format!("cue_{}", index + 1),
                    source_index: index as u32 + 1,
                    start_ms: index as u64 * 100,
                    end_ms: index as u64 * 100 + 100,
                    text: (*text).to_owned(),
                })
                .collect(),
        }
    }
}
