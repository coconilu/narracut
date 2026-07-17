use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier, Mutex},
    thread,
};

use narracut_contracts::{validate_contract_document, ArtifactDraft};
use narracut_core::{
    AcknowledgeCancellationOptions, CancelJobOptions, ClaimNextJobOptions, CompleteJobOptions,
    CreateProjectOptions, EnqueueStageJobOptions, FailJobOptions, GetJobOptions,
    InitializeWorkflowOptions, JobClock, JobErrorCode, JobFailureData, JobService, JobSnapshotData,
    JobStatusData, ListJobEventsOptions, ListJobsOptions, ProjectDescriptorData, ProjectService,
    RecordJobArtifactOptions, RecoverJobsOptions, RenewJobLeaseOptions, ReportJobProgressOptions,
    RetryPolicyData, RetryStageJobOptions, StorageService, StoreArtifactFileOptions,
    WorkflowService,
};
use serde_json::{json, Map, Value};
#[cfg(windows)]
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

#[derive(Debug)]
struct TestClock {
    now: Mutex<OffsetDateTime>,
}

impl TestClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(
                OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("test timestamp"),
            ),
        }
    }

    fn advance_ms(&self, milliseconds: i64) {
        let mut now = self.now.lock().expect("test clock lock");
        *now += Duration::milliseconds(milliseconds);
    }
}

impl JobClock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().expect("test clock lock")
    }
}

struct Fixture {
    _temp: TempDir,
    imports: TempDir,
    project: ProjectDescriptorData,
    storage: StorageService,
    workflow: WorkflowService,
    jobs: JobService,
    clock: Arc<TestClock>,
}

impl Fixture {
    fn new(initialize_workflow: bool) -> Self {
        let temp = tempfile::tempdir().expect("project parent");
        let imports = tempfile::tempdir().expect("import parent");
        let project_service = ProjectService::default();
        let project = project_service
            .create_project(CreateProjectOptions {
                parent_path: temp.path().to_string_lossy().into_owned(),
                directory_name: "demo".to_owned(),
                name: "任务队列示例".to_owned(),
                workflow_definition_id: "workflow_standard_v1".to_owned(),
                default_locale: Some("zh-CN".to_owned()),
            })
            .expect("create project");
        let storage = StorageService::new(
            temp.path().join("app-data/narracut-index.sqlite3"),
            project_service.clone(),
        );
        let workflow = WorkflowService::new(project_service.clone(), storage.clone());
        if initialize_workflow {
            workflow
                .initialize_project_workflow(InitializeWorkflowOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                })
                .expect("initialize workflow");
        }
        let clock = Arc::new(TestClock::new());
        let jobs = JobService::with_clock(
            project_service.clone(),
            storage.clone(),
            workflow.clone(),
            clock.clone(),
        );
        Self {
            _temp: temp,
            imports,
            project,
            storage,
            workflow,
            jobs,
            clock,
        }
    }

    fn initialize(&self) {
        self.workflow
            .initialize_project_workflow(InitializeWorkflowOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
            })
            .expect("initialize workflow");
    }

    fn enqueue(&self, run_id: &str, key: &str, max_attempts: u32) -> JobSnapshotData {
        self.jobs
            .enqueue_stage_job(self.enqueue_options(run_id, key, max_attempts))
            .expect("enqueue stage job")
    }

    fn enqueue_options(
        &self,
        run_id: &str,
        key: &str,
        max_attempts: u32,
    ) -> EnqueueStageJobOptions {
        EnqueueStageJobOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            run_id: run_id.to_owned(),
            input_refs: Vec::new(),
            executor: json!({
                "providerId": "local-test",
                "providerVersion": "1.0.0",
                "executionMode": "local"
            }),
            idempotency_key: key.to_owned(),
            retry_policy: RetryPolicyData {
                max_attempts,
                initial_backoff_ms: 1_000,
                backoff_multiplier: 2,
                max_backoff_ms: 8_000,
            },
        }
    }

    fn claim(&self, worker_id: &str, lease_duration_ms: u64) -> Option<JobSnapshotData> {
        self.jobs
            .claim_next_job(ClaimNextJobOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                worker_id: worker_id.to_owned(),
                lease_duration_ms,
            })
            .expect("claim next job")
    }

    fn get(&self, job_id: &str) -> JobSnapshotData {
        self.jobs
            .get_job(GetJobOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                job_id: job_id.to_owned(),
            })
            .expect("get job")
    }

    fn events(&self, job_id: &str) -> Vec<Value> {
        self.jobs
            .list_job_events(ListJobEventsOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                job_id: job_id.to_owned(),
                after_sequence: None,
                limit: 500,
            })
            .expect("list job events")
            .events
    }

    fn create_output_artifact(&self, run_id: &str) -> String {
        let source = self.imports.path().join(format!("{run_id}-brief.txt"));
        fs::write(&source, format!("output for {run_id}")).expect("write output source");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "brief",
            "runId": run_id,
            "kind": "brief",
            "mediaType": "text/plain",
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "generated",
                "providerId": "local-test",
                "model": "fixture"
            },
            "provenance": []
        }))
        .expect("artifact draft");
        let committed = self
            .storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import artifact");
        committed.artifact["artifactId"]
            .as_str()
            .expect("artifact id")
            .to_owned()
    }

    fn independent_jobs(&self, index_name: &str) -> JobService {
        let project_service = ProjectService::default();
        let storage = StorageService::new(
            self._temp
                .path()
                .join("app-data")
                .join(format!("{index_name}.sqlite3")),
            project_service.clone(),
        );
        let workflow = WorkflowService::new(project_service.clone(), storage.clone());
        JobService::with_clock(project_service, storage, workflow, self.clock.clone())
    }
}

