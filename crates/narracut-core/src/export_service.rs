use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};

use fs2::available_space;
use narracut_contracts::{
    validate_export_message, validate_media_document, validate_renderer_message, ArtifactDraft,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    ApprovedArtifactInputData, ArtifactCommitPlanEntryData, ArtifactTransferObserver,
    BeginJobCompletionOptions, ClaimStageJobRequestOptions, EnqueueExportOptions,
    EnqueueStageJobOptions, ExportCommitResultData, ExportEnqueueResultData, ExportErrorCode,
    ExportOperation, ExportRenderInputData, ExportServiceError, ExportTransferAbort,
    ExportTransferObserver, GetJobOptions, GetStageJobRequestOptions, JobFinalizationModeData,
    JobService, JobStatusData, ListJobsOptions, PreparedExportData, ProjectService,
    RetryExportOptions, RetryPolicyData, RunExportQaOptions, StorageService,
    StoreArtifactFileOptions, ValidateApprovedMediaInputsOptions, WorkflowService,
    CURRENT_PROJECT_FORMAT_VERSION, EXPORT_COMMAND_API_VERSION, EXPORT_MANIFEST_VERSION,
};

const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const MIN_DISK_RESERVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPORT_FILES: usize = 32;
const MAX_CAPTION_CHARS_PER_LINE: usize = 42;
const MAX_CAPTION_LINES: usize = 2;

#[derive(Clone)]
pub struct ExportService {
    project_service: ProjectService,
    storage_service: StorageService,
    workflow_service: WorkflowService,
    job_service: JobService,
}

impl ExportService {
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

