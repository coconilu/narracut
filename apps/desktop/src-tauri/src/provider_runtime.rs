use std::{
    collections::{BTreeSet, HashSet},
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use narracut_contracts::{validate_provider_message, ArtifactDraft};
use narracut_core::{
    AcknowledgeCancellationOptions, ClaimJobOptions, ClaimStageJobRequestOptions,
    CompleteJobOptions, EnqueueStageJobOptions, FailJobOptions, GetJobOptions, JobErrorCode,
    JobFailureData, JobService, JobServiceError, JobSnapshotData, JobStatusData, ListJobsOptions,
    PrepareStageRunOptions, RecordJobArtifactOptions, RecoverJobsOptions, RenewJobLeaseOptions,
    ReportJobProgressOptions, RetryPolicyData, StageStatusData, StorageService,
    StoreArtifactFileOptions, WorkflowErrorCode, WorkflowService,
};
use narracut_provider::{
    ProvenanceReferenceData, ProviderError, ProviderErrorCode, ProviderExecutionData,
    ProviderInputArtifactData, ProviderOperation, ProviderService, ScriptGenerationConfigData,
    StructuredProviderRequestData, PROVIDER_API_VERSION,
};
use serde_json::{json, Map, Value};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const MAX_PROVIDER_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROVIDER_TOTAL_INPUT_BYTES: usize = 8 * 1024 * 1024;
const PROVIDER_LEASE_MS: u64 = 180_000;
const PROVIDER_LEASE_HEARTBEAT_MS: u64 = PROVIDER_LEASE_MS / 3;
const WORKER_POLL_MS: u64 = 250;
const RECOVERY_POLL_MS: u64 = 1_000;
const PROVIDER_JOB_SCAN_LIMIT: u32 = 200;
const RECENT_PROJECT_SCAN_LIMIT: u32 = 25;

#[derive(Debug, Clone)]
pub struct ScriptEnqueueOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub provider_id: String,
    pub model: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub language: String,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ScriptEnqueueOutcome {
    pub owner_project_id: String,
    pub provider_request_id: String,
    pub job_id: String,
    pub run_id: String,
    pub status: JobStatusData,
}

#[derive(Clone)]
pub struct ProviderRuntime {
    provider: ProviderService,
    jobs: JobService,
    storage: StorageService,
    workflow: WorkflowService,
    active_jobs: Arc<Mutex<HashSet<String>>>,
}

impl ProviderRuntime {
    pub fn new(
        provider: ProviderService,
        jobs: JobService,
        storage: StorageService,
        workflow: WorkflowService,
    ) -> Self {
        Self {
            provider,
            jobs,
            storage,
            workflow,
            active_jobs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn provider(&self) -> &ProviderService {
        &self.provider
    }

    pub fn enqueue_script_stage(
        &self,
        options: ScriptEnqueueOptions,
    ) -> Result<ScriptEnqueueOutcome, ProviderError> {
        if options.provider_id.is_empty()
            || options.model.is_empty()
            || options.run_id.is_empty()
            || options.idempotency_key.is_empty()
        {
            return Err(provider_error(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::EnqueueScriptStage,
                "脚本任务的 Provider、模型、runId 与幂等键不能为空。",
                false,
            ));
        }
        let enqueue_request = script_enqueue_request(&options);
        validate_provider_message(&enqueue_request).map_err(|error| {
            provider_error(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::EnqueueScriptStage,
                format!("脚本 enqueue 请求不符合 Provider v1 契约：{error}"),
                false,
            )
        })?;
        let claim = self
            .jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                idempotency_key: options.idempotency_key.clone(),
                request: enqueue_request.clone(),
            })
            .map_err(provider_enqueue_job_error)?;
        validate_provider_message(&claim.request).map_err(|error| {
            provider_error(
                ProviderErrorCode::JobError,
                ProviderOperation::EnqueueScriptStage,
                format!("持久化 enqueue 请求不符合 Provider v1 契约：{error}"),
                false,
            )
        })?;

        let frozen = match self.workflow.get_stage_execution_snapshot(
            &options.project_path,
            &options.expected_project_id,
            "script",
            &options.run_id,
            &claim.job_id,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) if error.code == WorkflowErrorCode::RunNotFound => {
                let catalog = self.provider.catalog()?;
                let supported = catalog.providers.iter().any(|provider| {
                    provider.provider_id == options.provider_id
                        && provider.models.iter().any(|model| {
                            model.model_id == options.model
                                && model
                                    .supported_tasks
                                    .iter()
                                    .any(|task| task == "script_generation")
                                && model.structured_outputs
                                && options.max_output_tokens <= model.max_output_tokens
                        })
                });
                if !supported {
                    return Err(provider_error(
                        ProviderErrorCode::InvalidRequest,
                        ProviderOperation::EnqueueScriptStage,
                        "所选 Provider/模型不支持结构化脚本任务或输出上限。",
                        false,
                    )
                    .for_provider(&options.provider_id));
                }
                if !self
                    .provider
                    .credential_status(&options.provider_id)?
                    .configured
                {
                    return Err(provider_error(
                        ProviderErrorCode::CredentialMissing,
                        ProviderOperation::EnqueueScriptStage,
                        "请先把 API Key 保存到系统凭据库。",
                        false,
                    )
                    .for_provider(&options.provider_id));
                }
                let input_refs = self.resolve_approved_research_inputs(
                    &options.project_path,
                    &options.expected_project_id,
                )?;
                let config_snapshot = self.script_config_snapshot(
                    &options.project_path,
                    &options.language,
                    options.max_output_tokens,
                )?;
                let executor = json!({
                    "providerId": options.provider_id,
                    "providerVersion": PROVIDER_API_VERSION,
                    "executionMode": "remote_api",
                    "model": options.model,
                });
                self.workflow
                    .prepare_stage_run_with_config_snapshot(
                        PrepareStageRunOptions {
                            project_path: options.project_path.clone(),
                            expected_project_id: options.expected_project_id.clone(),
                            stage_id: "script".to_owned(),
                            run_id: options.run_id.clone(),
                            job_id: claim.job_id.clone(),
                            input_refs,
                            executor,
                        },
                        config_snapshot,
                    )
                    .map_err(workflow_error)?
                    .execution_snapshot
            }
            Err(error) => return Err(workflow_error(error)),
        };
        let input_refs = frozen
            .get("inputRefs")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| workflow_message("StageExecutionSnapshot 缺少 inputRefs。"))?;
        let executor = frozen
            .get("executor")
            .cloned()
            .ok_or_else(|| workflow_message("StageExecutionSnapshot 缺少 executor。"))?;
        let snapshot = self
            .jobs
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: options.project_path,
                    expected_project_id: options.expected_project_id,
                    stage_id: "script".to_owned(),
                    run_id: options.run_id.clone(),
                    input_refs,
                    executor,
                    idempotency_key: options.idempotency_key,
                    retry_policy: RetryPolicyData {
                        max_attempts: 3,
                        initial_backoff_ms: 1_000,
                        backoff_multiplier: 2,
                        max_backoff_ms: 15_000,
                    },
                },
                enqueue_request,
            )
            .map_err(provider_enqueue_job_error)?;
        let job_id = required_string(&snapshot.job, "jobId")?;
        let run_id = required_string(&snapshot.job, "stageRunId")?;
        Ok(ScriptEnqueueOutcome {
            owner_project_id: snapshot.owner_project_id,
            provider_request_id: provider_request_id(&job_id),
            job_id,
            run_id,
            status: snapshot.status,
        })
    }

    pub fn schedule_supported_job(
        &self,
        project_path: String,
        project_id: String,
        job_id: String,
    ) -> Result<bool, ProviderError> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.clone(),
                expected_project_id: project_id.clone(),
                job_id: job_id.clone(),
            })
            .map_err(job_service_error)?;
        if snapshot.status.is_terminal() || !self.supports_provider_job(&snapshot) {
            return Ok(false);
        }
        Ok(self.schedule(project_path, project_id, job_id))
    }

    pub fn resume_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, ProviderError> {
        self.jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
            })
            .map_err(job_service_error)?;
        self.schedule_project_jobs(project_path, project_id)
    }

    pub fn schedule_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, ProviderError> {
        let jobs = self
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                statuses: vec![
                    JobStatusData::Queued,
                    JobStatusData::Running,
                    JobStatusData::Retrying,
                ],
                limit: PROVIDER_JOB_SCAN_LIMIT,
            })
            .map_err(job_service_error)?;
        let mut scheduled = 0;
        for snapshot in jobs.jobs {
            if !self.supports_provider_job(&snapshot) {
                continue;
            }
            let job_id = required_string(&snapshot.job, "jobId")?;
            if self.schedule(project_path.to_owned(), project_id.to_owned(), job_id) {
                scheduled += 1;
            }
        }
        Ok(scheduled)
    }

    pub fn resume_recent_projects(&self) -> usize {
        let Ok(recent) = self
            .storage
            .list_recent_projects(RECENT_PROJECT_SCAN_LIMIT, false)
        else {
            return 0;
        };
        recent
            .projects
            .into_iter()
            .filter(|project| {
                self.resume_project_jobs(&project.project_path, &project.project_id)
                    .is_ok()
            })
            .count()
    }

    fn schedule(&self, project_path: String, project_id: String, job_id: String) -> bool {
        let key = format!("{project_id}:{job_id}");
        let should_start = self
            .active_jobs
            .lock()
            .map(|mut active| active.insert(key.clone()))
            .unwrap_or(false);
        if !should_start {
            return false;
        }
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            runtime
                .run_until_terminal(&project_path, &project_id, &job_id)
                .await;
            if let Ok(mut active) = runtime.active_jobs.lock() {
                active.remove(&key);
            }
        });
        true
    }

    pub async fn run_until_terminal(&self, project_path: &str, project_id: &str, job_id: &str) {
        let mut next_recovery = tokio::time::Instant::now();
        loop {
            let snapshot = match self.jobs.get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            }) {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            if snapshot.status.is_terminal() {
                return;
            }
            if !self.supports_provider_job(&snapshot) {
                return;
            }
            if matches!(
                snapshot.status,
                JobStatusData::Queued | JobStatusData::Retrying
            ) {
                match self.jobs.claim_job(ClaimJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.to_owned(),
                    worker_id: "worker_openai_api".to_owned(),
                    lease_duration_ms: PROVIDER_LEASE_MS,
                }) {
                    Ok(Some(claimed)) => self.run_claimed(project_path, project_id, claimed).await,
                    Ok(None) => {}
                    Err(_) => return,
                }
            } else if snapshot.status == JobStatusData::Running
                && tokio::time::Instant::now() >= next_recovery
            {
                let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                });
                next_recovery =
                    tokio::time::Instant::now() + Duration::from_millis(RECOVERY_POLL_MS);
            }
            tokio::time::sleep(Duration::from_millis(WORKER_POLL_MS)).await;
        }
    }

    fn supports_provider_job(&self, snapshot: &JobSnapshotData) -> bool {
        if snapshot.historical
            || snapshot.job.get("jobType").and_then(Value::as_str) != Some("stage_run")
            || snapshot.job.get("stageId").and_then(Value::as_str) != Some("script")
        {
            return false;
        }
        let Some(executor) = snapshot.job.get("executor") else {
            return false;
        };
        if executor.get("providerVersion").and_then(Value::as_str) != Some(PROVIDER_API_VERSION)
            || executor.get("executionMode").and_then(Value::as_str) != Some("remote_api")
        {
            return false;
        }
        let Some(provider_id) = executor.get("providerId").and_then(Value::as_str) else {
            return false;
        };
        let Some(model_id) = executor.get("model").and_then(Value::as_str) else {
            return false;
        };
        self.provider.catalog().is_ok_and(|catalog| {
            catalog.providers.iter().any(|provider| {
                provider.provider_id == provider_id
                    && provider.models.iter().any(|model| {
                        model.model_id == model_id
                            && model.structured_outputs
                            && model
                                .supported_tasks
                                .iter()
                                .any(|task| task == "script_generation")
                    })
            })
        })
    }

    async fn run_claimed(&self, project_path: &str, project_id: &str, claimed: JobSnapshotData) {
        let Some(lease_id) = claimed.lease.as_ref().map(|lease| lease.lease_id.clone()) else {
            return;
        };
        let job_id = match required_string(&claimed.job, "jobId") {
            Ok(value) => value,
            Err(error) => {
                self.fail_claimed(project_path, project_id, "unknown", &lease_id, error);
                return;
            }
        };
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.1,
            message: Some("正在验证已审核的 Brief/Research 输入".to_owned()),
        });
        let request = match self.build_request(project_path, project_id, &claimed) {
            Ok(request) => request,
            Err(error) => {
                self.fail_claimed(project_path, project_id, &job_id, &lease_id, error);
                return;
            }
        };
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.35,
            message: Some("正在调用结构化 AI Provider".to_owned()),
        });

        let execution = self
            .execute_cancelable(project_path, project_id, &job_id, &lease_id, &request)
            .await;
        let execution = match execution {
            Ok(Some(execution)) => execution,
            Ok(None) => return,
            Err(error) => {
                self.fail_claimed(project_path, project_id, &job_id, &lease_id, error);
                return;
            }
        };
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.8,
            message: Some("正在保存脚本 Artifact 与用量诊断".to_owned()),
        });
        let artifact_id =
            match self.persist_script_artifact(project_path, project_id, &request, &execution) {
                Ok(artifact_id) => artifact_id,
                Err(error) => {
                    self.fail_claimed(project_path, project_id, &job_id, &lease_id, error);
                    return;
                }
            };
        if let Err(error) = self.jobs.record_job_artifact(RecordJobArtifactOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            artifact_id: artifact_id.clone(),
        }) {
            self.fail_claimed(
                project_path,
                project_id,
                &job_id,
                &lease_id,
                provider_error(
                    ProviderErrorCode::JobError,
                    ProviderOperation::ExecuteProviderRequest,
                    error.to_string(),
                    false,
                ),
            );
            return;
        }
        let usage = &execution.result.usage;
        let log_summary = json!({
            "message": format!(
                "脚本生成完成；input={}，output={}，total={} tokens",
                usage.input_tokens, usage.output_tokens, usage.total_tokens
            ),
            "warnings": [],
            "errors": []
        });
        if let Err(error) = self.jobs.complete_job(CompleteJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            artifact_ids: vec![artifact_id],
            log_summary,
        }) {
            // completion_requested 可能已经提交而终态尚未物化；先让恢复路径幂等完成它。
            let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
            });
            let finalized = self
                .jobs
                .get_job(GetJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.clone(),
                })
                .is_ok_and(|snapshot| snapshot.status.is_terminal());
            if !finalized {
                self.fail_claimed(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    provider_error(
                        ProviderErrorCode::JobError,
                        ProviderOperation::ExecuteProviderRequest,
                        format!("脚本 Job 终态提交失败：{error}"),
                        false,
                    ),
                );
            }
        }
    }

    async fn execute_cancelable(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        request: &StructuredProviderRequestData,
    ) -> Result<Option<ProviderExecutionData>, ProviderError> {
        let execution = self.provider.execute(request);
        tokio::pin!(execution);
        let mut next_heartbeat =
            tokio::time::Instant::now() + Duration::from_millis(PROVIDER_LEASE_HEARTBEAT_MS);
        loop {
            tokio::select! {
                result = &mut execution => return result.map(Some),
                _ = tokio::time::sleep(Duration::from_millis(WORKER_POLL_MS)) => {
                    let snapshot = self.jobs.get_job(GetJobOptions {
                        project_path: project_path.to_owned(),
                        expected_project_id: project_id.to_owned(),
                        job_id: job_id.to_owned(),
                    }).map_err(|error| provider_error(
                        ProviderErrorCode::JobError,
                        ProviderOperation::ExecuteProviderRequest,
                        error.to_string(),
                        false,
                    ))?;
                    if snapshot.cancellation_requested {
                        self.jobs.acknowledge_cancellation(AcknowledgeCancellationOptions {
                            project_path: project_path.to_owned(),
                            expected_project_id: project_id.to_owned(),
                            job_id: job_id.to_owned(),
                            lease_id: lease_id.to_owned(),
                        }).map_err(|error| provider_error(
                            ProviderErrorCode::JobError,
                            ProviderOperation::ExecuteProviderRequest,
                            error.to_string(),
                            false,
                        ))?;
                        return Ok(None);
                    }
                    let now = tokio::time::Instant::now();
                    if now >= next_heartbeat {
                        self.jobs.renew_job_lease(RenewJobLeaseOptions {
                            project_path: project_path.to_owned(),
                            expected_project_id: project_id.to_owned(),
                            job_id: job_id.to_owned(),
                            lease_id: lease_id.to_owned(),
                            lease_duration_ms: PROVIDER_LEASE_MS,
                        }).map_err(|error| provider_error(
                            ProviderErrorCode::JobError,
                            ProviderOperation::ExecuteProviderRequest,
                            format!("无法续期 Provider worker 租约：{error}"),
                            true,
                        ))?;
                        next_heartbeat = now
                            + Duration::from_millis(PROVIDER_LEASE_HEARTBEAT_MS);
                    }
                }
            }
        }
    }

    fn resolve_approved_research_inputs(
        &self,
        project_path: &str,
        expected_project_id: &str,
    ) -> Result<Vec<Value>, ProviderError> {
        let workflow = self
            .workflow
            .get_project_workflow(project_path)
            .map_err(workflow_error)?;
        if workflow.owner_project_id != expected_project_id {
            return Err(provider_error(
                ProviderErrorCode::WorkflowError,
                ProviderOperation::EnqueueScriptStage,
                "项目身份与脚本任务请求不一致。",
                false,
            ));
        }
        let state = workflow
            .stage_states
            .iter()
            .find(|state| state.stage_id == "research")
            .ok_or_else(|| workflow_message("工作流缺少 research 阶段。"))?;
        if state.status != StageStatusData::Approved || !state.stale_because_stage_ids.is_empty() {
            return Err(workflow_message(
                "Research 阶段必须是当前有效的已审核版本。",
            ));
        }
        let run_id = state
            .approved_run_id
            .as_deref()
            .ok_or_else(|| workflow_message("Research 阶段缺少 approvedRunId。"))?;
        let history = self
            .workflow
            .list_stage_history(project_path, "research", 100)
            .map_err(workflow_error)?;
        let run = history
            .runs
            .iter()
            .find(|run| run.get("runId").and_then(Value::as_str) == Some(run_id))
            .ok_or_else(|| workflow_message("找不到当前已审核 Research StageRun。"))?;
        let review = history
            .reviews
            .iter()
            .find(|review| {
                review.get("runId").and_then(Value::as_str) == Some(run_id)
                    && review.get("decision").and_then(Value::as_str) == Some("approved")
            })
            .ok_or_else(|| workflow_message("找不到当前 Research 的批准记录。"))?;
        let review_id = required_string(review, "reviewId")?;
        let run_artifacts = string_array(run, "artifactIds")?;
        let review_artifacts = string_array(review, "artifactIds")?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut refs = Vec::new();
        let mut has_claim_set = false;
        let mut has_evidence_set = false;
        let mut has_provenance = false;
        for artifact_id in run_artifacts {
            if !review_artifacts.contains(&artifact_id) {
                continue;
            }
            let read = self
                .storage
                .get_artifact(project_path, &artifact_id)
                .map_err(storage_error)?;
            if read.owner_project_id != expected_project_id || !read.content_available {
                return Err(storage_message("已审核 Research Artifact 不可读取。"));
            }
            let kind = required_string(&read.artifact, "kind")?;
            if !matches!(kind.as_str(), "claim_set" | "evidence_set") {
                continue;
            }
            if read.artifact.get("stageId").and_then(Value::as_str) != Some("research")
                || read.artifact.get("runId").and_then(Value::as_str) != Some(run_id)
            {
                return Err(storage_message(
                    "已审核 Research Artifact 的 stageId/runId 与批准记录不一致。",
                ));
            }
            self.storage
                .read_artifact_content_bounded(
                    project_path,
                    expected_project_id,
                    &artifact_id,
                    MAX_PROVIDER_INPUT_BYTES,
                )
                .map_err(storage_error)?;
            let provenance = artifact_provenance(&read.artifact)?;
            let claim_ids = provenance
                .iter()
                .map(|reference| reference.claim_id.clone())
                .collect::<BTreeSet<_>>();
            let evidence_refs = provenance
                .iter()
                .map(|reference| reference.evidence_ref.clone())
                .collect::<BTreeSet<_>>();
            has_claim_set |= kind == "claim_set";
            has_evidence_set |= kind == "evidence_set";
            has_provenance |= !provenance.is_empty();
            refs.push(json!({
                "refId": format!("input_{artifact_id}"),
                "referenceType": "artifact",
                "kind": kind,
                "contentHash": required_string(&read.artifact, "contentHash")?,
                "artifactId": artifact_id,
                "sourceRunId": run_id,
                "reviewRecordId": review_id,
                "claimIds": claim_ids.into_iter().collect::<Vec<_>>(),
                "evidenceRefs": evidence_refs.into_iter().collect::<Vec<_>>(),
            }));
        }
        if refs.is_empty() || !has_claim_set || !has_evidence_set || !has_provenance {
            return Err(workflow_message(
                "当前 Research 批准产物必须分别包含已审核、可读取且哈希有效的 claim_set 与 evidence_set，并提供 claimId/evidenceRef provenance 对。",
            ));
        }
        Ok(refs)
    }

    fn build_request(
        &self,
        project_path: &str,
        project_id: &str,
        claimed: &JobSnapshotData,
    ) -> Result<StructuredProviderRequestData, ProviderError> {
        let job_id = required_string(&claimed.job, "jobId")?;
        let run_id = required_string(&claimed.job, "stageRunId")?;
        let frozen = self
            .workflow
            .get_stage_execution_snapshot(project_path, project_id, "script", &run_id, &job_id)
            .map_err(workflow_error)?;
        let executor = frozen
            .get("executor")
            .ok_or_else(|| job_message("StageExecutionSnapshot 缺少 executor。"))?;
        let provider_id = required_string(executor, "providerId")?;
        let model = required_string(executor, "model")?;
        let mut references = frozen
            .get("inputRefs")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| job_message("StageExecutionSnapshot 缺少 inputRefs。"))?;
        let research_run_ids = references
            .iter()
            .filter_map(|reference| {
                reference
                    .get("sourceRunId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        let research_history = self
            .workflow
            .list_stage_history(project_path, "research", 100)
            .map_err(workflow_error)?;
        for research_run_id in research_run_ids {
            if let Some(run) = research_history.runs.iter().find(|run| {
                run.get("runId").and_then(Value::as_str) == Some(research_run_id.as_str())
            }) {
                if let Some(inputs) = run.get("inputRefs").and_then(Value::as_array) {
                    references.extend(
                        inputs
                            .iter()
                            .filter(|reference| {
                                reference.get("referenceType").and_then(Value::as_str)
                                    == Some("artifact")
                                    && reference.get("kind").and_then(Value::as_str)
                                        == Some("brief")
                            })
                            .cloned(),
                    );
                }
            }
        }
        let mut seen = BTreeSet::new();
        let mut inputs = Vec::new();
        let mut total_bytes = 0_usize;
        for reference in references {
            let artifact_id = required_string(&reference, "artifactId")?;
            if !seen.insert(artifact_id.clone()) {
                continue;
            }
            let kind = required_string(&reference, "kind")?;
            if !matches!(kind.as_str(), "brief" | "claim_set" | "evidence_set") {
                return Err(workflow_message(
                    "脚本 Provider 收到未授权的 Artifact kind。",
                ));
            }
            let read = self
                .storage
                .get_artifact(project_path, &artifact_id)
                .map_err(storage_error)?;
            let content_hash = required_string(&reference, "contentHash")?;
            let source_run_id = required_string(&reference, "sourceRunId")?;
            let expected_stage_id = if kind == "brief" { "brief" } else { "research" };
            if read.owner_project_id != project_id
                || read.artifact.get("contentHash").and_then(Value::as_str)
                    != Some(content_hash.as_str())
                || read.artifact.get("kind").and_then(Value::as_str) != Some(kind.as_str())
                || read.artifact.get("stageId").and_then(Value::as_str) != Some(expected_stage_id)
                || read.artifact.get("runId").and_then(Value::as_str)
                    != Some(source_run_id.as_str())
            {
                return Err(storage_message(
                    "Provider 输入 Artifact 的项目、stage/run、kind 或 contentHash 已变化。",
                ));
            }
            let bytes = self
                .storage
                .read_artifact_content_bounded(
                    project_path,
                    project_id,
                    &artifact_id,
                    MAX_PROVIDER_INPUT_BYTES,
                )
                .map_err(storage_error)?;
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| storage_message("Provider 输入总字节数溢出。"))?;
            if total_bytes > MAX_PROVIDER_TOTAL_INPUT_BYTES {
                return Err(storage_message("Provider 输入总量超过 8 MiB 上限。"));
            }
            let content = String::from_utf8(bytes)
                .map_err(|_| storage_message("Provider 输入 Artifact 必须是 UTF-8 文本。"))?;
            let provenance = artifact_provenance(&read.artifact)?;
            inputs.push(ProviderInputArtifactData {
                artifact_id,
                kind,
                content_hash,
                source_run_id,
                review_record_id: required_string(&reference, "reviewRecordId")?,
                provenance,
                content,
            });
        }
        if !(2..=32).contains(&inputs.len()) {
            return Err(workflow_message(
                "结构化脚本请求必须解析出 2..=32 个已审核 Brief/Research Artifact。",
            ));
        }
        let script_config = frozen.get("configSnapshot");
        let language = script_config
            .and_then(|config| config.pointer("/values/language"))
            .and_then(Value::as_str)
            .unwrap_or("zh-CN")
            .to_owned();
        let max_output_tokens = script_config
            .and_then(|config| config.pointer("/values/maxOutputTokens"))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(4096)
            .clamp(256, 32768);
        let target_duration_seconds = script_config
            .and_then(|config| config.pointer("/values/targetDurationSeconds"))
            .and_then(Value::as_f64);
        Ok(StructuredProviderRequestData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_request".to_owned(),
            provider_request_id: provider_request_id(&job_id),
            provider_id,
            model,
            task: "script_generation".to_owned(),
            project_id: project_id.to_owned(),
            stage_id: "script".to_owned(),
            run_id,
            inputs,
            config: ScriptGenerationConfigData {
                language,
                max_output_tokens,
                target_duration_seconds,
            },
            output_schema_version: "narracut.script/v1".to_owned(),
            requested_at: now_timestamp()?,
        })
    }

    fn script_config_snapshot(
        &self,
        project_path: &str,
        language: &str,
        max_output_tokens: u32,
    ) -> Result<Value, ProviderError> {
        let snapshot = self
            .workflow
            .get_project_workflow(project_path)
            .map_err(workflow_error)?;
        let mut current = snapshot
            .configs
            .into_iter()
            .find(|config| config.get("stageId").and_then(Value::as_str) == Some("script"))
            .ok_or_else(|| workflow_message("工作流缺少 script 阶段配置。"))?;
        let mut values = current
            .get("values")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| workflow_message("script 阶段配置 values 无效。"))?;
        values.insert("language".to_owned(), Value::String(language.to_owned()));
        values.insert("maxOutputTokens".to_owned(), Value::from(max_output_tokens));
        current
            .as_object_mut()
            .expect("validated StageConfig is an object")
            .insert("values".to_owned(), Value::Object(values));
        Ok(current)
    }

    fn persist_script_artifact(
        &self,
        project_path: &str,
        project_id: &str,
        request: &StructuredProviderRequestData,
        execution: &ProviderExecutionData,
    ) -> Result<String, ProviderError> {
        let mut temporary = NamedTempFile::new()
            .map_err(|error| storage_message(format!("无法创建脚本 Artifact 临时文件：{error}")))?;
        serde_json::to_writer_pretty(&mut temporary, &execution.result)
            .map_err(|error| storage_message(format!("无法序列化脚本 Provider 结果：{error}")))?;
        temporary
            .write_all(b"\n")
            .map_err(|error| storage_message(format!("无法写入脚本临时文件：{error}")))?;
        temporary
            .flush()
            .map_err(|error| storage_message(format!("无法刷新脚本临时文件：{error}")))?;
        let provenance = script_provenance(&execution.result.output)?;
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "script",
            "runId": request.run_id,
            "kind": "script",
            "mediaType": "application/json",
            "evidenceRole": "expressive_material",
            "source": {
                "origin": "generated",
                "providerId": request.provider_id,
                "model": request.model,
            },
            "provenance": provenance,
        }))
        .map_err(|error| storage_message(format!("脚本 Artifact 草稿无效：{error}")))?;
        let committed = self
            .storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                source_path: temporary.path().to_string_lossy().into_owned(),
                artifact: draft,
            })
            .map_err(storage_error)?;
        required_string(&committed.artifact, "artifactId")
    }

    fn fail_claimed(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        error: ProviderError,
    ) {
        let mut details = Map::new();
        details.insert(
            "operation".to_owned(),
            Value::String(error.operation.as_str().to_owned()),
        );
        if let Some(provider_id) = &error.provider_id {
            details.insert("providerId".to_owned(), Value::String(provider_id.clone()));
        }
        let message = error.message.clone();
        let _ = self.jobs.fail_job(FailJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.to_owned(),
            lease_id: lease_id.to_owned(),
            error: JobFailureData {
                code: error.code.as_str().to_owned(),
                message: message.clone(),
                retryable: error.retryable,
                details,
            },
            log_summary: json!({
                "message": message,
                "warnings": [],
                "errors": [error.code.as_str()]
            }),
        });
    }
}

