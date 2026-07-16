use std::{fs, path::Path};

use narracut_contracts::{validate_workflow_command_message, ArtifactDraft};
use narracut_core::{
    CreateProjectOptions, InitializeWorkflowOptions, ProjectDescriptorData, ProjectService,
    RecordStageRunOptions, ReviewDecisionData, ReviewStageRunOptions, ReviewerReferenceData,
    StageStatusData, StorageService, StoreArtifactFileOptions, TerminalRunStatusData,
    UpdateStageConfigOptions, WorkflowErrorCode, WorkflowService,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    imports: TempDir,
    project: ProjectDescriptorData,
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
        let workflow = WorkflowService::new(project_service, storage.clone());
        Self {
            _temp: temp,
            imports,
            project,
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

    fn record(
        &self,
        stage_id: &str,
        run_id: &str,
        input_refs: Vec<Value>,
    ) -> narracut_core::StageRunCommitResultData {
        self.workflow
            .record_stage_run(run_options(&self.project, stage_id, run_id, input_refs))
            .expect("record stage run")
    }

    fn approve(
        &self,
        stage_id: &str,
        run_id: &str,
        review_id: &str,
    ) -> narracut_core::StageReviewResultData {
        self.workflow
            .review_stage_run(review_options(
                &self.project,
                stage_id,
                run_id,
                review_id,
                ReviewDecisionData::Approved,
            ))
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
    let options = run_options(&fixture.project, "brief", "run_brief_001", Vec::new());
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

    let mut conflict_options = run_options(&fixture.project, "brief", "run_brief_001", Vec::new());
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
    let mut options = run_options(&fixture.project, "brief", "run_brief_001", Vec::new());
    options.artifact_ids = vec![artifact_id.clone()];
    let error = fixture
        .workflow
        .record_stage_run(options)
        .expect_err("artifact run identity mismatch must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);
    assert!(
        !Path::new(&fixture.project.project_path)
            .join("runs/brief/run_brief_001")
            .exists(),
        "失败的运行预检不应留下空历史目录"
    );

    let mut forged_input = run_options(
        &fixture.project,
        "brief",
        "run_brief_input_001",
        vec![json!({
            "refId": "ref_artifact_001",
            "kind": "brief",
            "contentHash": "sha256:forged",
            "artifactId": artifact_id,
            "sourceRunId": "run_other_001",
            "claimIds": [],
            "evidenceRefs": []
        })],
    );
    forged_input.artifact_ids = Vec::new();
    let error = fixture
        .workflow
        .record_stage_run(forged_input)
        .expect_err("input artifact hash mismatch must fail");
    assert_eq!(error.code, WorkflowErrorCode::ArtifactMismatch);
    assert!(!Path::new(&fixture.project.project_path)
        .join("runs/brief/run_brief_input_001")
        .exists());
}

#[test]
fn run_and_review_ids_are_unique_across_the_project() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_global_001", "review_global_001", Vec::new());

    let duplicate_run = fixture
        .workflow
        .record_stage_run(run_options(
            &fixture.project,
            "research",
            "run_global_001",
            vec![input_ref("brief", "run_global_001")],
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
            ReviewDecisionData::Approved,
        ))
        .expect_err("review id must be globally unique");
    assert_eq!(duplicate_review.code, WorkflowErrorCode::ReviewConflict);
}

#[test]
fn failed_candidate_updates_latest_run_without_replacing_the_approved_version() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());
    let mut failed = run_options(&fixture.project, "brief", "run_brief_002", Vec::new());
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
fn approving_a_new_upstream_run_propagates_only_direct_stale_reasons() {
    let fixture = Fixture::new();
    fixture.initialize();
    fixture.record_and_approve("brief", "run_brief_001", "review_brief_001", Vec::new());
    fixture.record_and_approve(
        "research",
        "run_research_001",
        "review_research_001",
        vec![input_ref("brief", "run_brief_001")],
    );
    fixture.record_and_approve(
        "script",
        "run_script_001",
        "review_script_001",
        vec![input_ref("research", "run_research_001")],
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
        .review_stage_run(review_options(
            &fixture.project,
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
        .review_stage_run(review_options(
            &fixture.project,
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
    input_refs: Vec<Value>,
) -> RecordStageRunOptions {
    RecordStageRunOptions {
        project_path: project.project_path.clone(),
        expected_project_id: project.project_id.clone(),
        stage_id: stage_id.to_owned(),
        run_id: run_id.to_owned(),
        status: TerminalRunStatusData::Succeeded,
        job_id: format!("job_{run_id}"),
        input_refs,
        executor: json!({
            "providerId": "local-test",
            "providerVersion": "1.0.0",
            "executionMode": "local"
        }),
        artifact_ids: Vec::new(),
        log_summary: json!({
            "message": "done",
            "warnings": [],
            "errors": []
        }),
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

fn input_ref(stage_id: &str, run_id: &str) -> Value {
    json!({
        "refId": format!("ref_{stage_id}"),
        "kind": format!("{stage_id}_output"),
        "contentHash": format!("sha256:{stage_id}"),
        "sourceRunId": run_id,
        "claimIds": [],
        "evidenceRefs": []
    })
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