    pub fn run_qa(
        &self,
        options: RunExportQaOptions,
        current_renderer_identity: Option<&Value>,
    ) -> Result<Value, ExportServiceError> {
        let context =
            self.load_context(&options, current_renderer_identity, ExportOperation::RunQa)?;
        let mut checks = Vec::new();
        let mut diagnostics = Vec::new();

        let video_entry = context
            .render_result
            .get("artifacts")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|entry| {
                    entry.get("artifactId").and_then(Value::as_str)
                        == Some(options.render_input.artifact_id.as_str())
                })
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::HashMismatch,
                    ExportOperation::RunQa,
                    "RenderResult 未闭合到所选 rendered_video。",
                )
            })?;
        let timeline = &context.timeline_document;
        let canvas = timeline
            .get("canvas")
            .ok_or_else(|| contract_error(ExportOperation::RunQa, "Timeline 缺少 canvas。"))?;
        let canvas_ok = video_entry.get("width") == canvas.get("width")
            && video_entry.get("height") == canvas.get("height");
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_canvas", "canvas"),
            (canvas_ok, false),
            (
                "输出画布与已审核 Timeline 一致。",
                "输出画布与已审核 Timeline 不一致。",
            ),
            (&[], &[&options.render_input.artifact_id]),
        );

        let timeline_duration = u64_field(timeline, "durationMs", ExportOperation::RunQa)?;
        let video_duration = u64_field(video_entry, "durationMs", ExportOperation::RunQa)?;
        let duration_ok = timeline_duration.abs_diff(video_duration) <= 50;
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_duration", "duration"),
            (duration_ok, false),
            (
                "输出时长位于 50 ms 容差内。",
                "输出时长与 Timeline 超出 50 ms 容差。",
            ),
            (&[], &[&options.render_input.artifact_id]),
        );

        let audio_ok = video_entry.get("hasAudio").and_then(Value::as_bool) == Some(true)
            && context.documents_by_type.contains_key("audio_media");
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_audio", "audio"),
            (audio_ok, false),
            (
                "输出包含可探测音轨并闭合到音频文档。",
                "输出缺少音轨或音频追溯文档。",
            ),
            (&[], &[&options.render_input.artifact_id]),
        );

        let timeline_scene_ids = timeline
            .get("sceneTrack")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.get("sceneId").and_then(Value::as_str))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let rendered_scene_ids = video_entry
            .get("sceneIds")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let scenes_ok = !timeline_scene_ids.is_empty()
            && timeline_scene_ids == rendered_scene_ids
            && scenes_are_contiguous(timeline);
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_scenes", "scenes"),
            (scenes_ok, false),
            (
                "场景顺序与覆盖完整且连续。",
                "场景覆盖、顺序或连续性不满足冻结 Timeline。",
            ),
            (&timeline_scene_ids, &[&options.render_input.artifact_id]),
        );

        let captions = context.documents_by_type.get("captions_media");
        let captions_ok = captions
            .is_some_and(|document| captions_in_range(document, timeline_duration))
            && safe_area_valid(timeline);
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_captions", "captions"),
            (captions_ok, false),
            (
                "字幕范围与安全区满足可执行边界。",
                "字幕时间范围或安全区无效。",
            ),
            (&timeline_scene_ids, &[]),
        );

        let text_ok = captions.is_some_and(caption_text_fits);
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_text_layout", "text_layout"),
            (text_ok, false),
            (
                "字幕文本满足每行 42 字符、最多两行且无重叠规则。",
                "字幕文本溢出、重叠或包含禁止控制字符。",
            ),
            (&timeline_scene_ids, &[]),
        );

        let video_metadata = context
            .adopted_artifacts
            .iter()
            .find(|artifact| {
                artifact.get("artifactId").and_then(Value::as_str)
                    == Some(options.render_input.artifact_id.as_str())
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::ArtifactNotFound,
                    ExportOperation::RunQa,
                    "rendered_video 元数据缺失。",
                )
            })?;
        let provenance_ok =
            provenance_closed(&options.render_input, video_metadata, timeline, captions);
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_provenance", "provenance"),
            (provenance_ok, false),
            (
                "claim/evidence 引用为已审核输入子集。",
                "claim/evidence 断链或超出冻结输入集合。",
            ),
            (&timeline_scene_ids, &[&options.render_input.artifact_id]),
        );

        let rights_ok =
            context.licenses.iter().all(license_complete) && !context.licenses.is_empty();
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_rights", "rights"),
            (rights_ok, false),
            (
                "素材作者、许可、署名与声音授权记录完整。",
                "素材许可、署名或授权记录不完整。",
            ),
            (&[], &[]),
        );

        let hashes_ok = context.adopted_artifacts.iter().all(|artifact| {
            artifact
                .get("artifactId")
                .and_then(Value::as_str)
                .is_some_and(|id| {
                    context
                        .verified_artifact_ids
                        .iter()
                        .any(|verified| verified == id)
                })
        });
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_hash", "hash"),
            (hashes_ok, false),
            (
                "采用 Artifact 的 SHA-256 与字节数全部复验。",
                "至少一个采用 Artifact 的内容哈希或字节数漂移。",
            ),
            (
                &[],
                &context
                    .verified_artifact_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            ),
        );

        let frozen_identity = context.render_result.get("rendererIdentity");
        let identity_ok =
            current_renderer_identity.is_some() && frozen_identity == current_renderer_identity;
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_renderer", "renderer"),
            (identity_ok, false),
            (
                "当前 FFmpeg/FFprobe 与冻结 Renderer identity 完全一致。",
                "FFmpeg/FFprobe 缺失或 Renderer identity 已漂移。",
            ),
            (&[], &[&options.render_input.result_artifact_id]),
        );

        let probe_ok = video_entry.get("mediaType").and_then(Value::as_str) == Some("video/mp4")
            && video_entry
                .get("byteLength")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
            && video_entry
                .get("width")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
            && video_entry
                .get("height")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0);
        push_check(
            &mut checks,
            &mut diagnostics,
            ("qa_probe", "probe"),
            (probe_ok, false),
            (
                "Renderer 的 FFprobe 真相包含容器、尺寸、时长与音轨。",
                "输出缺少可探测媒体真相。",
            ),
            (&[], &[&options.render_input.artifact_id]),
        );

        let warning_count = checks
            .iter()
            .filter(|check| check.get("status").and_then(Value::as_str) == Some("warning"))
            .count();
        let blocking_count = checks
            .iter()
            .filter(|check| check.get("status").and_then(Value::as_str) == Some("blocked"))
            .count();
        let qa_hash = hash_json(
            &json!({ "renderInput": options.render_input, "checks": checks, "diagnostics": diagnostics }),
        )?;
        let qa = json!({
            "status": if blocking_count == 0 { "passed" } else { "blocked" },
            "passed": blocking_count == 0,
            "warningCount": warning_count,
            "blockingCount": blocking_count,
            "checks": checks,
            "diagnostics": diagnostics,
            "checkedAt": now()?,
            "qaHash": qa_hash,
        });
        let result = json!({ "apiVersion": EXPORT_COMMAND_API_VERSION, "operation": "run_export_qa", "ownerProjectId": options.expected_project_id, "renderInput": options.render_input, "qa": qa });
        validate_export_message(&result).map_err(|_| {
            contract_error(
                ExportOperation::RunQa,
                "ExportQaResult 未通过 Export v1 契约。",
            )
        })?;
        Ok(result)
    }

    pub fn enqueue_export(
        &self,
        options: EnqueueExportOptions,
        current_renderer_identity: Option<&Value>,
    ) -> Result<ExportEnqueueResultData, ExportServiceError> {
        validate_enqueue_shape(&options)?;
        let qa = self.run_qa(
            RunExportQaOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                render_input: options.render_input.clone(),
            },
            current_renderer_identity,
        )?;
        let actual_qa_hash = qa
            .pointer("/qa/qaHash")
            .and_then(Value::as_str)
            .ok_or_else(|| contract_error(ExportOperation::Enqueue, "QA 结果缺少 qaHash。"))?;
        if qa.pointer("/qa/passed").and_then(Value::as_bool) != Some(true) {
            return Err(error(
                ExportErrorCode::QaBlocked,
                ExportOperation::Enqueue,
                "导出 QA 存在阻塞项；修复后重新运行 QA。",
            ));
        }
        if actual_qa_hash != options.qa_hash {
            return Err(error(
                ExportErrorCode::QaChanged,
                ExportOperation::Enqueue,
                "QA 输入或结果已变化；必须采用最新 qaHash。",
            ));
        }
        let request = serde_json::to_value(&options)
            .map_err(|_| contract_error(ExportOperation::Enqueue, "无法冻结导出请求。"))?;
        let claim = self
            .job_service
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                idempotency_key: options.idempotency_key.clone(),
                request: request.clone(),
            })
            .map_err(map_job_error)?;
        let input_refs = vec![json!({
            "refId": format!("ref_{}", options.render_input.artifact_id), "referenceType": "artifact", "kind": "rendered_video",
            "artifactId": options.render_input.artifact_id, "sourceRunId": options.render_input.run_id, "reviewRecordId": options.render_input.review_record_id,
            "contentHash": options.render_input.content_hash, "claimIds": options.render_input.claim_ids, "evidenceRefs": options.render_input.evidence_refs,
        })];
        let snapshot = self.job_service.enqueue_stage_job_with_request(EnqueueStageJobOptions {
            project_path: options.project_path, expected_project_id: options.expected_project_id.clone(), stage_id: "export".to_owned(), run_id: options.run_id.clone(), input_refs,
            executor: json!({ "providerId": "narracut_export", "providerVersion": EXPORT_COMMAND_API_VERSION, "executionMode": "local", "model": "atomic_export" }),
            idempotency_key: options.idempotency_key,
            retry_policy: RetryPolicyData { max_attempts: 3, initial_backoff_ms: 1_000, backoff_multiplier: 2, max_backoff_ms: 15_000 },
        }, request).map_err(map_job_error)?;
        let job_id = snapshot
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .ok_or_else(|| contract_error(ExportOperation::Enqueue, "JobSnapshot 缺少 jobId。"))?
            .to_owned();
        Ok(ExportEnqueueResultData {
            api_version: EXPORT_COMMAND_API_VERSION.to_owned(),
            operation: "enqueue_export".to_owned(),
            owner_project_id: options.expected_project_id,
            run_id: options.run_id,
            job_id,
            status: snapshot.status.as_str().to_owned(),
            idempotent_replay: claim.idempotent_replay,
        })
    }

    pub fn retry_export(
        &self,
        options: RetryExportOptions,
        current_renderer_identity: Option<&Value>,
    ) -> Result<ExportEnqueueResultData, ExportServiceError> {
        let snapshot = self
            .job_service
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: options.source_job_id.clone(),
            })
            .map_err(map_job_error)?;
        if !matches!(
            snapshot.status,
            JobStatusData::Failed | JobStatusData::Canceled
        ) || snapshot.job.get("stageId").and_then(Value::as_str) != Some("export")
            || snapshot
                .job
                .pointer("/executor/providerId")
                .and_then(Value::as_str)
                != Some("narracut_export")
        {
            return Err(error(
                ExportErrorCode::InvalidRequest,
                ExportOperation::Enqueue,
                "只有 failed/canceled 的 NarraCut Export Job 可以专用重试。",
            ));
        }
        let receipt = self
            .job_service
            .get_stage_job_request(GetStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: options.source_job_id,
            })
            .map_err(map_job_error)?;
        let mut request: EnqueueExportOptions = serde_json::from_value(receipt.request)
            .map_err(|_| contract_error(ExportOperation::Enqueue, "源 Export receipt 无效。"))?;
        if request.project_path != options.project_path
            || request.expected_project_id != options.expected_project_id
        {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                ExportOperation::Enqueue,
                "源 Export receipt 不属于当前项目。",
            ));
        }
        request.run_id = options.new_run_id;
        request.idempotency_key = options.idempotency_key;
        self.enqueue_export(request, current_renderer_identity)
    }

    pub fn prepare_export(
        &self,
        options: EnqueueExportOptions,
        current_renderer_identity: Option<&Value>,
    ) -> Result<PreparedExportData, ExportServiceError> {
        let qa_result = self.run_qa(
            RunExportQaOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                render_input: options.render_input.clone(),
            },
            current_renderer_identity,
        )?;
        if qa_result.pointer("/qa/passed").and_then(Value::as_bool) != Some(true) {
            return Err(error(
                ExportErrorCode::QaBlocked,
                ExportOperation::Prepare,
                "导出前复验 QA 未通过。",
            ));
        }
        if qa_result.pointer("/qa/qaHash").and_then(Value::as_str) != Some(options.qa_hash.as_str())
        {
            return Err(error(
                ExportErrorCode::QaChanged,
                ExportOperation::Prepare,
                "导出执行前 QA 身份已变化。",
            ));
        }
        let context = self.load_context(
            &RunExportQaOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                render_input: options.render_input.clone(),
            },
            current_renderer_identity,
            ExportOperation::Prepare,
        )?;
        let video = context
            .adopted_artifacts
            .iter()
            .find(|value| {
                value.get("artifactId").and_then(Value::as_str)
                    == Some(options.render_input.artifact_id.as_str())
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::ArtifactNotFound,
                    ExportOperation::Prepare,
                    "找不到采用的 rendered_video。",
                )
            })?;
        let video_content_uri = string_field(video, "uri", ExportOperation::Prepare)?;
        let video_byte_length = u64_field(video, "byteLength", ExportOperation::Prepare)?;
        let video_content_hash = string_field(video, "contentHash", ExportOperation::Prepare)?;
        Ok(PreparedExportData {
            options,
            qa_result,
            render_result: context.render_result,
            adopted_artifacts: context.adopted_artifacts,
            adopted_review_record_ids: context.adopted_review_record_ids,
            source_documents: context.source_documents,
            video_content_uri,
            video_byte_length,
            video_content_hash,
        })
    }

    pub fn commit_export(
        &self,
        job_id: &str,
        prepared: PreparedExportData,
        observer: &dyn ExportTransferObserver,
    ) -> Result<ExportCommitResultData, ExportServiceError> {
        self.commit_export_controlled(job_id, None, prepared, observer)
    }

    pub fn commit_export_for_job(
        &self,
        job_id: &str,
        lease_id: &str,
        prepared: PreparedExportData,
        observer: &dyn ExportTransferObserver,
    ) -> Result<ExportCommitResultData, ExportServiceError> {
        self.commit_export_controlled(job_id, Some(lease_id), prepared, observer)
    }

    fn commit_export_controlled(
        &self,
        job_id: &str,
        lease_id: Option<&str>,
        prepared: PreparedExportData,
        observer: &dyn ExportTransferObserver,
    ) -> Result<ExportCommitResultData, ExportServiceError> {
        let operation = ExportOperation::Commit;
        let descriptor = self
            .project_service
            .open_project(&prepared.options.project_path)
            .map_err(map_project_error)?;
        if descriptor.project_id != prepared.options.expected_project_id {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                operation,
                "项目身份不匹配。",
            ));
        }
        let project_root = PathBuf::from(&descriptor.project_path);
        let destination_root = canonical_existing_directory(
            Path::new(&prepared.options.destination_directory),
            operation,
        )?;
        let export_id = stable_id("export_", job_id.as_bytes());
        let final_dir = destination_root.join(&prepared.options.export_name);
        if final_dir.exists() {
            if let Ok(result) = self.read_existing_result(&project_root, job_id) {
                if result.get("exportId").and_then(Value::as_str) == Some(export_id.as_str())
                    && final_dir
                        == Path::new(
                            result
                                .get("exportPath")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )
                {
                    return commit_result_from_value(result, true);
                }
            }
            if let Some(recovered) = self.recover_published_export(
                &prepared,
                job_id,
                &project_root,
                &final_dir,
                &export_id,
            )? {
                return Ok(recovered);
            }
            return Err(error(
                ExportErrorCode::DestinationConflict,
                operation,
                "目标导出目录已存在且不属于同一幂等导出。",
            ));
        }
        let required_bytes = prepared
            .video_byte_length
            .saturating_mul(2)
            .saturating_add(MIN_DISK_RESERVE_BYTES);
        let free_bytes = available_space(&destination_root)
            .map_err(|_| error(ExportErrorCode::Io, operation, "无法读取目标磁盘可用空间。"))?;
        if free_bytes < required_bytes
            || prepared.video_byte_length > prepared.options.max_temporary_bytes
        {
            return Err(error(
                ExportErrorCode::DiskSpaceInsufficient,
                operation,
                "目标磁盘空间或导出临时字节上限不足。",
            ));
        }
        let temp_dir = destination_root.join(format!(".narracut-{}.partial", export_id));
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).map_err(|_| {
                error(
                    ExportErrorCode::Io,
                    operation,
                    "无法清理上次中断的导出临时目录。",
                )
            })?;
        }
        fs::create_dir(&temp_dir)
            .map_err(|_| error(ExportErrorCode::Io, operation, "无法创建导出临时目录。"))?;
        let paths = ExportCommitPaths {
            project_root: &project_root,
            temp_dir: &temp_dir,
            final_dir: &final_dir,
            export_id: &export_id,
        };
        let marker_already_persisted = lease_id.is_some()
            && self
                .job_service
                .get_job(GetJobOptions {
                    project_path: prepared.options.project_path.clone(),
                    expected_project_id: prepared.options.expected_project_id.clone(),
                    job_id: job_id.to_owned(),
                })
                .ok()
                .is_some_and(|snapshot| {
                    snapshot.finalization_mode == Some(JobFinalizationModeData::ExternalCommit)
                });
        let result = self.commit_export_inner(
            job_id,
            lease_id,
            marker_already_persisted,
            &prepared,
            &paths,
            observer,
        );
        if result.is_err() {
            let marker_persisted = lease_id.is_some()
                && self
                    .job_service
                    .get_job(GetJobOptions {
                        project_path: prepared.options.project_path.clone(),
                        expected_project_id: prepared.options.expected_project_id.clone(),
                        job_id: job_id.to_owned(),
                    })
                    .ok()
                    .is_some_and(|snapshot| {
                        snapshot.finalization_mode == Some(JobFinalizationModeData::ExternalCommit)
                    });
            if !marker_persisted {
                let _ = self.storage_service.abort_artifact_commit_journal(
                    &prepared.options.project_path,
                    &prepared.options.expected_project_id,
                    job_id,
                );
                let _ = fs::remove_dir_all(&temp_dir);
            }
        }
        result
    }

    fn commit_export_inner(
        &self,
        job_id: &str,
        lease_id: Option<&str>,
        marker_already_persisted: bool,
        prepared: &PreparedExportData,
        paths: &ExportCommitPaths<'_>,
        observer: &dyn ExportTransferObserver,
    ) -> Result<ExportCommitResultData, ExportServiceError> {
        let ExportCommitPaths {
            project_root,
            temp_dir,
            final_dir,
            export_id,
        } = *paths;
        let operation = ExportOperation::Commit;
        let noop_observer = crate::NoopExportTransferObserver;
        let observer: &dyn ExportTransferObserver = if marker_already_persisted {
            &noop_observer
        } else {
            observer
        };
        let mut files = Vec::new();
        let video_source = safe_project_uri(project_root, &prepared.video_content_uri, operation)?;
        let video = copy_hashed(
            &video_source,
            &temp_dir.join("video.mp4"),
            "video",
            prepared.video_byte_length,
            observer,
        )?;
        ensure_frozen_hash(&video, &prepared.video_content_hash, "rendered_video")?;
        files.push(manifest_file(
            "video",
            "video.mp4",
            &prepared.video_content_uri,
            &video,
            "video/mp4",
        ));

        let mut documents = BTreeMap::new();
        for (metadata, document) in &prepared.source_documents {
            if let Some(document_type) = document.get("documentType").and_then(Value::as_str) {
                documents.insert(document_type.to_owned(), (metadata, document));
            }
        }
        let timeline_pair = documents
            .get("timeline")
            .ok_or_else(|| contract_error(operation, "导出缺少 Timeline 文档。"))?;
        let timeline_bytes = pretty_json_bytes(timeline_pair.1)?;
        let timeline_info = write_hashed(&temp_dir.join("timeline.json"), &timeline_bytes)?;
        files.push(manifest_file(
            "timeline",
            "timeline.json",
            string_field(timeline_pair.0, "uri", operation)?.as_str(),
            &timeline_info,
            "application/json",
        ));
        let captions_pair = documents
            .get("captions_media")
            .ok_or_else(|| contract_error(operation, "导出缺少 Captions 文档。"))?;
        let captions_bytes = pretty_json_bytes(captions_pair.1)?;
        let captions_info = write_hashed(&temp_dir.join("captions.json"), &captions_bytes)?;
        files.push(manifest_file(
            "captions",
            "captions.json",
            string_field(captions_pair.0, "uri", operation)?.as_str(),
            &captions_info,
            "application/json",
        ));
        let audio_pair = documents
            .get("audio_media")
            .ok_or_else(|| contract_error(operation, "导出缺少 Audio 文档。"))?;
        let raw_audio_id = audio_pair
            .0
            .get("source")
            .and_then(|s| s.get("sourceArtifactIds"))
            .and_then(Value::as_array)
            .and_then(|ids| {
                ids.iter().filter_map(Value::as_str).find(|id| {
                    prepared.adopted_artifacts.iter().any(|artifact| {
                        artifact.get("artifactId").and_then(Value::as_str) == Some(*id)
                            && artifact.get("kind").and_then(Value::as_str) == Some("audio_source")
                    })
                })
            })
            .ok_or_else(|| contract_error(operation, "Audio 文档未闭合到 audio_source。"))?;
        let raw_audio = prepared
            .adopted_artifacts
            .iter()
            .find(|artifact| {
                artifact.get("artifactId").and_then(Value::as_str) == Some(raw_audio_id)
            })
            .ok_or_else(|| contract_error(operation, "冻结的 audio_source Artifact 缺失。"))?;
        let raw_uri = string_field(raw_audio, "uri", operation)?;
        let raw_length = u64_field(raw_audio, "byteLength", operation)?;
        let raw_hash = string_field(raw_audio, "contentHash", operation)?;
        let audio_info = copy_hashed(
            &safe_project_uri(project_root, &raw_uri, operation)?,
            &temp_dir.join("audio.wav"),
            "audio",
            raw_length,
            observer,
        )?;
        ensure_frozen_hash(&audio_info, &raw_hash, "audio_source")?;
        files.push(manifest_file(
            "audio_reference",
            "audio.wav",
            &raw_uri,
            &audio_info,
            "audio/wav",
        ));

        let licenses = collect_license_records(
            &self.storage_service,
            &prepared.options.project_path,
            &prepared.options.expected_project_id,
            &prepared.source_documents,
            &prepared.adopted_artifacts,
        )?;
        let license_report = license_report(&licenses);
        let license_info = write_hashed(&temp_dir.join("LICENSES.txt"), license_report.as_bytes())?;
        files.push(manifest_file(
            "licenses",
            "LICENSES.txt",
            &format!("exports/{export_id}/LICENSES.txt"),
            &license_info,
            "text/plain",
        ));
        let checksums = build_checksums(
            temp_dir,
            &[
                "video.mp4",
                "audio.wav",
                "captions.json",
                "timeline.json",
                "LICENSES.txt",
            ],
        )?;
        let checksums_info = write_hashed(&temp_dir.join("SHA256SUMS"), checksums.as_bytes())?;
        files.push(manifest_file(
            "checksums",
            "SHA256SUMS",
            &format!("exports/{export_id}/SHA256SUMS"),
            &checksums_info,
            "text/plain",
        ));
        let render_result = &prepared.render_result;
        let media_entry = render_result
            .get("artifacts")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|v| {
                    v.get("artifactId").and_then(Value::as_str)
                        == Some(prepared.options.render_input.artifact_id.as_str())
                })
            })
            .ok_or_else(|| contract_error(operation, "RenderResult 缺少视频条目。"))?;
        let config = render_result
            .get("config")
            .ok_or_else(|| contract_error(operation, "RenderResult 缺少 config。"))?;
        let canvas = config
            .get("canvas")
            .ok_or_else(|| contract_error(operation, "Render config 缺少 canvas。"))?;
        let adopted = prepared
            .adopted_artifacts
            .iter()
            .map(|artifact| {
                let artifact_id = artifact
                    .get("artifactId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                json!({
                    "stageId": artifact.get("stageId"),
                    "runId": artifact.get("runId"),
                    "artifactId": artifact.get("artifactId"),
                    "kind": artifact.get("kind"),
                    "uri": artifact.get("uri"),
                    "contentHash": artifact.get("contentHash"),
                    "reviewRecordId": prepared.adopted_review_record_ids.get(artifact_id),
                })
            })
            .collect::<Vec<_>>();
        let provenance = prepared
            .adopted_artifacts
            .iter()
            .find(|artifact| {
                artifact.get("artifactId").and_then(Value::as_str)
                    == Some(prepared.options.render_input.artifact_id.as_str())
            })
            .and_then(|artifact| artifact.get("provenance"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let manifest = json!({
            "manifestVersion": EXPORT_MANIFEST_VERSION, "documentType": "export_manifest", "projectId": prepared.options.expected_project_id,
            "projectFormatVersion": CURRENT_PROJECT_FORMAT_VERSION, "exportId": export_id, "createdAt": now()?, "exportRunId": prepared.options.run_id,
            "renderRunId": prepared.options.render_input.run_id, "renderReviewRecordId": prepared.options.render_input.review_record_id,
            "adoptedArtifacts": adopted, "rendererIdentity": render_result.get("rendererIdentity"),
            "media": { "width": media_entry.get("width"), "height": media_entry.get("height"), "durationMs": media_entry.get("durationMs"), "frameRateNumerator": canvas.get("frameRateNumerator"), "frameRateDenominator": canvas.get("frameRateDenominator"), "videoCodec": config.get("videoCodec"), "audioCodec": config.get("audioCodec"), "pixelFormat": config.get("pixelFormat"), "hasAudio": media_entry.get("hasAudio") },
            "files": files, "provenance": provenance, "claimIds": prepared.options.render_input.claim_ids, "evidenceRefs": prepared.options.render_input.evidence_refs,
            "licenses": licenses, "qa": prepared.qa_result.get("qa"), "integrity": "complete",
        });
        validate_export_message(&manifest)
            .map_err(|_| contract_error(operation, "最终 Export Manifest 未通过 v1 契约。"))?;
        let manifest_bytes = pretty_json_bytes(&manifest)?;
        let manifest_info = write_hashed(&temp_dir.join("manifest.json"), &manifest_bytes)?;
        sync_directory(temp_dir)?;
        let created_at = now()?;
        let staged_video_path = temp_dir.join("video.mp4");
        let staged_manifest_path = temp_dir.join("manifest.json");
        let video_artifact_id = stable_id("artifact_", format!("{job_id}:video").as_bytes());
        let manifest_artifact_id = stable_id("artifact_", format!("{job_id}:manifest").as_bytes());
        let plan = vec![
            ArtifactCommitPlanEntryData {
                artifact_id: video_artifact_id.clone(),
                kind: "final_video".to_owned(),
            },
            ArtifactCommitPlanEntryData {
                artifact_id: manifest_artifact_id.clone(),
                kind: "render_manifest".to_owned(),
            },
        ];
        self.storage_service
            .begin_artifact_commit_journal(
                &prepared.options.project_path,
                &prepared.options.expected_project_id,
                job_id,
                &prepared.options.run_id,
                &created_at,
                &plan,
            )
            .map_err(map_storage_error)?;
        let source_ids = prepared
            .adopted_artifacts
            .iter()
            .filter_map(|v| v.get("artifactId").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let provenance = provenance.as_array().cloned().unwrap_or_default();
        let artifact_observer = ExportArtifactObserver { observer };
        let video_draft: ArtifactDraft = serde_json::from_value(json!({ "stageId": "export", "runId": prepared.options.run_id, "kind": "final_video", "mediaType": "video/mp4", "evidenceRole": "non_evidence", "source": { "origin": "derived", "sourceArtifactIds": source_ids }, "provenance": provenance })).map_err(|_| contract_error(operation, "final_video ArtifactDraft 无效。"))?;
        let video_commit = self
            .storage_service
            .import_artifact_file_idempotent_bounded_controlled(
                StoreArtifactFileOptions {
                    project_path: prepared.options.project_path.clone(),
                    expected_project_id: prepared.options.expected_project_id.clone(),
                    source_path: staged_video_path.to_string_lossy().into_owned(),
                    artifact: video_draft,
                },
                &video_artifact_id,
                &created_at,
                prepared.options.max_temporary_bytes,
                &artifact_observer,
            )
            .map_err(map_storage_error)?;
        let manifest_draft: ArtifactDraft = serde_json::from_value(json!({ "stageId": "export", "runId": prepared.options.run_id, "kind": "render_manifest", "mediaType": "application/json", "evidenceRole": "non_evidence", "source": { "origin": "derived", "sourceArtifactIds": source_ids }, "provenance": provenance })).map_err(|_| contract_error(operation, "render_manifest ArtifactDraft 无效。"))?;
        let manifest_commit = self
            .storage_service
            .import_artifact_file_idempotent_bounded_controlled(
                StoreArtifactFileOptions {
                    project_path: prepared.options.project_path.clone(),
                    expected_project_id: prepared.options.expected_project_id.clone(),
                    source_path: staged_manifest_path.to_string_lossy().into_owned(),
                    artifact: manifest_draft,
                },
                &manifest_artifact_id,
                &created_at,
                MAX_JSON_BYTES,
                &artifact_observer,
            )
            .map_err(map_storage_error)?;
        let artifact_ids = vec![
            string_field(&video_commit.artifact, "artifactId", operation)?,
            string_field(&manifest_commit.artifact, "artifactId", operation)?,
        ];
        if let Some(lease_id) = lease_id {
            if !marker_already_persisted {
                match observer.checkpoint(
                    "ready_to_finalize",
                    manifest_info.bytes,
                    manifest_info.bytes,
                ) {
                    Ok(()) => {}
                    Err(ExportTransferAbort::Canceled) => {
                        return Err(error(
                            ExportErrorCode::Canceled,
                            operation,
                            "导出在持久提交点前取消。",
                        ));
                    }
                    Err(ExportTransferAbort::LeaseLost) => {
                        return Err(error(
                            ExportErrorCode::Io,
                            operation,
                            "导出在持久提交点前失去 Job lease。",
                        ));
                    }
                }
            }
            let log_summary = json!({ "message": "QA 通过，最终导出已原子提交。", "warnings": [], "errors": [], "logArtifactId": manifest_artifact_id });
            if self
                .job_service
                .begin_job_completion(BeginJobCompletionOptions {
                    project_path: prepared.options.project_path.clone(),
                    expected_project_id: prepared.options.expected_project_id.clone(),
                    job_id: job_id.to_owned(),
                    lease_id: lease_id.to_owned(),
                    artifact_ids: artifact_ids.clone(),
                    log_summary,
                    finalization_mode: JobFinalizationModeData::ExternalCommit,
                })
                .is_err()
            {
                let cancellation_won = self
                    .job_service
                    .get_job(GetJobOptions {
                        project_path: prepared.options.project_path.clone(),
                        expected_project_id: prepared.options.expected_project_id.clone(),
                        job_id: job_id.to_owned(),
                    })
                    .ok()
                    .is_some_and(|snapshot| snapshot.cancellation_requested);
                return Err(error(
                    if cancellation_won {
                        ExportErrorCode::Canceled
                    } else {
                        ExportErrorCode::Io
                    },
                    operation,
                    if cancellation_won {
                        "取消请求先于持久提交点生效。"
                    } else {
                        "无法写入 Export Job 持久提交点。"
                    },
                ));
            }
        }
        fs::rename(temp_dir, final_dir)
            .map_err(|_| error(ExportErrorCode::Io, operation, "无法原子提交最终导出目录。"))?;
        sync_directory(final_dir.parent().unwrap_or(final_dir))?;
        let result = json!({ "apiVersion": EXPORT_COMMAND_API_VERSION, "operation": "get_export_result", "ownerProjectId": prepared.options.expected_project_id, "runId": prepared.options.run_id, "jobId": job_id, "status": "succeeded", "exportId": export_id, "exportPath": final_dir.to_string_lossy(), "manifest": manifest, "manifestHash": manifest_info.hash, "idempotentReplay": false });
        validate_export_message(&result)
            .map_err(|_| contract_error(operation, "ExportResult 未通过 v1 契约。"))?;
        write_result_atomic(project_root, job_id, &result)?;
        Ok(ExportCommitResultData {
            owner_project_id: prepared.options.expected_project_id.clone(),
            run_id: prepared.options.run_id.clone(),
            export_id: export_id.to_owned(),
            export_path: final_dir.to_string_lossy().into_owned(),
            artifact_ids,
            manifest,
            manifest_hash: manifest_info.hash,
            result: result.clone(),
            log_summary: json!({ "message": "QA 通过，最终导出已原子提交。", "warnings": [], "errors": [], "logArtifactId": manifest_artifact_id }),
            idempotent_replay: false,
        })
    }

    fn recover_published_export(
        &self,
        prepared: &PreparedExportData,
        job_id: &str,
        project_root: &Path,
        final_dir: &Path,
        export_id: &str,
    ) -> Result<Option<ExportCommitResultData>, ExportServiceError> {
        let manifest_artifact_id = stable_id("artifact_", format!("{job_id}:manifest").as_bytes());
        let video_artifact_id = stable_id("artifact_", format!("{job_id}:video").as_bytes());
        let Some((manifest_artifact, video_artifact)) = self
            .storage_service
            .get_artifact(&prepared.options.project_path, &manifest_artifact_id)
            .ok()
            .zip(
                self.storage_service
                    .get_artifact(&prepared.options.project_path, &video_artifact_id)
                    .ok(),
            )
        else {
            return Ok(None);
        };
        for artifact_id in [&manifest_artifact_id, &video_artifact_id] {
            let verification = self
                .storage_service
                .verify_artifact(&prepared.options.project_path, artifact_id)
                .map_err(map_storage_error)?;
            if verification.status != crate::ArtifactVerificationStatusData::Verified {
                return Ok(None);
            }
        }
        let manifest_bytes = read_bounded(&final_dir.join("manifest.json"), MAX_JSON_BYTES)?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| contract_error(ExportOperation::Commit, "已发布 manifest.json 无效。"))?;
        validate_export_message(&manifest).map_err(|_| {
            contract_error(
                ExportOperation::Commit,
                "已发布 manifest.json 未通过 Export v1 契约。",
            )
        })?;
        let manifest_hash = hash_bytes(&manifest_bytes);
        let identity_matches = manifest.get("projectId").and_then(Value::as_str)
            == Some(prepared.options.expected_project_id.as_str())
            && manifest.get("exportId").and_then(Value::as_str) == Some(export_id)
            && manifest.get("exportRunId").and_then(Value::as_str)
                == Some(prepared.options.run_id.as_str())
            && manifest_artifact
                .artifact
                .get("contentHash")
                .and_then(Value::as_str)
                == Some(manifest_hash.as_str());
        let (checked, diagnostics) = verify_manifest_files(final_dir, &manifest)?;
        let video_hash_matches = manifest
            .get("files")
            .and_then(Value::as_array)
            .and_then(|files| {
                files
                    .iter()
                    .find(|file| file.get("role").and_then(Value::as_str) == Some("video"))
            })
            .and_then(|file| file.get("contentHash").and_then(Value::as_str))
            == video_artifact
                .artifact
                .get("contentHash")
                .and_then(Value::as_str);
        if !identity_matches || !video_hash_matches || !diagnostics.is_empty() || checked == 0 {
            return Ok(None);
        }
        let result = json!({ "apiVersion": EXPORT_COMMAND_API_VERSION, "operation": "get_export_result", "ownerProjectId": prepared.options.expected_project_id, "runId": prepared.options.run_id, "jobId": job_id, "status": "succeeded", "exportId": export_id, "exportPath": final_dir.to_string_lossy(), "manifest": manifest, "manifestHash": manifest_hash, "idempotentReplay": true });
        validate_export_message(&result).map_err(|_| {
            contract_error(
                ExportOperation::Commit,
                "恢复的 ExportResult 未通过 v1 契约。",
            )
        })?;
        write_result_atomic(project_root, job_id, &result)?;
        commit_result_from_value(result, true).map(Some)
    }

    pub fn get_result(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
    ) -> Result<Value, ExportServiceError> {
        let descriptor = self
            .project_service
            .open_project(project_path)
            .map_err(map_project_error)?;
        if descriptor.project_id != project_id {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                ExportOperation::GetResult,
                "项目身份不匹配。",
            ));
        }
        let value = self.read_existing_result(Path::new(&descriptor.project_path), job_id)?;
        validate_export_message(&value).map_err(|_| {
            contract_error(ExportOperation::GetResult, "持久化 ExportResult 无效。")
        })?;
        Ok(value)
    }

    pub fn reconcile_succeeded_export_journals(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, ExportServiceError> {
        let jobs = self
            .job_service
            .list_jobs(ListJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                statuses: vec![
                    JobStatusData::Succeeded,
                    JobStatusData::Failed,
                    JobStatusData::Canceled,
                ],
                limit: 100,
            })
            .map_err(map_job_error)?;
        let mut reconciled = 0;
        for snapshot in jobs.jobs {
            if snapshot.job.get("stageId").and_then(Value::as_str) != Some("export")
                || snapshot
                    .job
                    .pointer("/executor/providerId")
                    .and_then(Value::as_str)
                    != Some("narracut_export")
            {
                continue;
            }
            let Some(job_id) = snapshot.job.get("jobId").and_then(Value::as_str) else {
                continue;
            };
            if snapshot.status == JobStatusData::Succeeded {
                if self.get_result(project_path, project_id, job_id).is_ok()
                    && self
                        .storage_service
                        .complete_artifact_commit_journal(project_path, project_id, job_id)
                        .is_ok()
                {
                    reconciled += 1;
                }
            } else if snapshot.finalization_mode.is_none() {
                let aborted = self
                    .storage_service
                    .abort_artifact_commit_journal(project_path, project_id, job_id)
                    .unwrap_or(0);
                if let Ok(receipt) =
                    self.job_service
                        .get_stage_job_request(GetStageJobRequestOptions {
                            project_path: project_path.to_owned(),
                            expected_project_id: project_id.to_owned(),
                            job_id: job_id.to_owned(),
                        })
                {
                    if let Ok(request) =
                        serde_json::from_value::<EnqueueExportOptions>(receipt.request)
                    {
                        if let Ok(destination) = canonical_existing_directory(
                            Path::new(&request.destination_directory),
                            ExportOperation::Commit,
                        ) {
                            let export_id = stable_id("export_", job_id.as_bytes());
                            let partial =
                                destination.join(format!(".narracut-{export_id}.partial"));
                            if partial.exists() {
                                let _ = fs::remove_dir_all(partial);
                            }
                        }
                    }
                }
                if aborted > 0 {
                    reconciled += 1;
                }
            }
        }
        Ok(reconciled)
    }

    pub fn verify_export(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        export_directory: &str,
    ) -> Result<Value, ExportServiceError> {
        let operation = ExportOperation::Verify;
        let descriptor = self
            .project_service
            .open_project(project_path)
            .map_err(map_project_error)?;
        if descriptor.project_id != project_id {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                operation,
                "项目身份不匹配。",
            ));
        }
        let trusted = self.read_existing_result(Path::new(&descriptor.project_path), job_id)?;
        validate_export_message(&trusted)
            .map_err(|_| contract_error(operation, "可信 ExportResult 无效。"))?;
        let root = canonical_existing_directory(Path::new(export_directory), operation)?;
        let trusted_root = canonical_existing_directory(
            Path::new(
                trusted
                    .get("exportPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            operation,
        )?;
        if root != trusted_root
            || trusted.get("ownerProjectId").and_then(Value::as_str) != Some(project_id)
            || trusted.get("jobId").and_then(Value::as_str) != Some(job_id)
        {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                operation,
                "待校验目录未绑定到当前项目 Job 的持久 ExportResult。",
            ));
        }
        let manifest_path = root.join("manifest.json");
        let manifest_bytes = read_bounded(&manifest_path, MAX_JSON_BYTES)?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| contract_error(operation, "manifest.json 不是合法 JSON。"))?;
        validate_export_message(&manifest)
            .map_err(|_| contract_error(operation, "manifest.json 未通过 Export v1 契约。"))?;
        let manifest_hash = hash_bytes(&manifest_bytes);
        let (checked, mut diagnostics) = verify_manifest_files(&root, &manifest)?;
        if trusted.get("manifestHash").and_then(Value::as_str) != Some(manifest_hash.as_str())
            || trusted.get("manifest") != Some(&manifest)
            || trusted.get("exportId") != manifest.get("exportId")
        {
            diagnostics.push(diagnostic(
                "verify_manifest_anchor_mismatch",
                "blocking",
                "hash_mismatch",
                "manifest.json 与项目内持久 ExportResult 可信锚不一致。",
                &[],
                &[],
            ));
        }
        let status = if diagnostics.is_empty() {
            "verified"
        } else {
            "corrupt"
        };
        let result = json!({ "apiVersion": EXPORT_COMMAND_API_VERSION, "operation": "verify_export", "status": status, "manifestHash": manifest_hash, "filesChecked": checked, "diagnostics": diagnostics });
        validate_export_message(&result)
            .map_err(|_| contract_error(operation, "ExportVerificationResult 无效。"))?;
        Ok(result)
    }

    fn load_context(
        &self,
        options: &RunExportQaOptions,
        current_renderer_identity: Option<&Value>,
        operation: ExportOperation,
    ) -> Result<QaContext, ExportServiceError> {
        let descriptor = self
            .project_service
            .open_project(&options.project_path)
            .map_err(map_project_error)?;
        if descriptor.project_id != options.expected_project_id {
            return Err(error(
                ExportErrorCode::ProjectMismatch,
                operation,
                "项目身份不匹配。",
            ));
        }
        if options.render_input.stage_id != "render" {
            return Err(error(
                ExportErrorCode::InvalidRequest,
                operation,
                "导出只接受 render 阶段输入。",
            ));
        }
        self.workflow_service
            .validate_approved_media_inputs(ValidateApprovedMediaInputsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                target_stage_id: "export".to_owned(),
                inputs: vec![ApprovedArtifactInputData {
                    ref_id: format!("ref_{}", options.render_input.artifact_id),
                    kind: "rendered_video".to_owned(),
                    artifact_id: options.render_input.artifact_id.clone(),
                    source_run_id: options.render_input.run_id.clone(),
                    review_record_id: options.render_input.review_record_id.clone(),
                    content_hash: options.render_input.content_hash.clone(),
                    claim_ids: options.render_input.claim_ids.clone(),
                    evidence_refs: options.render_input.evidence_refs.clone(),
                }],
            })
            .map_err(|_| {
                error(
                    ExportErrorCode::RenderNotApproved,
                    operation,
                    "Render 必须是当前项目中非 stale 的有效批准运行。",
                )
            })?;
        let history = self
            .workflow_service
            .list_stage_history(&options.project_path, "render", 100)
            .map_err(|_| {
                error(
                    ExportErrorCode::InvalidProject,
                    operation,
                    "无法读取 Render 历史。",
                )
            })?;
        let run = history
            .runs
            .iter()
            .find(|run| {
                run.get("runId").and_then(Value::as_str)
                    == Some(options.render_input.run_id.as_str())
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::RenderNotApproved,
                    operation,
                    "采用的 Render StageRun 不存在。",
                )
            })?;
        let review = history
            .reviews
            .iter()
            .find(|review| {
                review.get("reviewId").and_then(Value::as_str)
                    == Some(options.render_input.review_record_id.as_str())
                    && review.get("decision").and_then(Value::as_str) == Some("approved")
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::RenderNotApproved,
                    operation,
                    "采用的 Render ReviewRecord 不存在或未批准。",
                )
            })?;
        let run_artifact_ids = string_array(run, "artifactIds", operation)?;
        let review_artifact_ids = string_array(review, "artifactIds", operation)?;
        for required in [
            &options.render_input.artifact_id,
            &options.render_input.result_artifact_id,
        ] {
            if !run_artifact_ids.contains(required) || !review_artifact_ids.contains(required) {
                return Err(error(
                    ExportErrorCode::RenderNotApproved,
                    operation,
                    "批准记录必须同时覆盖 rendered_video 与 render_log。",
                ));
            }
        }
        let mut adopted_artifacts = Vec::new();
        let mut adopted_review_record_ids = BTreeMap::new();
        let mut verified_artifact_ids = Vec::new();
        for artifact_id in [
            &options.render_input.artifact_id,
            &options.render_input.result_artifact_id,
        ] {
            let read = self
                .storage_service
                .get_artifact(&options.project_path, artifact_id)
                .map_err(map_storage_error)?;
            let verification = self
                .storage_service
                .verify_artifact(&options.project_path, artifact_id)
                .map_err(map_storage_error)?;
            if verification.status != crate::ArtifactVerificationStatusData::Verified {
                return Err(error(
                    ExportErrorCode::HashMismatch,
                    operation,
                    "采用的 Render Artifact 哈希或字节数复验失败。",
                ));
            }
            verified_artifact_ids.push(artifact_id.clone());
            adopted_review_record_ids.insert(
                artifact_id.to_string(),
                options.render_input.review_record_id.clone(),
            );
            adopted_artifacts.push(read.artifact);
        }
        let result_bytes = self
            .storage_service
            .read_artifact_content_bounded(
                &options.project_path,
                &options.expected_project_id,
                &options.render_input.result_artifact_id,
                MAX_JSON_BYTES,
            )
            .map_err(map_storage_error)?;
        let render_result: Value = serde_json::from_slice(&result_bytes)
            .map_err(|_| contract_error(operation, "render_log 内容不是合法 JSON。"))?;
        validate_renderer_message(&render_result)
            .map_err(|_| contract_error(operation, "render_log 未通过 Renderer v1 契约。"))?;
        if render_result.get("status").and_then(Value::as_str) != Some("succeeded")
            || render_result.get("target").and_then(Value::as_str) != Some("timeline")
        {
            return Err(error(
                ExportErrorCode::InvalidRequest,
                operation,
                "最终导出只接受成功的全片 RenderResult。",
            ));
        }
        if current_renderer_identity.is_some()
            && render_result.get("rendererIdentity") != current_renderer_identity
        {
            return Err(error(
                ExportErrorCode::RendererIdentityChanged,
                operation,
                "当前 Renderer identity 与冻结 RenderResult 不一致。",
            ));
        }
        let video_metadata = adopted_artifacts
            .iter()
            .find(|v| {
                v.get("artifactId").and_then(Value::as_str)
                    == Some(options.render_input.artifact_id.as_str())
            })
            .ok_or_else(|| {
                error(
                    ExportErrorCode::ArtifactNotFound,
                    operation,
                    "rendered_video 元数据缺失。",
                )
            })?;
        if video_metadata.get("contentHash").and_then(Value::as_str)
            != Some(options.render_input.content_hash.as_str())
        {
            return Err(error(
                ExportErrorCode::HashMismatch,
                operation,
                "rendered_video 请求哈希已漂移。",
            ));
        }
        let source_ids = video_metadata
            .get("source")
            .and_then(|v| v.get("sourceArtifactIds"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                contract_error(operation, "rendered_video 缺少冻结 sourceArtifactIds。")
            })?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut source_documents = Vec::new();
        let mut documents_by_type = BTreeMap::new();
        let mut licenses = Vec::new();
        for artifact_id in &source_ids {
            let read = self
                .storage_service
                .get_artifact(&options.project_path, artifact_id)
                .map_err(map_storage_error)?;
            let verification = self
                .storage_service
                .verify_artifact(&options.project_path, artifact_id)
                .map_err(map_storage_error)?;
            if verification.status != crate::ArtifactVerificationStatusData::Verified {
                return Err(error(
                    ExportErrorCode::HashMismatch,
                    operation,
                    "Render 上游 Artifact 哈希或字节数复验失败。",
                ));
            }
            verified_artifact_ids.push(artifact_id.to_owned());
            if read
                .artifact
                .get("mediaType")
                .and_then(Value::as_str)
                .is_some_and(|media_type| {
                    media_type == "application/json" || media_type.ends_with("+json")
                })
            {
                let bytes = self
                    .storage_service
                    .read_artifact_content_bounded(
                        &options.project_path,
                        &options.expected_project_id,
                        artifact_id,
                        MAX_JSON_BYTES,
                    )
                    .map_err(map_storage_error)?;
                if let Ok(document) = serde_json::from_slice::<Value>(&bytes) {
                    if validate_media_document(&document).is_ok() {
                        let review_id = self.approved_review_record_id(
                            &options.project_path,
                            &read.artifact,
                            operation,
                        )?;
                        adopted_review_record_ids.insert(artifact_id.to_owned(), review_id);
                        adopted_artifacts.push(read.artifact.clone());
                        if let Some(document_type) =
                            document.get("documentType").and_then(Value::as_str)
                        {
                            documents_by_type.insert(document_type.to_owned(), document.clone());
                        }
                        if let Some(rights) = document.get("rights") {
                            licenses.push(rights.clone());
                        }
                        source_documents.push((read.artifact, document));
                    }
                }
            }
        }
        let nested_source_ids = source_documents
            .iter()
            .flat_map(|(metadata, _)| {
                metadata
                    .pointer("/source/sourceArtifactIds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeSet<_>>();
        for artifact_id in nested_source_ids {
            if adopted_review_record_ids.contains_key(&artifact_id) {
                continue;
            }
            let read = self
                .storage_service
                .get_artifact(&options.project_path, &artifact_id)
                .map_err(map_storage_error)?;
            if !read
                .artifact
                .pointer("/source/authorizationRecordIds")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty())
            {
                continue;
            }
            let verification = self
                .storage_service
                .verify_artifact(&options.project_path, &artifact_id)
                .map_err(map_storage_error)?;
            if verification.status != crate::ArtifactVerificationStatusData::Verified {
                return Err(error(
                    ExportErrorCode::HashMismatch,
                    operation,
                    "Media 文档引用的源 Artifact 哈希或字节数复验失败。",
                ));
            }
            let review_id = source_documents
                .iter()
                .find(|(metadata, _)| {
                    metadata
                        .pointer("/source/sourceArtifactIds")
                        .and_then(Value::as_array)
                        .is_some_and(|ids| {
                            ids.iter()
                                .any(|id| id.as_str() == Some(artifact_id.as_str()))
                        })
                })
                .and_then(|(metadata, _)| metadata.get("artifactId").and_then(Value::as_str))
                .and_then(|document_id| adopted_review_record_ids.get(document_id))
                .cloned()
                .ok_or_else(|| {
                    contract_error(operation, "源 Artifact 未闭合到已审核的 Media 文档。")
                })?;
            verified_artifact_ids.push(artifact_id.clone());
            adopted_review_record_ids.insert(artifact_id, review_id);
            adopted_artifacts.push(read.artifact);
        }
        let timeline_artifact_id = render_result
            .pointer("/timelineInput/artifactId")
            .and_then(Value::as_str)
            .ok_or_else(|| contract_error(operation, "RenderResult 缺少 Timeline input。"))?;
        let timeline_document = source_documents
            .iter()
            .find(|(metadata, document)| {
                metadata.get("artifactId").and_then(Value::as_str) == Some(timeline_artifact_id)
                    && document.get("documentType").and_then(Value::as_str) == Some("timeline")
            })
            .map(|(_, document)| document.clone())
            .ok_or_else(|| contract_error(operation, "Render 上游未闭合到 Timeline 文档。"))?;
        verified_artifact_ids.sort();
        verified_artifact_ids.dedup();
        Ok(QaContext {
            render_result,
            timeline_document,
            adopted_artifacts,
            adopted_review_record_ids,
            verified_artifact_ids,
            source_documents,
            documents_by_type,
            licenses,
        })
    }

    fn approved_review_record_id(
        &self,
        project_path: &str,
        artifact: &Value,
        operation: ExportOperation,
    ) -> Result<String, ExportServiceError> {
        let stage_id = string_field(artifact, "stageId", operation)?;
        let run_id = string_field(artifact, "runId", operation)?;
        let artifact_id = string_field(artifact, "artifactId", operation)?;
        let history = self
            .workflow_service
            .list_stage_history(project_path, &stage_id, 100)
            .map_err(|_| {
                error(
                    ExportErrorCode::RenderNotApproved,
                    operation,
                    "无法读取源 Artifact 审核历史。",
                )
            })?;
        history
            .reviews
            .iter()
            .find(|review| {
                review.get("runId").and_then(Value::as_str) == Some(run_id.as_str())
                    && review.get("decision").and_then(Value::as_str) == Some("approved")
                    && review
                        .get("artifactIds")
                        .and_then(Value::as_array)
                        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&artifact_id)))
            })
            .and_then(|review| review.get("reviewId").and_then(Value::as_str))
            .map(str::to_owned)
            .ok_or_else(|| {
                let message =
                    format!("源 Artifact {artifact_id} 缺少覆盖其内容的 approved ReviewRecord。");
                error(ExportErrorCode::RenderNotApproved, operation, &message)
            })
    }

    fn read_existing_result(
        &self,
        project_root: &Path,
        job_id: &str,
    ) -> Result<Value, ExportServiceError> {
        let path = project_root
            .join("exports")
            .join("results")
            .join(format!("{job_id}.json"));
        let bytes = read_bounded(&path, MAX_JSON_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|_| {
            contract_error(
                ExportOperation::GetResult,
                "持久化 ExportResult 不是合法 JSON。",
            )
        })
    }
}

