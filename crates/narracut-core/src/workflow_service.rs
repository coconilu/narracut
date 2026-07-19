use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use narracut_contracts::{validate_contract_document, NARRACUT_CONTRACT_VERSION};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    AffectedStageData, ArtifactReadResultData, InitializeWorkflowOptions, PrepareStageRunOptions,
    ProjectDescriptorData, ProjectErrorCode, ProjectOperation, ProjectService, ProjectServiceError,
    RecordStageRunOptions, RegenerationImpactResultData, ReviewDecisionData, ReviewStageRunOptions,
    StageConfigUpdateResultData, StageHistoryResultData, StageReviewResultData,
    StageRunCommitResultData, StageRunPreparationResultData, StageStateData, StageStatusData,
    StorageErrorCode, StorageService, StorageServiceError, TerminalRunStatusData,
    UpdateStageConfigOptions, ValidateApprovedMediaInputsOptions, WorkflowErrorCode,
    WorkflowOperation, WorkflowServiceError, WorkflowSnapshotData, WORKFLOW_COMMAND_API_VERSION,
};

const STANDARD_WORKFLOW_ID: &str = "workflow_standard_v1";
const MAX_WORKFLOW_STAGES: usize = 64;
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STAGE_RUNS: usize = 1024;
const MAX_STAGE_REVIEWS: usize = 1024;

#[derive(Clone, Copy)]
struct BuiltinStageSpec {
    stage_id: &'static str,
    title: &'static str,
    description: &'static str,
    dependencies: &'static [&'static str],
    input_kinds: &'static [&'static str],
    output_kinds: &'static [&'static str],
    requires_approved_inputs: bool,
    supports_partial_regeneration: bool,
}

const STANDARD_STAGES: &[BuiltinStageSpec] = &[
    BuiltinStageSpec {
        stage_id: "brief",
        title: "创作简报",
        description: "明确受众、目标、边界与叙事方向。",
        dependencies: &[],
        input_kinds: &["source_material"],
        output_kinds: &["brief"],
        requires_approved_inputs: false,
        supports_partial_regeneration: false,
    },
    BuiltinStageSpec {
        stage_id: "research",
        title: "事实研究",
        description: "整理主张、证据、反证与来源追溯。",
        dependencies: &["brief"],
        input_kinds: &["brief", "source_material"],
        output_kinds: &["claim_set", "evidence_set"],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "script",
        title: "事实脚本",
        description: "把已审核的简报和证据组织为可追溯口播稿。",
        dependencies: &["research"],
        input_kinds: &["brief", "claim_set", "evidence_set"],
        output_kinds: &["script"],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "audio",
        title: "口播音频",
        description: "根据已审核脚本生成或导入口播音频。",
        dependencies: &["script"],
        input_kinds: &["script"],
        output_kinds: &["audio_source", "voice_audio"],
        requires_approved_inputs: true,
        supports_partial_regeneration: false,
    },
    BuiltinStageSpec {
        stage_id: "captions",
        title: "字幕",
        description: "基于已审核脚本与音频生成时间对齐字幕。",
        dependencies: &["script", "audio"],
        input_kinds: &["script", "voice_audio"],
        output_kinds: &["captions_source", "captions"],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "scene_plan",
        title: "场景规划",
        description: "把事实脚本拆成镜头、素材需求与证据引用。",
        dependencies: &["research", "script", "captions"],
        input_kinds: &[
            "claim_set",
            "evidence_set",
            "script",
            "captions",
            "scene_plan",
        ],
        output_kinds: &["scene_plan"],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "timeline",
        title: "时间轴",
        description: "组合音频、字幕、场景与素材为可编辑时间轴。",
        dependencies: &["audio", "captions", "scene_plan"],
        input_kinds: &["voice_audio", "captions", "scene_plan", "timeline"],
        output_kinds: &["timeline"],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "render",
        title: "渲染",
        description: "通过 Renderer 接口把已审核时间轴渲染为候选视频。",
        dependencies: &["timeline"],
        input_kinds: &["timeline"],
        output_kinds: &[
            "scene_snapshot",
            "rendered_scene",
            "rendered_video",
            "render_log",
        ],
        requires_approved_inputs: true,
        supports_partial_regeneration: true,
    },
    BuiltinStageSpec {
        stage_id: "export",
        title: "导出",
        description: "封装最终媒体与可追踪 manifest。",
        dependencies: &["render"],
        input_kinds: &["rendered_video"],
        output_kinds: &["final_video", "render_manifest"],
        requires_approved_inputs: true,
        supports_partial_regeneration: false,
    },
];

#[derive(Clone)]
pub struct WorkflowService {
    project_service: ProjectService,
    storage_service: StorageService,
}

impl WorkflowService {
    pub fn new(project_service: ProjectService, storage_service: StorageService) -> Self {
        Self {
            project_service,
            storage_service,
        }
    }

