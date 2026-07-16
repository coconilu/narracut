use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use narracut_contracts::{validate_workflow_command_message, ArtifactDraft};
use narracut_core::{
    CopyProjectOptions, CreateProjectOptions, InitializeWorkflowOptions, PrepareStageRunOptions,
    ProjectDescriptorData, ProjectService, RecordStageRunOptions, ReviewDecisionData,
    ReviewStageRunOptions, ReviewerReferenceData, StageStatusData, StorageService,
    StoreArtifactFileOptions, TerminalRunStatusData, UpdateStageConfigOptions, WorkflowErrorCode,
    WorkflowService,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    imports: TempDir,
    project: ProjectDescriptorData,
    project_service: ProjectService,
    storage: StorageService,
    workflow: WorkflowService,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("project parent");
        let imports = tempfile::tempdir().expect("import parent");
        let project_service = ProjectService::default();
        let project = project_service
            .create_project(CreateProjectOptions {
                parent_path: temp.path().to_string_lossy().into_owned(),
                directory_name: "demo".to_owned(),
                name: "工作流示例".to_owned(),
                workflow_definition_id: "workflow_standard_v1".to_owned(),
                default_locale: Some("zh-CN".to_owned()),
            })
            .expect("create project");
        let storage = StorageService::new(
            temp.path().join("app-data/narracut-index.sqlite3"),
            project_service.clone(),
        );
        let workflow = WorkflowService::new(project_service.clone(), storage.clone());
        Self {
            _temp: temp,
            imports,
            project,
            project_service,
            storage,
            workflow,
        }
    }

    fn initialize(&self) -> narracut_core::WorkflowSnapshotData {
        self.workflow
            .initialize_project_workflow(InitializeWorkflowOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
            })
            .expect("initialize workflow")
    }

    fn independent_workflow(&self, index_name: &str) -> WorkflowService {
        let project_service = ProjectService::default();
        WorkflowService::new(
            project_service.clone(),
            StorageService::new(
                self._temp
                    .path()
                    .join("app-data")
                    .join(format!("{index_name}.sqlite3")),
                project_service,
            ),
        )
    }

    fn record(
        &self,
        stage_id: &str,
        run_id: &str,
        input_refs: Vec<Value>,
    ) -> narracut_core::StageRunCommitResultData {
        self.prepare(stage_id, run_id, input_refs);
        let artifact_id = self.create_output_artifact(stage_id, run_id, output_kind(stage_id));
        let mut options = run_options(&self.project, stage_id, run_id);
        options.artifact_ids = vec![artifact_id];
        self.workflow
            .record_stage_run(options)
            .expect("record stage run")
    }

    fn prepare(
        &self,
        stage_id: &str,
        run_id: &str,
        input_refs: Vec<Value>,
    ) -> narracut_core::StageRunPreparationResultData {
        self.workflow
            .prepare_stage_run(prepare_options(&self.project, stage_id, run_id, input_refs))
            .expect("prepare stage run")
    }

    fn create_output_artifact(&self, stage_id: &str, run_id: &str, kind: &str) -> String {
        let source = self.imports.path().join(format!("{run_id}-{kind}.txt"));
        fs::write(&source, format!("output for {run_id}/{kind}")).expect("write output source");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": stage_id,
            "runId": run_id,
            "kind": kind,
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
            .expect("import output artifact");
        committed.artifact["artifactId"]
            .as_str()
            .expect("artifact id")
            .to_owned()
    }

    fn input_ref(&self, stage_id: &str, run_id: &str, review_id: &str) -> Value {
        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&self.project.project_path)
                    .join(format!("runs/{stage_id}/{run_id}/run.json")),
            )
            .expect("read source run"),
        )
        .expect("source run JSON");
        let artifact_id = run["artifactIds"][0].as_str().expect("source artifact id");
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, artifact_id)
            .expect("read source artifact")
            .artifact;
        json!({
            "refId": format!("ref_{stage_id}_{run_id}"),
            "referenceType": "artifact",
            "kind": artifact["kind"],
            "contentHash": artifact["contentHash"],
            "artifactId": artifact_id,
            "sourceRunId": run_id,
            "reviewRecordId": review_id,
            "claimIds": [],
            "evidenceRefs": []
        })
    }

    fn review_input(
        &self,
        stage_id: &str,
        run_id: &str,
        review_id: &str,
        decision: ReviewDecisionData,
    ) -> ReviewStageRunOptions {
        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&self.project.project_path)
                    .join(format!("runs/{stage_id}/{run_id}/run.json")),
            )
            .expect("read run for review input"),
        )
        .expect("run JSON for review input");
        let mut options = review_options(&self.project, stage_id, run_id, review_id, decision);
        options.artifact_ids = run["artifactIds"]
            .as_array()
            .expect("run artifact ids")
            .iter()
            .map(|value| value.as_str().expect("artifact id string").to_owned())
            .collect();
        options
    }

    fn approve(
        &self,
        stage_id: &str,
        run_id: &str,
        review_id: &str,
    ) -> narracut_core::StageReviewResultData {
        let run: Value = serde_json::from_slice(
            &fs::read(
                Path::new(&self.project.project_path)
                    .join(format!("runs/{stage_id}/{run_id}/run.json")),
            )
            .expect("read run for review"),
        )
        .expect("run JSON for review");
        let artifact_ids = run["artifactIds"]
            .as_array()
            .expect("run artifact ids")
            .iter()
            .map(|value| value.as_str().expect("artifact id string").to_owned())
            .collect();
        let mut options = review_options(
            &self.project,
            stage_id,
            run_id,
            review_id,
            ReviewDecisionData::Approved,
        );
        options.artifact_ids = artifact_ids;
        self.workflow
            .review_stage_run(options)
            .expect("approve stage run")
    }

    fn record_and_approve(
        &self,
        stage_id: &str,
        run_id: &str,
        review_id: &str,
        input_refs: Vec<Value>,
    ) {
        self.record(stage_id, run_id, input_refs);
        self.approve(stage_id, run_id, review_id);
    }
}