struct QaContext {
    render_result: Value,
    timeline_document: Value,
    adopted_artifacts: Vec<Value>,
    adopted_review_record_ids: BTreeMap<String, String>,
    verified_artifact_ids: Vec<String>,
    source_documents: Vec<(Value, Value)>,
    documents_by_type: BTreeMap<String, Value>,
    licenses: Vec<Value>,
}

struct ExportCommitPaths<'a> {
    project_root: &'a Path,
    temp_dir: &'a Path,
    final_dir: &'a Path,
    export_id: &'a str,
}

struct FileInfo {
    hash: String,
    bytes: u64,
}

struct ExportArtifactObserver<'a> {
    observer: &'a dyn ExportTransferObserver,
}
impl ArtifactTransferObserver for ExportArtifactObserver<'_> {
    fn checkpoint(
        &self,
        artifact_id: &str,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), crate::ArtifactTransferAbort> {
        self.observer
            .checkpoint(artifact_id, completed_bytes, total_bytes)
            .map_err(|abort| match abort {
                ExportTransferAbort::Canceled => crate::ArtifactTransferAbort::Canceled,
                ExportTransferAbort::LeaseLost => crate::ArtifactTransferAbort::LeaseLost,
            })
    }
}

fn push_check(
    checks: &mut Vec<Value>,
    diagnostics: &mut Vec<Value>,
    identity: (&str, &str),
    outcome: (bool, bool),
    messages: (&str, &str),
    references: (&[&str], &[&str]),
) {
    let (check_id, category) = identity;
    let (passed, warning_only) = outcome;
    let (pass_message, fail_message) = messages;
    let (scene_ids, artifact_ids) = references;
    let status = if passed {
        "passed"
    } else if warning_only {
        "warning"
    } else {
        "blocked"
    };
    checks.push(json!({ "checkId": check_id, "category": category, "status": status, "message": if passed { pass_message } else { fail_message }, "sceneIds": scene_ids, "artifactIds": artifact_ids }));
    if !passed {
        diagnostics.push(diagnostic(
            &format!("diag_{check_id}"),
            if warning_only { "warning" } else { "blocking" },
            check_id,
            fail_message,
            scene_ids,
            artifact_ids,
        ));
    }
}