#[test]
fn enqueue_is_exactly_idempotent_and_conflicting_payloads_are_rejected() {
    let fixture = Fixture::new(true);
    let first = fixture.enqueue("run_brief_idempotent", "stable-key", 3);
    let replay = fixture.enqueue("run_brief_idempotent", "stable-key", 3);

    assert_eq!(first, replay);
    assert_eq!(first.status, JobStatusData::Queued);
    assert!(first.index_synchronized);
    assert_contract_documents(&first, &fixture.events(job_id(&first)));

    let conflict = fixture
        .jobs
        .enqueue_stage_job(fixture.enqueue_options("run_brief_other", "stable-key", 3))
        .expect_err("same key with another request must conflict");
    assert_eq!(conflict.code, JobErrorCode::IdempotencyConflict);
    assert_eq!(fixture.events(job_id(&first)).len(), 1);
}

#[test]
fn claim_is_fifo_and_independent_services_cannot_claim_the_same_job_twice() {
    let fixture = Fixture::new(true);
    let oldest = fixture.enqueue("run_brief_oldest", "fifo-oldest", 2);
    fixture.clock.advance_ms(100);
    let newest = fixture.enqueue("run_brief_newest", "fifo-newest", 2);

    let listed = fixture
        .jobs
        .list_jobs(ListJobsOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            statuses: vec![JobStatusData::Queued],
            limit: 20,
        })
        .expect("list same-second jobs");
    assert_eq!(
        listed.jobs.iter().map(job_id).collect::<Vec<_>>(),
        [job_id(&newest), job_id(&oldest)]
    );

    let first_claim = fixture.claim("worker_fifo", 5_000).expect("oldest claim");
    assert_eq!(job_id(&first_claim), job_id(&oldest));
    let second_claim = fixture.claim("worker_fifo", 5_000).expect("newest claim");
    assert_eq!(job_id(&second_claim), job_id(&newest));
    assert!(fixture.claim("worker_fifo", 5_000).is_none());

    let concurrent = Fixture::new(true);
    let queued = concurrent.enqueue("run_brief_atomic", "atomic-claim", 2);
    let service_a = concurrent.independent_jobs("claim-a");
    let service_b = concurrent.independent_jobs("claim-b");
    let barrier = Arc::new(Barrier::new(3));
    let project_a = concurrent.project.clone();
    let project_b = concurrent.project.clone();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();
    let handle_a = thread::spawn(move || {
        barrier_a.wait();
        service_a
            .claim_next_job(claim_options(&project_a, "worker_a"))
            .expect("claim A")
    });
    let handle_b = thread::spawn(move || {
        barrier_b.wait();
        service_b
            .claim_next_job(claim_options(&project_b, "worker_b"))
            .expect("claim B")
    });
    barrier.wait();
    let claims = [
        handle_a.join().expect("join A"),
        handle_b.join().expect("join B"),
    ];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    assert_eq!(
        concurrent.get(job_id(&queued)).status,
        JobStatusData::Running
    );
}