#[test]
fn initialization_is_idempotent_and_installs_a_valid_nine_stage_dag() {
    let fixture = Fixture::new();
    let first = fixture.initialize();
    let marker_after_first = fs::read(&fixture.project.marker_path).expect("read marker");
    let second = fixture.initialize();

    assert_eq!(first, second);
    assert_eq!(first.stage_definitions.len(), 9);
    assert_eq!(first.configs.len(), 9);
    assert_eq!(first.stage_states.len(), 9);
    assert_eq!(first.stage_states[0].stage_id, "brief");
    assert_eq!(first.stage_states[0].status, StageStatusData::Ready);
    assert!(first
        .stage_states
        .iter()
        .skip(1)
        .all(|state| state.status == StageStatusData::Draft));
    assert_eq!(
        fs::read(&fixture.project.marker_path).expect("read marker again"),
        marker_after_first
    );
    for stage_id in [
        "brief",
        "research",
        "script",
        "audio",
        "captions",
        "scene_plan",
        "timeline",
        "render",
        "export",
    ] {
        assert!(Path::new(&fixture.project.project_path)
            .join(format!("contracts/stages/{stage_id}.json"))
            .is_file());
        assert!(Path::new(&fixture.project.project_path)
            .join(format!("stages/{stage_id}/config.json"))
            .is_file());
    }
    assert_workflow_contract(&first);

    let mismatch = fixture
        .workflow
        .initialize_project_workflow(InitializeWorkflowOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: "project_wrong".to_owned(),
        })
        .expect_err("identity mismatch must fail");
    assert_eq!(mismatch.code, WorkflowErrorCode::ProjectIdentityMismatch);
}

#[test]
fn config_updates_use_optimistic_revision_and_mark_an_approved_stage_stale() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());

    let updated = fixture
        .workflow
        .update_stage_config(UpdateStageConfigOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            expected_revision: 1,
            values: Map::from_iter([("tone".to_owned(), json!("calm"))]),
            decisions: vec![json!({
                "decisionId": "decision_tone_001",
                "key": "tone",
                "value": "calm",
                "madeBy": "user_001",
                "madeAt": "2026-07-16T08:00:00Z"
            })],
        })
        .expect("update config");
    assert_eq!(updated.config["revision"], 2);
    assert_eq!(updated.affected_stages.len(), 9);
    assert_eq!(updated.affected_stages[0].stage_id, "brief");
    assert_workflow_contract(&updated);

    let snapshot = fixture
        .workflow
        .get_project_workflow(&fixture.project.project_path)
        .expect("get workflow");
    let brief = stage(&snapshot.stage_states, "brief");
    assert_eq!(brief.status, StageStatusData::Stale);
    assert_eq!(brief.stale_because_stage_ids, ["brief"]);
    assert_eq!(brief.approved_run_id.as_deref(), Some("run_brief_001"));

    let conflict = fixture
        .workflow
        .update_stage_config(UpdateStageConfigOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            expected_revision: 1,
            values: Map::new(),
            decisions: Vec::new(),
        })
        .expect_err("stale revision must fail");
    assert_eq!(conflict.code, WorkflowErrorCode::ConfigConflict);
}