fn diagnostic(
    id: &str,
    severity: &str,
    code: &str,
    message: &str,
    scene_ids: &[&str],
    artifact_ids: &[&str],
) -> Value {
    json!({ "diagnosticId": id, "severity": severity, "code": code, "message": message, "sceneIds": scene_ids, "artifactIds": artifact_ids })
}

fn scenes_are_contiguous(timeline: &Value) -> bool {
    let Some(scenes) = timeline.get("sceneTrack").and_then(Value::as_array) else {
        return false;
    };
    !scenes.is_empty()
        && scenes.iter().enumerate().all(|(index, scene)| {
            let start = scene.get("startMs").and_then(Value::as_u64);
            let end = scene.get("endMs").and_then(Value::as_u64);
            start.is_some()
                && end.is_some_and(|e| Some(e) > start)
                && (index == 0 || scenes[index - 1].get("endMs") == scene.get("startMs"))
        })
        && scenes
            .first()
            .and_then(|s| s.get("startMs"))
            .and_then(Value::as_u64)
            == Some(0)
        && scenes.last().and_then(|s| s.get("endMs")) == timeline.get("durationMs")
}

fn captions_in_range(document: &Value, duration_ms: u64) -> bool {
    let Some(cues) = document.get("cues").and_then(Value::as_array) else {
        return false;
    };
    !cues.is_empty()
        && cues.iter().all(|cue| {
            let start = cue.get("startMs").and_then(Value::as_u64);
            let end = cue.get("endMs").and_then(Value::as_u64);
            start.is_some() && end.is_some_and(|e| Some(e) > start && e <= duration_ms)
        })
}