    /// 校验 Audio/Captions/Scene Plan worker 的冻结输入是否仍是当前有效批准版本。
    ///
    /// 此 helper 只返回已经存在的 Artifact 元数据，不读取任意路径，也不暴露为 Tauri command。
    pub fn validate_approved_media_inputs(
        &self,
        options: ValidateApprovedMediaInputsOptions,
    ) -> Result<Vec<ArtifactReadResultData>, WorkflowServiceError> {
        let operation = WorkflowOperation::ValidateApprovedMediaInputs;
        if !matches!(
            options.target_stage_id.as_str(),
            "audio" | "captions" | "scene_plan" | "timeline" | "render" | "export"
        ) || options.inputs.is_empty()
            || options.inputs.len() > 8
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "受审核结构化输入校验仅支持 audio/captions/scene_plan/timeline/render/export，且必须包含 1..=8 个冻结引用。",
            ));
        }

        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        let definition = context.require_stage(&options.target_stage_id, operation)?;
        let input_refs = options
            .inputs
            .iter()
            .map(|input| {
                json!({
                    "refId": input.ref_id,
                    "referenceType": "artifact",
                    "kind": input.kind,
                    "artifactId": input.artifact_id,
                    "sourceRunId": input.source_run_id,
                    "reviewRecordId": input.review_record_id,
                    "contentHash": input.content_hash,
                    "claimIds": input.claim_ids,
                    "evidenceRefs": input.evidence_refs,
                })
            })
            .collect::<Vec<_>>();
        validate_input_references(
            &self.storage_service,
            &descriptor,
            &context,
            definition,
            &input_refs,
            operation,
        )?;

        options
            .inputs
            .iter()
            .map(|input| {
                self.storage_service
                    .read_artifact_for_workflow_unlocked(&descriptor, &input.artifact_id)
                    .map_err(|error| storage_error_to_workflow(error, operation))
            })
            .collect()
    }

    pub fn initialize_project_workflow(
        &self,
        options: InitializeWorkflowOptions,
    ) -> Result<WorkflowSnapshotData, WorkflowServiceError> {
        let operation = WorkflowOperation::InitializeWorkflow;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        require_supported_workflow(&descriptor, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let mut marker = read_json_file(
            &project_dir,
            Path::new(&descriptor.marker_path),
            operation,
            WorkflowErrorCode::InvalidProject,
        )?;

        let definitions_dir =
            ensure_project_directories(&project_dir, &["contracts", "stages"], operation)?;
        let stages_dir = ensure_project_directories(&project_dir, &["stages"], operation)?;
        ensure_project_directories(&project_dir, &["runs", "reservations"], operation)?;

        let now = current_timestamp(operation)?;
        let mut definitions = Vec::with_capacity(STANDARD_STAGES.len());
        let mut configs = BTreeMap::new();
        for spec in STANDARD_STAGES {
            let definition_path = definitions_dir.join(format!("{}.json", spec.stage_id));
            let definition = match inspect_project_path(&project_dir, &definition_path, operation)?
            {
                Some(_) => read_json_file(
                    &project_dir,
                    &definition_path,
                    operation,
                    WorkflowErrorCode::InvalidProject,
                )?,
                None => {
                    let definition = stage_definition_document(spec);
                    validate_persistent_document(&definition, operation, "内置阶段定义")?;
                    write_immutable_json(&project_dir, &definition_path, &definition, operation)?;
                    definition
                }
            };
            definitions.push(StageDefinitionEntry::from_document(
                definition,
                &definition_path,
                operation,
            )?);

            let stage_dir =
                ensure_project_directories(&project_dir, &["stages", spec.stage_id], operation)?;
            let config_path = stage_dir.join("config.json");
            let config = match inspect_project_path(&project_dir, &config_path, operation)? {
                Some(_) => read_json_file(
                    &project_dir,
                    &config_path,
                    operation,
                    WorkflowErrorCode::InvalidProject,
                )?,
                None => {
                    let document =
                        initial_stage_config_document(&descriptor.project_id, spec.stage_id, &now);
                    validate_persistent_document(&document, operation, "初始阶段配置")?;
                    write_immutable_json(&project_dir, &config_path, &document, operation)?;
                    document
                }
            };
            validate_stage_config_identity(
                &config,
                &descriptor.project_id,
                spec.stage_id,
                &config_path,
                operation,
            )?;
            configs.insert(spec.stage_id.to_owned(), config);
        }

        validate_stage_graph(&definitions, operation)?;
        let existing_states = marker
            .get("stages")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let marker_needs_initialization = existing_states.as_array().is_some_and(Vec::is_empty);
        let states = if marker_needs_initialization {
            initial_stage_states(&definitions, operation)?
        } else {
            let states = deserialize_stage_states(existing_states, operation)?;
            validate_stage_state_membership(&states, &definitions, operation)?;
            states
        };

        if marker_needs_initialization {
            set_marker_states(&mut marker, &states, &now, operation)?;
            write_json_atomic(
                &project_dir,
                Path::new(&descriptor.marker_path),
                &marker,
                operation,
            )?;
        }

        let _ = stages_dir;
        Ok(snapshot_from_parts(
            &descriptor,
            definitions,
            states,
            configs,
        ))
    }

    pub fn get_project_workflow(
        &self,
        project_path: impl AsRef<Path>,
    ) -> Result<WorkflowSnapshotData, WorkflowServiceError> {
        let operation = WorkflowOperation::GetWorkflow;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        Ok(context.snapshot())
    }

    pub fn update_stage_config(
        &self,
        options: UpdateStageConfigOptions,
    ) -> Result<StageConfigUpdateResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::UpdateStageConfig;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let mut context = load_workflow_context(&descriptor, operation)?;
        context.require_stage(&options.stage_id, operation)?;
        if options.decisions.len() > 256 {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "单次阶段配置最多包含 256 条决策记录。",
            )
            .for_stage(&options.stage_id));
        }
        let current = context.configs.get(&options.stage_id).ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                "阶段配置缺失。",
            )
            .for_stage(&options.stage_id)
        })?;
        let revision = required_u64(current, "revision", operation)?;
        if revision != u64::from(options.expected_revision) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ConfigConflict,
                operation,
                format!(
                    "阶段配置修订冲突：期望 {}，实际 {revision}。",
                    options.expected_revision
                ),
            )
            .for_stage(&options.stage_id));
        }

        let now = current_timestamp(operation)?;
        let config_id = required_string(current, "configId", operation)?;
        let candidate = json!({
            "schemaVersion": NARRACUT_CONTRACT_VERSION,
            "documentType": "stage_config",
            "configId": config_id,
            "projectId": descriptor.project_id,
            "stageId": options.stage_id,
            "revision": revision + 1,
            "values": options.values,
            "decisions": options.decisions,
            "updatedAt": now,
        });
        validate_request_derived_document(
            &candidate,
            operation,
            "阶段配置 values 或 decisions 不符合持久化契约",
        )?;

        context
            .configs
            .insert(options.stage_id.clone(), candidate.clone());
        let recomputed = recompute_stage_states(&context, operation)?;
        set_marker_states(&mut context.marker, &recomputed, &now, operation)?;
        write_json_atomic(
            &context.project_dir,
            Path::new(&descriptor.marker_path),
            &context.marker,
            operation,
        )?;

        let config_path = stage_config_path(&context.project_dir, &options.stage_id);
        write_json_atomic(&context.project_dir, &config_path, &candidate, operation)?;
        context.states = recomputed;
        let affected_stages =
            regeneration_impact(&context, std::slice::from_ref(&options.stage_id), operation)?;

        Ok(StageConfigUpdateResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            config: candidate,
            config_uri: format!("stages/{}/config.json", options.stage_id),
            affected_stages,
        })
    }

    pub fn prepare_stage_run(
        &self,
        options: PrepareStageRunOptions,
    ) -> Result<StageRunPreparationResultData, WorkflowServiceError> {
        self.prepare_stage_run_internal(options, None)
    }

    pub fn prepare_stage_run_with_config_snapshot(
        &self,
        options: PrepareStageRunOptions,
        config_snapshot: Value,
    ) -> Result<StageRunPreparationResultData, WorkflowServiceError> {
        self.prepare_stage_run_internal(options, Some(config_snapshot))
    }

    fn prepare_stage_run_internal(
        &self,
        options: PrepareStageRunOptions,
        config_snapshot: Option<Value>,
    ) -> Result<StageRunPreparationResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::PrepareStageRun;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        let definition = context.require_stage(&options.stage_id, operation)?.clone();
        if let Some(config_snapshot) = config_snapshot.as_ref() {
            validate_request_derived_document(
                config_snapshot,
                operation,
                "显式运行配置不符合 StageConfig 契约",
            )?;
            if config_snapshot.get("documentType").and_then(Value::as_str) != Some("stage_config")
                || config_snapshot.get("projectId").and_then(Value::as_str)
                    != Some(descriptor.project_id.as_str())
                || config_snapshot.get("stageId").and_then(Value::as_str)
                    != Some(options.stage_id.as_str())
            {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidRequest,
                    operation,
                    "显式运行配置必须属于当前项目和阶段。",
                )
                .for_stage(&options.stage_id)
                .for_run(&options.run_id));
            }
        }
        validate_portable_id(&options.run_id, "run_", operation, "runId")?;
        validate_job_id(&options.job_id, operation)?;
        if options.input_refs.len() > 256 {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "单次阶段运行最多包含 256 个输入引用。",
            )
            .for_stage(&options.stage_id)
            .for_run(&options.run_id));
        }

        let snapshot_path =
            stage_execution_snapshot_path(&context.project_dir, &options.stage_id, &options.run_id);
        let reservation_path = run_reservation_path(&context.project_dir, &options.run_id);
        let (execution_snapshot, idempotent_replay) =
            match inspect_project_path(&context.project_dir, &snapshot_path, operation)? {
                Some(_) => {
                    let existing = read_json_file(
                        &context.project_dir,
                        &snapshot_path,
                        operation,
                        WorkflowErrorCode::InvalidProject,
                    )?;
                    validate_execution_snapshot_replay(
                        &existing,
                        &descriptor,
                        &options,
                        config_snapshot.as_ref(),
                        &snapshot_path,
                        operation,
                    )?;
                    bind_run_reservation_to_snapshot(
                        &context.project_dir,
                        &reservation_path,
                        &existing,
                        &options.stage_id,
                        &options.run_id,
                        operation,
                    )?;
                    (existing, true)
                }
                None => {
                    let run_path =
                        stage_run_path(&context.project_dir, &options.stage_id, &options.run_id);
                    if inspect_project_path(&context.project_dir, &run_path, operation)?.is_some() {
                        return Err(WorkflowServiceError::new(
                            WorkflowErrorCode::RunConflict,
                            operation,
                            "既有 StageRun 缺少可信执行快照，不能用同一 runId 重新预留。",
                        )
                        .at_path(&run_path)
                        .for_stage(&options.stage_id)
                        .for_run(&options.run_id));
                    }
                    ensure_run_id_available(
                        &context,
                        &options.stage_id,
                        &options.run_id,
                        operation,
                    )?;
                    let (document, reservation_replay) = match inspect_project_path(
                        &context.project_dir,
                        &reservation_path,
                        operation,
                    )? {
                        Some(_) => {
                            let existing = read_json_file(
                                &context.project_dir,
                                &reservation_path,
                                operation,
                                WorkflowErrorCode::RunConflict,
                            )?;
                            validate_execution_snapshot_replay(
                                &existing,
                                &descriptor,
                                &options,
                                config_snapshot.as_ref(),
                                &reservation_path,
                                operation,
                            )?;
                            (existing, true)
                        }
                        None => {
                            scan_stage_history(&context.project_dir, &options.stage_id, operation)?;
                            ensure_stage_run_slot_available(
                                &context.project_dir,
                                &options.stage_id,
                                operation,
                            )?;
                            validate_stage_ready_for_run(
                                &context,
                                &definition,
                                &options.input_refs,
                                operation,
                            )?;
                            validate_input_references(
                                &self.storage_service,
                                &descriptor,
                                &context,
                                &definition,
                                &options.input_refs,
                                operation,
                            )?;
                            let config = config_snapshot
                                .as_ref()
                                .or_else(|| context.configs.get(&options.stage_id))
                                .ok_or_else(|| {
                                    WorkflowServiceError::new(
                                        WorkflowErrorCode::InvalidProject,
                                        operation,
                                        "阶段配置缺失。",
                                    )
                                    .for_stage(&options.stage_id)
                                })?;
                            let input_hash =
                                hash_json(&Value::Array(options.input_refs.clone()), operation)?;
                            let config_hash = hash_json(config, operation)?;
                            let idempotency_key = hash_json(
                                &json!({
                                    "stageId": options.stage_id,
                                    "inputHash": input_hash,
                                    "configHash": config_hash,
                                    "executor": options.executor,
                                }),
                                operation,
                            )?;
                            let candidate = json!({
                                "schemaVersion": NARRACUT_CONTRACT_VERSION,
                                "documentType": "stage_execution_snapshot",
                                "runId": options.run_id,
                                "projectId": descriptor.project_id,
                                "stageId": options.stage_id,
                                "stageDefinitionVersion": definition.definition_version,
                                "jobId": options.job_id,
                                "inputHash": input_hash,
                                "configHash": config_hash,
                                "idempotencyKey": idempotency_key,
                                "inputRefs": options.input_refs,
                                "configSnapshot": config,
                                "executor": options.executor,
                                "createdAt": current_timestamp(operation)?,
                            });
                            validate_request_derived_document(
                                &candidate,
                                operation,
                                "运行输入、配置或执行器不符合 StageExecutionSnapshot 契约",
                            )?;
                            claim_run_reservation(
                                &context.project_dir,
                                &reservation_path,
                                &descriptor,
                                &options,
                                &candidate,
                                config_snapshot.as_ref(),
                                operation,
                            )?
                        }
                    };
                    ensure_project_directories(
                        &context.project_dir,
                        &["runs", &options.stage_id, &options.run_id],
                        operation,
                    )?;
                    let snapshot_replay = write_immutable_json(
                        &context.project_dir,
                        &snapshot_path,
                        &document,
                        operation,
                    )?;
                    (document, reservation_replay || snapshot_replay)
                }
            };

        Ok(StageRunPreparationResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            execution_snapshot,
            execution_snapshot_uri: format!(
                "runs/{}/{}/execution.json",
                options.stage_id, options.run_id
            ),
            idempotent_replay,
        })
    }

    pub fn record_stage_run(
        &self,
        options: RecordStageRunOptions,
    ) -> Result<StageRunCommitResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::RecordStageRun;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let mut context = load_workflow_context(&descriptor, operation)?;
        let definition = context.require_stage(&options.stage_id, operation)?.clone();
        validate_portable_id(&options.run_id, "run_", operation, "runId")?;
        validate_job_id(&options.job_id, operation)?;
        if options.artifact_ids.len() > 256 {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "单次阶段终态最多包含 256 个 Artifact。",
            )
            .for_stage(&options.stage_id)
            .for_run(&options.run_id));
        }

        let snapshot_path =
            stage_execution_snapshot_path(&context.project_dir, &options.stage_id, &options.run_id);
        let execution_snapshot = read_json_file(
            &context.project_dir,
            &snapshot_path,
            operation,
            WorkflowErrorCode::RunNotFound,
        )?;
        validate_execution_snapshot_for_terminal(
            &execution_snapshot,
            &descriptor,
            &options.stage_id,
            &options.run_id,
            &options.job_id,
            &snapshot_path,
            operation,
        )?;
        let reservation_path = run_reservation_path(&context.project_dir, &options.run_id);
        bind_run_reservation_to_snapshot(
            &context.project_dir,
            &reservation_path,
            &execution_snapshot,
            &options.stage_id,
            &options.run_id,
            operation,
        )?;

        let run_path = stage_run_path(&context.project_dir, &options.stage_id, &options.run_id);
        let existing = match inspect_project_path(&context.project_dir, &run_path, operation)? {
            Some(_) => Some(read_json_file(
                &context.project_dir,
                &run_path,
                operation,
                WorkflowErrorCode::InvalidProject,
            )?),
            None => None,
        };

        let (run, idempotent_replay) = if let Some(existing) = existing {
            validate_run_replay(
                &existing,
                &descriptor,
                &execution_snapshot,
                &options,
                &run_path,
                operation,
            )?;
            (existing, true)
        } else {
            ensure_run_id_available(&context, &options.stage_id, &options.run_id, operation)?;
            if options.status == TerminalRunStatusData::Succeeded && options.artifact_ids.is_empty()
            {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::ArtifactMismatch,
                    operation,
                    "成功运行必须提交至少一个属于该运行的输出 Artifact。",
                )
                .for_stage(&options.stage_id)
                .for_run(&options.run_id));
            }
            validate_artifacts_for_run(
                &self.storage_service,
                &descriptor,
                &definition,
                &options.run_id,
                &options.artifact_ids,
                operation,
            )?;
            let _current_config = context.configs.get(&options.stage_id).ok_or_else(|| {
                WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "阶段配置缺失。",
                )
                .for_stage(&options.stage_id)
            })?;
            let now = current_timestamp(operation)?;
            let input_hash = required_string(&execution_snapshot, "inputHash", operation)?;
            let config_hash = required_string(&execution_snapshot, "configHash", operation)?;
            let idempotency_key =
                required_string(&execution_snapshot, "idempotencyKey", operation)?;
            let input_refs = execution_snapshot
                .get("inputRefs")
                .cloned()
                .ok_or_else(|| invalid_snapshot_field(operation, "inputRefs"))?;
            let config_snapshot = execution_snapshot
                .get("configSnapshot")
                .cloned()
                .ok_or_else(|| invalid_snapshot_field(operation, "configSnapshot"))?;
            let executor = execution_snapshot
                .get("executor")
                .cloned()
                .ok_or_else(|| invalid_snapshot_field(operation, "executor"))?;
            let started_at = required_string(&execution_snapshot, "createdAt", operation)?;
            let stage_definition_version =
                required_string(&execution_snapshot, "stageDefinitionVersion", operation)?;
            let previous_latest = context
                .state(&options.stage_id)
                .and_then(|state| state.latest_run_id.clone());
            let mut object = Map::from_iter([
                (
                    "schemaVersion".to_owned(),
                    Value::String(NARRACUT_CONTRACT_VERSION.to_owned()),
                ),
                (
                    "documentType".to_owned(),
                    Value::String("stage_run".to_owned()),
                ),
                ("runId".to_owned(), Value::String(options.run_id.clone())),
                (
                    "projectId".to_owned(),
                    Value::String(descriptor.project_id.clone()),
                ),
                (
                    "stageId".to_owned(),
                    Value::String(options.stage_id.clone()),
                ),
                (
                    "stageDefinitionVersion".to_owned(),
                    Value::String(stage_definition_version),
                ),
                (
                    "status".to_owned(),
                    Value::String(options.status.as_str().to_owned()),
                ),
                ("jobId".to_owned(), Value::String(options.job_id.clone())),
                ("inputHash".to_owned(), Value::String(input_hash)),
                ("configHash".to_owned(), Value::String(config_hash)),
                ("idempotencyKey".to_owned(), Value::String(idempotency_key)),
                ("inputRefs".to_owned(), input_refs),
                ("configSnapshot".to_owned(), config_snapshot),
                ("executor".to_owned(), executor),
                (
                    "artifactIds".to_owned(),
                    serde_json::to_value(&options.artifact_ids).expect("string vector serializes"),
                ),
                ("logSummary".to_owned(), options.log_summary.clone()),
                ("createdAt".to_owned(), Value::String(started_at.clone())),
                ("startedAt".to_owned(), Value::String(started_at)),
                ("completedAt".to_owned(), Value::String(now)),
            ]);
            if let Some(previous_latest) = previous_latest {
                object.insert("supersedesRunId".to_owned(), Value::String(previous_latest));
            }
            let document = Value::Object(object);
            validate_request_derived_document(
                &document,
                operation,
                "运行输入、执行器或日志摘要不符合 StageRun 契约",
            )?;
            ensure_project_directories(
                &context.project_dir,
                &["runs", &options.stage_id, &options.run_id],
                operation,
            )?;
            write_immutable_json(&context.project_dir, &run_path, &document, operation)?;
            (document, false)
        };

        let states_before = context.states.clone();
        let latest_run_id =
            latest_run_id_for_stage(&context.project_dir, &options.stage_id, operation)?
                .unwrap_or_else(|| options.run_id.clone());
        context
            .state_mut(&options.stage_id)
            .expect("validated stage state exists")
            .latest_run_id = Some(latest_run_id);
        let recomputed = recompute_stage_states(&context, operation)?;
        if states_before != recomputed {
            let now = current_timestamp(operation)?;
            set_marker_states(&mut context.marker, &recomputed, &now, operation)?;
            write_json_atomic(
                &context.project_dir,
                Path::new(&descriptor.marker_path),
                &context.marker,
                operation,
            )?;
        }
        context.states = recomputed;
        let stage_state = context
            .state(&options.stage_id)
            .cloned()
            .expect("validated stage state exists");
        let execution_outdated =
            !run_matches_current_inputs(&context, &options.stage_id, &run, operation)?;

        Ok(StageRunCommitResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            run,
            run_uri: format!("runs/{}/{}/run.json", options.stage_id, options.run_id),
            stage_state,
            review_required: options.status == TerminalRunStatusData::Succeeded,
            execution_outdated,
            idempotent_replay,
        })
    }

    pub(crate) fn validate_stage_run_artifacts(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
        stage_id: &str,
        run_id: &str,
        artifact_ids: &[String],
    ) -> Result<(), WorkflowServiceError> {
        let operation = WorkflowOperation::RecordStageRun;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        let definition = context.require_stage(stage_id, operation)?;
        validate_portable_id(run_id, "run_", operation, "runId")?;
        if artifact_ids.len() > 256 {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "单次阶段终态最多包含 256 个 Artifact。",
            )
            .for_stage(stage_id)
            .for_run(run_id));
        }
        validate_artifacts_for_run(
            &self.storage_service,
            &descriptor,
            definition,
            run_id,
            artifact_ids,
            operation,
        )
    }

    pub fn review_stage_run(
        &self,
        options: ReviewStageRunOptions,
    ) -> Result<StageReviewResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::ReviewStageRun;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let mut context = load_workflow_context(&descriptor, operation)?;
        let definition = context.require_stage(&options.stage_id, operation)?.clone();
        validate_portable_id(&options.run_id, "run_", operation, "runId")?;
        validate_portable_id(&options.review_id, "review_", operation, "reviewId")?;
        if options.artifact_ids.len() > 256 {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "单条审核记录最多引用 256 个 Artifact。",
            )
            .for_stage(&options.stage_id)
            .for_run(&options.run_id));
        }
        let run = read_stage_run(
            &context.project_dir,
            &options.stage_id,
            &options.run_id,
            operation,
        )?;
        if required_string(&run, "status", operation)? != "succeeded"
            && options.decision == ReviewDecisionData::Approved
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                operation,
                "只有 succeeded 的阶段运行可以被批准。",
            )
            .for_stage(&options.stage_id)
            .for_run(&options.run_id));
        }
        if options.decision == ReviewDecisionData::Approved && options.artifact_ids.is_empty() {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ArtifactMismatch,
                operation,
                "批准运行时必须明确列出至少一个被审核采用的 Artifact。",
            )
            .for_stage(&options.stage_id)
            .for_run(&options.run_id));
        }
        let approval_before_review = context
            .state(&options.stage_id)
            .and_then(|state| state.approved_run_id.clone());
        validate_artifacts_for_review(
            &self.storage_service,
            &descriptor,
            &definition,
            &run,
            &options.run_id,
            &options.artifact_ids,
            operation,
        )?;
        if options.decision == ReviewDecisionData::Approved {
            validate_required_media_review_closure(
                &self.storage_service,
                &descriptor,
                &definition,
                &run,
                &options.run_id,
                &options.artifact_ids,
                operation,
            )?;
        }

        let review_path = context
            .project_dir
            .join("runs")
            .join(&options.stage_id)
            .join(&options.run_id)
            .join("reviews")
            .join(format!("{}.json", options.review_id));
        let (review, idempotent_replay) = match inspect_project_path(
            &context.project_dir,
            &review_path,
            operation,
        )? {
            Some(_) => {
                let existing = read_json_file(
                    &context.project_dir,
                    &review_path,
                    operation,
                    WorkflowErrorCode::InvalidProject,
                )?;
                validate_review_replay(&existing, &descriptor, &options, &review_path, operation)?;
                (existing, true)
            }
            None => {
                if options.decision == ReviewDecisionData::Approved
                    && approval_before_review.is_none()
                    && !run_matches_current_inputs(&context, &options.stage_id, &run, operation)?
                {
                    return Err(WorkflowServiceError::new(
                            WorkflowErrorCode::StageNotReady,
                            operation,
                            "该历史运行不再匹配当前配置或上游批准版本，且阶段没有可回退的既有批准运行。",
                        )
                        .for_stage(&options.stage_id)
                        .for_run(&options.run_id));
                }
                ensure_review_id_available(&context, &options.review_id, operation)?;
                let (_, existing_reviews) =
                    scan_stage_history(&context.project_dir, &options.stage_id, operation)?;
                if existing_reviews.len() >= MAX_STAGE_REVIEWS {
                    return Err(WorkflowServiceError::new(
                        WorkflowErrorCode::ScanLimitExceeded,
                        operation,
                        format!("单阶段审核历史已达到同步上限 {MAX_STAGE_REVIEWS}。"),
                    )
                    .for_stage(&options.stage_id));
                }
                let document = json!({
                    "schemaVersion": NARRACUT_CONTRACT_VERSION,
                    "documentType": "review_record",
                    "reviewId": options.review_id,
                    "projectId": descriptor.project_id,
                    "stageId": options.stage_id,
                    "runId": options.run_id,
                    "decision": options.decision.as_str(),
                    "reviewer": options.reviewer,
                    "comments": options.comments,
                    "artifactIds": options.artifact_ids,
                    "createdAt": current_timestamp(operation)?,
                });
                validate_request_derived_document(
                    &document,
                    operation,
                    "审核人或审核内容不符合 ReviewRecord 契约",
                )?;
                ensure_project_directories(
                    &context.project_dir,
                    &["runs", &options.stage_id, &options.run_id, "reviews"],
                    operation,
                )?;
                write_immutable_json(&context.project_dir, &review_path, &document, operation)?;
                (document, false)
            }
        };

        let latest_review =
            latest_review_key_for_stage(&context.project_dir, &options.stage_id, operation)?;
        let this_key = (
            required_string(&review, "createdAt", operation)?,
            options.review_id.clone(),
        );
        let is_latest_review = latest_review.as_ref() == Some(&this_key);
        let old_states = context.states.clone();
        let mut applied = false;
        if is_latest_review {
            if options.decision == ReviewDecisionData::Approved {
                let can_apply = approval_before_review.is_some()
                    || run_matches_current_inputs(&context, &options.stage_id, &run, operation)?;
                if can_apply {
                    context
                        .state_mut(&options.stage_id)
                        .expect("validated stage state exists")
                        .approved_run_id = Some(options.run_id.clone());
                }
            } else if approval_before_review.as_deref() == Some(options.run_id.as_str()) {
                context
                    .state_mut(&options.stage_id)
                    .expect("validated stage state exists")
                    .approved_run_id = None;
            }

            let recomputed = recompute_stage_states(&context, operation)?;
            applied = old_states != recomputed;
            if applied {
                let now = current_timestamp(operation)?;
                set_marker_states(&mut context.marker, &recomputed, &now, operation)?;
                write_json_atomic(
                    &context.project_dir,
                    Path::new(&descriptor.marker_path),
                    &context.marker,
                    operation,
                )?;
            }
            context.states = recomputed;
        }

        let invalidated_stage_ids =
            invalidated_stage_ids(&old_states, &context.states, &options.stage_id);
        Ok(StageReviewResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            review,
            review_uri: format!(
                "runs/{}/{}/reviews/{}.json",
                options.stage_id, options.run_id, options.review_id
            ),
            stage_states: context.states,
            invalidated_stage_ids,
            applied,
            idempotent_replay,
        })
    }

    pub fn preview_regeneration(
        &self,
        project_path: impl AsRef<Path>,
        changed_stage_ids: Vec<String>,
    ) -> Result<RegenerationImpactResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::PreviewRegeneration;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        let affected_stages = regeneration_impact(&context, &changed_stage_ids, operation)?;
        Ok(RegenerationImpactResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            changed_stage_ids,
            affected_stages,
        })
    }

    /// 读取并完整校验某次任务冻结的 StageExecutionSnapshot。
    ///
    /// 该内部服务接口供有界 worker 使用；桌面前端不会获得任意项目文件读取能力。
    pub fn get_stage_execution_snapshot(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
        stage_id: &str,
        run_id: &str,
        job_id: &str,
    ) -> Result<Value, WorkflowServiceError> {
        let operation = WorkflowOperation::GetWorkflow;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        context.require_stage(stage_id, operation)?;
        let path = stage_execution_snapshot_path(&context.project_dir, stage_id, run_id);
        let snapshot = read_json_file(
            &context.project_dir,
            &path,
            operation,
            WorkflowErrorCode::RunNotFound,
        )?;
        validate_execution_snapshot_for_terminal(
            &snapshot,
            &descriptor,
            stage_id,
            run_id,
            job_id,
            &path,
            operation,
        )?;
        Ok(snapshot)
    }

    pub fn list_stage_history(
        &self,
        project_path: impl AsRef<Path>,
        stage_id: &str,
        limit: u32,
    ) -> Result<StageHistoryResultData, WorkflowServiceError> {
        let operation = WorkflowOperation::ListStageHistory;
        let _guard = self.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        let context = load_workflow_context(&descriptor, operation)?;
        context.require_stage(stage_id, operation)?;
        if !(1..=100).contains(&limit) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "limit 必须在 1 到 100 之间。",
            )
            .for_stage(stage_id));
        }
        let (mut runs, mut reviews) =
            scan_stage_history(&context.project_dir, stage_id, operation)?;
        runs.sort_by_key(|run| Reverse(document_order_key(run)));
        runs.truncate(limit as usize);
        let selected_run_ids = runs
            .iter()
            .filter_map(|run| run.get("runId").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        reviews.retain(|review| {
            review
                .get("runId")
                .and_then(Value::as_str)
                .is_some_and(|run_id| selected_run_ids.contains(run_id))
        });
        reviews.sort_by_key(|review| Reverse(document_order_key(review)));

        Ok(StageHistoryResultData {
            api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            stage_id: stage_id.to_owned(),
            runs,
            reviews,
        })
    }

    fn open_project_unlocked(
        &self,
        project_path: impl AsRef<Path>,
        operation: WorkflowOperation,
    ) -> Result<ProjectDescriptorData, WorkflowServiceError> {
        self.project_service
            .open_project_unlocked(project_path.as_ref(), ProjectOperation::Open)
            .map_err(|error| project_error_to_workflow(error, operation))
    }
}