#[test]
fn run_ids_are_immutable_and_exact_retries_reconcile_the_marker() {
    let fixture = Fixture::new();
    fixture.initialize();
    let prepared = fixture.prepare("brief", "run_brief_001", Vec::new());
    assert!(!prepared.idempotent_replay);
    assert_workflow_contract(&prepared);
    let prepared_replay = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "brief",
            "run_brief_001",
            Vec::new(),
        ))
        .expect("exact prepare retry");
    assert!(prepared_replay.idempotent_replay);
    assert_eq!(
        prepared_replay.execution_snapshot,
        prepared.execution_snapshot
    );
    let reservation_path =
        Path::new(&fixture.project.project_path).join("runs/reservations/run_brief_001.json");
    let reservation: Value =
        serde_json::from_slice(&fs::read(&reservation_path).expect("read global run reservation"))
            .expect("global run reservation JSON");
    assert_eq!(reservation, prepared.execution_snapshot);
    let execution_path =
        Path::new(&fixture.project.project_path).join("runs/brief/run_brief_001/execution.json");
    fs::remove_file(&execution_path).expect("simulate crash before stage snapshot materialization");
    let recovered = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "brief",
            "run_brief_001",
            Vec::new(),
        ))
        .expect("recover stage snapshot from the atomic reservation");
    assert!(recovered.idempotent_replay);
    assert_eq!(recovered.execution_snapshot, prepared.execution_snapshot);
    assert!(execution_path.is_file());
    let mut prepare_conflict =
        prepare_options(&fixture.project, "brief", "run_brief_001", Vec::new());
    prepare_conflict.job_id = "job_prepare_conflict".to_owned();
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_conflict)
        .expect_err("same run id cannot change its frozen job");
    assert_eq!(error.code, WorkflowErrorCode::RunConflict);
    let artifact_id =
        fixture.create_output_artifact("brief", "run_brief_001", output_kind("brief"));
    let mut options = run_options(&fixture.project, "brief", "run_brief_001");
    options.artifact_ids = vec![artifact_id];
    let first = fixture
        .workflow
        .record_stage_run(options.clone())
        .expect("record first run");
    assert!(!first.idempotent_replay);
    assert_eq!(first.stage_state.status, StageStatusData::NeedsReview);
    assert_workflow_contract(&first);

    let replay = fixture
        .workflow
        .record_stage_run(options)
        .expect("exact retry");
    assert!(replay.idempotent_replay);
    assert_eq!(replay.run, first.run);

    let mut conflict_options = run_options(&fixture.project, "brief", "run_brief_001");
    conflict_options.artifact_ids = first.run["artifactIds"]
        .as_array()
        .expect("artifact ids")
        .iter()
        .map(|value| value.as_str().expect("artifact id").to_owned())
        .collect();
    conflict_options.job_id = "job_different".to_owned();
    let conflict = fixture
        .workflow
        .record_stage_run(conflict_options)
        .expect_err("same run id with different payload must fail");
    assert_eq!(conflict.code, WorkflowErrorCode::RunConflict);
    assert_eq!(
        fs::read_to_string(
            Path::new(&fixture.project.project_path).join("runs/brief/run_brief_001/run.json")
        )
        .expect("read immutable run"),
        serde_json::to_string_pretty(&first.run).expect("serialize run") + "\n"
    );
}

#[test]
fn artifacts_must_match_the_run_identity_before_a_run_can_be_committed() {
    let fixture = Fixture::new();
    fixture.initialize();
    let source = fixture.imports.path().join("brief.md");
    fs::write(&source, b"brief artifact").expect("write artifact source");
    let draft: ArtifactDraft = serde_json::from_value(json!({
        "stageId": "brief",
        "runId": "run_other_001",
        "kind": "brief",
        "mediaType": "text/markdown",
        "evidenceRole": "non_evidence",
        "source": {
            "origin": "generated",
            "providerId": "local",
            "model": "fixture"
        },
        "provenance": []
    }))
    .expect("artifact draft");
    let committed = fixture
        .storage
        .import_artifact_file(StoreArtifactFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: source.to_string_lossy().into_owned(),
            artifact: draft,
        })
        .expect("import artifact");
    let artifact_id = committed.artifact["artifactId"]
        .as_str()
        .expect("artifact id")
        .to_owned();
    fixture.prepare("brief", "run_brief_001", Vec::new());
    let mut options = run_options(&fixture.project, "brief", "run_brief_001");
    options.artifact_ids = vec![artifact_id.clone()];
    let error = fixture
        .workflow
        .record_stage_run(options)
        .expect_err("artifact run identity mismatch must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);
    assert!(
        !Path::new(&fixture.project.project_path)
            .join("runs/brief/run_brief_001/run.json")
            .exists(),
        "失败的运行预检不应留下空历史目录"
    );

    fixture.record_and_approve(
        "brief",
        "run_brief_source",
        "review_brief_source",
        Vec::new(),
    );
    let mut forged_ref = fixture.input_ref("brief", "run_brief_source", "review_brief_source");
    forged_ref["contentHash"] = json!("sha256:forged");
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "research",
            "run_research_forged",
            vec![forged_ref],
        ))
        .expect_err("input artifact hash mismatch must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);
    assert!(!Path::new(&fixture.project.project_path)
        .join("runs/research/run_research_forged")
        .exists());
}

#[test]
fn run_and_review_ids_are_unique_across_the_project() {
    let fixture = Fixture::new();
    fixture.initialize();

    fixture.prepare("brief", "run_reserved_global", Vec::new());
    let duplicate_reservation = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "research",
            "run_reserved_global",
            Vec::new(),
        ))
        .expect_err("reserved run id must already be globally unique");
    assert_eq!(duplicate_reservation.code, WorkflowErrorCode::RunConflict);

    fixture.record_and_approve("brief", "run_global_001", "review_global_001", Vec::new());

    let duplicate_run = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "research",
            "run_global_001",
            vec![fixture.input_ref("brief", "run_global_001", "review_global_001")],
        ))
        .expect_err("run id must be globally unique");
    assert_eq!(duplicate_run.code, WorkflowErrorCode::RunConflict);

    fixture.record("brief", "run_brief_002", Vec::new());
    let duplicate_review = fixture
        .workflow
        .review_stage_run(review_options(
            &fixture.project,
            "brief",
            "run_brief_002",
            "review_global_001",
            ReviewDecisionData::ChangesRequested,
        ))
        .expect_err("review id must be globally unique");
    assert_eq!(duplicate_review.code, WorkflowErrorCode::ReviewConflict);
}