fn safe_area_valid(timeline: &Value) -> bool {
    let Some(canvas) = timeline.get("canvas") else {
        return false;
    };
    let Some(area) = timeline.get("safeArea") else {
        return false;
    };
    let (Some(cw), Some(ch), Some(x), Some(y), Some(w), Some(h)) = (
        canvas.get("width").and_then(Value::as_u64),
        canvas.get("height").and_then(Value::as_u64),
        area.get("x").and_then(Value::as_u64),
        area.get("y").and_then(Value::as_u64),
        area.get("width").and_then(Value::as_u64),
        area.get("height").and_then(Value::as_u64),
    ) else {
        return false;
    };
    x + w <= cw && y + h <= ch && w * 100 >= cw * 50 && h * 100 >= ch * 50
}

fn caption_text_fits(document: &Value) -> bool {
    let Some(cues) = document.get("cues").and_then(Value::as_array) else {
        return false;
    };
    let mut previous_end = 0;
    cues.iter().all(|cue| {
        let start = cue.get("startMs").and_then(Value::as_u64).unwrap_or(0);
        let end = cue.get("endMs").and_then(Value::as_u64).unwrap_or(0);
        let text = cue.get("text").and_then(Value::as_str).unwrap_or("");
        let lines = text.lines().collect::<Vec<_>>();
        let ok = start >= previous_end
            && end > start
            && !text.chars().any(|c| c.is_control() && c != '\n')
            && !lines.is_empty()
            && lines.len() <= MAX_CAPTION_LINES
            && lines
                .iter()
                .all(|line| line.chars().count() <= MAX_CAPTION_CHARS_PER_LINE);
        previous_end = end;
        ok
    })
}