#[derive(Clone)]
struct StageDefinitionEntry {
    document: Value,
    stage_id: String,
    definition_version: String,
    dependencies: Vec<String>,
    input_kinds: Vec<String>,
    output_kinds: Vec<String>,
    requires_approved_inputs: bool,
    supports_partial_regeneration: bool,
}

impl StageDefinitionEntry {
    fn from_document(
        document: Value,
        path: &Path,
        operation: WorkflowOperation,
    ) -> Result<Self, WorkflowServiceError> {
        validate_persistent_document(&document, operation, "阶段定义")
            .map_err(|error| error.at_path(path))?;
        if document.get("documentType").and_then(Value::as_str) != Some("stage_definition") {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidStageGraph,
                operation,
                "阶段定义文档类型错误。",
            )
            .at_path(path));
        }
        let stage_id = required_string(&document, "stageId", operation)?;
        let definition_version = required_string(&document, "definitionVersion", operation)?;
        let dependencies = required_string_array(&document, "dependencies", operation)?;
        let declared_input_kinds = required_string_array(&document, "inputKinds", operation)?;
        let requires_approved_inputs =
            required_bool(&document, "requiresApprovedInputs", operation)?;
        let supports_partial_regeneration =
            required_bool(&document, "supportsPartialRegeneration", operation)?;
        let output_kinds = legacy_compatible_output_kinds(
            &stage_id,
            &definition_version,
            &dependencies,
            &declared_input_kinds,
            requires_approved_inputs,
            supports_partial_regeneration,
            required_string_array(&document, "outputKinds", operation)?,
        );
        let input_kinds = legacy_compatible_input_kinds(
            &stage_id,
            &definition_version,
            &dependencies,
            requires_approved_inputs,
            supports_partial_regeneration,
            declared_input_kinds,
        );
        Ok(Self {
            document,
            stage_id,
            definition_version,
            dependencies,
            input_kinds,
            output_kinds,
            requires_approved_inputs,
            supports_partial_regeneration,
        })
    }
}

struct WorkflowContext {
    descriptor: ProjectDescriptorData,
    project_dir: PathBuf,
    marker: Value,
    definitions: Vec<StageDefinitionEntry>,
    configs: BTreeMap<String, Value>,
    states: Vec<StageStateData>,
}

impl WorkflowContext {
    fn snapshot(&self) -> WorkflowSnapshotData {
        snapshot_from_parts(
            &self.descriptor,
            self.definitions.clone(),
            self.states.clone(),
            self.configs.clone(),
        )
    }

    fn require_stage(
        &self,
        stage_id: &str,
        operation: WorkflowOperation,
    ) -> Result<&StageDefinitionEntry, WorkflowServiceError> {
        self.definitions
            .iter()
            .find(|definition| definition.stage_id == stage_id)
            .ok_or_else(|| {
                WorkflowServiceError::new(
                    WorkflowErrorCode::StageNotFound,
                    operation,
                    "工作流中不存在该阶段。",
                )
                .for_stage(stage_id)
            })
    }

    fn state(&self, stage_id: &str) -> Option<&StageStateData> {
        self.states.iter().find(|state| state.stage_id == stage_id)
    }

    fn state_mut(&mut self, stage_id: &str) -> Option<&mut StageStateData> {
        self.states
            .iter_mut()
            .find(|state| state.stage_id == stage_id)
    }
}

fn load_workflow_context(
    descriptor: &ProjectDescriptorData,
    operation: WorkflowOperation,
) -> Result<WorkflowContext, WorkflowServiceError> {
    require_supported_workflow(descriptor, operation)?;
    let project_dir = PathBuf::from(&descriptor.project_path);
    let marker = read_json_file(
        &project_dir,
        Path::new(&descriptor.marker_path),
        operation,
        WorkflowErrorCode::InvalidProject,
    )?;
    let states_value = marker
        .get("stages")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if states_value.as_array().is_some_and(Vec::is_empty) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::WorkflowNotInitialized,
            operation,
            "项目尚未初始化阶段工作流。",
        )
        .at_path(Path::new(&descriptor.marker_path)));
    }
    let states = deserialize_stage_states(states_value, operation)?;
    let mut definitions = Vec::with_capacity(STANDARD_STAGES.len());
    let mut configs = BTreeMap::new();
    for spec in STANDARD_STAGES {
        let definition_path = stage_definition_path(&project_dir, spec.stage_id);
        let document = read_json_file(
            &project_dir,
            &definition_path,
            operation,
            WorkflowErrorCode::WorkflowNotInitialized,
        )?;
        definitions.push(StageDefinitionEntry::from_document(
            document,
            &definition_path,
            operation,
        )?);

        let config_path = stage_config_path(&project_dir, spec.stage_id);
        let config = read_json_file(
            &project_dir,
            &config_path,
            operation,
            WorkflowErrorCode::WorkflowNotInitialized,
        )?;
        validate_stage_config_identity(
            &config,
            &descriptor.project_id,
            spec.stage_id,
            &config_path,
            operation,
        )?;
        configs.insert(spec.stage_id.to_owned(), config);
    }
    validate_stage_graph(&definitions, operation)?;
    validate_stage_state_membership(&states, &definitions, operation)?;
    let context = WorkflowContext {
        descriptor: descriptor.clone(),
        project_dir,
        marker,
        definitions,
        configs,
        states,
    };
    validate_current_state_references(&context, operation)?;
    Ok(context)
}

fn snapshot_from_parts(
    descriptor: &ProjectDescriptorData,
    definitions: Vec<StageDefinitionEntry>,
    states: Vec<StageStateData>,
    configs: BTreeMap<String, Value>,
) -> WorkflowSnapshotData {
    WorkflowSnapshotData {
        api_version: WORKFLOW_COMMAND_API_VERSION.to_owned(),
        owner_project_id: descriptor.project_id.clone(),
        workflow_definition_id: descriptor.workflow_definition_id.clone(),
        stage_definitions: definitions
            .into_iter()
            .map(|definition| definition.document)
            .collect(),
        stage_states: states,
        configs: configs.into_values().collect(),
    }
}