#[test]
fn independent_services_atomically_reserve_a_run_id_for_only_one_stage() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve(
        "brief",
        "run_brief_atomic_source",
        "review_brief_atomic_source",
        Vec::new(),
    );
    let research_input = fixture.input_ref(
        "brief",
        "run_brief_atomic_source",
        "review_brief_atomic_source",
    );

    let workflow_a = fixture.independent_workflow("concurrent-a");
    let workflow_b = fixture.independent_workflow("concurrent-b");
    let barrier = Arc::new(Barrier::new(3));
    let barrier_a = Arc::clone(&barrier);
    let brief_options = prepare_options(
        &fixture.project,
        "brief",
        "run_atomic_cross_stage",
        Vec::new(),
    );
    let brief_thread = thread::spawn(move || {
        barrier_a.wait();
        workflow_a.prepare_stage_run(brief_options)
    });
    let barrier_b = Arc::clone(&barrier);
    let research_options = prepare_options(
        &fixture.project,
        "research",
        "run_atomic_cross_stage",
        vec![research_input],
    );
    let research_thread = thread::spawn(move || {
        barrier_b.wait();
        workflow_b.prepare_stage_run(research_options)
    });
    barrier.wait();

    let brief_result = brief_thread.join().expect("brief reservation thread");
    let research_result = research_thread.join().expect("research reservation thread");
    let success_count = usize::from(brief_result.is_ok()) + usize::from(research_result.is_ok());
    assert_eq!(success_count, 1, "exactly one stage may reserve the runId");
    for error in [brief_result.as_ref().err(), research_result.as_ref().err()]
        .into_iter()
        .flatten()
    {
        assert_eq!(error.code, WorkflowErrorCode::RunConflict);
    }

    let project_dir = Path::new(&fixture.project.project_path);
    assert!(project_dir
        .join("runs/reservations/run_atomic_cross_stage.json")
        .is_file());
    let materialized_count = usize::from(
        project_dir
            .join("runs/brief/run_atomic_cross_stage/execution.json")
            .is_file(),
    ) + usize::from(
        project_dir
            .join("runs/research/run_atomic_cross_stage/execution.json")
            .is_file(),
    );
    assert_eq!(materialized_count, 1);
}

#[test]
fn independent_services_reconcile_the_same_run_reservation_idempotently() {
    let fixture = Fixture::new();
    fixture.initialize();
    let workflow_a = fixture.independent_workflow("same-request-a");
    let workflow_b = fixture.independent_workflow("same-request-b");
    let barrier = Arc::new(Barrier::new(3));

    let barrier_a = Arc::clone(&barrier);
    let options_a = prepare_options(
        &fixture.project,
        "brief",
        "run_atomic_same_request",
        Vec::new(),
    );
    let first = thread::spawn(move || {
        barrier_a.wait();
        workflow_a.prepare_stage_run(options_a)
    });
    let barrier_b = Arc::clone(&barrier);
    let options_b = prepare_options(
        &fixture.project,
        "brief",
        "run_atomic_same_request",
        Vec::new(),
    );
    let second = thread::spawn(move || {
        barrier_b.wait();
        workflow_b.prepare_stage_run(options_b)
    });
    barrier.wait();

    let first = first
        .join()
        .expect("first exact reservation thread")
        .expect("first exact reservation result");
    let second = second
        .join()
        .expect("second exact reservation thread")
        .expect("second exact reservation result");
    assert_eq!(first.execution_snapshot, second.execution_snapshot);
    assert!(first.idempotent_replay || second.idempotent_replay);
    assert!(Path::new(&fixture.project.project_path)
        .join("runs/brief/run_atomic_same_request/execution.json")
        .is_file());
}

#[test]
fn failed_candidate_updates_latest_run_without_replacing_the_approved_version() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());
    fixture.prepare("brief", "run_brief_002", Vec::new());
    let mut failed = run_options(&fixture.project, "brief", "run_brief_002");
    failed.status = TerminalRunStatusData::Failed;
    let result = fixture
        .workflow
        .record_stage_run(failed)
        .expect("record failed candidate");
    assert_eq!(result.stage_state.status, StageStatusData::Approved);
    assert_eq!(
        result.stage_state.approved_run_id.as_deref(),
        Some("run_brief_001")
    );
    assert_eq!(
        result.stage_state.latest_run_id.as_deref(),
        Some("run_brief_002")
    );
    let snapshot = fixture
        .workflow
        .get_project_workflow(&fixture.project.project_path)
        .expect("get workflow");
    assert_eq!(
        stage(&snapshot.stage_states, "brief")
            .latest_run_id
            .as_deref(),
        Some("run_brief_002")
    );
    assert_workflow_contract(&result);
}