fn provenance_closed(
    input: &ExportRenderInputData,
    video_entry: &Value,
    timeline: &Value,
    captions: Option<&Value>,
) -> bool {
    let allowed_claims = input.claim_ids.iter().collect::<BTreeSet<_>>();
    let allowed_evidence = input.evidence_refs.iter().collect::<BTreeSet<_>>();
    let mut pairs = Vec::new();
    if let Some(values) = video_entry.get("provenance").and_then(Value::as_array) {
        pairs.extend(values.iter());
    }
    if let Some(values) = timeline.get("provenance").and_then(Value::as_array) {
        pairs.extend(values.iter());
    }
    if let Some(captions) = captions {
        if let Some(cues) = captions.get("cues").and_then(Value::as_array) {
            for cue in cues {
                if let Some(values) = cue.get("provenance").and_then(Value::as_array) {
                    pairs.extend(values.iter());
                }
            }
        }
    }
    !pairs.is_empty()
        && pairs.into_iter().all(|pair| {
            pair.get("claimId")
                .and_then(Value::as_str)
                .is_some_and(|id| allowed_claims.contains(&id.to_owned()))
                && pair
                    .get("evidenceRef")
                    .and_then(Value::as_str)
                    .is_some_and(|id| allowed_evidence.contains(&id.to_owned()))
        })
}