fn provider_request_id(job_id: &str) -> String {
    format!(
        "provider_request_{}",
        job_id.strip_prefix("job_").unwrap_or(job_id)
    )
}

fn artifact_provenance(artifact: &Value) -> Result<Vec<ProvenanceReferenceData>, ProviderError> {
    let mut pairs = BTreeSet::new();
    for reference in artifact
        .get("provenance")
        .and_then(Value::as_array)
        .ok_or_else(|| storage_message("Artifact 缺少 provenance。"))?
    {
        pairs.insert((
            required_string(reference, "claimId")?,
            required_string(reference, "evidenceRef")?,
        ));
        if pairs.len() > 4096 {
            return Err(storage_message("Artifact provenance 超过 4096 条上限。"));
        }
    }
    Ok(pairs
        .into_iter()
        .map(|(claim_id, evidence_ref)| ProvenanceReferenceData {
            claim_id,
            evidence_ref,
        })
        .collect())
}

fn script_provenance(
    output: &narracut_provider::StructuredScriptOutputData,
) -> Result<Vec<Value>, ProviderError> {
    let mut pairs = BTreeSet::new();
    for segment in &output.segments {
        if segment.provenance.is_empty() {
            return Err(storage_message("脚本片段缺少 provenance 对。"));
        }
        for reference in &segment.provenance {
            pairs.insert((reference.claim_id.clone(), reference.evidence_ref.clone()));
        }
        if pairs.len() > 4096 {
            return Err(storage_message("脚本 provenance 超过 4096 条上限。"));
        }
    }
    Ok(pairs
        .into_iter()
        .map(|(claim_id, evidence_ref)| json!({"claimId": claim_id, "evidenceRef": evidence_ref}))
        .collect())
}