fn stage_definition_document(spec: &BuiltinStageSpec) -> Value {
    json!({
        "schemaVersion": NARRACUT_CONTRACT_VERSION,
        "documentType": "stage_definition",
        "stageId": spec.stage_id,
        "definitionVersion": if matches!(
            spec.stage_id,
            "audio" | "captions" | "scene_plan" | "timeline"
        ) {
            "1.1.0"
        } else {
            "1.0.0"
        },
        "title": spec.title,
        "description": spec.description,
        "dependencies": spec.dependencies,
        "inputKinds": spec.input_kinds,
        "outputKinds": spec.output_kinds,
        "configSchemaRef": format!("narracut://schemas/stages/{}/config/v1", spec.stage_id),
        "requiresApprovedInputs": spec.requires_approved_inputs,
        "supportsPartialRegeneration": spec.supports_partial_regeneration,
    })
}

fn legacy_compatible_output_kinds(
    stage_id: &str,
    definition_version: &str,
    dependencies: &[String],
    input_kinds: &[String],
    requires_approved_inputs: bool,
    supports_partial_regeneration: bool,
    output_kinds: Vec<String>,
) -> Vec<String> {
    if definition_version != "1.0.0" {
        return output_kinds;
    }
    match (stage_id, output_kinds.as_slice()) {
        ("audio", [kind])
            if kind == "voice_audio"
                && dependencies == ["script"]
                && input_kinds == ["script"]
                && requires_approved_inputs
                && !supports_partial_regeneration =>
        {
            vec!["audio_source".to_owned(), "voice_audio".to_owned()]
        }
        ("captions", [kind])
            if kind == "captions"
                && dependencies == ["script", "audio"]
                && input_kinds == ["script", "voice_audio"]
                && requires_approved_inputs
                && supports_partial_regeneration =>
        {
            vec!["captions_source".to_owned(), "captions".to_owned()]
        }
        _ => output_kinds,
    }
}

fn legacy_compatible_input_kinds(
    stage_id: &str,
    definition_version: &str,
    dependencies: &[String],
    requires_approved_inputs: bool,
    supports_partial_regeneration: bool,
    input_kinds: Vec<String>,
) -> Vec<String> {
    if definition_version != "1.0.0" {
        return input_kinds;
    }
    match (stage_id, input_kinds.as_slice()) {
        ("scene_plan", [claim_set, evidence_set, script, captions])
            if claim_set == "claim_set"
                && evidence_set == "evidence_set"
                && script == "script"
                && captions == "captions"
                && dependencies == ["research", "script", "captions"]
                && requires_approved_inputs
                && supports_partial_regeneration =>
        {
            vec![
                "claim_set".to_owned(),
                "evidence_set".to_owned(),
                "script".to_owned(),
                "captions".to_owned(),
                "scene_plan".to_owned(),
            ]
        }
        ("timeline", [voice_audio, captions, scene_plan])
            if voice_audio == "voice_audio"
                && captions == "captions"
                && scene_plan == "scene_plan"
                && dependencies == ["audio", "captions", "scene_plan"]
                && requires_approved_inputs
                && supports_partial_regeneration =>
        {
            vec![
                "voice_audio".to_owned(),
                "captions".to_owned(),
                "scene_plan".to_owned(),
                "timeline".to_owned(),
            ]
        }
        _ => input_kinds,
    }
}

fn initial_stage_config_document(project_id: &str, stage_id: &str, now: &str) -> Value {
    json!({
        "schemaVersion": NARRACUT_CONTRACT_VERSION,
        "documentType": "stage_config",
        "configId": format!("config_{stage_id}"),
        "projectId": project_id,
        "stageId": stage_id,
        "revision": 1,
        "values": {},
        "decisions": [],
        "updatedAt": now,
    })
}

fn initial_stage_states(
    definitions: &[StageDefinitionEntry],
    operation: WorkflowOperation,
) -> Result<Vec<StageStateData>, WorkflowServiceError> {
    let order = topological_stage_ids(definitions, operation)?;
    Ok(order
        .into_iter()
        .map(|stage_id| {
            let definition = definitions
                .iter()
                .find(|definition| definition.stage_id == stage_id)
                .expect("topological id comes from definitions");
            StageStateData {
                stage_id,
                status: if definition.dependencies.is_empty() {
                    StageStatusData::Ready
                } else {
                    StageStatusData::Draft
                },
                approved_run_id: None,
                latest_run_id: None,
                stale_because_stage_ids: Vec::new(),
            }
        })
        .collect())
}

fn deserialize_stage_states(
    value: Value,
    operation: WorkflowOperation,
) -> Result<Vec<StageStateData>, WorkflowServiceError> {
    serde_json::from_value(value).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            format!("项目 stages 无法解析为阶段状态：{error}"),
        )
    })
}

fn validate_stage_state_membership(
    states: &[StageStateData],
    definitions: &[StageDefinitionEntry],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let state_ids = states
        .iter()
        .map(|state| state.stage_id.as_str())
        .collect::<BTreeSet<_>>();
    let definition_ids = definitions
        .iter()
        .map(|definition| definition.stage_id.as_str())
        .collect::<BTreeSet<_>>();
    if state_ids.len() != states.len() || state_ids != definition_ids {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidStageGraph,
            operation,
            "项目阶段状态必须与工作流阶段定义一一对应且不能重复。",
        ));
    }
    Ok(())
}

fn validate_current_state_references(
    context: &WorkflowContext,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let stage_ids = context
        .definitions
        .iter()
        .map(|definition| definition.stage_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut run_owners = BTreeMap::<&str, &str>::new();
    for state in &context.states {
        if state
            .stale_because_stage_ids
            .iter()
            .any(|stage_id| !stage_ids.contains(stage_id.as_str()))
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                "阶段状态包含不存在的 stale 直接原因。",
            )
            .for_stage(&state.stage_id));
        }
        for run_id in [
            state.approved_run_id.as_deref(),
            state.latest_run_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>()
        {
            if let Some(existing_stage_id) = run_owners.insert(run_id, &state.stage_id) {
                if existing_stage_id != state.stage_id {
                    return Err(WorkflowServiceError::new(
                        WorkflowErrorCode::InvalidProject,
                        operation,
                        "同一 runId 不能同时作为两个阶段的当前运行引用。",
                    )
                    .for_stage(&state.stage_id)
                    .for_run(run_id));
                }
            }
            let run = read_stage_run(&context.project_dir, &state.stage_id, run_id, operation)?;
            validate_current_project_run(&run, context, &state.stage_id, run_id, operation)?;
        }
        if let Some(approved_run_id) = state.approved_run_id.as_deref() {
            let run = read_stage_run(
                &context.project_dir,
                &state.stage_id,
                approved_run_id,
                operation,
            )?;
            if run.get("status").and_then(Value::as_str) != Some("succeeded") {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "approvedRunId 只能引用 succeeded 的 StageRun。",
                )
                .for_stage(&state.stage_id)
                .for_run(approved_run_id));
            }
        }
    }
    Ok(())
}

fn validate_stage_graph(
    definitions: &[StageDefinitionEntry],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if definitions.is_empty() || definitions.len() > MAX_WORKFLOW_STAGES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidStageGraph,
            operation,
            format!("阶段图必须包含 1 到 {MAX_WORKFLOW_STAGES} 个阶段。"),
        ));
    }
    let ids = definitions
        .iter()
        .map(|definition| definition.stage_id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != definitions.len() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidStageGraph,
            operation,
            "阶段图包含重复 stageId。",
        ));
    }
    for definition in definitions {
        validate_portable_component(&definition.stage_id, operation)?;
        if definition
            .dependencies
            .iter()
            .any(|dependency| dependency == &definition.stage_id)
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidStageGraph,
                operation,
                "阶段不能依赖自身。",
            )
            .for_stage(&definition.stage_id));
        }
        if definition
            .dependencies
            .iter()
            .any(|dependency| !ids.contains(dependency.as_str()))
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidStageGraph,
                operation,
                "阶段依赖指向不存在的 stageId。",
            )
            .for_stage(&definition.stage_id));
        }
    }
    topological_stage_ids(definitions, operation).map(|_| ())
}

fn topological_stage_ids(
    definitions: &[StageDefinitionEntry],
    operation: WorkflowOperation,
) -> Result<Vec<String>, WorkflowServiceError> {
    let mut indegree = definitions
        .iter()
        .map(|definition| (definition.stage_id.clone(), definition.dependencies.len()))
        .collect::<BTreeMap<_, _>>();
    let mut queue = definitions
        .iter()
        .filter(|definition| definition.dependencies.is_empty())
        .map(|definition| definition.stage_id.clone())
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(definitions.len());
    while let Some(stage_id) = queue.pop_front() {
        order.push(stage_id.clone());
        for dependent in definitions
            .iter()
            .filter(|definition| definition.dependencies.contains(&stage_id))
        {
            let value = indegree
                .get_mut(&dependent.stage_id)
                .expect("all stage ids initialized");
            *value -= 1;
            if *value == 0 {
                queue.push_back(dependent.stage_id.clone());
            }
        }
    }
    if order.len() != definitions.len() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidStageGraph,
            operation,
            "阶段依赖图包含环。",
        ));
    }
    Ok(order)
}

fn recompute_stage_states(
    context: &WorkflowContext,
    operation: WorkflowOperation,
) -> Result<Vec<StageStateData>, WorkflowServiceError> {
    let order = topological_stage_ids(&context.definitions, operation)?;
    let existing = context
        .states
        .iter()
        .map(|state| (state.stage_id.clone(), state))
        .collect::<BTreeMap<_, _>>();
    let mut computed = BTreeMap::<String, StageStateData>::new();
    for stage_id in order {
        let definition = context
            .definitions
            .iter()
            .find(|definition| definition.stage_id == stage_id)
            .expect("topological id comes from definitions");
        let previous = existing.get(&stage_id).ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                "阶段状态缺失。",
            )
            .for_stage(&stage_id)
        })?;
        let mut state = (*previous).clone();
        state.stale_because_stage_ids.clear();

        if let Some(approved_run_id) = state.approved_run_id.clone() {
            let run = read_stage_run(&context.project_dir, &stage_id, &approved_run_id, operation)?;
            validate_current_project_run(&run, context, &stage_id, &approved_run_id, operation)?;
            if run.get("status").and_then(Value::as_str) != Some("succeeded") {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "approvedRunId 只能引用 succeeded 的 StageRun。",
                )
                .for_stage(&stage_id)
                .for_run(&approved_run_id));
            }
            let current_config = context.configs.get(&stage_id).ok_or_else(|| {
                WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "阶段配置缺失。",
                )
                .for_stage(&stage_id)
            })?;
            let current_config_hash = hash_json(current_config, operation)?;
            if run.get("configHash").and_then(Value::as_str) != Some(current_config_hash.as_str()) {
                state.stale_because_stage_ids.push(stage_id.clone());
            }
            for dependency in &definition.dependencies {
                let dependency_state = computed
                    .get(dependency)
                    .expect("dependencies precede dependent stage");
                let dependency_approved = dependency_state.approved_run_id.as_deref();
                let dependency_usable = dependency_approved.is_some()
                    && dependency_state.stale_because_stage_ids.is_empty();
                if !dependency_usable
                    || !run_references_source_run(&run, dependency_approved.expect("checked some"))
                {
                    state.stale_because_stage_ids.push(dependency.clone());
                }
            }
            state.stale_because_stage_ids.sort();
            state.stale_because_stage_ids.dedup();
            if state.stale_because_stage_ids.is_empty() {
                let pending_success = match state.latest_run_id.as_deref() {
                    Some(latest) if latest != approved_run_id => {
                        let latest_run =
                            read_stage_run(&context.project_dir, &stage_id, latest, operation)?;
                        validate_current_project_run(
                            &latest_run,
                            context,
                            &stage_id,
                            latest,
                            operation,
                        )?;
                        latest_run.get("status").and_then(Value::as_str) == Some("succeeded")
                    }
                    _ => false,
                };
                state.status = if pending_success {
                    StageStatusData::NeedsReview
                } else {
                    StageStatusData::Approved
                };
            } else {
                state.status = StageStatusData::Stale;
            }
        } else {
            let dependencies_ready = definition.dependencies.iter().all(|dependency| {
                computed.get(dependency).is_some_and(|state| {
                    state.approved_run_id.is_some() && state.stale_because_stage_ids.is_empty()
                })
            });
            state.status = match state.latest_run_id.as_deref() {
                Some(run_id) => {
                    let latest_run =
                        read_stage_run(&context.project_dir, &stage_id, run_id, operation)?;
                    validate_current_project_run(
                        &latest_run,
                        context,
                        &stage_id,
                        run_id,
                        operation,
                    )?;
                    match latest_run.get("status").and_then(Value::as_str) {
                        Some("succeeded") => StageStatusData::NeedsReview,
                        Some("failed") => StageStatusData::Failed,
                        Some("canceled") => {
                            if dependencies_ready {
                                StageStatusData::Ready
                            } else {
                                StageStatusData::Draft
                            }
                        }
                        _ => {
                            return Err(WorkflowServiceError::new(
                                WorkflowErrorCode::InvalidProject,
                                operation,
                                "阶段状态引用的运行不是终态运行。",
                            )
                            .for_stage(&stage_id)
                            .for_run(run_id));
                        }
                    }
                }
                None => {
                    if dependencies_ready {
                        StageStatusData::Ready
                    } else {
                        StageStatusData::Draft
                    }
                }
            };
        }
        computed.insert(stage_id, state);
    }
    Ok(context
        .definitions
        .iter()
        .filter_map(|definition| computed.remove(&definition.stage_id))
        .collect())
}