#[test]
fn progress_artifacts_completion_and_crash_finalization_are_auditable() {
    let fixture = Fixture::new(true);
    let queued = fixture.enqueue("run_brief_success", "success-key", 2);
    let running = fixture.claim("worker_success", 10_000).expect("claim job");
    let lease_id = lease_id(&running);
    let progressed = fixture
        .jobs
        .report_job_progress(ReportJobProgressOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&queued).to_owned(),
            lease_id: lease_id.to_owned(),
            progress: 0.5,
            message: Some("正在生成摘要".to_owned()),
        })
        .expect("report progress");
    assert_eq!(progressed.progress, 0.5);

    let events_before_invalid_artifact = fixture.events(job_id(&queued)).len();
    let invalid_artifact = fixture
        .jobs
        .record_job_artifact(RecordJobArtifactOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&queued).to_owned(),
            lease_id: lease_id.to_owned(),
            artifact_id: "artifact_00000000000000000000000000000000".to_owned(),
        })
        .expect_err("missing artifact cannot enter the immutable event stream");
    assert_eq!(invalid_artifact.code, JobErrorCode::StageNotReady);
    assert_eq!(
        fixture.events(job_id(&queued)).len(),
        events_before_invalid_artifact
    );

    let artifact_id = fixture.create_output_artifact("run_brief_success");
    fixture
        .jobs
        .record_job_artifact(RecordJobArtifactOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&queued).to_owned(),
            lease_id: lease_id.to_owned(),
            artifact_id: artifact_id.clone(),
        })
        .expect("record artifact event");
    let complete_options = CompleteJobOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        job_id: job_id(&queued).to_owned(),
        lease_id: lease_id.to_owned(),
        artifact_ids: vec![artifact_id.clone()],
        log_summary: log_summary("done"),
    };
    let artifact_read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &artifact_id)
        .expect("read artifact path");
    let content_path = Path::new(&fixture.project.project_path).join(&artifact_read.content_uri);
    fs::remove_file(&content_path).expect("simulate missing content before completion");
    let events_before_invalid_completion = fixture.events(job_id(&queued)).len();
    let invalid_completion = fixture
        .jobs
        .complete_job(complete_options.clone())
        .expect_err("invalid completion must fail before terminal request persistence");
    assert_eq!(invalid_completion.code, JobErrorCode::StageNotReady);
    assert_eq!(
        fixture.events(job_id(&queued)).len(),
        events_before_invalid_completion
    );
    assert!(!fixture.get(job_id(&queued)).finalization_pending);
    fs::copy(
        fixture.imports.path().join("run_brief_success-brief.txt"),
        &content_path,
    )
    .expect("restore artifact content");
    let completed = fixture
        .jobs
        .complete_job(complete_options.clone())
        .expect("complete job");
    assert_eq!(completed.status, JobStatusData::Succeeded);
    assert_eq!(completed.progress, 1.0);
    assert_eq!(
        completed.artifact_ids.as_slice(),
        std::slice::from_ref(&artifact_id)
    );
    let replayed = fixture
        .jobs
        .complete_job(complete_options.clone())
        .expect("exact completion replay is idempotent");
    assert_eq!(replayed.last_sequence, completed.last_sequence);
    let mismatch = fixture
        .jobs
        .complete_job(CompleteJobOptions {
            log_summary: log_summary("different completion"),
            ..complete_options
        })
        .expect_err("different completion payload is rejected");
    assert_eq!(mismatch.code, JobErrorCode::InvalidTransition);

    let run = read_run(&fixture.project, "run_brief_success");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["jobId"], job_id(&queued));

    let completed_event = Path::new(&fixture.project.project_path).join(format!(
        "jobs/{}/events/{:010}.json",
        job_id(&queued),
        completed.last_sequence
    ));
    fs::remove_file(&completed_event).expect("simulate crash before terminal event commit");
    let pending = fixture.get(job_id(&queued));
    assert!(pending.finalization_pending);
    let recovered = fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("recover terminal event");
    assert_eq!(recovered.finalized_job_ids, [job_id(&queued)]);
    let reconciled = fixture.get(job_id(&queued));
    assert_eq!(reconciled.status, JobStatusData::Succeeded);
    assert_contract_documents(&reconciled, &fixture.events(job_id(&queued)));
}