fn license_complete(rights: &Value) -> bool {
    ["author", "rightsStatement", "licenseId"]
        .iter()
        .all(|key| {
            rights
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
        })
        && rights
            .pointer("/voiceAuthorization/applicability")
            .and_then(Value::as_str)
            == Some("not_applicable")
        && rights
            .pointer("/voiceAuthorization/reason")
            .and_then(Value::as_str)
            == Some("not_voice_clone")
}

fn collect_license_records(
    storage: &StorageService,
    project_path: &str,
    project_id: &str,
    documents: &[(Value, Value)],
    adopted_artifacts: &[Value],
) -> Result<Vec<Value>, ExportServiceError> {
    let mut records = Vec::new();
    for (metadata, document) in documents {
        let Some(rights) = document.get("rights") else {
            continue;
        };
        if !license_complete(rights) {
            return Err(error(
                ExportErrorCode::RightsIncomplete,
                ExportOperation::Commit,
                "素材许可或声音授权不完整。",
            ));
        }
        let media_document_artifact_id =
            string_field(metadata, "artifactId", ExportOperation::Commit)?;
        let source_ids = metadata
            .pointer("/source/sourceArtifactIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                contract_error(ExportOperation::Commit, "Media 文档缺少源 Artifact 闭包。")
            })?;
        let before = records.len();
        for source_id in source_ids.iter().filter_map(Value::as_str) {
            let Some(source) = adopted_artifacts.iter().find(|artifact| {
                artifact.get("artifactId").and_then(Value::as_str) == Some(source_id)
            }) else {
                continue;
            };
            let authorization_record_ids = source
                .pointer("/source/authorizationRecordIds")
                .and_then(Value::as_array)
                .filter(|ids| !ids.is_empty())
                .cloned()
                .unwrap_or_default();
            let source_hash = source
                .get("contentHash")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if authorization_record_ids.is_empty() || source_hash.is_empty() {
                continue;
            }
            let mut resolved_ids = Vec::with_capacity(authorization_record_ids.len());
            for authorization_record_id in authorization_record_ids.iter().filter_map(Value::as_str)
            {
                let record = storage
                    .get_authorization_record(project_path, project_id, authorization_record_id)
                    .map_err(|_| {
                        error(
                            ExportErrorCode::RightsIncomplete,
                            ExportOperation::Commit,
                            "源 Artifact 引用的 AuthorizationRecord 不可解析。",
                        )
                    })?;
                if record.source_content_hash != source_hash
                    || record.status != "granted"
                    || record.authorization_type != "material_use"
                {
                    return Err(error(
                        ExportErrorCode::RightsIncomplete,
                        ExportOperation::Commit,
                        "AuthorizationRecord 未授权当前源内容或状态无效。",
                    ));
                }
                resolved_ids.push(Value::String(record.authorization_record_id));
            }
            if resolved_ids.len() != authorization_record_ids.len() {
                return Err(error(
                    ExportErrorCode::RightsIncomplete,
                    ExportOperation::Commit,
                    "AuthorizationRecord ID 必须是非空可解析字符串。",
                ));
            }
            records.push(json!({
                "artifactId": source.get("artifactId"),
                "mediaDocumentArtifactId": media_document_artifact_id,
                "sourceUri": source.get("uri"),
                "contentHash": source.get("contentHash"),
                "sourceFileName": document.pointer("/source/sourceFileName"),
                "author": rights.get("author"),
                "licenseId": rights.get("licenseId"),
                "rightsStatement": rights.get("rightsStatement"),
                "attributionText": rights.get("attributionText"),
                "authorizationRecordIds": resolved_ids,
            }));
        }
        if records.len() == before {
            return Err(error(
                ExportErrorCode::RightsIncomplete,
                ExportOperation::Commit,
                "Media 文档未闭合到带真实授权记录 ID 的源 Artifact。",
            ));
        }
    }
    records.sort_by_key(|v| {
        v.get("artifactId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    });
    records.dedup();
    if records.is_empty() {
        return Err(error(
            ExportErrorCode::RightsIncomplete,
            ExportOperation::Commit,
            "导出没有任何许可记录。",
        ));
    }
    Ok(records)
}

fn license_report(records: &[Value]) -> String {
    let mut output = String::from("NarraCut Alpha 导出素材许可与署名\n\n");
    for record in records {
        let authorizations = record
            .get("authorizationRecordIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!("Artifact: {}\nMedia document: {}\nSource URI: {}\nContent hash: {}\nAuthor: {}\nLicense: {}\nAttribution: {}\nRights: {}\nAuthorization records: {}\n\n", record.get("artifactId").and_then(Value::as_str).unwrap_or(""), record.get("mediaDocumentArtifactId").and_then(Value::as_str).unwrap_or(""), record.get("sourceUri").and_then(Value::as_str).unwrap_or(""), record.get("contentHash").and_then(Value::as_str).unwrap_or(""), record.get("author").and_then(Value::as_str).unwrap_or(""), record.get("licenseId").and_then(Value::as_str).unwrap_or(""), record.get("attributionText").and_then(Value::as_str).unwrap_or(""), record.get("rightsStatement").and_then(Value::as_str).unwrap_or(""), authorizations));
    }
    output
}

fn manifest_file(
    role: &str,
    relative_path: &str,
    source_uri: &str,
    info: &FileInfo,
    media_type: &str,
) -> Value {
    json!({ "role": role, "relativePath": relative_path, "sourceUri": source_uri, "contentHash": info.hash, "byteLength": info.bytes, "mediaType": media_type })
}

fn copy_hashed(
    source: &Path,
    destination: &Path,
    phase: &str,
    total: u64,
    observer: &dyn ExportTransferObserver,
) -> Result<FileInfo, ExportServiceError> {
    let mut reader = BufReader::new(File::open(source).map_err(|_| {
        error(
            ExportErrorCode::ArtifactNotFound,
            ExportOperation::Commit,
            "导出源 Artifact 内容不存在。",
        )
    })?);
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|_| {
                error(
                    ExportErrorCode::Io,
                    ExportOperation::Commit,
                    "无法创建导出文件。",
                )
            })?,
    );
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "读取导出源文件失败。",
            )
        })?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "写入导出文件失败。",
            )
        })?;
        hasher.update(&buffer[..read]);
        copied += read as u64;
        observer.checkpoint(phase, copied, total).map_err(|abort| {
            error(
                if abort == ExportTransferAbort::Canceled {
                    ExportErrorCode::Canceled
                } else {
                    ExportErrorCode::Io
                },
                ExportOperation::Commit,
                if abort == ExportTransferAbort::Canceled {
                    "导出已取消。"
                } else {
                    "导出 worker 租约已丢失。"
                },
            )
        })?;
    }
    writer
        .flush()
        .and_then(|_| writer.get_ref().sync_all())
        .map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "同步导出文件失败。",
            )
        })?;
    if copied != total {
        return Err(error(
            ExportErrorCode::HashMismatch,
            ExportOperation::Commit,
            "导出源 Artifact 字节数已漂移。",
        ));
    }
    Ok(FileInfo {
        hash: format_sha256(&hasher.finalize()),
        bytes: copied,
    })
}

fn ensure_frozen_hash(
    copied: &FileInfo,
    expected_hash: &str,
    source_kind: &str,
) -> Result<(), ExportServiceError> {
    if copied.hash != expected_hash {
        let message = format!("{source_kind} 内容在 prepare 与 commit 之间发生漂移。");
        return Err(error(
            ExportErrorCode::HashMismatch,
            ExportOperation::Commit,
            &message,
        ));
    }
    Ok(())
}

fn write_hashed(path: &Path, bytes: &[u8]) -> Result<FileInfo, ExportServiceError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "无法创建导出元数据文件。",
            )
        })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "无法写入导出元数据文件。",
            )
        })?;
    Ok(FileInfo {
        hash: hash_bytes(bytes),
        bytes: bytes.len() as u64,
    })
}