fn regeneration_impact(
    context: &WorkflowContext,
    changed_stage_ids: &[String],
    operation: WorkflowOperation,
) -> Result<Vec<AffectedStageData>, WorkflowServiceError> {
    if changed_stage_ids.is_empty() || changed_stage_ids.len() > MAX_WORKFLOW_STAGES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            "changedStageIds 必须包含 1 到 64 个阶段。",
        ));
    }
    if changed_stage_ids.iter().collect::<BTreeSet<_>>().len() != changed_stage_ids.len() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            "changedStageIds 不能包含重复阶段。",
        ));
    }
    let mut distances = BTreeMap::<String, u32>::new();
    let mut queue = VecDeque::new();
    for stage_id in changed_stage_ids {
        context.require_stage(stage_id, operation)?;
        if distances.insert(stage_id.clone(), 0).is_none() {
            queue.push_back(stage_id.clone());
        }
    }
    while let Some(stage_id) = queue.pop_front() {
        let next_distance = distances[&stage_id] + 1;
        for dependent in context
            .definitions
            .iter()
            .filter(|definition| definition.dependencies.contains(&stage_id))
        {
            let should_update = distances
                .get(&dependent.stage_id)
                .is_none_or(|distance| next_distance < *distance);
            if should_update {
                distances.insert(dependent.stage_id.clone(), next_distance);
                queue.push_back(dependent.stage_id.clone());
            }
        }
    }
    let definition_index = context
        .definitions
        .iter()
        .enumerate()
        .map(|(index, definition)| (definition.stage_id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut affected = distances
        .iter()
        .map(|(stage_id, distance)| {
            let definition = context
                .definitions
                .iter()
                .find(|definition| definition.stage_id == *stage_id)
                .expect("distance ids come from graph");
            let state = context.state(stage_id).expect("state membership validated");
            let mut direct_cause_stage_ids = if *distance == 0 {
                vec![stage_id.clone()]
            } else {
                definition
                    .dependencies
                    .iter()
                    .filter(|dependency| {
                        distances
                            .get(*dependency)
                            .is_some_and(|dependency_distance| dependency_distance < distance)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };
            direct_cause_stage_ids.sort();
            AffectedStageData {
                stage_id: stage_id.clone(),
                distance: *distance,
                direct_cause_stage_ids,
                current_status: state.status,
                has_approved_run: state.approved_run_id.is_some(),
                supports_partial_regeneration: definition.supports_partial_regeneration,
            }
        })
        .collect::<Vec<_>>();
    affected.sort_by_key(|stage| {
        (
            stage.distance,
            definition_index
                .get(stage.stage_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
        )
    });
    Ok(affected)
}

fn validate_stage_ready_for_run(
    context: &WorkflowContext,
    definition: &StageDefinitionEntry,
    input_refs: &[Value],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if !definition.requires_approved_inputs {
        return Ok(());
    }
    for dependency in &definition.dependencies {
        let dependency_state = context
            .state(dependency)
            .expect("state membership validated");
        let approved_run_id = dependency_state.approved_run_id.as_deref().ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                operation,
                "上游阶段尚未批准。",
            )
            .for_stage(&definition.stage_id)
        })?;
        if !dependency_state.stale_because_stage_ids.is_empty()
            || !input_refs_reference_source_run(input_refs, approved_run_id)
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                operation,
                format!("输入引用未绑定上游阶段 {dependency} 的当前批准运行。"),
            )
            .for_stage(&definition.stage_id));
        }
    }
    Ok(())
}

fn run_matches_current_inputs(
    context: &WorkflowContext,
    stage_id: &str,
    run: &Value,
    operation: WorkflowOperation,
) -> Result<bool, WorkflowServiceError> {
    let definition = context.require_stage(stage_id, operation)?;
    let run_id = required_string(run, "runId", operation)?;
    validate_current_project_run(run, context, stage_id, &run_id, operation)?;
    let config = context.configs.get(stage_id).ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "阶段配置缺失。",
        )
        .for_stage(stage_id)
    })?;
    if run.get("configHash").and_then(Value::as_str) != Some(hash_json(config, operation)?.as_str())
    {
        return Ok(false);
    }
    for dependency in &definition.dependencies {
        let dependency_state = context
            .state(dependency)
            .expect("state membership validated");
        let Some(approved_run_id) = dependency_state.approved_run_id.as_deref() else {
            return Ok(false);
        };
        if !dependency_state.stale_because_stage_ids.is_empty()
            || !run_references_source_run(run, approved_run_id)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn run_references_source_run(run: &Value, run_id: &str) -> bool {
    run.get("inputRefs")
        .and_then(Value::as_array)
        .is_some_and(|refs| input_refs_reference_source_run(refs, run_id))
}

fn input_refs_reference_source_run(input_refs: &[Value], run_id: &str) -> bool {
    input_refs
        .iter()
        .any(|reference| reference.get("sourceRunId").and_then(Value::as_str) == Some(run_id))
}

fn read_stage_run(
    project_dir: &Path,
    stage_id: &str,
    run_id: &str,
    operation: WorkflowOperation,
) -> Result<Value, WorkflowServiceError> {
    let path = stage_run_path(project_dir, stage_id, run_id);
    let run = read_json_file(
        project_dir,
        &path,
        operation,
        WorkflowErrorCode::RunNotFound,
    )?;
    validate_persistent_document(&run, operation, "阶段运行")
        .map_err(|error| error.at_path(&path))?;
    validate_stage_run_hashes(&run, operation).map_err(|error| error.at_path(&path))?;
    if run.get("documentType").and_then(Value::as_str) != Some("stage_run")
        || run.get("stageId").and_then(Value::as_str) != Some(stage_id)
        || run.get("runId").and_then(Value::as_str) != Some(run_id)
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "运行文件路径与 StageRun 身份不一致。",
        )
        .at_path(&path)
        .for_stage(stage_id)
        .for_run(run_id));
    }
    Ok(run)
}

fn validate_current_project_run(
    run: &Value,
    context: &WorkflowContext,
    stage_id: &str,
    run_id: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if run.get("projectId").and_then(Value::as_str) != Some(context.descriptor.project_id.as_str())
        || run.get("stageId").and_then(Value::as_str) != Some(stage_id)
        || run.get("runId").and_then(Value::as_str) != Some(run_id)
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "当前阶段状态引用的 StageRun 不属于该项目或阶段。",
        )
        .for_stage(stage_id)
        .for_run(run_id));
    }
    Ok(())
}

fn validate_stage_run_hashes(
    run: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let input_refs = run.get("inputRefs").cloned().ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "StageRun 缺少 inputRefs。",
        )
    })?;
    let config_snapshot = run.get("configSnapshot").ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "StageRun 缺少 configSnapshot。",
        )
    })?;
    let input_hash = hash_json(&input_refs, operation)?;
    let config_hash = hash_json(config_snapshot, operation)?;
    let idempotency_key = hash_json(
        &json!({
            "stageId": required_string(run, "stageId", operation)?,
            "inputHash": input_hash,
            "configHash": config_hash,
            "executor": run.get("executor").cloned().ok_or_else(|| {
                WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "StageRun 缺少 executor。",
                )
            })?,
        }),
        operation,
    )?;
    if run.get("inputHash").and_then(Value::as_str) != Some(input_hash.as_str())
        || run.get("configHash").and_then(Value::as_str) != Some(config_hash.as_str())
        || run.get("idempotencyKey").and_then(Value::as_str) != Some(idempotency_key.as_str())
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "StageRun 的 inputHash、configHash 或 idempotencyKey 与不可变快照不一致。",
        ));
    }
    Ok(())
}

fn validate_execution_snapshot_hashes(
    snapshot: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let input_refs = snapshot
        .get("inputRefs")
        .cloned()
        .ok_or_else(|| invalid_snapshot_field(operation, "inputRefs"))?;
    let config_snapshot = snapshot
        .get("configSnapshot")
        .ok_or_else(|| invalid_snapshot_field(operation, "configSnapshot"))?;
    let executor = snapshot
        .get("executor")
        .cloned()
        .ok_or_else(|| invalid_snapshot_field(operation, "executor"))?;
    let input_hash = hash_json(&input_refs, operation)?;
    let config_hash = hash_json(config_snapshot, operation)?;
    let idempotency_key = hash_json(
        &json!({
            "stageId": required_string(snapshot, "stageId", operation)?,
            "inputHash": input_hash,
            "configHash": config_hash,
            "executor": executor,
        }),
        operation,
    )?;
    if snapshot.get("inputHash").and_then(Value::as_str) != Some(input_hash.as_str())
        || snapshot.get("configHash").and_then(Value::as_str) != Some(config_hash.as_str())
        || snapshot.get("idempotencyKey").and_then(Value::as_str) != Some(idempotency_key.as_str())
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "StageExecutionSnapshot 的输入、配置或幂等键哈希不一致。",
        ));
    }
    Ok(())
}

fn validate_execution_snapshot_for_terminal(
    snapshot: &Value,
    descriptor: &ProjectDescriptorData,
    stage_id: &str,
    run_id: &str,
    job_id: &str,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_persistent_document(snapshot, operation, "阶段执行快照")
        .map_err(|error| error.at_path(path))?;
    validate_execution_snapshot_hashes(snapshot, operation).map_err(|error| error.at_path(path))?;
    if snapshot.get("documentType").and_then(Value::as_str) != Some("stage_execution_snapshot")
        || snapshot.get("projectId").and_then(Value::as_str) != Some(descriptor.project_id.as_str())
        || snapshot.get("stageId").and_then(Value::as_str) != Some(stage_id)
        || snapshot.get("runId").and_then(Value::as_str) != Some(run_id)
        || snapshot.get("jobId").and_then(Value::as_str) != Some(job_id)
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::RunConflict,
            operation,
            "终态提交与不可变 StageExecutionSnapshot 的身份不一致。",
        )
        .at_path(path)
        .for_stage(stage_id)
        .for_run(run_id));
    }
    Ok(())
}

fn validate_execution_snapshot_replay(
    existing: &Value,
    descriptor: &ProjectDescriptorData,
    options: &PrepareStageRunOptions,
    expected_config_snapshot: Option<&Value>,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_persistent_document(existing, operation, "既有阶段执行快照")
        .map_err(|error| error.at_path(path))?;
    validate_execution_snapshot_hashes(existing, operation).map_err(|error| error.at_path(path))?;
    let expected = [
        (
            "documentType",
            Value::String("stage_execution_snapshot".to_owned()),
        ),
        ("projectId", Value::String(descriptor.project_id.clone())),
        ("stageId", Value::String(options.stage_id.clone())),
        ("runId", Value::String(options.run_id.clone())),
        ("jobId", Value::String(options.job_id.clone())),
        ("inputRefs", Value::Array(options.input_refs.clone())),
        ("executor", options.executor.clone()),
    ];
    if expected
        .iter()
        .any(|(field, value)| existing.get(*field) != Some(value))
        || expected_config_snapshot
            .is_some_and(|config| existing.get("configSnapshot") != Some(config))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::RunConflict,
            operation,
            "相同 runId 已预留，但执行输入、配置、jobId 或执行器不同。",
        )
        .at_path(path)
        .for_stage(&options.stage_id)
        .for_run(&options.run_id));
    }
    Ok(())
}

fn claim_run_reservation(
    project_dir: &Path,
    reservation_path: &Path,
    descriptor: &ProjectDescriptorData,
    options: &PrepareStageRunOptions,
    candidate: &Value,
    expected_config_snapshot: Option<&Value>,
    operation: WorkflowOperation,
) -> Result<(Value, bool), WorkflowServiceError> {
    match write_immutable_json(project_dir, reservation_path, candidate, operation) {
        Ok(false) => Ok((candidate.clone(), false)),
        Ok(true) => {
            let existing = read_json_file(
                project_dir,
                reservation_path,
                operation,
                WorkflowErrorCode::RunConflict,
            )?;
            validate_execution_snapshot_replay(
                &existing,
                descriptor,
                options,
                expected_config_snapshot,
                reservation_path,
                operation,
            )?;
            Ok((existing, true))
        }
        Err(error) if error.code == WorkflowErrorCode::ImmutableConflict => {
            let existing = read_json_file(
                project_dir,
                reservation_path,
                operation,
                WorkflowErrorCode::RunConflict,
            )?;
            validate_execution_snapshot_replay(
                &existing,
                descriptor,
                options,
                expected_config_snapshot,
                reservation_path,
                operation,
            )?;
            Ok((existing, true))
        }
        Err(error) => Err(error),
    }
}

fn bind_run_reservation_to_snapshot(
    project_dir: &Path,
    reservation_path: &Path,
    snapshot: &Value,
    stage_id: &str,
    run_id: &str,
    operation: WorkflowOperation,
) -> Result<bool, WorkflowServiceError> {
    match write_immutable_json(project_dir, reservation_path, snapshot, operation) {
        Ok(idempotent_replay) => Ok(idempotent_replay),
        Err(error) if error.code == WorkflowErrorCode::ImmutableConflict => {
            Err(WorkflowServiceError::new(
                WorkflowErrorCode::RunConflict,
                operation,
                "全项目 runId 原子预留与阶段执行快照不一致。",
            )
            .at_path(reservation_path)
            .for_stage(stage_id)
            .for_run(run_id))
        }
        Err(error) => Err(error),
    }
}

fn invalid_snapshot_field(operation: WorkflowOperation, field: &str) -> WorkflowServiceError {
    WorkflowServiceError::new(
        WorkflowErrorCode::InvalidProject,
        operation,
        format!("StageExecutionSnapshot 缺少字段 {field}。"),
    )
}