#[test]
fn queued_and_running_cancellation_preserve_terminal_run_history() {
    let fixture = Fixture::new(true);
    let queued = fixture.enqueue("run_brief_cancel_queued", "cancel-queued", 2);
    let canceled = fixture
        .jobs
        .cancel_job(CancelJobOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&queued).to_owned(),
            message: "用户取消排队任务".to_owned(),
        })
        .expect("cancel queued job");
    assert_eq!(canceled.status, JobStatusData::Canceled);
    assert_eq!(
        read_run(&fixture.project, "run_brief_cancel_queued")["status"],
        "canceled"
    );

    let second = fixture.enqueue("run_brief_cancel_running", "cancel-running", 2);
    let running = fixture
        .claim("worker_cancel", 10_000)
        .expect("claim running job");
    assert_eq!(job_id(&running), job_id(&second));
    let lease_id = lease_id(&running).to_owned();
    let requested = fixture
        .jobs
        .cancel_job(CancelJobOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&second).to_owned(),
            message: "停止正在执行的任务".to_owned(),
        })
        .expect("request running cancellation");
    assert_eq!(requested.status, JobStatusData::Running);
    assert!(requested.cancellation_requested);
    let renew_error = fixture
        .jobs
        .renew_job_lease(RenewJobLeaseOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&second).to_owned(),
            lease_id: lease_id.clone(),
            lease_duration_ms: 10_000,
        })
        .expect_err("cancellation request closes lease renewal");
    assert_eq!(renew_error.code, JobErrorCode::InvalidTransition);
    let progress_error = fixture
        .jobs
        .report_job_progress(ReportJobProgressOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&second).to_owned(),
            lease_id: lease_id.clone(),
            progress: 0.2,
            message: None,
        })
        .expect_err("canceled worker cannot keep reporting progress");
    assert_eq!(progress_error.code, JobErrorCode::InvalidTransition);
    let acknowledged = fixture
        .jobs
        .acknowledge_cancellation(AcknowledgeCancellationOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id(&second).to_owned(),
            lease_id,
        })
        .expect("acknowledge cancellation");
    assert_eq!(acknowledged.status, JobStatusData::Canceled);
    assert_eq!(
        read_run(&fixture.project, "run_brief_cancel_running")["status"],
        "canceled"
    );
}

#[test]
fn retry_backoff_lease_recovery_and_manual_retry_create_distinct_history() {
    let fixture = Fixture::new(true);
    let queued = fixture.enqueue("run_brief_retry", "retry-key", 2);
    let first = fixture
        .claim("worker_retry_1", 1_000)
        .expect("first attempt");
    let retry_options = FailJobOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        job_id: job_id(&queued).to_owned(),
        lease_id: lease_id(&first).to_owned(),
        error: retryable_error("temporary_failure"),
        log_summary: log_summary("attempt one failed"),
    };
    let retrying = fixture
        .jobs
        .fail_job(retry_options.clone())
        .expect("schedule retry");
    assert_eq!(retrying.status, JobStatusData::Retrying);
    assert_eq!(retrying.attempt, 2);
    assert!(retrying.next_attempt_at.is_some());
    let retry_replay = fixture
        .jobs
        .fail_job(retry_options.clone())
        .expect("exact retryable failure replay is idempotent");
    assert_eq!(retry_replay.last_sequence, retrying.last_sequence);
    assert_eq!(
        fixture.events(job_id(&queued))[2]["logSummary"],
        log_summary("attempt one failed")
    );
    let retry_mismatch = fixture
        .jobs
        .fail_job(FailJobOptions {
            log_summary: log_summary("different attempt log"),
            ..retry_options
        })
        .expect_err("different attempt log is not an idempotent replay");
    assert_eq!(retry_mismatch.code, JobErrorCode::InvalidTransition);
    assert!(fixture.claim("worker_too_early", 1_000).is_none());

    fixture.clock.advance_ms(1_000);
    let second = fixture
        .claim("worker_retry_2", 1_000)
        .expect("second attempt");
    assert_eq!(second.attempt, 2);
    fixture.clock.advance_ms(1_001);
    let recovery = fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("recover expired lease");
    assert_eq!(recovery.recovered_job_ids, [job_id(&queued)]);
    let failed = fixture.get(job_id(&queued));
    assert_eq!(failed.status, JobStatusData::Failed);
    assert_eq!(
        read_run(&fixture.project, "run_brief_retry")["status"],
        "failed"
    );
    assert_eq!(
        fixture
            .events(job_id(&queued))
            .iter()
            .map(|event| event["eventType"].as_str().expect("event type"))
            .collect::<Vec<_>>(),
        [
            "queued",
            "started",
            "attempt_failed",
            "retrying",
            "started",
            "failure_requested",
            "failed"
        ]
    );

    let terminal_queued = fixture.enqueue("run_brief_terminal_failure", "terminal-failure", 1);
    let terminal_running = fixture
        .claim("worker_terminal_failure", 1_000)
        .expect("claim terminal failure job");
    assert_eq!(job_id(&terminal_running), job_id(&terminal_queued));
    let terminal_failure = FailJobOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        job_id: job_id(&terminal_queued).to_owned(),
        lease_id: lease_id(&terminal_running).to_owned(),
        error: retryable_error("permanent_after_limit"),
        log_summary: log_summary("attempt limit reached"),
    };
    let terminal_failed = fixture
        .jobs
        .fail_job(terminal_failure.clone())
        .expect("fail after attempt limit");
    assert_eq!(terminal_failed.status, JobStatusData::Failed);
    let terminal_replay = fixture
        .jobs
        .fail_job(terminal_failure)
        .expect("exact terminal failure replay is idempotent");
    assert_eq!(terminal_replay.last_sequence, terminal_failed.last_sequence);

    let manual = fixture
        .jobs
        .retry_stage_job(RetryStageJobOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_job_id: job_id(&queued).to_owned(),
            new_run_id: "run_brief_retry_manual".to_owned(),
            idempotency_key: "retry-manual-key".to_owned(),
        })
        .expect("manual retry creates another run");
    assert_eq!(manual.status, JobStatusData::Queued);
    assert_ne!(job_id(&manual), job_id(&queued));
    assert!(Path::new(&fixture.project.project_path)
        .join("runs/brief/run_brief_retry_manual/execution.json")
        .is_file());
}