#[test]
fn execution_snapshot_survives_config_changes_and_terminal_failures() {
    let fixture = Fixture::new();
    fixture.initialize();
    let prepared = fixture.prepare("brief", "run_brief_snapshot", Vec::new());
    assert_eq!(prepared.execution_snapshot["configSnapshot"]["revision"], 1);

    fixture
        .workflow
        .update_stage_config(UpdateStageConfigOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            expected_revision: 1,
            values: Map::from_iter([("revisionLabel".to_owned(), json!("v2"))]),
            decisions: Vec::new(),
        })
        .expect("change config while run is executing");
    let artifact_id =
        fixture.create_output_artifact("brief", "run_brief_snapshot", output_kind("brief"));
    let mut completed = run_options(&fixture.project, "brief", "run_brief_snapshot");
    completed.artifact_ids = vec![artifact_id];
    let result = fixture
        .workflow
        .record_stage_run(completed)
        .expect("terminal run keeps its frozen snapshot");
    assert_eq!(result.run["configSnapshot"]["revision"], 1);
    assert_eq!(result.stage_state.status, StageStatusData::NeedsReview);
    assert!(result.execution_outdated);

    let failed_snapshot = fixture.prepare("brief", "run_brief_failed", Vec::new());
    assert_eq!(
        failed_snapshot.execution_snapshot["configSnapshot"]["revision"],
        2
    );
    fixture
        .workflow
        .update_stage_config(UpdateStageConfigOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            expected_revision: 2,
            values: Map::from_iter([("revisionLabel".to_owned(), json!("v3"))]),
            decisions: Vec::new(),
        })
        .expect("change config before failed terminal event");
    let mut failed = run_options(&fixture.project, "brief", "run_brief_failed");
    failed.status = TerminalRunStatusData::Failed;
    let failed_result = fixture
        .workflow
        .record_stage_run(failed)
        .expect("failed execution still becomes immutable history");
    assert_eq!(failed_result.run["configSnapshot"]["revision"], 2);
    assert_eq!(failed_result.run["status"], "failed");

    fixture.prepare("brief", "run_brief_canceled", Vec::new());
    let mut canceled = run_options(&fixture.project, "brief", "run_brief_canceled");
    canceled.status = TerminalRunStatusData::Canceled;
    let canceled_result = fixture
        .workflow
        .record_stage_run(canceled)
        .expect("canceled execution still becomes immutable history");
    assert_eq!(canceled_result.run["status"], "canceled");
    assert!(Path::new(&fixture.project.project_path)
        .join("runs/brief/run_brief_canceled/run.json")
        .is_file());
}

#[test]
fn upstream_changes_after_prepare_do_not_erase_the_executed_candidate() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());
    let old_input = fixture.input_ref("brief", "run_brief_001", "review_brief_001");
    fixture.prepare("research", "run_research_snapshot", vec![old_input]);

    fixture.record("brief", "run_brief_002", Vec::new());
    fixture.approve("brief", "run_brief_002", "review_brief_002");
    let artifact_id = fixture.create_output_artifact(
        "research",
        "run_research_snapshot",
        output_kind("research"),
    );
    let mut terminal = run_options(&fixture.project, "research", "run_research_snapshot");
    terminal.artifact_ids = vec![artifact_id];
    let committed = fixture
        .workflow
        .record_stage_run(terminal)
        .expect("historical execution must remain committable");
    assert_eq!(
        committed.run["inputRefs"][0]["sourceRunId"],
        "run_brief_001"
    );
    assert!(Path::new(&fixture.project.project_path)
        .join("runs/research/run_research_snapshot/run.json")
        .is_file());
    let approval = fixture.review_input(
        "research",
        "run_research_snapshot",
        "review_research_outdated",
        ReviewDecisionData::Approved,
    );
    let error = fixture
        .workflow
        .review_stage_run(approval)
        .expect_err("outdated candidate cannot become the first approval");
    assert_eq!(error.code, WorkflowErrorCode::StageNotReady);
}