fn validate_run_replay(
    existing: &Value,
    descriptor: &ProjectDescriptorData,
    execution_snapshot: &Value,
    options: &RecordStageRunOptions,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_persistent_document(existing, operation, "既有阶段运行")
        .map_err(|error| error.at_path(path))?;
    validate_stage_run_hashes(existing, operation).map_err(|error| error.at_path(path))?;
    let expected = [
        ("documentType", Value::String("stage_run".to_owned())),
        ("projectId", Value::String(descriptor.project_id.clone())),
        ("stageId", Value::String(options.stage_id.clone())),
        ("runId", Value::String(options.run_id.clone())),
        ("status", Value::String(options.status.as_str().to_owned())),
        ("jobId", Value::String(options.job_id.clone())),
        (
            "stageDefinitionVersion",
            execution_snapshot["stageDefinitionVersion"].clone(),
        ),
        ("inputHash", execution_snapshot["inputHash"].clone()),
        ("configHash", execution_snapshot["configHash"].clone()),
        (
            "idempotencyKey",
            execution_snapshot["idempotencyKey"].clone(),
        ),
        ("inputRefs", execution_snapshot["inputRefs"].clone()),
        (
            "configSnapshot",
            execution_snapshot["configSnapshot"].clone(),
        ),
        ("executor", execution_snapshot["executor"].clone()),
        (
            "artifactIds",
            serde_json::to_value(&options.artifact_ids).expect("string vector serializes"),
        ),
        ("logSummary", options.log_summary.clone()),
    ];
    if expected
        .iter()
        .any(|(field, value)| existing.get(*field) != Some(value))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::RunConflict,
            operation,
            "相同 runId 已存在，但不可变运行载荷与本次请求不同。",
        )
        .at_path(path)
        .for_stage(&options.stage_id)
        .for_run(&options.run_id));
    }
    Ok(())
}

fn validate_review_replay(
    existing: &Value,
    descriptor: &ProjectDescriptorData,
    options: &ReviewStageRunOptions,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_persistent_document(existing, operation, "既有审核记录")
        .map_err(|error| error.at_path(path))?;
    let expected = [
        ("documentType", Value::String("review_record".to_owned())),
        ("projectId", Value::String(descriptor.project_id.clone())),
        ("stageId", Value::String(options.stage_id.clone())),
        ("runId", Value::String(options.run_id.clone())),
        ("reviewId", Value::String(options.review_id.clone())),
        (
            "decision",
            Value::String(options.decision.as_str().to_owned()),
        ),
        (
            "reviewer",
            serde_json::to_value(&options.reviewer).expect("reviewer serializes"),
        ),
        ("comments", Value::String(options.comments.clone())),
        (
            "artifactIds",
            serde_json::to_value(&options.artifact_ids).expect("string vector serializes"),
        ),
    ];
    if expected
        .iter()
        .any(|(field, value)| existing.get(*field) != Some(value))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ReviewConflict,
            operation,
            "相同 reviewId 已存在，但不可变审核载荷与本次请求不同。",
        )
        .at_path(path)
        .for_stage(&options.stage_id)
        .for_run(&options.run_id));
    }
    Ok(())
}

fn ensure_run_id_available(
    context: &WorkflowContext,
    stage_id: &str,
    run_id: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    for definition in &context.definitions {
        if definition.stage_id == stage_id {
            continue;
        }
        let path = context
            .project_dir
            .join("runs")
            .join(&definition.stage_id)
            .join(run_id);
        if inspect_project_path(&context.project_dir, &path, operation)?.is_some() {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::RunConflict,
                operation,
                format!(
                    "runId 已被阶段 {} 预留或使用；StageRun 身份在项目内必须全局唯一。",
                    definition.stage_id
                ),
            )
            .at_path(&path)
            .for_stage(stage_id)
            .for_run(run_id));
        }
    }
    Ok(())
}

fn ensure_review_id_available(
    context: &WorkflowContext,
    review_id: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    for definition in &context.definitions {
        let (_, reviews) =
            scan_stage_history(&context.project_dir, &definition.stage_id, operation)?;
        if reviews
            .iter()
            .any(|review| review.get("reviewId").and_then(Value::as_str) == Some(review_id))
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ReviewConflict,
                operation,
                "reviewId 已被其他审核记录使用；ReviewRecord 身份在项目内必须全局唯一。",
            ));
        }
    }
    Ok(())
}

fn validate_input_references(
    storage_service: &StorageService,
    descriptor: &ProjectDescriptorData,
    context: &WorkflowContext,
    definition: &StageDefinitionEntry,
    input_refs: &[Value],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let mut ref_ids = BTreeSet::new();
    for input_ref in input_refs {
        let ref_id = required_string(input_ref, "refId", operation)?;
        if !ref_ids.insert(ref_id) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidRequest,
                operation,
                "同一次执行中的 InputReference.refId 必须唯一。",
            )
            .for_stage(&definition.stage_id));
        }
        let kind = required_string(input_ref, "kind", operation)?;
        if !definition.input_kinds.contains(&kind) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ArtifactMismatch,
                operation,
                format!(
                    "输入 kind {kind} 不在阶段 {} 的 inputKinds 中。",
                    definition.stage_id
                ),
            )
            .for_stage(&definition.stage_id));
        }
        match input_ref.get("referenceType").and_then(Value::as_str) {
            Some("artifact") => validate_artifact_input_reference(
                storage_service,
                descriptor,
                context,
                definition,
                input_ref,
                operation,
            )?,
            Some("project_document") => validate_project_document_input_reference(
                &context.project_dir,
                input_ref,
                operation,
            )?,
            _ => {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidRequest,
                    operation,
                    "InputReference.referenceType 只能是 artifact 或 project_document。",
                )
                .for_stage(&definition.stage_id));
            }
        }
    }

    for dependency in &definition.dependencies {
        let dependency_state = context
            .state(dependency)
            .expect("stage membership validated");
        let approved_run_id = dependency_state.approved_run_id.as_deref().ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                operation,
                format!("依赖阶段 {dependency} 尚未批准运行。"),
            )
            .for_stage(&definition.stage_id)
        })?;
        let approval_review =
            current_approval_review(context, dependency, approved_run_id, operation)?;
        let approval_review_id = required_string(&approval_review, "reviewId", operation)?;
        let approved_artifacts = required_string_array(
            &read_stage_run(&context.project_dir, dependency, approved_run_id, operation)?,
            "artifactIds",
            operation,
        )?;
        let review_artifacts = required_string_array(&approval_review, "artifactIds", operation)?;
        let has_bound_artifact = input_refs.iter().any(|input_ref| {
            input_ref.get("referenceType").and_then(Value::as_str) == Some("artifact")
                && input_ref.get("sourceRunId").and_then(Value::as_str) == Some(approved_run_id)
                && input_ref.get("reviewRecordId").and_then(Value::as_str)
                    == Some(approval_review_id.as_str())
                && input_ref
                    .get("artifactId")
                    .and_then(Value::as_str)
                    .is_some_and(|artifact_id| {
                        approved_artifacts.iter().any(|id| id == artifact_id)
                            && review_artifacts.iter().any(|id| id == artifact_id)
                    })
        });
        if !has_bound_artifact {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                operation,
                format!(
                    "依赖阶段 {dependency} 必须至少提供一个同时属于已批准 StageRun 与 ReviewRecord 的 Artifact 引用。"
                ),
            )
            .for_stage(&definition.stage_id));
        }
    }
    Ok(())
}

fn validate_artifact_input_reference(
    storage_service: &StorageService,
    descriptor: &ProjectDescriptorData,
    context: &WorkflowContext,
    definition: &StageDefinitionEntry,
    input_ref: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let artifact_id = required_string(input_ref, "artifactId", operation)?;
    let source_run_id = required_string(input_ref, "sourceRunId", operation)?;
    let review_record_id = required_string(input_ref, "reviewRecordId", operation)?;
    let kind = required_string(input_ref, "kind", operation)?;
    let read = storage_service
        .read_artifact_for_workflow_unlocked(descriptor, &artifact_id)
        .map_err(|error| storage_error_to_workflow(error, operation))?;
    let source_stage_id = required_string(&read.artifact, "stageId", operation)?;
    if !definition.dependencies.contains(&source_stage_id) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ArtifactMismatch,
            operation,
            "Artifact 输入必须来自当前阶段声明的直接依赖。",
        )
        .for_stage(&definition.stage_id));
    }
    let source_definition = context.require_stage(&source_stage_id, operation)?;
    let source_run = read_stage_run(
        &context.project_dir,
        &source_stage_id,
        &source_run_id,
        operation,
    )?;
    validate_current_project_run(
        &source_run,
        context,
        &source_stage_id,
        &source_run_id,
        operation,
    )?;
    let source_artifact_ids = required_string_array(&source_run, "artifactIds", operation)?;
    let approval_review =
        current_approval_review(context, &source_stage_id, &source_run_id, operation)?;
    let approval_artifact_ids = required_string_array(&approval_review, "artifactIds", operation)?;
    let identity_matches = read.owner_project_id == descriptor.project_id
        && read.content_available
        && read.artifact.get("projectId").and_then(Value::as_str)
            == Some(descriptor.project_id.as_str())
        && read.artifact.get("runId").and_then(Value::as_str) == Some(source_run_id.as_str())
        && read.artifact.get("kind").and_then(Value::as_str) == Some(kind.as_str())
        && input_ref.get("contentHash").and_then(Value::as_str)
            == read.artifact.get("contentHash").and_then(Value::as_str)
        && approval_review.get("reviewId").and_then(Value::as_str)
            == Some(review_record_id.as_str())
        && source_artifact_ids.iter().any(|id| id == &artifact_id)
        && approval_artifact_ids.iter().any(|id| id == &artifact_id)
        && source_definition.output_kinds.contains(&kind);
    if !identity_matches {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ArtifactMismatch,
            operation,
            "Artifact 输入必须绑定当前项目中已批准运行的不可变产物清单、审核记录、kind 与 contentHash。",
        )
        .for_stage(&definition.stage_id)
        .for_run(source_run_id));
    }
    Ok(())
}

fn current_approval_review(
    context: &WorkflowContext,
    stage_id: &str,
    run_id: &str,
    operation: WorkflowOperation,
) -> Result<Value, WorkflowServiceError> {
    let state = context.state(stage_id).expect("stage membership validated");
    if state.approved_run_id.as_deref() != Some(run_id) || !state.stale_because_stage_ids.is_empty()
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::StageNotReady,
            operation,
            "输入引用的来源运行不是当前有效批准版本。",
        )
        .for_stage(stage_id)
        .for_run(run_id));
    }
    let (_, reviews) = scan_stage_history(&context.project_dir, stage_id, operation)?;
    reviews
        .into_iter()
        .filter(|review| review.get("runId").and_then(Value::as_str) == Some(run_id))
        .max_by_key(document_order_key)
        .filter(|review| review.get("decision").and_then(Value::as_str) == Some("approved"))
        .ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                "当前 approvedRunId 缺少对应的最新批准 ReviewRecord。",
            )
            .for_stage(stage_id)
            .for_run(run_id)
        })
}

fn validate_project_document_input_reference(
    project_dir: &Path,
    input_ref: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let uri = required_string(input_ref, "uri", operation)?;
    let relative = uri.strip_prefix("project://").ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "项目文档引用必须使用 project:// URI。",
        )
    })?;
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "project:// URI 必须是项目内无穿越的正斜杠相对路径。",
        ));
    }
    let path = project_dir.join(relative_path);
    let metadata = inspect_project_path(project_dir, &path, operation)?.ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "项目文档引用不存在。",
        )
        .at_path(&path)
    })?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "项目文档引用必须是 16 MiB 以内的普通文件。",
        )
        .at_path(&path));
    }
    let mut file = File::open(&path).map_err(|error| {
        WorkflowServiceError::io(operation, &path, "读取项目文档引用失败", &error)
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            WorkflowServiceError::io(operation, &path, "计算项目文档哈希失败", &error)
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut actual = String::with_capacity(71);
    actual.push_str("sha256:");
    for byte in digest {
        actual.push_str(&format!("{byte:02x}"));
    }
    if input_ref.get("contentHash").and_then(Value::as_str) != Some(actual.as_str()) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ArtifactMismatch,
            operation,
            "project_document 的 contentHash 与受控项目文件不一致。",
        )
        .at_path(&path));
    }
    Ok(())
}

fn validate_artifacts_for_run(
    storage_service: &StorageService,
    descriptor: &ProjectDescriptorData,
    definition: &StageDefinitionEntry,
    run_id: &str,
    artifact_ids: &[String],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let stage_id = definition.stage_id.as_str();
    let mut unique_ids = BTreeSet::new();
    for artifact_id in artifact_ids {
        if !unique_ids.insert(artifact_id) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ArtifactMismatch,
                operation,
                "StageRun.artifactIds 不能重复。",
            )
            .for_stage(stage_id)
            .for_run(run_id));
        }
        let read = storage_service
            .read_artifact_for_workflow_unlocked(descriptor, artifact_id)
            .map_err(|error| storage_error_to_workflow(error, operation))?;
        if !read.content_available
            || read.artifact.get("projectId").and_then(Value::as_str)
                != Some(descriptor.project_id.as_str())
            || read.artifact.get("stageId").and_then(Value::as_str) != Some(stage_id)
            || read.artifact.get("runId").and_then(Value::as_str) != Some(run_id)
            || !read
                .artifact
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| renderer_output_kind_allowed(definition, kind))
        {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ArtifactMismatch,
                operation,
                "Artifact 必须属于当前项目、阶段和运行，且内容对象必须可用。",
            )
            .for_stage(stage_id)
            .for_run(run_id));
        }
    }
    Ok(())
}

fn renderer_output_kind_allowed(definition: &StageDefinitionEntry, kind: &str) -> bool {
    definition
        .output_kinds
        .iter()
        .any(|allowed| allowed == kind)
        || (definition.stage_id == "render"
            && matches!(
                kind,
                "scene_snapshot" | "rendered_scene" | "rendered_video" | "render_log"
            ))
}