fn now_timestamp() -> Result<String, ProviderError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| job_message(format!("无法生成 Provider 时间戳：{error}")))
}

fn required_string(value: &Value, field: &str) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| job_message(format!("缺少字符串字段 {field}。")))
}

fn string_array(value: &Value, field: &str) -> Result<Vec<String>, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| job_message(format!("缺少数组字段 {field}。")))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| job_message(format!("{field} 必须是字符串数组。")))
        })
        .collect()
}

fn provider_error(
    code: ProviderErrorCode,
    operation: ProviderOperation,
    message: impl Into<String>,
    retryable: bool,
) -> ProviderError {
    ProviderError::new(code, operation, message, retryable)
}

fn script_enqueue_request(options: &ScriptEnqueueOptions) -> Value {
    json!({
        "apiVersion": PROVIDER_API_VERSION,
        "messageType": "script_stage_enqueue_request",
        "projectPath": options.project_path,
        "expectedProjectId": options.expected_project_id,
        "stageId": "script",
        "providerId": options.provider_id,
        "model": options.model,
        "runId": options.run_id,
        "idempotencyKey": options.idempotency_key,
        "language": options.language,
        "maxOutputTokens": options.max_output_tokens,
    })
}

fn provider_enqueue_job_error(error: JobServiceError) -> ProviderError {
    provider_error(
        if error.code == JobErrorCode::IdempotencyConflict {
            ProviderErrorCode::IdempotencyConflict
        } else {
            ProviderErrorCode::JobError
        },
        ProviderOperation::EnqueueScriptStage,
        error.to_string(),
        false,
    )
}