#[test]
fn dependency_inputs_reject_bare_refs_post_run_injection_and_wrong_output_kind() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());

    let bare = json!({
        "refId": "ref_bare_document",
        "referenceType": "project_document",
        "kind": "source_material",
        "contentHash": format!("sha256:{}", "00".repeat(32)),
        "uri": "project://narracut.project.json",
        "claimIds": [],
        "evidenceRefs": []
    });
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "research",
            "run_research_bare",
            vec![bare],
        ))
        .expect_err("project document cannot impersonate an approved dependency");
    assert_eq!(error.code, WorkflowErrorCode::StageNotReady);

    let injected_id =
        fixture.create_output_artifact("brief", "run_brief_001", output_kind("brief"));
    let injected = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &injected_id)
        .expect("read injected artifact")
        .artifact;
    let injected_ref = json!({
        "refId": "ref_injected_artifact",
        "referenceType": "artifact",
        "kind": injected["kind"],
        "contentHash": injected["contentHash"],
        "artifactId": injected_id,
        "sourceRunId": "run_brief_001",
        "reviewRecordId": "review_brief_001",
        "claimIds": [],
        "evidenceRefs": []
    });
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "research",
            "run_research_injected",
            vec![injected_ref],
        ))
        .expect_err("post-run artifact injection must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);

    fixture.prepare("brief", "run_brief_wrong_kind", Vec::new());
    let wrong_kind = fixture.create_output_artifact("brief", "run_brief_wrong_kind", "script");
    let mut terminal = run_options(&fixture.project, "brief", "run_brief_wrong_kind");
    terminal.artifact_ids = vec![wrong_kind];
    let error = fixture
        .workflow
        .record_stage_run(terminal)
        .expect_err("output kind outside StageDefinition.outputKinds must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);
}

#[test]
fn project_document_inputs_use_a_bounded_hash_checked_resolver() {
    let fixture = Fixture::new();
    fixture.initialize();
    let source_path = Path::new(&fixture.project.project_path).join("sources/brief-source.txt");
    fs::write(&source_path, b"trusted project source").expect("write project source");
    let digest = Sha256::digest(b"trusted project source");
    let content_hash = format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let reference = json!({
        "refId": "ref_project_source",
        "referenceType": "project_document",
        "kind": "source_material",
        "contentHash": content_hash,
        "uri": "project://sources/brief-source.txt",
        "claimIds": [],
        "evidenceRefs": []
    });
    let prepared = fixture.prepare("brief", "run_brief_document", vec![reference.clone()]);
    assert_eq!(prepared.execution_snapshot["inputRefs"][0], reference);

    let mut wrong_hash = reference.clone();
    wrong_hash["contentHash"] = json!(format!("sha256:{}", "00".repeat(32)));
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "brief",
            "run_brief_wrong_hash",
            vec![wrong_hash],
        ))
        .expect_err("resolver must recompute project document hash");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);

    let mut traversal = reference;
    traversal["uri"] = json!("project://../outside.txt");
    let error = fixture
        .workflow
        .prepare_stage_run(prepare_options(
            &fixture.project,
            "brief",
            "run_brief_traversal",
            vec![traversal],
        ))
        .expect_err("resolver must reject path traversal");
    assert_eq!(error.code, WorkflowErrorCode::InvalidPath);
}

#[test]
fn copied_workflow_reinitializes_root_readiness_from_the_dag() {
    let fixture = Fixture::new();
    fixture.initialize();
    let destination = tempfile::tempdir().expect("copy destination");
    let copied = fixture
        .project_service
        .copy_project(CopyProjectOptions {
            source_project_path: fixture.project.project_path.clone(),
            destination_parent_path: destination.path().to_string_lossy().into_owned(),
            directory_name: "copy".to_owned(),
            name: "workflow copy".to_owned(),
        })
        .expect("copy initialized project");
    let before = fixture
        .workflow
        .get_project_workflow(&copied.project.project_path)
        .expect_err("copied state projection must require reinitialization");
    assert_eq!(before.code, WorkflowErrorCode::WorkflowNotInitialized);
    let first = fixture
        .workflow
        .initialize_project_workflow(InitializeWorkflowOptions {
            project_path: copied.project.project_path.clone(),
            expected_project_id: copied.project.project_id.clone(),
        })
        .expect("initialize copied workflow");
    assert_eq!(
        stage(&first.stage_states, "brief").status,
        StageStatusData::Ready
    );
    assert!(first
        .stage_states
        .iter()
        .filter(|state| state.stage_id != "brief")
        .all(|state| state.status == StageStatusData::Draft));
    let second = fixture
        .workflow
        .initialize_project_workflow(InitializeWorkflowOptions {
            project_path: copied.project.project_path,
            expected_project_id: copied.project.project_id,
        })
        .expect("copied workflow initialization is idempotent");
    assert_eq!(first, second);
}

#[test]
fn approving_a_new_upstream_run_propagates_only_direct_stale_reasons() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());
    fixture.record_and_approve(
        "research",
        "run_research_001",
        "review_research_001",
        vec![fixture.input_ref("brief", "run_brief_001", "review_brief_001")],
    );
    fixture.record_and_approve(
        "script",
        "run_script_001",
        "review_script_001",
        vec![fixture.input_ref("research", "run_research_001", "review_research_001")],
    );

    fixture.record("brief", "run_brief_002", Vec::new());
    let reviewed = fixture.approve("brief", "run_brief_002", "review_brief_002");
    assert_eq!(reviewed.invalidated_stage_ids, ["research", "script"]);
    assert_workflow_contract(&reviewed);
    let research = stage(&reviewed.stage_states, "research");
    assert_eq!(research.status, StageStatusData::Stale);
    assert_eq!(research.stale_because_stage_ids, ["brief"]);
    let script = stage(&reviewed.stage_states, "script");
    assert_eq!(script.status, StageStatusData::Stale);
    assert_eq!(script.stale_because_stage_ids, ["research"]);
}