fn validate_artifacts_for_review(
    storage_service: &StorageService,
    descriptor: &ProjectDescriptorData,
    definition: &StageDefinitionEntry,
    run: &Value,
    run_id: &str,
    artifact_ids: &[String],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_artifacts_for_run(
        storage_service,
        descriptor,
        definition,
        run_id,
        artifact_ids,
        operation,
    )?;
    let run_artifact_ids = required_string_array(run, "artifactIds", operation)?;
    if artifact_ids
        .iter()
        .any(|artifact_id| !run_artifact_ids.iter().any(|id| id == artifact_id))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ArtifactMismatch,
            operation,
            "ReviewRecord 只能引用 StageRun 不可变 artifactIds 中的产物。",
        )
        .for_stage(&definition.stage_id)
        .for_run(run_id));
    }
    Ok(())
}

fn validate_required_media_review_closure(
    storage_service: &StorageService,
    descriptor: &ProjectDescriptorData,
    definition: &StageDefinitionEntry,
    run: &Value,
    run_id: &str,
    reviewed_artifact_ids: &[String],
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let required_kinds: &[&str] = match definition.stage_id.as_str() {
        "audio" => &["audio_source", "voice_audio"],
        "captions" => &["captions_source", "captions"],
        _ => return Ok(()),
    };
    let run_artifact_ids = required_string_array(run, "artifactIds", operation)?;
    let mut run_kinds = BTreeSet::new();
    let mut reviewed_kinds = BTreeSet::new();
    for artifact_id in run_artifact_ids {
        let read = storage_service
            .read_artifact_for_workflow_unlocked(descriptor, &artifact_id)
            .map_err(|error| storage_error_to_workflow(error, operation))?;
        let kind = required_string(&read.artifact, "kind", operation)?;
        if required_kinds.contains(&kind.as_str()) {
            run_kinds.insert(kind.clone());
            if reviewed_artifact_ids.iter().any(|id| id == &artifact_id) {
                reviewed_kinds.insert(kind);
            }
        }
    }
    let run_has_complete_pair = required_kinds.iter().all(|kind| run_kinds.contains(*kind));
    if run_has_complete_pair
        && required_kinds
            .iter()
            .any(|kind| !reviewed_kinds.contains(*kind))
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ArtifactMismatch,
            operation,
            "Audio/Captions 批准记录必须同时覆盖原始来源与结构化派生产物。",
        )
        .for_stage(&definition.stage_id)
        .for_run(run_id));
    }
    Ok(())
}

fn latest_run_id_for_stage(
    project_dir: &Path,
    stage_id: &str,
    operation: WorkflowOperation,
) -> Result<Option<String>, WorkflowServiceError> {
    let (runs, _) = scan_stage_history(project_dir, stage_id, operation)?;
    Ok(runs
        .into_iter()
        .max_by_key(document_order_key)
        .and_then(|run| run.get("runId").and_then(Value::as_str).map(str::to_owned)))
}

fn latest_review_key_for_stage(
    project_dir: &Path,
    stage_id: &str,
    operation: WorkflowOperation,
) -> Result<Option<(String, String)>, WorkflowServiceError> {
    let (_, reviews) = scan_stage_history(project_dir, stage_id, operation)?;
    Ok(reviews
        .into_iter()
        .filter_map(|review| {
            Some((
                review.get("createdAt")?.as_str()?.to_owned(),
                review.get("reviewId")?.as_str()?.to_owned(),
            ))
        })
        .max())
}

fn ensure_stage_run_slot_available(
    project_dir: &Path,
    stage_id: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let stage_runs_dir = project_dir.join("runs").join(stage_id);
    let Some(metadata) = inspect_project_path(project_dir, &stage_runs_dir, operation)? else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "阶段运行路径不是目录。",
        )
        .at_path(&stage_runs_dir)
        .for_stage(stage_id));
    }
    let mut slots = 0_usize;
    for entry in fs::read_dir(&stage_runs_dir).map_err(|error| {
        WorkflowServiceError::io(operation, &stage_runs_dir, "读取阶段运行目录失败", &error)
    })? {
        let path = entry
            .map_err(|error| {
                WorkflowServiceError::io(operation, &stage_runs_dir, "遍历阶段运行目录失败", &error)
            })?
            .path();
        if inspect_project_path(project_dir, &path, operation)?.is_some_and(|item| item.is_dir()) {
            slots += 1;
        }
    }
    if slots >= MAX_STAGE_RUNS {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ScanLimitExceeded,
            operation,
            format!("单阶段运行记录已达到同步上限 {MAX_STAGE_RUNS}。"),
        )
        .at_path(&stage_runs_dir)
        .for_stage(stage_id));
    }
    Ok(())
}

fn scan_stage_history(
    project_dir: &Path,
    stage_id: &str,
    operation: WorkflowOperation,
) -> Result<(Vec<Value>, Vec<Value>), WorkflowServiceError> {
    let stage_runs_dir = project_dir.join("runs").join(stage_id);
    match inspect_project_path(project_dir, &stage_runs_dir, operation)? {
        None => return Ok((Vec::new(), Vec::new())),
        Some(metadata) if !metadata.is_dir() => {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "阶段运行路径不是目录。",
            )
            .at_path(&stage_runs_dir)
            .for_stage(stage_id));
        }
        Some(_) => {}
    }
    let entries = fs::read_dir(&stage_runs_dir).map_err(|error| {
        WorkflowServiceError::io(operation, &stage_runs_dir, "读取阶段运行目录失败", &error)
    })?;
    let mut runs = Vec::new();
    let mut reviews = Vec::new();
    let mut run_slots = 0_usize;
    for entry in entries {
        let entry = entry.map_err(|error| {
            WorkflowServiceError::io(operation, &stage_runs_dir, "遍历阶段运行目录失败", &error)
        })?;
        let path = entry.path();
        let metadata = inspect_project_path(project_dir, &path, operation)?.ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::IoError,
                operation,
                "阶段运行目录项在扫描中消失。",
            )
            .at_path(&path)
        })?;
        if !metadata.is_dir() {
            continue;
        }
        let run_id = entry.file_name().into_string().map_err(|_| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "阶段运行目录名必须是 Unicode 可移植 ID。",
            )
            .at_path(&path)
            .for_stage(stage_id)
        })?;
        if !portable_id_is_valid(&run_id, "run_") {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "阶段运行目录名不是合法 runId。",
            )
            .at_path(&path)
            .for_stage(stage_id));
        }
        run_slots += 1;
        if run_slots > MAX_STAGE_RUNS {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ScanLimitExceeded,
                operation,
                format!("单阶段运行记录超过同步扫描上限 {MAX_STAGE_RUNS}。"),
            )
            .at_path(&stage_runs_dir)
            .for_stage(stage_id));
        }
        let run_path = path.join("run.json");
        if inspect_project_path(project_dir, &run_path, operation)?.is_none() {
            let snapshot_path = path.join("execution.json");
            let snapshot = read_json_file(
                project_dir,
                &snapshot_path,
                operation,
                WorkflowErrorCode::RunNotFound,
            )?;
            validate_persistent_document(&snapshot, operation, "阶段执行快照")
                .map_err(|error| error.at_path(&snapshot_path))?;
            validate_execution_snapshot_hashes(&snapshot, operation)
                .map_err(|error| error.at_path(&snapshot_path))?;
            if snapshot.get("documentType").and_then(Value::as_str)
                != Some("stage_execution_snapshot")
                || snapshot.get("stageId").and_then(Value::as_str) != Some(stage_id)
                || snapshot.get("runId").and_then(Value::as_str) != Some(run_id.as_str())
            {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    "执行快照路径与文档身份不一致。",
                )
                .at_path(&snapshot_path)
                .for_stage(stage_id)
                .for_run(run_id));
            }
            continue;
        }
        runs.push(read_stage_run(project_dir, stage_id, &run_id, operation)?);
        if runs.len() > MAX_STAGE_RUNS {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::ScanLimitExceeded,
                operation,
                format!("单阶段运行历史超过同步扫描上限 {MAX_STAGE_RUNS}。"),
            )
            .at_path(&stage_runs_dir)
            .for_stage(stage_id));
        }
        let reviews_dir = path.join("reviews");
        if let Some(metadata) = inspect_project_path(project_dir, &reviews_dir, operation)? {
            if !metadata.is_dir() {
                return Err(WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidPath,
                    operation,
                    "审核记录路径不是目录。",
                )
                .at_path(&reviews_dir));
            }
            for review_entry in fs::read_dir(&reviews_dir).map_err(|error| {
                WorkflowServiceError::io(operation, &reviews_dir, "读取审核记录目录失败", &error)
            })? {
                let review_entry = review_entry.map_err(|error| {
                    WorkflowServiceError::io(
                        operation,
                        &reviews_dir,
                        "遍历审核记录目录失败",
                        &error,
                    )
                })?;
                let review_path = review_entry.path();
                let metadata = inspect_project_path(project_dir, &review_path, operation)?
                    .ok_or_else(|| {
                        WorkflowServiceError::new(
                            WorkflowErrorCode::IoError,
                            operation,
                            "审核记录在扫描中消失。",
                        )
                        .at_path(&review_path)
                    })?;
                if !metadata.is_file()
                    || review_path.extension().and_then(|value| value.to_str()) != Some("json")
                {
                    continue;
                }
                let review_id = review_path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| {
                        WorkflowServiceError::new(
                            WorkflowErrorCode::InvalidPath,
                            operation,
                            "审核记录文件名必须是 Unicode 可移植 ID。",
                        )
                        .at_path(&review_path)
                    })?;
                if !portable_id_is_valid(review_id, "review_") {
                    return Err(WorkflowServiceError::new(
                        WorkflowErrorCode::InvalidPath,
                        operation,
                        "审核记录文件名不是合法 reviewId。",
                    )
                    .at_path(&review_path));
                }
                let review = read_json_file(
                    project_dir,
                    &review_path,
                    operation,
                    WorkflowErrorCode::InvalidProject,
                )?;
                validate_persistent_document(&review, operation, "审核记录")
                    .map_err(|error| error.at_path(&review_path))?;
                if review.get("documentType").and_then(Value::as_str) != Some("review_record")
                    || review.get("reviewId").and_then(Value::as_str) != Some(review_id)
                    || review.get("stageId").and_then(Value::as_str) != Some(stage_id)
                    || review.get("runId").and_then(Value::as_str)
                        != runs
                            .last()
                            .and_then(|run| run.get("runId"))
                            .and_then(Value::as_str)
                {
                    return Err(WorkflowServiceError::new(
                        WorkflowErrorCode::InvalidProject,
                        operation,
                        "审核记录路径与 ReviewRecord 身份不一致。",
                    )
                    .at_path(&review_path));
                }
                reviews.push(review);
                if reviews.len() > MAX_STAGE_REVIEWS {
                    return Err(WorkflowServiceError::new(
                        WorkflowErrorCode::ScanLimitExceeded,
                        operation,
                        format!("单阶段审核历史超过同步扫描上限 {MAX_STAGE_REVIEWS}。"),
                    )
                    .at_path(&reviews_dir)
                    .for_stage(stage_id));
                }
            }
        }
    }
    Ok((runs, reviews))
}

fn invalidated_stage_ids(
    before: &[StageStateData],
    after: &[StageStateData],
    reviewed_stage_id: &str,
) -> Vec<String> {
    let before = before
        .iter()
        .map(|state| (state.stage_id.as_str(), state))
        .collect::<BTreeMap<_, _>>();
    let mut invalidated = after
        .iter()
        .filter(|state| state.stage_id != reviewed_stage_id)
        .filter(|state| {
            let prior = before.get(state.stage_id.as_str());
            state.status == StageStatusData::Stale
                && prior.is_some_and(|prior| prior.status != StageStatusData::Stale)
        })
        .map(|state| state.stage_id.clone())
        .collect::<Vec<_>>();
    invalidated.sort();
    invalidated
}

fn validate_stage_config_identity(
    config: &Value,
    project_id: &str,
    stage_id: &str,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    validate_persistent_document(config, operation, "阶段配置")
        .map_err(|error| error.at_path(path))?;
    if config.get("documentType").and_then(Value::as_str) != Some("stage_config")
        || config.get("projectId").and_then(Value::as_str) != Some(project_id)
        || config.get("stageId").and_then(Value::as_str) != Some(stage_id)
    {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "阶段配置文件路径与配置身份不一致。",
        )
        .at_path(path)
        .for_stage(stage_id));
    }
    Ok(())
}

fn require_supported_workflow(
    descriptor: &ProjectDescriptorData,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if descriptor.workflow_definition_id != STANDARD_WORKFLOW_ID {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::UnsupportedWorkflow,
            operation,
            format!(
                "当前版本仅支持 {STANDARD_WORKFLOW_ID}，项目使用的是 {}。",
                descriptor.workflow_definition_id
            ),
        ));
    }
    Ok(())
}

fn require_project_identity(
    descriptor: &ProjectDescriptorData,
    expected_project_id: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if descriptor.project_id != expected_project_id {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ProjectIdentityMismatch,
            operation,
            "项目路径当前指向的 projectId 与调用方快照不一致。",
        )
        .at_path(Path::new(&descriptor.project_path)));
    }
    Ok(())
}

fn set_marker_states(
    marker: &mut Value,
    states: &[StageStateData],
    updated_at: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    let object = marker.as_object_mut().ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            "项目 marker 不是 JSON 对象。",
        )
    })?;
    object.insert(
        "stages".to_owned(),
        serde_json::to_value(states).map_err(|error| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InternalContractError,
                operation,
                format!("序列化阶段状态失败：{error}"),
            )
        })?,
    );
    object.insert("updatedAt".to_owned(), Value::String(updated_at.to_owned()));
    validate_persistent_document(marker, operation, "项目 marker")
}

fn validate_persistent_document(
    document: &Value,
    operation: WorkflowOperation,
    label: &str,
) -> Result<(), WorkflowServiceError> {
    validate_contract_document(document).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            format!("{label}违反 NarraCut v1 持久化契约：{error}"),
        )
    })
}

fn validate_request_derived_document(
    document: &Value,
    operation: WorkflowOperation,
    label: &str,
) -> Result<(), WorkflowServiceError> {
    validate_contract_document(document).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            format!("{label}：{error}"),
        )
    })
}