#[test]
fn clock_rollback_keeps_event_time_monotonic_and_lease_renewable() {
    let fixture = Fixture::new(true);
    let queued = fixture.enqueue("run_brief_clock_rollback", "clock-rollback", 2);
    fixture.clock.advance_ms(-5_000);

    let running = fixture
        .claim("worker_clock_rollback", 10_000)
        .expect("claim despite wall clock rollback");
    let first_expiry = running
        .lease
        .as_ref()
        .expect("running lease")
        .expires_at
        .clone();
    let renew_options = RenewJobLeaseOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        job_id: job_id(&queued).to_owned(),
        lease_id: lease_id(&running).to_owned(),
        lease_duration_ms: 10_000,
    };
    let mut renewed = fixture
        .jobs
        .renew_job_lease(renew_options.clone())
        .expect("renew lease despite wall clock rollback");
    for _ in 0..63 {
        renewed = fixture
            .jobs
            .renew_job_lease(renew_options.clone())
            .expect("dense heartbeat does not accumulate a full lease window");
    }
    let first_expiry = OffsetDateTime::parse(&first_expiry, &Rfc3339).expect("first expiry");
    let renewed_expiry = OffsetDateTime::parse(
        &renewed.lease.as_ref().expect("renewed lease").expires_at,
        &Rfc3339,
    )
    .expect("renewed expiry");
    assert!(
        renewed_expiry > first_expiry,
        "renewal must extend the existing lease"
    );
    assert_contract_documents(&renewed, &fixture.events(job_id(&queued)));
    fixture.clock.advance_ms(15_001);
    let recovery = fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("dense heartbeat lease still expires near logical now");
    assert_eq!(recovery.recovered_job_ids, [job_id(&queued)]);
}

#[test]
fn invalid_preparation_is_visible_terminal_and_does_not_poison_recovery() {
    let fixture = Fixture::new(true);
    let mut invalid = fixture.enqueue_options("run_missing_stage", "missing-stage", 2);
    invalid.stage_id = "missing_stage".to_owned();

    let first_error = fixture
        .jobs
        .enqueue_stage_job(invalid.clone())
        .expect_err("unknown stage is rejected");
    assert_eq!(first_error.code, JobErrorCode::InvalidRequest);
    let rejected_job_id = first_error.job_id.as_deref().expect("rejected job id");
    let rejected = fixture.get(rejected_job_id);
    assert_eq!(rejected.status, JobStatusData::Failed);
    assert_eq!(
        fixture.events(rejected_job_id)[0]["eventType"],
        "preparation_failed"
    );
    let failed_jobs = fixture
        .jobs
        .list_jobs(ListJobsOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            statuses: vec![JobStatusData::Failed],
            limit: 20,
        })
        .expect("list rejected jobs");
    assert!(failed_jobs
        .jobs
        .iter()
        .any(|job| job_id(job) == rejected_job_id));
    let replay_error = fixture
        .jobs
        .enqueue_stage_job(invalid)
        .expect_err("rejected enqueue replays the same structured error");
    assert_eq!(replay_error.code, first_error.code);

    fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("one rejected job does not poison project recovery");
    let valid = fixture.enqueue("run_brief_after_rejection", "after-rejection", 2);
    let claimed = fixture
        .claim("worker_after_rejection", 5_000)
        .expect("valid work remains claimable");
    assert_eq!(job_id(&claimed), job_id(&valid));
    assert_contract_documents(&rejected, &fixture.events(rejected_job_id));
}