#[test]
fn regeneration_preview_is_transitive_and_does_not_mutate_project_files() {
    let fixture = Fixture::new();
    fixture.initialize();
    let marker_before = fs::read(&fixture.project.marker_path).expect("read marker before preview");
    let preview = fixture
        .workflow
        .preview_regeneration(&fixture.project.project_path, vec!["script".to_owned()])
        .expect("preview regeneration");
    assert_eq!(
        preview.affected_stages[0].direct_cause_stage_ids,
        ["script"]
    );
    assert_eq!(
        preview
            .affected_stages
            .iter()
            .map(|stage| stage.stage_id.as_str())
            .collect::<Vec<_>>(),
        [
            "script",
            "audio",
            "captions",
            "scene_plan",
            "timeline",
            "render",
            "export"
        ]
    );
    assert_eq!(
        preview
            .affected_stages
            .iter()
            .find(|stage| stage.stage_id == "timeline")
            .expect("timeline impact")
            .direct_cause_stage_ids,
        ["audio", "captions", "scene_plan"]
    );
    assert_eq!(
        fs::read(&fixture.project.marker_path).expect("read marker after preview"),
        marker_before
    );
    assert_workflow_contract(&preview);
}

#[test]
fn first_approval_rejects_an_outdated_run_but_existing_approval_allows_explicit_rollback() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record("brief", "run_brief_001", Vec::new());
    fixture
        .workflow
        .update_stage_config(UpdateStageConfigOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            stage_id: "brief".to_owned(),
            expected_revision: 1,
            values: Map::from_iter([("version".to_owned(), json!(2))]),
            decisions: Vec::new(),
        })
        .expect("change config");
    let rejected = fixture
        .workflow
        .review_stage_run(fixture.review_input(
            "brief",
            "run_brief_001",
            "review_brief_old",
            ReviewDecisionData::Approved,
        ))
        .expect_err("outdated first approval must fail");
    assert_eq!(rejected.code, WorkflowErrorCode::StageNotReady);
    assert!(
        !Path::new(&fixture.project.project_path)
            .join("runs/brief/run_brief_001/reviews/review_brief_old.json")
            .exists(),
        "业务预检失败不能留下声称已批准的审核记录"
    );

    fixture.record("brief", "run_brief_002", Vec::new());
    fixture.approve("brief", "run_brief_002", "review_brief_new");
    let rollback = fixture.approve("brief", "run_brief_001", "review_brief_back");
    let brief = stage(&rollback.stage_states, "brief");
    assert_eq!(brief.approved_run_id.as_deref(), Some("run_brief_001"));
    assert_eq!(brief.status, StageStatusData::Stale);
    assert_eq!(brief.stale_because_stage_ids, ["brief"]);
}

#[test]
fn review_replays_do_not_override_a_later_review_and_history_is_bounded_by_limit() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record("brief", "run_brief_001", Vec::new());
    let approved = fixture.approve("brief", "run_brief_001", "review_brief_001");
    assert!(approved.applied);
    let changed = fixture
        .workflow
        .review_stage_run(review_options(
            &fixture.project,
            "brief",
            "run_brief_001",
            "review_brief_002",
            ReviewDecisionData::ChangesRequested,
        ))
        .expect("request changes");
    assert!(changed.applied);
    assert_eq!(
        stage(&changed.stage_states, "brief").status,
        StageStatusData::NeedsReview
    );

    let replay = fixture
        .workflow
        .review_stage_run(fixture.review_input(
            "brief",
            "run_brief_001",
            "review_brief_001",
            ReviewDecisionData::Approved,
        ))
        .expect("replay old review");
    assert!(replay.idempotent_replay);
    assert!(!replay.applied);
    assert_eq!(
        stage(&replay.stage_states, "brief").status,
        StageStatusData::NeedsReview
    );

    fixture.record("brief", "run_brief_002", Vec::new());
    let history = fixture
        .workflow
        .list_stage_history(&fixture.project.project_path, "brief", 1)
        .expect("list history");
    assert_eq!(history.runs.len(), 1);
    assert_eq!(history.runs[0]["runId"], "run_brief_002");
    assert!(history.reviews.is_empty());
    assert_workflow_contract(&history);
}

#[test]
fn review_history_accepts_1024_records_and_rejects_the_1025th() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record("brief", "run_brief_limit", Vec::new());
    let reviews_dir =
        Path::new(&fixture.project.project_path).join("runs/brief/run_brief_limit/reviews");
    fs::create_dir_all(&reviews_dir).expect("create review limit directory");
    for index in 0..1024 {
        let review_id = format!("review_limit_{index:04}");
        let document = json!({
            "schemaVersion": "1.0.0",
            "documentType": "review_record",
            "reviewId": review_id,
            "projectId": fixture.project.project_id,
            "stageId": "brief",
            "runId": "run_brief_limit",
            "decision": "changes_requested",
            "reviewer": {
                "kind": "system",
                "reviewerId": "review-limit-test",
                "displayName": "review limit test"
            },
            "comments": "boundary",
            "artifactIds": [],
            "createdAt": "2026-07-16T12:00:00Z"
        });
        fs::write(
            reviews_dir.join(format!("{review_id}.json")),
            serde_json::to_vec_pretty(&document).expect("serialize review boundary"),
        )
        .expect("write review boundary");
    }
    let history = fixture
        .workflow
        .list_stage_history(&fixture.project.project_path, "brief", 1)
        .expect("1024 reviews fit the service and response contract");
    assert_eq!(history.runs.len(), 1);
    assert_eq!(history.reviews.len(), 1024);
    assert_workflow_contract(&history);

    let overflow_id = "review_limit_1024";
    let overflow = json!({
        "schemaVersion": "1.0.0",
        "documentType": "review_record",
        "reviewId": overflow_id,
        "projectId": fixture.project.project_id,
        "stageId": "brief",
        "runId": "run_brief_limit",
        "decision": "changes_requested",
        "reviewer": {
            "kind": "system",
            "reviewerId": "review-limit-test",
            "displayName": "review limit test"
        },
        "comments": "overflow",
        "artifactIds": [],
        "createdAt": "2026-07-16T12:00:00Z"
    });
    fs::write(
        reviews_dir.join(format!("{overflow_id}.json")),
        serde_json::to_vec_pretty(&overflow).expect("serialize review overflow"),
    )
    .expect("write review overflow");
    let error = fixture
        .workflow
        .list_stage_history(&fixture.project.project_path, "brief", 1)
        .expect_err("1025 reviews exceed both service and response bounds");
    assert_eq!(error.code, WorkflowErrorCode::ScanLimitExceeded);
}