fn build_checksums(root: &Path, names: &[&str]) -> Result<String, ExportServiceError> {
    if names.len() > MAX_EXPORT_FILES {
        return Err(contract_error(
            ExportOperation::Commit,
            "导出文件数超过上限。",
        ));
    }
    let mut output = String::new();
    for name in names {
        let (hash, _) = hash_file(&root.join(name))?;
        output.push_str(hash.strip_prefix("sha256:").unwrap_or(&hash));
        output.push_str("  ");
        output.push_str(name);
        output.push('\n');
    }
    Ok(output)
}

fn write_result_atomic(
    project_root: &Path,
    job_id: &str,
    value: &Value,
) -> Result<(), ExportServiceError> {
    let dir = project_root.join("exports").join("results");
    fs::create_dir_all(&dir).map_err(|_| {
        error(
            ExportErrorCode::Io,
            ExportOperation::Commit,
            "无法创建 ExportResult 目录。",
        )
    })?;
    let destination = dir.join(format!("{job_id}.json"));
    if destination.exists() {
        return Ok(());
    }
    let mut temporary = NamedTempFile::new_in(&dir).map_err(|_| {
        error(
            ExportErrorCode::Io,
            ExportOperation::Commit,
            "无法创建 ExportResult 临时文件。",
        )
    })?;
    let bytes = pretty_json_bytes(value)?;
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "无法写入 ExportResult。",
            )
        })?;
    temporary.persist_noclobber(destination).map_err(|_| {
        error(
            ExportErrorCode::DestinationConflict,
            ExportOperation::Commit,
            "ExportResult 幂等冲突。",
        )
    })?;
    Ok(())
}

fn commit_result_from_value(
    result: Value,
    replay: bool,
) -> Result<ExportCommitResultData, ExportServiceError> {
    let manifest = result.get("manifest").cloned().ok_or_else(|| {
        contract_error(ExportOperation::Commit, "已有 ExportResult 缺少 manifest。")
    })?;
    let job_id = string_field(&result, "jobId", ExportOperation::Commit)?;
    let artifact_ids = vec![
        stable_id("artifact_", format!("{job_id}:video").as_bytes()),
        stable_id("artifact_", format!("{job_id}:manifest").as_bytes()),
    ];
    let log_artifact_id = artifact_ids[1].clone();
    Ok(ExportCommitResultData {
        owner_project_id: string_field(&result, "ownerProjectId", ExportOperation::Commit)?,
        run_id: string_field(&result, "runId", ExportOperation::Commit)?,
        export_id: string_field(&result, "exportId", ExportOperation::Commit)?,
        export_path: string_field(&result, "exportPath", ExportOperation::Commit)?,
        artifact_ids,
        manifest,
        manifest_hash: string_field(&result, "manifestHash", ExportOperation::Commit)?,
        result,
        log_summary: json!({"message":"幂等复用已完成导出。","warnings":[],"errors":[],"logArtifactId":log_artifact_id}),
        idempotent_replay: replay,
    })
}

fn validate_enqueue_shape(options: &EnqueueExportOptions) -> Result<(), ExportServiceError> {
    if !valid_component(&options.export_name)
        || !options.run_id.starts_with("run_")
        || options.max_temporary_bytes < 1024 * 1024
    {
        return Err(error(
            ExportErrorCode::InvalidRequest,
            ExportOperation::Enqueue,
            "导出名称、runId 或资源上限无效。",
        ));
    }
    Ok(())
}
fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphanumeric() || (i > 0 && matches!(b, b'.' | b'_' | b'-')))
        && value != "."
        && value != ".."
}
fn canonical_existing_directory(
    path: &Path,
    operation: ExportOperation,
) -> Result<PathBuf, ExportServiceError> {
    let canonical = fs::canonicalize(path).map_err(|_| {
        error(
            ExportErrorCode::DestinationInvalid,
            operation,
            "目录不存在或无法访问。",
        )
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        error(
            ExportErrorCode::DestinationInvalid,
            operation,
            "无法读取目录元数据。",
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error(
            ExportErrorCode::DestinationInvalid,
            operation,
            "目标必须是非符号链接目录。",
        ));
    }
    Ok(canonical)
}
fn safe_project_uri(
    root: &Path,
    uri: &str,
    operation: ExportOperation,
) -> Result<PathBuf, ExportServiceError> {
    if uri.contains('\\')
        || uri
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(error(
            ExportErrorCode::InvalidProject,
            operation,
            "Artifact URI 不可迁移或试图逃逸项目。",
        ));
    }
    let path = root.join(uri);
    if !path.starts_with(root) {
        return Err(error(
            ExportErrorCode::InvalidProject,
            operation,
            "Artifact URI 逃逸项目根目录。",
        ));
    }
    Ok(path)
}
fn safe_relative_path(
    root: &Path,
    relative: &str,
    operation: ExportOperation,
) -> Result<PathBuf, ExportServiceError> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(
            ExportErrorCode::InvalidRequest,
            operation,
            "Manifest 包含不安全相对路径。",
        ));
    }
    Ok(root.join(path))
}

fn verify_manifest_files(
    root: &Path,
    manifest: &Value,
) -> Result<(u64, Vec<Value>), ExportServiceError> {
    let operation = ExportOperation::Verify;
    let mut diagnostics = Vec::new();
    let mut checked = 0_u64;
    for file in manifest
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let relative = string_field(file, "relativePath", operation)?;
        let expected = string_field(file, "contentHash", operation)?;
        let path = safe_relative_path(root, &relative, operation)?;
        match hash_file(&path) {
            Ok((actual, _)) if actual == expected => checked += 1,
            _ => diagnostics.push(diagnostic(
                "verify_hash_mismatch",
                "blocking",
                "hash_mismatch",
                "导出文件缺失或 SHA-256 不匹配。",
                &[],
                &[],
            )),
        }
    }
    Ok((checked, diagnostics))
}
fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, ExportServiceError> {
    let metadata = fs::metadata(path).map_err(|_| {
        error(
            ExportErrorCode::ArtifactNotFound,
            ExportOperation::Verify,
            "文件不存在。",
        )
    })?;
    if metadata.len() > max {
        return Err(error(
            ExportErrorCode::InvalidRequest,
            ExportOperation::Verify,
            "文件超过读取上限。",
        ));
    }
    fs::read(path).map_err(|_| {
        error(
            ExportErrorCode::Io,
            ExportOperation::Verify,
            "读取文件失败。",
        )
    })
}
fn hash_file(path: &Path) -> Result<(String, u64), ExportServiceError> {
    let mut file = File::open(path).map_err(|_| {
        error(
            ExportErrorCode::ArtifactNotFound,
            ExportOperation::Verify,
            "文件不存在。",
        )
    })?;
    let mut hasher = Sha256::new();
    let mut total = 0;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    loop {
        let count = file.read(&mut buffer).map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Verify,
                "读取文件失败。",
            )
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        total += count as u64;
    }
    Ok((format_sha256(&hasher.finalize()), total))
}
fn hash_bytes(bytes: &[u8]) -> String {
    format_sha256(&Sha256::digest(bytes))
}
fn hash_json(value: &Value) -> Result<String, ExportServiceError> {
    serde_json::to_vec(&canonicalize(value))
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|_| contract_error(ExportOperation::RunQa, "无法计算 QA 身份。"))
}
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}
fn stable_id(prefix: &str, seed: &[u8]) -> String {
    format!("{prefix}{}", hex(&Sha256::digest(seed)))
}
fn format_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(bytes))
}
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
fn pretty_json_bytes(value: &Value) -> Result<Vec<u8>, ExportServiceError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| contract_error(ExportOperation::Commit, "无法序列化导出 JSON。"))?;
    bytes.push(b'\n');
    Ok(bytes)
}
#[cfg(not(windows))]
fn sync_directory(path: &Path) -> Result<(), ExportServiceError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| {
            error(
                ExportErrorCode::Io,
                ExportOperation::Commit,
                "无法同步导出目录。",
            )
        })
}
#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), ExportServiceError> {
    Ok(())
}
fn now() -> Result<String, ExportServiceError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| contract_error(ExportOperation::Commit, "无法生成时间戳。"))
}
fn string_field(
    value: &Value,
    key: &str,
    operation: ExportOperation,
) -> Result<String, ExportServiceError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| contract_error(operation, &format!("缺少 {key}。")))
}
fn string_array(
    value: &Value,
    key: &str,
    operation: ExportOperation,
) -> Result<Vec<String>, ExportServiceError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| contract_error(operation, &format!("缺少 {key}。")))
}
fn u64_field(
    value: &Value,
    key: &str,
    operation: ExportOperation,
) -> Result<u64, ExportServiceError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| contract_error(operation, &format!("缺少 {key}。")))
}
fn error(code: ExportErrorCode, operation: ExportOperation, message: &str) -> ExportServiceError {
    ExportServiceError::new(code, operation, message)
}
fn contract_error(operation: ExportOperation, message: &str) -> ExportServiceError {
    error(ExportErrorCode::InternalContract, operation, message)
}
fn map_project_error(_: crate::ProjectServiceError) -> ExportServiceError {
    error(
        ExportErrorCode::InvalidProject,
        ExportOperation::Prepare,
        "项目不可用、需要迁移或格式受损。",
    )
}
fn map_storage_error(error_value: crate::StorageServiceError) -> ExportServiceError {
    let code = match error_value.code {
        crate::StorageErrorCode::ArtifactNotFound => ExportErrorCode::ArtifactNotFound,
        crate::StorageErrorCode::ContentCorrupt => ExportErrorCode::HashMismatch,
        crate::StorageErrorCode::OperationCanceled => ExportErrorCode::Canceled,
        crate::StorageErrorCode::LeaseLost => ExportErrorCode::Io,
        _ => ExportErrorCode::InvalidProject,
    };
    error(
        code,
        ExportOperation::Commit,
        "Artifact Store 校验或提交失败。",
    )
}
fn map_job_error(_: crate::JobServiceError) -> ExportServiceError {
    error(
        ExportErrorCode::InvalidProject,
        ExportOperation::Enqueue,
        "Export Job 无法创建或发生幂等冲突。",
    )
}