fn job_message(message: impl Into<String>) -> ProviderError {
    provider_error(
        ProviderErrorCode::JobError,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
}

fn workflow_message(message: impl Into<String>) -> ProviderError {
    provider_error(
        ProviderErrorCode::WorkflowError,
        ProviderOperation::EnqueueScriptStage,
        message,
        false,
    )
}

fn storage_message(message: impl Into<String>) -> ProviderError {
    provider_error(
        ProviderErrorCode::StorageError,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
}

fn workflow_error(error: narracut_core::WorkflowServiceError) -> ProviderError {
    workflow_message(error.to_string())
}

fn job_service_error(error: narracut_core::JobServiceError) -> ProviderError {
    job_message(error.to_string())
}

fn storage_error(error: narracut_core::StorageServiceError) -> ProviderError {
    storage_message(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Barrier,
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use narracut_contracts::{validate_provider_message, ArtifactDraft};
    use narracut_core::{
        CancelJobOptions, ClaimJobOptions, ClaimStageJobRequestOptions, CreateProjectOptions,
        EnqueueStageJobOptions, FailJobOptions, GetJobOptions, InitializeWorkflowOptions,
        JobErrorCode, JobFailureData, JobSnapshotData, JobStatusData, ListJobsOptions,
        PrepareStageRunOptions, ProjectDescriptorData, ProjectService, RecordStageRunOptions,
        RecoverJobsOptions, RetryPolicyData, ReviewDecisionData, ReviewStageRunOptions,
        ReviewerReferenceData, StorageService, StoreArtifactFileOptions, TerminalRunStatusData,
        UpdateStageConfigOptions, WorkflowService,
    };
    use narracut_provider::{
        AiProvider, InMemoryCredentialStore, ProvenanceReferenceData, ProviderCapabilityData,
        ProviderError, ProviderErrorCode, ProviderExecutionData, ProviderModelCapabilityData,
        ProviderOperation, ProviderService, ProviderUsageData, ScriptSegmentData, SecretString,
        StructuredProviderRequestData, StructuredProviderResultData, StructuredScriptOutputData,
        PROVIDER_API_VERSION,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{script_enqueue_request, ProviderRuntime, ScriptEnqueueOptions};

    const TEST_SECRET: &str = "sk-test-secret-not-real-123456";

    struct Fixture {
        _temp: TempDir,
        imports: TempDir,
        project: ProjectDescriptorData,
        runtime: ProviderRuntime,
    }

    impl Fixture {
        fn new(provider: Arc<dyn AiProvider>) -> Self {
            Self::new_with_reviewed_research(provider, true, true)
        }

        fn new_with_reviewed_research(
            provider: Arc<dyn AiProvider>,
            approve_claim_set: bool,
            approve_evidence_set: bool,
        ) -> Self {
            let temp = tempfile::tempdir().expect("project parent");
            let imports = tempfile::tempdir().expect("imports");
            let projects = ProjectService::default();
            let project = projects
                .create_project(CreateProjectOptions {
                    parent_path: temp.path().to_string_lossy().into_owned(),
                    directory_name: "provider-worker".to_owned(),
                    name: "Provider worker test".to_owned(),
                    workflow_definition_id: "workflow_standard_v1".to_owned(),
                    default_locale: Some("zh-CN".to_owned()),
                })
                .expect("create project");
            let storage = StorageService::new(temp.path().join("index.sqlite3"), projects.clone());
            let workflow = WorkflowService::new(projects.clone(), storage.clone());
            workflow
                .initialize_project_workflow(InitializeWorkflowOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                })
                .expect("initialize workflow");
            let jobs =
                narracut_core::JobService::new(projects.clone(), storage.clone(), workflow.clone());
            let credentials = Arc::new(InMemoryCredentialStore::default());
            let service = ProviderService::new(credentials, [provider]).expect("provider service");
            service
                .set_credential("openai_api", SecretString::new(TEST_SECRET))
                .expect("test credential");
            let runtime = ProviderRuntime::new(service, jobs, storage, workflow);
            let fixture = Self {
                _temp: temp,
                imports,
                project,
                runtime,
            };
            fixture.prepare_approved_inputs(approve_claim_set, approve_evidence_set);
            fixture
        }

        fn prepare_approved_inputs(&self, approve_claim_set: bool, approve_evidence_set: bool) {
            let brief_run = "run_brief_provider_001";
            let brief_review = "review_brief_provider_001";
            self.runtime
                .workflow
                .prepare_stage_run(prepare_options(
                    &self.project,
                    "brief",
                    brief_run,
                    "1",
                    vec![],
                ))
                .expect("prepare brief");
            let brief_artifact = self.create_artifact(
                "brief",
                brief_run,
                "brief",
                r#"{"goal":"explain lunar dust"}"#,
                vec![],
            );
            self.record_and_approve(
                "brief",
                brief_run,
                brief_review,
                "1",
                vec![brief_artifact.clone()],
                vec![brief_artifact.clone()],
            );
            let brief_meta = self
                .runtime
                .storage
                .get_artifact(&self.project.project_path, &brief_artifact)
                .expect("brief metadata")
                .artifact;
            let brief_ref = json!({
                "refId": "input_brief_provider_001",
                "referenceType": "artifact",
                "kind": "brief",
                "contentHash": brief_meta["contentHash"],
                "artifactId": brief_artifact,
                "sourceRunId": brief_run,
                "reviewRecordId": brief_review,
                "claimIds": [],
                "evidenceRefs": []
            });

            let research_run = "run_research_provider_001";
            let research_review = "review_research_provider_001";
            self.runtime
                .workflow
                .prepare_stage_run(prepare_options(
                    &self.project,
                    "research",
                    research_run,
                    "2",
                    vec![brief_ref],
                ))
                .expect("prepare research");
            let provenance = vec![json!({
                "claimId": "claim_dust",
                "evidenceRef": "evidence_nasa_dust"
            })];
            let claims = self.create_artifact(
                "research",
                research_run,
                "claim_set",
                r#"{"claims":[{"claimId":"claim_dust"}]}"#,
                provenance.clone(),
            );
            let evidence = self.create_artifact(
                "research",
                research_run,
                "evidence_set",
                r#"{"evidence":[{"evidenceRef":"evidence_nasa_dust"}]}"#,
                provenance,
            );
            let mut reviewed_artifacts = Vec::new();
            if approve_claim_set {
                reviewed_artifacts.push(claims.clone());
            }
            if approve_evidence_set {
                reviewed_artifacts.push(evidence.clone());
            }
            self.record_and_approve(
                "research",
                research_run,
                research_review,
                "2",
                vec![claims, evidence],
                reviewed_artifacts,
            );
        }

        fn create_artifact(
            &self,
            stage_id: &str,
            run_id: &str,
            kind: &str,
            content: &str,
            provenance: Vec<Value>,
        ) -> String {
            let path = self.imports.path().join(format!("{run_id}-{kind}.json"));
            fs::write(&path, content).expect("write fixture artifact");
            let draft: ArtifactDraft = serde_json::from_value(json!({
                "stageId": stage_id,
                "runId": run_id,
                "kind": kind,
                "mediaType": "application/json",
                "evidenceRole": "non_evidence",
                "source": {
                    "origin": "generated",
                    "providerId": "fixture",
                    "model": "fixture"
                },
                "provenance": provenance
            }))
            .expect("artifact draft");
            let committed = self
                .runtime
                .storage
                .import_artifact_file(StoreArtifactFileOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    source_path: path.to_string_lossy().into_owned(),
                    artifact: draft,
                })
                .expect("import fixture artifact");
            committed.artifact["artifactId"]
                .as_str()
                .expect("artifact id")
                .to_owned()
        }

        fn record_and_approve(
            &self,
            stage_id: &str,
            run_id: &str,
            review_id: &str,
            job_digit: &str,
            run_artifact_ids: Vec<String>,
            reviewed_artifact_ids: Vec<String>,
        ) {
            self.runtime
                .workflow
                .record_stage_run(RecordStageRunOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.to_owned(),
                    status: TerminalRunStatusData::Succeeded,
                    job_id: format!("job_{}", job_digit.repeat(64)),
                    artifact_ids: run_artifact_ids,
                    log_summary: json!({"message":"fixture", "warnings":[], "errors":[]}),
                })
                .expect("record stage run");
            self.runtime
                .workflow
                .review_stage_run(ReviewStageRunOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.to_owned(),
                    review_id: review_id.to_owned(),
                    decision: ReviewDecisionData::Approved,
                    reviewer: ReviewerReferenceData {
                        kind: "human".to_owned(),
                        reviewer_id: "reviewer_fixture".to_owned(),
                        display_name: "Fixture reviewer".to_owned(),
                    },
                    comments: "approved fixture".to_owned(),
                    artifact_ids: reviewed_artifact_ids,
                })
                .expect("approve stage run");
        }

        fn enqueue(&self, suffix: &str) -> super::ScriptEnqueueOutcome {
            self.runtime
                .enqueue_script_stage(self.enqueue_options(suffix))
                .expect("enqueue script")
        }

        fn enqueue_options(&self, suffix: &str) -> ScriptEnqueueOptions {
            ScriptEnqueueOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                provider_id: "openai_api".to_owned(),
                model: "gpt-5.6-terra".to_owned(),
                run_id: format!("run_script_provider_{suffix}"),
                idempotency_key: format!("idem_script_provider_{suffix}"),
                language: "zh-CN".to_owned(),
                max_output_tokens: 4096,
            }
        }

        fn create_legacy_script_job(
            &self,
            suffix: &str,
        ) -> (ScriptEnqueueOptions, JobSnapshotData) {
            let options = self.enqueue_options(suffix);
            let current = self.stage_config("script");
            let revision = current["revision"].as_u64().expect("script revision") as u32;
            let mut values = current["values"]
                .as_object()
                .expect("script values")
                .clone();
            values.insert(
                "language".to_owned(),
                Value::String(options.language.clone()),
            );
            values.insert(
                "maxOutputTokens".to_owned(),
                Value::from(options.max_output_tokens),
            );
            let decisions = current["decisions"]
                .as_array()
                .expect("script decisions")
                .clone();
            self.runtime
                .workflow
                .update_stage_config(UpdateStageConfigOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                    stage_id: "script".to_owned(),
                    expected_revision: revision,
                    values,
                    decisions,
                })
                .expect("freeze legacy script config");

            let input_refs = self
                .runtime
                .resolve_approved_research_inputs(
                    &options.project_path,
                    &options.expected_project_id,
                )
                .expect("legacy approved inputs");
            let executor = json!({
                "providerId": options.provider_id,
                "providerVersion": PROVIDER_API_VERSION,
                "executionMode": "remote_api",
                "model": options.model,
            });
            let snapshot = self
                .runtime
                .jobs
                .enqueue_stage_job(EnqueueStageJobOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                    stage_id: "script".to_owned(),
                    run_id: options.run_id.clone(),
                    input_refs,
                    executor,
                    idempotency_key: options.idempotency_key.clone(),
                    retry_policy: RetryPolicyData {
                        max_attempts: 3,
                        initial_backoff_ms: 1_000,
                        backoff_multiplier: 2,
                        max_backoff_ms: 15_000,
                    },
                })
                .expect("old API creates script job");
            assert_eq!(
                snapshot
                    .job
                    .get("requestHashVersion")
                    .and_then(Value::as_u64),
                Some(2)
            );
            assert!(snapshot.job.get("requestReceiptHash").is_none());
            let job_id = snapshot.job["jobId"].as_str().expect("legacy job id");
            let job_path = Path::new(&options.project_path)
                .join("jobs")
                .join(job_id)
                .join("job.json");
            let mut legacy_job = snapshot.job.clone();
            legacy_job
                .as_object_mut()
                .expect("job object")
                .remove("requestHashVersion");
            let mut bytes = serde_json::to_vec_pretty(&legacy_job).expect("serialize legacy job");
            bytes.push(b'\n');
            fs::write(&job_path, bytes).expect("replace fixture with legacy JobDefinition");
            let binding_path = Path::new(&options.project_path)
                .join("requests/job-bindings")
                .join(format!("{job_id}.json"));
            fs::remove_file(binding_path).expect("remove post-v2 binding from legacy fixture");
            assert!(!Path::new(&options.project_path)
                .join("requests/jobs")
                .join(format!("{job_id}.json"))
                .exists());
            (options, snapshot)
        }

        fn create_transition_script_job(
            &self,
            suffix: &str,
        ) -> (ScriptEnqueueOptions, JobSnapshotData) {
            let options = self.enqueue_options(suffix);
            let outcome = self
                .runtime
                .enqueue_script_stage(options.clone())
                .expect("create current receipt-backed Job");
            let job_path = Path::new(&options.project_path)
                .join("jobs")
                .join(&outcome.job_id)
                .join("job.json");
            let mut transition_job: Value = serde_json::from_slice(
                &fs::read(&job_path).expect("read current receipt-backed JobDefinition"),
            )
            .expect("parse current receipt-backed JobDefinition");
            let enqueue_request = script_enqueue_request(&options);
            let mut hash_payload = json!({
                "projectId": transition_job["projectId"],
                "stageId": transition_job["stageId"],
                "stageRunId": transition_job["stageRunId"],
                "inputRefs": transition_job["inputRefs"],
                "executor": transition_job["executor"],
                "retryPolicy": transition_job["retryPolicy"],
            });
            hash_payload
                .as_object_mut()
                .expect("transition hash payload")
                .insert("enqueueRequest".to_owned(), enqueue_request);
            let digest = Sha256::digest(
                serde_json::to_vec(&hash_payload).expect("serialize transition hash payload"),
            );
            let request_hash = format!(
                "sha256:{}",
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            let object = transition_job
                .as_object_mut()
                .expect("transition JobDefinition object");
            object.remove("requestHashVersion");
            object.remove("requestReceiptHash");
            object.insert("requestHash".to_owned(), Value::String(request_hash));
            let mut bytes =
                serde_json::to_vec_pretty(&transition_job).expect("serialize transition Job");
            bytes.push(b'\n');
            fs::write(&job_path, bytes).expect("write true 2addb7a transition JobDefinition");
            let binding_path = Path::new(&options.project_path)
                .join("requests/job-bindings")
                .join(format!("{}.json", outcome.job_id));
            fs::remove_file(&binding_path).expect("transition fixture predates binding records");
            assert!(!binding_path.exists());

            let snapshot = self
                .runtime
                .jobs
                .get_job(GetJobOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                    job_id: outcome.job_id,
                })
                .expect("true transition JobDefinition is readable");
            (options, snapshot)
        }

        fn fresh_runtime(&self) -> ProviderRuntime {
            ProviderRuntime::new(
                self.runtime.provider.clone(),
                self.runtime.jobs.clone(),
                self.runtime.storage.clone(),
                self.runtime.workflow.clone(),
            )
        }

        fn stage_config(&self, stage_id: &str) -> Value {
            self.runtime
                .workflow
                .get_project_workflow(&self.project.project_path)
                .expect("workflow snapshot")
                .configs
                .into_iter()
                .find(|config| config.get("stageId").and_then(Value::as_str) == Some(stage_id))
                .expect("stage config")
        }

        fn mark_research_stale(&self) {
            let current = self.stage_config("brief");
            let revision = current["revision"].as_u64().expect("brief revision") as u32;
            let mut values = current["values"].as_object().expect("brief values").clone();
            values.insert("fixtureRevision".to_owned(), Value::from(revision + 1));
            let decisions = current["decisions"]
                .as_array()
                .expect("brief decisions")
                .clone();
            self.runtime
                .workflow
                .update_stage_config(UpdateStageConfigOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    stage_id: "brief".to_owned(),
                    expected_revision: revision,
                    values,
                    decisions,
                })
                .expect("stale research through upstream config change");
            let workflow = self
                .runtime
                .workflow
                .get_project_workflow(&self.project.project_path)
                .expect("stale workflow snapshot");
            let research = workflow
                .stage_states
                .iter()
                .find(|state| state.stage_id == "research")
                .expect("research state");
            assert!(!research.stale_because_stage_ids.is_empty());
        }
    }

    struct MockProvider {
        calls: Arc<AtomicUsize>,
        failures_before_success: usize,
    }

    #[async_trait]
    impl AiProvider for MockProvider {
        fn capability(&self) -> ProviderCapabilityData {
            capability()
        }

        async fn execute(
            &self,
            request: &StructuredProviderRequestData,
            _credential: &SecretString,
        ) -> Result<ProviderExecutionData, ProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.failures_before_success {
                return Err(ProviderError::new(
                    ProviderErrorCode::RateLimited,
                    ProviderOperation::ExecuteProviderRequest,
                    "mock rate limit",
                    true,
                )
                .for_provider("openai_api"));
            }
            assert!(request.inputs.iter().any(|input| input.kind == "brief"));
            assert!(request.inputs.iter().any(|input| input.kind == "claim_set"));
            assert!(request
                .inputs
                .iter()
                .any(|input| input.source_run_id == "run_brief_provider_001"));
            assert!(request
                .inputs
                .iter()
                .filter(|input| matches!(input.kind.as_str(), "claim_set" | "evidence_set"))
                .all(|input| input.source_run_id == "run_research_provider_001"));
            assert_eq!(request.config.language, "zh-CN");
            Ok(completed_execution(request, call + 1))
        }
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct SlowProvider {
        dropped: Arc<AtomicBool>,
    }

    #[async_trait]
    impl AiProvider for SlowProvider {
        fn capability(&self) -> ProviderCapabilityData {
            capability()
        }

        async fn execute(
            &self,
            request: &StructuredProviderRequestData,
            _credential: &SecretString,
        ) -> Result<ProviderExecutionData, ProviderError> {
            let _guard = DropFlag(self.dropped.clone());
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(completed_execution(request, 1))
        }
    }

    #[test]
    fn exact_replay_ignores_deleted_credential_and_stale_research() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let options = fixture.enqueue_options("replay_frozen");
        let original = fixture
            .runtime
            .enqueue_script_stage(options.clone())
            .expect("initial enqueue");
        fixture
            .runtime
            .provider
            .delete_credential("openai_api")
            .expect("delete credential");
        fixture.mark_research_stale();

        let replay = fixture
            .runtime
            .enqueue_script_stage(options)
            .expect("exact replay uses frozen execution");
        assert_eq!(replay.job_id, original.job_id);
        assert_eq!(replay.run_id, original.run_id);
        assert_eq!(replay.status, original.status);
    }

    #[test]
    fn legacy_job_replay_is_exact_and_different_payloads_leave_no_receipt() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let (options, original) = fixture.create_legacy_script_job("legacy_upgrade");
        let job_id = original.job["jobId"].as_str().expect("legacy job id");
        let receipt_path = Path::new(&options.project_path)
            .join("requests/jobs")
            .join(format!("{job_id}.json"));

        let mut changed_language = options.clone();
        changed_language.language = "en-US".to_owned();
        let mut changed_tokens = options.clone();
        changed_tokens.max_output_tokens = 8192;
        let mut changed_provider = options.clone();
        changed_provider.provider_id = "other_provider".to_owned();
        let mut changed_model = options.clone();
        changed_model.model = "other-model".to_owned();
        for changed in [
            changed_language,
            changed_tokens,
            changed_provider,
            changed_model,
        ] {
            let error = fixture
                .runtime
                .enqueue_script_stage(changed)
                .expect_err("legacy differing replay conflicts");
            assert_eq!(error.code, ProviderErrorCode::IdempotencyConflict);
            assert!(!receipt_path.exists());
            assert_eq!(
                fixture
                    .runtime
                    .jobs
                    .get_job(GetJobOptions {
                        project_path: options.project_path.clone(),
                        expected_project_id: options.expected_project_id.clone(),
                        job_id: job_id.to_owned(),
                    })
                    .expect("legacy get remains usable")
                    .status,
                original.status
            );
            fixture
                .runtime
                .jobs
                .recover_project_jobs(RecoverJobsOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                })
                .expect("legacy recovery remains usable");
        }

        fs::create_dir_all(receipt_path.parent().expect("receipt parent"))
            .expect("create erroneous legacy receipt parent");
        fs::write(&receipt_path, b"{\"wrong\":true}\n").expect("write erroneous legacy receipt");
        fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: job_id.to_owned(),
            })
            .expect("legacy hash ignores erroneous side receipt");
        fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect("legacy list ignores erroneous side receipt");
        fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
            })
            .expect("legacy recovery ignores erroneous side receipt");
        let wrong_receipt_error = fixture
            .runtime
            .enqueue_script_stage(options.clone())
            .expect_err("erroneous legacy receipt is never overwritten");
        assert_eq!(
            wrong_receipt_error.code,
            ProviderErrorCode::IdempotencyConflict
        );
        fs::remove_file(&receipt_path).expect("remove erroneous legacy receipt");

        let replay = fixture
            .runtime
            .enqueue_script_stage(options.clone())
            .expect("legacy exact replay attaches receipt");
        assert_eq!(replay.job_id, job_id);
        assert_eq!(replay.run_id, options.run_id);
        assert_eq!(replay.status, original.status);
        let stored: Value = serde_json::from_slice(&fs::read(&receipt_path).expect("read receipt"))
            .expect("parse receipt");
        assert_eq!(stored, script_enqueue_request(&options));

        let fetched = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: job_id.to_owned(),
            })
            .expect("legacy get after receipt");
        assert!(fetched.job.get("requestHashVersion").is_none());
        assert_eq!(fetched.status, original.status);
        let listed = fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect("legacy list after receipt");
        assert!(listed.jobs.iter().any(|job| job.job["jobId"] == job_id));
        fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
            })
            .expect("legacy recover after receipt");
    }

    #[test]
    fn transition_receipt_job_uses_hash_evidence_and_requires_the_exact_receipt() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let (options, original) = fixture.create_transition_script_job("transition_2addb7a");
        let job_id = original.job["jobId"]
            .as_str()
            .expect("transition job id")
            .to_owned();
        assert!(original.job.get("requestHashVersion").is_none());
        assert!(original.job.get("requestReceiptHash").is_none());
        let receipt_path = Path::new(&options.project_path)
            .join("requests/jobs")
            .join(format!("{job_id}.json"));
        let exact_receipt = fs::read(&receipt_path).expect("read transition receipt bytes");

        let listed = fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect("transition Job is listable");
        assert!(listed.jobs.iter().any(|job| job.job["jobId"] == job_id));
        fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
            })
            .expect("transition Job is recoverable");

        let mut changed_language = options.clone();
        changed_language.language = "en-US".to_owned();
        let mut changed_tokens = options.clone();
        changed_tokens.max_output_tokens = 8192;
        let mut changed_provider = options.clone();
        changed_provider.provider_id = "other_provider".to_owned();
        let mut changed_model = options.clone();
        changed_model.model = "other-model".to_owned();
        for changed in [
            changed_language,
            changed_tokens,
            changed_provider,
            changed_model,
        ] {
            let error = fixture
                .runtime
                .enqueue_script_stage(changed)
                .expect_err("different transition replay conflicts");
            assert_eq!(error.code, ProviderErrorCode::IdempotencyConflict);
            assert_eq!(
                fs::read(&receipt_path).expect("transition receipt remains readable"),
                exact_receipt
            );
        }

        let replay = fixture
            .runtime
            .enqueue_script_stage(options.clone())
            .expect("exact transition replay succeeds");
        assert_eq!(replay.job_id, job_id);
        assert_eq!(replay.run_id, options.run_id);
        assert_eq!(replay.status, original.status);

        fs::write(&receipt_path, b"{\"wrong\":true}\n").expect("write wrong transition receipt");
        let wrong_get = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: job_id.clone(),
            })
            .expect_err("wrong transition receipt fails get");
        assert_eq!(wrong_get.code, JobErrorCode::InvalidProject);
        let wrong_list = fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect_err("wrong transition receipt fails list");
        assert_eq!(wrong_list.code, JobErrorCode::InvalidProject);
        let wrong_recover = fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
            })
            .expect_err("wrong transition receipt fails recover");
        assert_eq!(wrong_recover.code, JobErrorCode::InvalidProject);

        fs::write(&receipt_path, &exact_receipt).expect("restore exact transition receipt");
        fs::remove_file(&receipt_path).expect("remove transition receipt");
        let missing_get = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: job_id.clone(),
            })
            .expect_err("missing transition receipt fails get");
        assert_eq!(missing_get.code, JobErrorCode::InvalidProject);
        let missing_list = fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect_err("missing transition receipt fails list");
        assert_eq!(missing_list.code, JobErrorCode::InvalidProject);
        let missing_recover = fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
            })
            .expect_err("missing transition receipt fails recover");
        assert_eq!(missing_recover.code, JobErrorCode::InvalidProject);

        fs::write(&receipt_path, exact_receipt).expect("restore transition receipt after tests");
        let fetched = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                job_id,
            })
            .expect("restored transition Job remains readable");
        assert_eq!(fetched.status, original.status);
    }

    #[test]
    fn legacy_exact_and_different_replays_cannot_race_in_a_wrong_receipt() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let (options, original) = fixture.create_legacy_script_job("legacy_race");
        let job_id = original.job["jobId"]
            .as_str()
            .expect("legacy race job id")
            .to_owned();
        let mut different = options.clone();
        different.language = "en-US".to_owned();
        let exact_runtime = fixture.fresh_runtime();
        let different_runtime = fixture.fresh_runtime();
        let barrier = Arc::new(Barrier::new(3));
        let exact_barrier = barrier.clone();
        let exact_options = options.clone();
        let exact = std::thread::spawn(move || {
            exact_barrier.wait();
            exact_runtime.enqueue_script_stage(exact_options)
        });
        let different_barrier = barrier.clone();
        let wrong = std::thread::spawn(move || {
            different_barrier.wait();
            different_runtime.enqueue_script_stage(different)
        });
        barrier.wait();
        let exact = exact
            .join()
            .expect("exact legacy worker joins")
            .expect("exact legacy replay succeeds");
        let wrong = wrong
            .join()
            .expect("different legacy worker joins")
            .expect_err("different legacy replay conflicts");
        assert_eq!(wrong.code, ProviderErrorCode::IdempotencyConflict);
        assert_eq!(exact.job_id, job_id);
        assert_eq!(exact.status, original.status);
        let receipt_path = Path::new(&options.project_path)
            .join("requests/jobs")
            .join(format!("{job_id}.json"));
        let stored: Value = serde_json::from_slice(&fs::read(receipt_path).expect("read receipt"))
            .expect("parse receipt");
        assert_eq!(stored, script_enqueue_request(&options));
        fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: job_id.clone(),
            })
            .expect("legacy race job remains readable");
        fixture
            .runtime
            .jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
            })
            .expect("legacy race job remains recoverable");
    }

    #[test]
    fn changed_language_or_token_limit_conflicts_without_rewriting_global_config() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let original_config = fixture.stage_config("script");
        let options = fixture.enqueue_options("payload_conflict");
        fixture
            .runtime
            .enqueue_script_stage(options.clone())
            .expect("initial enqueue");

        let mut changed_language = options.clone();
        changed_language.language = "en-US".to_owned();
        let language_error = fixture
            .runtime
            .enqueue_script_stage(changed_language)
            .expect_err("changed language conflicts");
        assert_eq!(language_error.code, ProviderErrorCode::IdempotencyConflict);

        let mut changed_limit = options;
        changed_limit.max_output_tokens = 8192;
        let limit_error = fixture
            .runtime
            .enqueue_script_stage(changed_limit)
            .expect_err("changed token limit conflicts");
        assert_eq!(limit_error.code, ProviderErrorCode::IdempotencyConflict);
        assert_eq!(fixture.stage_config("script"), original_config);
    }

    #[test]
    fn concurrent_exact_enqueues_claim_one_request_and_one_job() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let runtime = fixture.fresh_runtime();
            let options = fixture.enqueue_options("concurrent_claim");
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                runtime.enqueue_script_stage(options)
            }));
        }
        barrier.wait();
        let first = workers
            .remove(0)
            .join()
            .expect("first worker joins")
            .expect("first enqueue");
        let second = workers
            .remove(0)
            .join()
            .expect("second worker joins")
            .expect("second enqueue");
        assert_eq!(first.job_id, second.job_id);
        let jobs = fixture
            .runtime
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                statuses: Vec::new(),
                limit: 10,
            })
            .expect("list jobs");
        assert_eq!(jobs.jobs.len(), 1);
        let requests =
            fs::read_dir(std::path::Path::new(&fixture.project.project_path).join("requests/jobs"))
                .expect("request directory")
                .count();
        assert_eq!(requests, 1);
    }

    #[test]
    fn replay_recovers_crash_after_frozen_snapshot_before_job_definition() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));
        let options = fixture.enqueue_options("crash_boundary");
        let enqueue_request = super::script_enqueue_request(&options);
        let claim = fixture
            .runtime
            .jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                idempotency_key: options.idempotency_key.clone(),
                request: enqueue_request,
            })
            .expect("claim request before simulated crash");
        let input_refs = fixture
            .runtime
            .resolve_approved_research_inputs(&options.project_path, &options.expected_project_id)
            .expect("approved inputs");
        let config_snapshot = fixture
            .runtime
            .script_config_snapshot(
                &options.project_path,
                &options.language,
                options.max_output_tokens,
            )
            .expect("script config snapshot");
        fixture
            .runtime
            .workflow
            .prepare_stage_run_with_config_snapshot(
                PrepareStageRunOptions {
                    project_path: options.project_path.clone(),
                    expected_project_id: options.expected_project_id.clone(),
                    stage_id: "script".to_owned(),
                    run_id: options.run_id.clone(),
                    job_id: claim.job_id.clone(),
                    input_refs,
                    executor: json!({
                        "providerId": options.provider_id,
                        "providerVersion": "1.0.0",
                        "executionMode": "remote_api",
                        "model": options.model,
                    }),
                },
                config_snapshot,
            )
            .expect("freeze execution before simulated crash");
        fixture
            .runtime
            .provider
            .delete_credential("openai_api")
            .expect("delete credential after crash");
        fixture.mark_research_stale();

        let recovered = fixture
            .runtime
            .enqueue_script_stage(options)
            .expect("replay completes job from frozen snapshot");
        assert_eq!(recovered.job_id, claim.job_id);
        assert_eq!(recovered.status, JobStatusData::Queued);
    }

    #[test]
    fn failed_and_canceled_jobs_replay_their_original_terminal_status() {
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            failures_before_success: 0,
        }));

        let failed_options = fixture.enqueue_options("terminal_failed");
        let failed_job = fixture
            .runtime
            .enqueue_script_stage(failed_options.clone())
            .expect("enqueue failed fixture");
        let claimed = fixture
            .runtime
            .jobs
            .claim_job(ClaimJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: failed_job.job_id.clone(),
                worker_id: "worker_terminal_failure".to_owned(),
                lease_duration_ms: 60_000,
            })
            .expect("claim failed fixture")
            .expect("job is claimable");
        let lease_id = claimed.lease.expect("active lease").lease_id;
        let failed = fixture
            .runtime
            .jobs
            .fail_job(FailJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: failed_job.job_id.clone(),
                lease_id,
                error: JobFailureData {
                    code: "provider_test_failure".to_owned(),
                    message: "non-retryable fixture".to_owned(),
                    retryable: false,
                    details: Default::default(),
                },
                log_summary: json!({
                    "message": "failed fixture",
                    "warnings": [],
                    "errors": ["provider_test_failure"]
                }),
            })
            .expect("fail job");
        assert_eq!(failed.status, JobStatusData::Failed);

        let canceled_options = fixture.enqueue_options("terminal_canceled");
        let canceled_job = fixture
            .runtime
            .enqueue_script_stage(canceled_options.clone())
            .expect("enqueue canceled fixture");
        let canceled = fixture
            .runtime
            .jobs
            .cancel_job(CancelJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: canceled_job.job_id.clone(),
                message: "cancel terminal fixture".to_owned(),
            })
            .expect("cancel queued job");
        assert_eq!(canceled.status, JobStatusData::Canceled);

        fixture
            .runtime
            .provider
            .delete_credential("openai_api")
            .expect("delete credential before terminal replay");
        let failed_replay = fixture
            .runtime
            .enqueue_script_stage(failed_options)
            .expect("failed replay");
        let canceled_replay = fixture
            .runtime
            .enqueue_script_stage(canceled_options)
            .expect("canceled replay");
        assert_eq!(failed_replay.job_id, failed_job.job_id);
        assert_eq!(failed_replay.status, JobStatusData::Failed);
        assert_eq!(canceled_replay.job_id, canceled_job.job_id);
        assert_eq!(canceled_replay.status, JobStatusData::Canceled);
    }

    #[tokio::test]
    async fn worker_retries_and_persists_provider_result_usage_as_script_artifact() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: calls.clone(),
            failures_before_success: 1,
        }));
        let outcome = fixture.enqueue("retry_001");
        fixture
            .runtime
            .run_until_terminal(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &outcome.job_id,
            )
            .await;
        let snapshot = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id,
            })
            .expect("completed job");
        assert_eq!(snapshot.status, narracut_core::JobStatusData::Succeeded);
        assert_eq!(snapshot.attempt, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let bytes = fixture
            .runtime
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &snapshot.artifact_ids[0],
                2 * 1024 * 1024,
            )
            .expect("script content");
        let result: Value = serde_json::from_slice(&bytes).expect("provider result JSON");
        validate_provider_message(&result).expect("artifact content follows provider v1");
        assert_eq!(result["messageType"], "provider_result");
        assert_eq!(result["usage"]["totalTokens"], 30);
        assert_eq!(
            result["output"]["segments"][0]["provenance"][0],
            json!({
                "claimId": "claim_dust",
                "evidenceRef": "evidence_nasa_dust"
            })
        );
        let script_metadata = fixture
            .runtime
            .storage
            .get_artifact(&fixture.project.project_path, &snapshot.artifact_ids[0])
            .expect("script metadata");
        assert_eq!(
            script_metadata.artifact["provenance"],
            json!([{
                "claimId": "claim_dust",
                "evidenceRef": "evidence_nasa_dust"
            }])
        );
        let history = fixture
            .runtime
            .workflow
            .list_stage_history(&fixture.project.project_path, "script", 10)
            .expect("script history");
        assert_eq!(history.runs[0]["status"], "succeeded");
        assert_eq!(history.runs[0]["artifactIds"][0], snapshot.artifact_ids[0]);
        assert_tree_does_not_contain(fixture._temp.path(), TEST_SECRET.as_bytes());
    }

    #[tokio::test]
    async fn fresh_runtime_resumes_queued_provider_job_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: calls.clone(),
            failures_before_success: 0,
        }));
        let outcome = fixture.enqueue("resume_queued");
        let fresh = fixture.fresh_runtime();

        assert_eq!(
            fresh
                .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id)
                .expect("resume queued job"),
            1
        );
        let _ = fresh
            .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id)
            .expect("deduplicated resume");

        let snapshot = wait_for_terminal(&fresh, &fixture.project, &outcome.job_id).await;
        assert_eq!(snapshot.status, JobStatusData::Succeeded);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fresh_runtime_resumes_retrying_provider_job() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: calls.clone(),
            failures_before_success: 1,
        }));
        let outcome = fixture.enqueue("resume_retrying");
        let claimed = fixture
            .runtime
            .jobs
            .claim_job(ClaimJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id.clone(),
                worker_id: "worker_retry_fixture".to_owned(),
                lease_duration_ms: 60_000,
            })
            .expect("claim retry fixture")
            .expect("job claimed");
        fixture
            .runtime
            .run_claimed(
                &fixture.project.project_path,
                &fixture.project.project_id,
                claimed,
            )
            .await;
        assert_eq!(
            fixture
                .runtime
                .jobs
                .get_job(GetJobOptions {
                    project_path: fixture.project.project_path.clone(),
                    expected_project_id: fixture.project.project_id.clone(),
                    job_id: outcome.job_id.clone(),
                })
                .expect("retrying snapshot")
                .status,
            JobStatusData::Retrying
        );

        let fresh = fixture.fresh_runtime();
        assert_eq!(
            fresh
                .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id)
                .expect("resume retrying job"),
            1
        );
        let snapshot = wait_for_terminal(&fresh, &fixture.project, &outcome.job_id).await;
        assert_eq!(snapshot.status, JobStatusData::Succeeded);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fresh_runtime_recovers_expired_running_provider_job() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: calls.clone(),
            failures_before_success: 0,
        }));
        let outcome = fixture.enqueue("resume_expired");
        fixture
            .runtime
            .jobs
            .claim_job(ClaimJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id.clone(),
                worker_id: "worker_expired_fixture".to_owned(),
                lease_duration_ms: 1_000,
            })
            .expect("claim expiring job")
            .expect("job claimed");
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        let fresh = fixture.fresh_runtime();
        assert_eq!(
            fresh
                .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id)
                .expect("recover expired job"),
            1
        );
        let snapshot = wait_for_terminal(&fresh, &fixture.project, &outcome.job_id).await;
        assert_eq!(snapshot.status, JobStatusData::Succeeded);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recovery_scan_does_not_claim_foreign_executor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::new(Arc::new(MockProvider {
            calls: calls.clone(),
            failures_before_success: 0,
        }));
        let input_refs = fixture
            .runtime
            .resolve_approved_research_inputs(
                &fixture.project.project_path,
                &fixture.project.project_id,
            )
            .expect("approved inputs");
        let foreign = fixture
            .runtime
            .jobs
            .enqueue_stage_job(EnqueueStageJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                stage_id: "script".to_owned(),
                run_id: "run_script_foreign_executor".to_owned(),
                input_refs,
                executor: json!({
                    "providerId": "codex_cli",
                    "providerVersion": "1.0.0",
                    "executionMode": "codex_cli",
                    "model": "gpt-5.6-sol"
                }),
                idempotency_key: "idem_script_foreign_executor".to_owned(),
                retry_policy: RetryPolicyData {
                    max_attempts: 3,
                    initial_backoff_ms: 1_000,
                    backoff_multiplier: 2,
                    max_backoff_ms: 15_000,
                },
            })
            .expect("enqueue foreign job");
        let foreign_job_id = foreign.job["jobId"]
            .as_str()
            .expect("foreign job id")
            .to_owned();
        let fresh = fixture.fresh_runtime();

        assert_eq!(
            fresh
                .resume_project_jobs(&fixture.project.project_path, &fixture.project.project_id)
                .expect("scan foreign job"),
            0
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        let snapshot = fresh
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: foreign_job_id,
            })
            .expect("foreign snapshot");
        assert_eq!(snapshot.status, JobStatusData::Queued);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enqueue_rejects_claim_only_approved_research_inputs() {
        let fixture = Fixture::new_with_reviewed_research(
            Arc::new(MockProvider {
                calls: Arc::new(AtomicUsize::new(0)),
                failures_before_success: 0,
            }),
            true,
            false,
        );
        let error = fixture
            .runtime
            .enqueue_script_stage(fixture.enqueue_options("claim_only"))
            .expect_err("evidence_set is mandatory");
        assert_eq!(error.code, ProviderErrorCode::WorkflowError);
        assert!(error.message.contains("claim_set 与 evidence_set"));
    }

    #[test]
    fn enqueue_rejects_evidence_only_approved_research_inputs() {
        let fixture = Fixture::new_with_reviewed_research(
            Arc::new(MockProvider {
                calls: Arc::new(AtomicUsize::new(0)),
                failures_before_success: 0,
            }),
            false,
            true,
        );
        let error = fixture
            .runtime
            .enqueue_script_stage(fixture.enqueue_options("evidence_only"))
            .expect_err("claim_set is mandatory");
        assert_eq!(error.code, ProviderErrorCode::WorkflowError);
        assert!(error.message.contains("claim_set 与 evidence_set"));
    }

    #[tokio::test]
    async fn cancellation_drops_provider_future_and_acknowledges_job() {
        let dropped = Arc::new(AtomicBool::new(false));
        let fixture = Fixture::new(Arc::new(SlowProvider {
            dropped: dropped.clone(),
        }));
        let outcome = fixture.enqueue("cancel_001");
        let runtime = fixture.runtime.clone();
        let project_path = fixture.project.project_path.clone();
        let project_id = fixture.project.project_id.clone();
        let job_id = outcome.job_id.clone();
        let worker = tokio::spawn(async move {
            runtime
                .run_until_terminal(&project_path, &project_id, &job_id)
                .await;
        });
        for _ in 0..100 {
            let status = fixture
                .runtime
                .jobs
                .get_job(GetJobOptions {
                    project_path: fixture.project.project_path.clone(),
                    expected_project_id: fixture.project.project_id.clone(),
                    job_id: outcome.job_id.clone(),
                })
                .expect("poll job")
                .status;
            if status == narracut_core::JobStatusData::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        fixture
            .runtime
            .jobs
            .cancel_job(CancelJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id.clone(),
                message: "cancel provider test".to_owned(),
            })
            .expect("request cancellation");
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .expect("worker stops")
            .expect("worker joins");
        let snapshot = fixture
            .runtime
            .jobs
            .get_job(GetJobOptions {
                project_path: fixture.project.project_path.clone(),
                expected_project_id: fixture.project.project_id.clone(),
                job_id: outcome.job_id,
            })
            .expect("canceled job");
        assert_eq!(snapshot.status, narracut_core::JobStatusData::Canceled);
        assert!(dropped.load(Ordering::SeqCst));
    }

    async fn wait_for_terminal(
        runtime: &ProviderRuntime,
        project: &ProjectDescriptorData,
        job_id: &str,
    ) -> JobSnapshotData {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = runtime
                    .jobs
                    .get_job(GetJobOptions {
                        project_path: project.project_path.clone(),
                        expected_project_id: project.project_id.clone(),
                        job_id: job_id.to_owned(),
                    })
                    .expect("poll recovered job");
                if snapshot.status.is_terminal() {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("recovered provider job reaches terminal state")
    }

    fn prepare_options(
        project: &ProjectDescriptorData,
        stage_id: &str,
        run_id: &str,
        job_digit: &str,
        input_refs: Vec<Value>,
    ) -> PrepareStageRunOptions {
        PrepareStageRunOptions {
            project_path: project.project_path.clone(),
            expected_project_id: project.project_id.clone(),
            stage_id: stage_id.to_owned(),
            run_id: run_id.to_owned(),
            job_id: format!("job_{}", job_digit.repeat(64)),
            input_refs,
            executor: json!({
                "providerId": "fixture",
                "providerVersion": "1.0.0",
                "executionMode": "local"
            }),
        }
    }

    fn assert_tree_does_not_contain(root: &std::path::Path, needle: &[u8]) {
        for entry in fs::read_dir(root).expect("scan test project") {
            let entry = entry.expect("read test project entry");
            let path = entry.path();
            if path.is_dir() {
                assert_tree_does_not_contain(&path, needle);
                continue;
            }
            let bytes = fs::read(&path).expect("read persisted test file");
            assert!(
                !bytes.windows(needle.len()).any(|window| window == needle),
                "test credential leaked into {}",
                path.display()
            );
        }
    }

    fn capability() -> ProviderCapabilityData {
        ProviderCapabilityData {
            provider_id: "openai_api".to_owned(),
            display_name: "Mock OpenAI".to_owned(),
            transport: "remote_api".to_owned(),
            credential_storage: "system_keyring".to_owned(),
            supports_streaming: false,
            supports_cancellation: true,
            reports_usage: true,
            default_model: "gpt-5.6-terra".to_owned(),
            models: vec![ProviderModelCapabilityData {
                model_id: "gpt-5.6-terra".to_owned(),
                display_name: "GPT-5.6 Terra".to_owned(),
                supported_tasks: vec!["script_generation".to_owned()],
                structured_outputs: true,
                max_output_tokens: 32768,
            }],
        }
    }

    fn completed_execution(
        request: &StructuredProviderRequestData,
        call: usize,
    ) -> ProviderExecutionData {
        ProviderExecutionData {
            result: StructuredProviderResultData {
                api_version: "1.0.0".to_owned(),
                message_type: "provider_result".to_owned(),
                provider_request_id: request.provider_request_id.clone(),
                provider_id: request.provider_id.clone(),
                model: request.model.clone(),
                response_id: format!("resp_mock_{call}"),
                status: "completed".to_owned(),
                output: StructuredScriptOutputData {
                    schema_version: "narracut.script/v1".to_owned(),
                    title: "Lunar dust".to_owned(),
                    language: request.config.language.clone(),
                    summary: "Traceable script".to_owned(),
                    estimated_duration_seconds: 30.0,
                    segments: vec![ScriptSegmentData {
                        segment_id: "segment_001".to_owned(),
                        order: 0,
                        title: "Dust".to_owned(),
                        narration: "Reviewed lunar dust claim.".to_owned(),
                        provenance: vec![ProvenanceReferenceData {
                            claim_id: "claim_dust".to_owned(),
                            evidence_ref: "evidence_nasa_dust".to_owned(),
                        }],
                    }],
                },
                usage: ProviderUsageData {
                    input_tokens: 10,
                    output_tokens: 20,
                    total_tokens: 30,
                    cached_input_tokens: Some(2),
                    reasoning_tokens: Some(3),
                },
                completed_at: "2026-07-17T08:00:00Z".to_owned(),
            },
        }
    }
}