#[test]
fn tampered_stage_definition_cycle_is_rejected_before_workflow_use() {
    let fixture = Fixture::new();
    fixture.initialize();
    let definition_path =
        Path::new(&fixture.project.project_path).join("contracts/stages/brief.json");
    let mut definition: Value =
        serde_json::from_slice(&fs::read(&definition_path).expect("read brief definition"))
            .expect("brief definition JSON");
    definition["dependencies"] = json!(["export"]);
    fs::write(
        &definition_path,
        serde_json::to_vec_pretty(&definition).expect("serialize tampered definition"),
    )
    .expect("write tampered definition");

    let error = fixture
        .workflow
        .get_project_workflow(&fixture.project.project_path)
        .expect_err("cycle must fail");
    assert_eq!(error.code, WorkflowErrorCode::InvalidStageGraph);
}

#[test]
fn tampered_run_hashes_are_rejected_when_history_is_read() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record("brief", "run_brief_001", Vec::new());
    let run_path =
        Path::new(&fixture.project.project_path).join("runs/brief/run_brief_001/run.json");
    let mut run: Value =
        serde_json::from_slice(&fs::read(&run_path).expect("read run")).expect("run JSON");
    run["inputHash"] = json!("sha256:forged");
    fs::write(
        &run_path,
        serde_json::to_vec_pretty(&run).expect("serialize tampered run"),
    )
    .expect("write tampered run");

    let error = fixture
        .workflow
        .list_stage_history(&fixture.project.project_path, "brief", 20)
        .expect_err("tampered hash must fail");
    assert_eq!(error.code, WorkflowErrorCode::InvalidProject);
}

fn run_options(
    project: &ProjectDescriptorData,
    stage_id: &str,
    run_id: &str,
) -> RecordStageRunOptions {
    RecordStageRunOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
        stage_id: stage_id.to_owned(),
        run_id: run_id.to_owned(),
        status: TerminalRunStatusData::Succeeded,
        job_id: format!("job_{run_id}"),
        artifact_ids: Vec::new(),
        log_summary: json!({
            "message": "done",
            "warnings": [],
            "errors": []
        }),
    }
}

fn prepare_options(
    project: &ProjectDescriptorData,
    stage_id: &str,
    run_id: &str,
    input_refs: Vec<Value>,
) -> PrepareStageRunOptions {
    PrepareStageRunOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
        stage_id: stage_id.to_owned(),
        run_id: run_id.to_owned(),
        job_id: format!("job_{run_id}"),
        input_refs,
        executor: json!({
            "providerId": "local-test",
            "providerVersion": "1.0.0",
            "executionMode": "local"
        }),
    }
}

fn output_kind(stage_id: &str) -> &'static str {
    match stage_id {
        "brief" => "brief",
        "research" => "claim_set",
        "script" => "script",
        "audio" => "voice_audio",
        "captions" => "captions",
        "scene_plan" => "scene_plan",
        "timeline" => "timeline",
        "render" => "rendered_video",
        "export" => "final_video",
        _ => panic!("unknown stage {stage_id}"),
    }
}

fn review_options(
    project: &ProjectDescriptorData,
    stage_id: &str,
    run_id: &str,
    review_id: &str,
    decision: ReviewDecisionData,
) -> ReviewStageRunOptions {
    ReviewStageRunOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
        stage_id: stage_id.to_owned(),
        run_id: run_id.to_owned(),
        review_id: review_id.to_owned(),
        decision,
        reviewer: ReviewerReferenceData {
            kind: "human".to_owned(),
            reviewer_id: "user_test".to_owned(),
            display_name: "测试用户".to_owned(),
        },
        comments: "reviewed".to_owned(),
        artifact_ids: Vec::new(),
    }
}

fn stage<'a>(
    states: &'a [narracut_core::StageStateData],
    stage_id: &str,
) -> &'a narracut_core::StageStateData {
    states
        .iter()
        .find(|state| state.stage_id == stage_id)
        .unwrap_or_else(|| panic!("missing stage {stage_id}"))
}

fn assert_workflow_contract<T: Serialize>(value: &T) {
    let value = serde_json::to_value(value).expect("serialize workflow response");
    validate_workflow_command_message(&value).unwrap_or_else(|error| {
        panic!("workflow response must follow command schema: {error}; value={value}")
    });
}