#[test]
fn recovery_finishes_a_definition_claimed_before_workflow_initialization() {
    let fixture = Fixture::new(false);
    let error = fixture
        .jobs
        .enqueue_stage_job(fixture.enqueue_options("run_brief_orphan", "orphan-key", 2))
        .expect_err("workflow is not initialized yet");
    assert_eq!(error.code, JobErrorCode::WorkflowNotInitialized);
    assert_eq!(
        fs::read_dir(Path::new(&fixture.project.project_path).join("jobs"))
            .expect("read jobs")
            .count(),
        1
    );

    fixture.initialize();
    let recovered = fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("recover definition without queued event");
    assert_eq!(recovered.recovered_job_ids.len(), 1);
    let listed = fixture
        .jobs
        .list_jobs(ListJobsOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            statuses: vec![JobStatusData::Queued],
            limit: 20,
        })
        .expect("list recovered jobs");
    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].status, JobStatusData::Queued);
}

#[cfg(windows)]
#[test]
fn transient_preparation_io_error_recovers_after_project_document_unlocks() {
    let fixture = Fixture::new(true);
    let sources_dir = Path::new(&fixture.project.project_path).join("sources");
    fs::create_dir_all(&sources_dir).expect("create source directory");
    let source_path = sources_dir.join("locked-source.txt");
    let source_bytes = b"temporarily locked project source";
    fs::write(&source_path, source_bytes).expect("write locked source");
    let content_hash = format!(
        "sha256:{}",
        Sha256::digest(source_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut options = fixture.enqueue_options("run_brief_locked_source", "locked-source", 2);
    options.input_refs = vec![json!({
        "refId": "ref_locked_source",
        "referenceType": "project_document",
        "kind": "source_material",
        "contentHash": content_hash,
        "uri": "project://sources/locked-source.txt",
        "claimIds": [],
        "evidenceRefs": []
    })];
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&source_path)
        .expect("lock source without Windows sharing");

    let error = fixture
        .jobs
        .enqueue_stage_job(options)
        .expect_err("sharing violation is a retryable preparation error");
    assert_eq!(error.code, JobErrorCode::IoError);
    let job_id = error.job_id.as_deref().expect("claimed job id").to_owned();
    assert!(fixture.events(&job_id).is_empty());

    drop(lock);
    let recovered = fixture
        .jobs
        .recover_project_jobs(recover_options(&fixture.project))
        .expect("unlocked preparation resumes during recovery");
    assert_eq!(recovered.recovered_job_ids, [job_id.as_str()]);
    let queued = fixture.get(&job_id);
    assert_eq!(queued.status, JobStatusData::Queued);
    assert_contract_documents(&queued, &fixture.events(&job_id));
}

fn claim_options(project: &ProjectDescriptorData, worker_id: &str) -> ClaimNextJobOptions {
    ClaimNextJobOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
        worker_id: worker_id.to_owned(),
        lease_duration_ms: 5_000,
    }
}

fn recover_options(project: &ProjectDescriptorData) -> RecoverJobsOptions {
    RecoverJobsOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
    }
}

fn retryable_error(code: &str) -> JobFailureData {
    JobFailureData {
        code: code.to_owned(),
        message: "temporary worker failure".to_owned(),
        retryable: true,
        details: Map::new(),
    }
}

fn log_summary(message: &str) -> Value {
    json!({
        "message": message,
        "warnings": [],
        "errors": []
    })
}

fn job_id(snapshot: &JobSnapshotData) -> &str {
    snapshot.job["jobId"].as_str().expect("job id")
}

fn lease_id(snapshot: &JobSnapshotData) -> &str {
    &snapshot.lease.as_ref().expect("active lease").lease_id
}

fn read_run(project: &ProjectDescriptorData, run_id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(Path::new(&project.project_path).join(format!("runs/brief/{run_id}/run.json")))
            .expect("read run"),
    )
    .expect("run JSON")
}

fn assert_contract_documents(snapshot: &JobSnapshotData, events: &[Value]) {
    validate_contract_document(&snapshot.job).expect("JobDefinition contract");
    for event in events {
        validate_contract_document(event).expect("JobEvent contract");
    }
}