fn required_string(
    value: &Value,
    field: &str,
    operation: WorkflowOperation,
) -> Result<String, WorkflowServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                format!("持久化文档缺少字符串字段 {field}。"),
            )
        })
}

fn required_u64(
    value: &Value,
    field: &str,
    operation: WorkflowOperation,
) -> Result<u64, WorkflowServiceError> {
    value.get(field).and_then(Value::as_u64).ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            format!("持久化文档缺少整数数字段 {field}。"),
        )
    })
}

fn required_bool(
    value: &Value,
    field: &str,
    operation: WorkflowOperation,
) -> Result<bool, WorkflowServiceError> {
    value.get(field).and_then(Value::as_bool).ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            format!("持久化文档缺少布尔字段 {field}。"),
        )
    })
}

fn required_string_array(
    value: &Value,
    field: &str,
    operation: WorkflowOperation,
) -> Result<Vec<String>, WorkflowServiceError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkflowServiceError::new(
                WorkflowErrorCode::InvalidProject,
                operation,
                format!("持久化文档缺少数组字段 {field}。"),
            )
        })?
        .iter()
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| {
                WorkflowServiceError::new(
                    WorkflowErrorCode::InvalidProject,
                    operation,
                    format!("持久化文档字段 {field} 包含非字符串成员。"),
                )
            })
        })
        .collect()
}

fn hash_json(value: &Value, operation: WorkflowOperation) -> Result<String, WorkflowServiceError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InternalContractError,
            operation,
            format!("序列化哈希输入失败：{error}"),
        )
    })?;
    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{hex}"))
}

fn current_timestamp(operation: WorkflowOperation) -> Result<String, WorkflowServiceError> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::IoError,
            operation,
            format!("生成 RFC 3339 时间失败：{error}"),
        )
    })
}

fn document_order_key(document: &Value) -> (String, String) {
    (
        document
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        document
            .get("runId")
            .or_else(|| document.get("reviewId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn stage_definition_path(project_dir: &Path, stage_id: &str) -> PathBuf {
    project_dir
        .join("contracts")
        .join("stages")
        .join(format!("{stage_id}.json"))
}

fn stage_config_path(project_dir: &Path, stage_id: &str) -> PathBuf {
    project_dir
        .join("stages")
        .join(stage_id)
        .join("config.json")
}

fn stage_run_path(project_dir: &Path, stage_id: &str, run_id: &str) -> PathBuf {
    project_dir
        .join("runs")
        .join(stage_id)
        .join(run_id)
        .join("run.json")
}

fn stage_execution_snapshot_path(project_dir: &Path, stage_id: &str, run_id: &str) -> PathBuf {
    project_dir
        .join("runs")
        .join(stage_id)
        .join(run_id)
        .join("execution.json")
}

fn run_reservation_path(project_dir: &Path, run_id: &str) -> PathBuf {
    project_dir
        .join("runs")
        .join("reservations")
        .join(format!("{run_id}.json"))
}

fn validate_job_id(value: &str, operation: WorkflowOperation) -> Result<(), WorkflowServiceError> {
    if value.is_empty() || value.len() > 160 || !portable_component_is_valid(value) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            "jobId 只能包含可移植 ASCII 字符且长度不能超过 160。",
        ));
    }
    Ok(())
}

fn validate_portable_id(
    value: &str,
    required_prefix: &str,
    operation: WorkflowOperation,
    label: &str,
) -> Result<(), WorkflowServiceError> {
    value.strip_prefix(required_prefix).ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            format!("{label} 必须以 {required_prefix} 开头。"),
        )
    })?;
    if !portable_id_is_valid(value, required_prefix) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidRequest,
            operation,
            format!("{label} 包含非法字符或长度超限。"),
        ));
    }
    Ok(())
}

fn portable_id_is_valid(value: &str, required_prefix: &str) -> bool {
    value
        .strip_prefix(required_prefix)
        .is_some_and(|suffix| !suffix.is_empty())
        && value.len() <= 160
        && portable_component_is_valid(value)
}

fn validate_portable_component(
    value: &str,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if portable_component_is_valid(value) {
        Ok(())
    } else {
        Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidStageGraph,
            operation,
            "stageId 只能包含 ASCII 字母、数字、点、下划线和连字符。",
        )
        .for_stage(value))
    }
}

fn portable_component_is_valid(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 160
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn read_json_file(
    project_dir: &Path,
    path: &Path,
    operation: WorkflowOperation,
    missing_code: WorkflowErrorCode,
) -> Result<Value, WorkflowServiceError> {
    let metadata = inspect_project_path(project_dir, path, operation)?.ok_or_else(|| {
        WorkflowServiceError::new(missing_code, operation, "所需工作流文档不存在。").at_path(path)
    })?;
    if !metadata.is_file() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "工作流文档路径不是普通文件。",
        )
        .at_path(path));
    }
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ScanLimitExceeded,
            operation,
            format!("工作流 JSON 超过同步读取上限 {MAX_DOCUMENT_BYTES} 字节。"),
        )
        .at_path(path));
    }
    let file = File::open(path).map_err(|error| {
        WorkflowServiceError::io(operation, path, "打开工作流 JSON 失败", &error)
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            WorkflowServiceError::io(operation, path, "读取工作流 JSON 失败", &error)
        })?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ScanLimitExceeded,
            operation,
            "工作流 JSON 在读取期间超过同步上限。",
        )
        .at_path(path));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidProject,
            operation,
            format!("工作流 JSON 无法解析：{error}"),
        )
        .at_path(path)
    })
}

fn write_json_atomic(
    project_dir: &Path,
    path: &Path,
    value: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    inspect_project_path(project_dir, path, operation)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InternalContractError,
            operation,
            format!("序列化工作流 JSON 失败：{error}"),
        )
        .at_path(path)
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ScanLimitExceeded,
            operation,
            format!("工作流 JSON 超过同步写入上限 {MAX_DOCUMENT_BYTES} 字节。"),
        )
        .at_path(path));
    }
    let mut file = AtomicWriteFile::options().open(path).map_err(|error| {
        WorkflowServiceError::io(operation, path, "创建工作流原子写入文件失败", &error)
    })?;
    file.write_all(&bytes).map_err(|error| {
        WorkflowServiceError::io(operation, path, "写入工作流原子临时文件失败", &error)
    })?;
    file.commit().map_err(|error| {
        WorkflowServiceError::io(operation, path, "提交工作流原子文件失败", &error)
    })
}

fn write_immutable_json(
    project_dir: &Path,
    path: &Path,
    value: &Value,
    operation: WorkflowOperation,
) -> Result<bool, WorkflowServiceError> {
    if inspect_project_path(project_dir, path, operation)?.is_some() {
        let existing = read_json_file(
            project_dir,
            path,
            operation,
            WorkflowErrorCode::ImmutableConflict,
        )?;
        if existing == *value {
            return Ok(true);
        }
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ImmutableConflict,
            operation,
            "不可变工作流文档已存在且内容不同。",
        )
        .at_path(path));
    }
    let parent = path.parent().ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "不可变文档缺少父目录。",
        )
        .at_path(path)
    })?;
    inspect_project_path(project_dir, parent, operation)?.ok_or_else(|| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "不可变文档父目录不存在。",
        )
        .at_path(parent)
    })?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InternalContractError,
            operation,
            format!("序列化不可变工作流 JSON 失败：{error}"),
        )
        .at_path(path)
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::ScanLimitExceeded,
            operation,
            format!("不可变工作流 JSON 超过同步写入上限 {MAX_DOCUMENT_BYTES} 字节。"),
        )
        .at_path(path));
    }
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        WorkflowServiceError::io(operation, parent, "创建不可变文档临时文件失败", &error)
    })?;
    temporary.write_all(&bytes).map_err(|error| {
        WorkflowServiceError::io(operation, temporary.path(), "写入不可变文档失败", &error)
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        WorkflowServiceError::io(operation, temporary.path(), "同步不可变文档失败", &error)
    })?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(false),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_json_file(
                project_dir,
                path,
                operation,
                WorkflowErrorCode::ImmutableConflict,
            )?;
            if existing == *value {
                Ok(true)
            } else {
                Err(WorkflowServiceError::new(
                    WorkflowErrorCode::ImmutableConflict,
                    operation,
                    "不可变工作流文档并发创建冲突。",
                )
                .at_path(path))
            }
        }
        Err(error) => Err(WorkflowServiceError::io(
            operation,
            path,
            "提交不可变工作流文档失败",
            &error.error,
        )),
    }
}

fn ensure_project_directories(
    project_dir: &Path,
    components: &[&str],
    operation: WorkflowOperation,
) -> Result<PathBuf, WorkflowServiceError> {
    let mut current = project_dir.to_path_buf();
    for component in components {
        if !portable_component_is_valid(component) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "工作流目录组件不安全。",
            )
            .at_path(&current));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                require_safe_directory_metadata(&current, &metadata, operation)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(WorkflowServiceError::io(
                            operation,
                            &current,
                            "创建工作流目录失败",
                            &error,
                        ));
                    }
                }
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    WorkflowServiceError::io(operation, &current, "检查新工作流目录失败", &error)
                })?;
                require_safe_directory_metadata(&current, &metadata, operation)?;
            }
            Err(error) => {
                return Err(WorkflowServiceError::io(
                    operation,
                    &current,
                    "检查工作流目录失败",
                    &error,
                ));
            }
        }
    }
    Ok(current)
}

fn inspect_project_path(
    project_dir: &Path,
    path: &Path,
    operation: WorkflowOperation,
) -> Result<Option<fs::Metadata>, WorkflowServiceError> {
    let relative = path.strip_prefix(project_dir).map_err(|_| {
        WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "工作流路径逃逸出项目根目录。",
        )
        .at_path(path)
    })?;
    let project_metadata = fs::symlink_metadata(project_dir).map_err(|error| {
        WorkflowServiceError::io(operation, project_dir, "读取项目根目录失败", &error)
    })?;
    require_safe_directory_metadata(project_dir, &project_metadata, operation)?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Ok(Some(project_metadata));
    }
    let mut current = project_dir.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "工作流路径包含非法组件。",
            )
            .at_path(path));
        };
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(WorkflowServiceError::io(
                    operation,
                    &current,
                    "逐级检查工作流路径失败",
                    &error,
                ));
            }
        };
        if metadata_is_link(&metadata) {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::PathContainsSymlink,
                operation,
                "工作流路径不能经过符号链接或重解析点。",
            )
            .at_path(&current));
        }
        let is_final = index + 1 == components.len();
        if !is_final && !metadata.is_dir() {
            return Err(WorkflowServiceError::new(
                WorkflowErrorCode::InvalidPath,
                operation,
                "工作流路径的中间组件不是目录。",
            )
            .at_path(&current));
        }
        if is_final {
            return Ok(Some(metadata));
        }
    }
    unreachable!("non-empty path components return inside loop")
}

fn require_safe_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    operation: WorkflowOperation,
) -> Result<(), WorkflowServiceError> {
    if metadata_is_link(metadata) {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::PathContainsSymlink,
            operation,
            "工作流目录不能是符号链接或重解析点。",
        )
        .at_path(path));
    }
    if !metadata.is_dir() {
        return Err(WorkflowServiceError::new(
            WorkflowErrorCode::InvalidPath,
            operation,
            "工作流目录路径不是目录。",
        )
        .at_path(path));
    }
    Ok(())
}

fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn project_error_to_workflow(
    error: ProjectServiceError,
    operation: WorkflowOperation,
) -> WorkflowServiceError {
    let code = match error.code {
        ProjectErrorCode::InvalidPath | ProjectErrorCode::InvalidName => {
            WorkflowErrorCode::InvalidPath
        }
        ProjectErrorCode::PathContainsSymlink => WorkflowErrorCode::PathContainsSymlink,
        ProjectErrorCode::ProjectNotFound | ProjectErrorCode::MarkerMissing => {
            WorkflowErrorCode::ProjectNotFound
        }
        ProjectErrorCode::MigrationRequired => WorkflowErrorCode::MigrationRequired,
        ProjectErrorCode::UnsupportedNewerVersion => WorkflowErrorCode::UnsupportedNewerVersion,
        ProjectErrorCode::InvalidProject | ProjectErrorCode::MarkerTooLarge => {
            WorkflowErrorCode::InvalidProject
        }
        ProjectErrorCode::IoError => WorkflowErrorCode::IoError,
        _ => WorkflowErrorCode::InternalContractError,
    };
    let mut mapped = WorkflowServiceError::new(code, operation, error.message);
    mapped.path = error.path;
    mapped
}

fn storage_error_to_workflow(
    error: StorageServiceError,
    operation: WorkflowOperation,
) -> WorkflowServiceError {
    let code = match error.code {
        StorageErrorCode::InvalidRequest | StorageErrorCode::InvalidArtifact => {
            WorkflowErrorCode::ArtifactMismatch
        }
        StorageErrorCode::InvalidPath => WorkflowErrorCode::InvalidPath,
        StorageErrorCode::PathContainsSymlink => WorkflowErrorCode::PathContainsSymlink,
        StorageErrorCode::ProjectNotFound => WorkflowErrorCode::ProjectNotFound,
        StorageErrorCode::ProjectIdentityMismatch => WorkflowErrorCode::ProjectIdentityMismatch,
        StorageErrorCode::InvalidProject => WorkflowErrorCode::InvalidProject,
        StorageErrorCode::MigrationRequired => WorkflowErrorCode::MigrationRequired,
        StorageErrorCode::UnsupportedNewerVersion => WorkflowErrorCode::UnsupportedNewerVersion,
        StorageErrorCode::ScanLimitExceeded => WorkflowErrorCode::ScanLimitExceeded,
        StorageErrorCode::IoError => WorkflowErrorCode::IoError,
        StorageErrorCode::InternalContractError => WorkflowErrorCode::InternalContractError,
        _ => WorkflowErrorCode::ArtifactMismatch,
    };
    let mut mapped = WorkflowServiceError::new(code, operation, error.message);
    mapped.path = error.path;
    mapped
}
