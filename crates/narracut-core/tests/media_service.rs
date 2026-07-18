use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use narracut_contracts::{parse_media_document, validate_media_document, ArtifactDraft};
use narracut_core::{
    apply_scene_plan_edits, validate_timeline_semantics, ArtifactVerificationStatusData,
    CreateProjectOptions, FrozenArtifactInputData, GenerateScenePlanOptions,
    GenerateTimelineOptions, GetJobOptions, GetMediaDocumentOptions, ImportAudioOptions,
    ImportCaptionsOptions, InitializeWorkflowOptions, JobService, JobStatusData,
    ListJobEventsOptions, MediaClock, MediaErrorCode, MediaRightsData, MediaSaveResultData,
    MediaService, PcmWavParseLimits, PrepareStageRunOptions, ProjectDescriptorData, ProjectService,
    RecordStageRunOptions, ReviewDecisionData, ReviewStageRunOptions, ReviewerReferenceData,
    SaveScenePlanOptions, SaveTimelineOptions, ScenePlanEditData, SrtParseLimits, StageStatusData,
    StorageService, StoreArtifactFileOptions, TerminalRunStatusData, TimelineCanvasData,
    TimelineEditData, TimelineSafeAreaData, WorkflowService,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug)]
struct FixedClock;

impl MediaClock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::parse("2026-07-18T08:00:00Z", &Rfc3339).expect("fixed clock")
    }
}

struct Fixture {
    _temp: TempDir,
    external_dir: PathBuf,
    project: ProjectDescriptorData,
    storage: StorageService,
    workflow: WorkflowService,
    media: MediaService,
    script_input: FrozenArtifactInputData,
}

#[derive(Debug, Clone)]
struct ApprovedCaptionChain {
    audio_input: FrozenArtifactInputData,
    captions_input: FrozenArtifactInputData,
}

#[derive(Debug, Clone)]
struct ApprovedTimelineChain {
    audio_input: FrozenArtifactInputData,
    captions_input: FrozenArtifactInputData,
    scene_plan_input: FrozenArtifactInputData,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_initial_script(None)
    }

    fn new_with_initial_script(initial_script: Option<(Value, Value)>) -> Self {
        let temp = tempfile::tempdir().expect("project parent");
        let external_dir = temp.path().join("EXTERNAL_ABSOLUTE_PATH_CANARY");
        fs::create_dir(&external_dir).expect("external source directory");
        let project_service = ProjectService::default();
        let project = project_service
            .create_project(CreateProjectOptions {
                parent_path: temp.path().to_string_lossy().into_owned(),
                directory_name: "demo".to_owned(),
                name: "Audio media fixture".to_owned(),
                workflow_definition_id: "workflow_standard_v1".to_owned(),
                default_locale: Some("zh-CN".to_owned()),
            })
            .expect("create project");
        let storage = StorageService::new(
            temp.path().join("app-data/narracut-index.sqlite3"),
            project_service.clone(),
        );
        let workflow = WorkflowService::new(project_service.clone(), storage.clone());
        workflow
            .initialize_project_workflow(InitializeWorkflowOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
            })
            .expect("initialize workflow");

        let mut fixture = Self {
            _temp: temp,
            external_dir,
            project,
            storage: storage.clone(),
            workflow: workflow.clone(),
            media: MediaService::with_clock(
                project_service,
                storage,
                workflow,
                Arc::new(FixedClock),
            ),
            script_input: FrozenArtifactInputData {
                stage_id: String::new(),
                run_id: String::new(),
                artifact_id: String::new(),
                content_hash: String::new(),
                review_record_id: String::new(),
                claim_ids: Vec::new(),
                evidence_refs: Vec::new(),
            },
        };
        if let Some((payload, provenance)) = initial_script {
            fixture.record_and_approve(
                "brief",
                "run_brief_media",
                "review_brief_media",
                Vec::new(),
            );
            let brief = fixture.workflow_input("brief", "run_brief_media", "review_brief_media");
            fixture.record_and_approve(
                "research",
                "run_research_media",
                "review_research_media",
                vec![brief],
            );
            fixture.install_approved_script_payload(
                "run_script_media",
                "review_script_media",
                payload,
                provenance,
            );
        } else {
            fixture.install_approved_script();
        }
        fixture
    }

    fn install_approved_script(&mut self) {
        self.record_and_approve("brief", "run_brief_media", "review_brief_media", Vec::new());
        let brief = self.workflow_input("brief", "run_brief_media", "review_brief_media");
        self.record_and_approve(
            "research",
            "run_research_media",
            "review_research_media",
            vec![brief],
        );
        let research =
            self.workflow_input("research", "run_research_media", "review_research_media");
        self.prepare("script", "run_script_media", vec![research]);
        let narration = format!(
            "Hello world! Traceable captions. First. Second. Third. Fourth. One. Two. Three. Four. Five. Six. Seven. 你好，OpenAI world! 第二句？ Line one. Line two! {} Stable captions. Changed captions. First version. Second version.",
            "字".repeat(60)
        );
        let script_artifact_id = self.create_artifact(
            "script",
            "run_script_media",
            "script",
            "application/json",
            &serde_json::to_vec(&json!({
                "schemaVersion": "narracut.script/v1",
                "title": "Media integration fixture",
                "language": "zh-CN",
                "summary": "Covers deterministic media integration captions.",
                "estimatedDurationSeconds": 1,
                "segments": [{
                    "segmentId": "segment_media_fixture",
                    "order": 0,
                    "title": "Fixture narration",
                    "narration": narration,
                    "provenance": [{
                        "claimId": "claim_audio_1",
                        "evidenceRef": "evidence_audio_1"
                    }]
                }]
            }))
            .expect("script JSON"),
            json!([{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}]),
        );
        self.record(
            "script",
            "run_script_media",
            vec![script_artifact_id.clone()],
        );
        self.approve("script", "run_script_media", "review_script_media");
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &script_artifact_id)
            .expect("read script artifact")
            .artifact;
        self.script_input = FrozenArtifactInputData {
            stage_id: "script".to_owned(),
            run_id: "run_script_media".to_owned(),
            artifact_id: script_artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("script hash")
                .to_owned(),
            review_record_id: "review_script_media".to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        };
    }

    fn record_and_approve(
        &self,
        stage_id: &str,
        run_id: &str,
        review_id: &str,
        inputs: Vec<Value>,
    ) {
        self.prepare(stage_id, run_id, inputs);
        let artifact_id = self.create_artifact(
            stage_id,
            run_id,
            output_kind(stage_id),
            "application/json",
            br#"{"fixture":true}"#,
            json!([]),
        );
        self.record(stage_id, run_id, vec![artifact_id]);
        self.approve(stage_id, run_id, review_id);
    }

    fn prepare(&self, stage_id: &str, run_id: &str, input_refs: Vec<Value>) {
        self.workflow
            .prepare_stage_run(PrepareStageRunOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.to_owned(),
                job_id: format!("job_{run_id}"),
                input_refs,
                executor: json!({
                    "providerId": "local-test",
                    "providerVersion": "1.0.0",
                    "executionMode": "local"
                }),
            })
            .expect("prepare stage");
    }

    fn record(&self, stage_id: &str, run_id: &str, artifact_ids: Vec<String>) {
        self.workflow
            .record_stage_run(RecordStageRunOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.to_owned(),
                status: TerminalRunStatusData::Succeeded,
                job_id: format!("job_{run_id}"),
                artifact_ids,
                log_summary: json!({"message":"done","warnings":[],"errors":[]}),
            })
            .expect("record stage");
    }

    fn approve(&self, stage_id: &str, run_id: &str, review_id: &str) {
        let run = read_json(
            Path::new(&self.project.project_path)
                .join(format!("runs/{stage_id}/{run_id}/run.json")),
        );
        let artifact_ids = run["artifactIds"]
            .as_array()
            .expect("artifact ids")
            .iter()
            .map(|value| value.as_str().expect("artifact id").to_owned())
            .collect();
        self.workflow
            .review_stage_run(ReviewStageRunOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                stage_id: stage_id.to_owned(),
                run_id: run_id.to_owned(),
                review_id: review_id.to_owned(),
                decision: ReviewDecisionData::Approved,
                reviewer: ReviewerReferenceData {
                    kind: "human".to_owned(),
                    reviewer_id: "reviewer_media".to_owned(),
                    display_name: "Media Reviewer".to_owned(),
                },
                comments: "approved".to_owned(),
                artifact_ids,
            })
            .expect("approve stage");
    }

    fn create_artifact(
        &self,
        stage_id: &str,
        run_id: &str,
        kind: &str,
        media_type: &str,
        content: &[u8],
        provenance: Value,
    ) -> String {
        let source = self.external_dir.join(format!("{run_id}-{kind}.json"));
        fs::write(&source, content).expect("write artifact source");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": stage_id,
            "runId": run_id,
            "kind": kind,
            "mediaType": media_type,
            "evidenceRole": "non_evidence",
            "source": {"origin":"generated","providerId":"local-test","model":"fixture"},
            "provenance": provenance,
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

    fn workflow_input(&self, stage_id: &str, run_id: &str, review_id: &str) -> Value {
        let run = read_json(
            Path::new(&self.project.project_path)
                .join(format!("runs/{stage_id}/{run_id}/run.json")),
        );
        let artifact_id = run["artifactIds"][0].as_str().expect("artifact id");
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, artifact_id)
            .expect("read artifact")
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

    fn audio_options(&self, key: &str) -> ImportAudioOptions {
        let source = self.external_dir.join("private-narration.wav");
        fs::write(&source, pcm_wave(16_000, 1, 16, 1_600)).expect("write PCM wave");
        ImportAudioOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: "run_audio_media".to_owned(),
            source_path: source.to_string_lossy().into_owned(),
            expected_source_content_hash: None,
            script_input: self.script_input.clone(),
            rights: MediaRightsData {
                ownership: "self_recorded".to_owned(),
                author: "Fixture Author".to_owned(),
                rights_statement: "Fixture owns this recording.".to_owned(),
                license_id: "fixture-owned-audio".to_owned(),
                attribution_text: String::new(),
                voice_authorization: "not_voice_clone".to_owned(),
            },
            limits: PcmWavParseLimits {
                max_bytes: 64 * 1024 * 1024,
            },
            config_snapshot: json!({"normalize":false}),
            idempotency_key: key.to_owned(),
        }
    }

    fn install_approved_audio(&self, key: &str) -> FrozenArtifactInputData {
        self.audio_candidate("run_audio_media", "review_audio_media", key, true)
    }

    fn audio_candidate(
        &self,
        run_id: &str,
        review_id: &str,
        key: &str,
        approved: bool,
    ) -> FrozenArtifactInputData {
        let script_ref = self.workflow_input("script", "run_script_media", "review_script_media");
        self.prepare("audio", run_id, vec![script_ref]);
        let mut options = self.audio_options(key);
        options.run_id = run_id.to_owned();
        let result = self
            .media
            .import_audio(options)
            .expect("import fixture Audio");
        self.record("audio", run_id, vec![result.artifact_id.clone()]);
        if approved {
            self.approve("audio", run_id, review_id);
        }
        FrozenArtifactInputData {
            stage_id: "audio".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id: result.artifact_id,
            content_hash: result.content_hash,
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn approved_audio_payload(&self, payload: &[u8]) -> FrozenArtifactInputData {
        let run_id = "run_audio_payload";
        let review_id = "review_audio_payload";
        let script_ref = self.workflow_input("script", "run_script_media", "review_script_media");
        self.prepare("audio", run_id, vec![script_ref]);
        let artifact_id = self.create_artifact(
            "audio",
            run_id,
            "voice_audio",
            "application/vnd.narracut.audio+json",
            payload,
            json!([{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}]),
        );
        self.record("audio", run_id, vec![artifact_id.clone()]);
        self.approve("audio", run_id, review_id);
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read custom Audio artifact")
            .artifact;
        FrozenArtifactInputData {
            stage_id: "audio".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("custom Audio hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn approved_derived_audio_payload(
        &self,
        run_id: &str,
        review_id: &str,
        payload: &[u8],
    ) -> FrozenArtifactInputData {
        self.prepare(
            "audio",
            run_id,
            vec![self.workflow_input("script", "run_script_media", "review_script_media")],
        );
        let source = self
            .external_dir
            .join(format!("{run_id}-derived-audio-document.json"));
        fs::write(&source, payload).expect("write derived Audio document");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "audio",
            "runId": run_id,
            "kind": "voice_audio",
            "mediaType": "application/vnd.narracut.audio+json",
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "derived",
                "sourceArtifactIds": [self.script_input.artifact_id],
            },
            "provenance": [{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}],
        }))
        .expect("derived Audio Artifact draft");
        let committed = self
            .storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import derived Audio document");
        let artifact_id = committed.artifact["artifactId"]
            .as_str()
            .expect("derived Audio artifact ID")
            .to_owned();
        self.record("audio", run_id, vec![artifact_id.clone()]);
        self.approve("audio", run_id, review_id);
        FrozenArtifactInputData {
            stage_id: "audio".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: committed.artifact["contentHash"]
                .as_str()
                .expect("derived Audio hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn prepare_captions(&self) {
        self.prepare(
            "captions",
            "run_captions_media",
            vec![
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input("audio", "run_audio_media", "review_audio_media"),
            ],
        );
    }

    fn captions_options(
        &self,
        key: &str,
        audio_input: &FrozenArtifactInputData,
        source_bytes: &[u8],
    ) -> ImportCaptionsOptions {
        let source = self.external_dir.join("private-captions.srt");
        fs::write(&source, source_bytes).expect("write SRT");
        ImportCaptionsOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: "run_captions_media".to_owned(),
            source_path: source.to_string_lossy().into_owned(),
            expected_source_content_hash: None,
            script_input: self.script_input.clone(),
            audio_input: audio_input.clone(),
            audio_duration_ms: 100,
            rights: MediaRightsData {
                ownership: "self_recorded".to_owned(),
                author: "Fixture Captioner".to_owned(),
                rights_statement: "Fixture owns this subtitle file.".to_owned(),
                license_id: "fixture-owned-captions".to_owned(),
                attribution_text: String::new(),
                voice_authorization: "not_voice_clone".to_owned(),
            },
            limits: SrtParseLimits {
                max_bytes: 4 * 1024 * 1024,
                max_cue_count: 10_000,
                max_cue_text_bytes: 8_000,
            },
            config_snapshot: json!({"language":"zh-CN","interpolation":"deterministic-v1"}),
            idempotency_key: key.to_owned(),
        }
    }

    fn research_input(&self) -> FrozenArtifactInputData {
        let input = self.workflow_input("research", "run_research_media", "review_research_media");
        FrozenArtifactInputData {
            stage_id: "research".to_owned(),
            run_id: "run_research_media".to_owned(),
            artifact_id: input["artifactId"]
                .as_str()
                .expect("research artifact id")
                .to_owned(),
            content_hash: input["contentHash"]
                .as_str()
                .expect("research content hash")
                .to_owned(),
            review_record_id: "review_research_media".to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn install_approved_caption_chain(
        &self,
        audio_key: &str,
        captions_key: &str,
    ) -> ApprovedCaptionChain {
        self.install_approved_caption_chain_with_srt(
            audio_key,
            captions_key,
            valid_scene_plan_srt(),
        )
    }

    fn install_approved_caption_chain_with_srt(
        &self,
        audio_key: &str,
        captions_key: &str,
        srt: &[u8],
    ) -> ApprovedCaptionChain {
        let audio_input = self.install_approved_audio(audio_key);
        self.prepare_captions();
        let result = self
            .media
            .import_captions(self.captions_options(captions_key, &audio_input, srt))
            .expect("import approved Scene Plan Captions");
        self.record(
            "captions",
            "run_captions_media",
            vec![result.artifact_id.clone()],
        );
        self.approve("captions", "run_captions_media", "review_captions_media");
        ApprovedCaptionChain {
            audio_input,
            captions_input: FrozenArtifactInputData {
                stage_id: "captions".to_owned(),
                run_id: "run_captions_media".to_owned(),
                artifact_id: result.artifact_id,
                content_hash: result.content_hash,
                review_record_id: "review_captions_media".to_owned(),
                claim_ids: vec!["claim_audio_1".to_owned()],
                evidence_refs: vec!["evidence_audio_1".to_owned()],
            },
        }
    }

    fn scene_plan_options(
        &self,
        key: &str,
        captions_input: &FrozenArtifactInputData,
    ) -> GenerateScenePlanOptions {
        GenerateScenePlanOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: "run_scene_plan_media".to_owned(),
            research_input: self.research_input(),
            script_input: self.script_input.clone(),
            captions_input: captions_input.clone(),
            idempotency_key: key.to_owned(),
        }
    }

    fn generated_scene_plan_base(
        &self,
        key: &str,
    ) -> (ApprovedCaptionChain, MediaSaveResultData, Value) {
        let chain = self
            .install_approved_caption_chain(&format!("{key}-audio"), &format!("{key}-captions"));
        let result = self
            .media
            .generate_scene_plan(self.scene_plan_options(key, &chain.captions_input))
            .expect("generate editable Scene Plan base");
        let document = read_artifact_json(self, &result.artifact_id);
        (chain, result, document)
    }

    fn install_approved_timeline_chain(&self, key: &str) -> ApprovedTimelineChain {
        let captions = self
            .install_approved_caption_chain(&format!("{key}-audio"), &format!("{key}-captions"));
        self.prepare(
            "scene_plan",
            "run_scene_plan_media",
            vec![
                self.workflow_input("research", "run_research_media", "review_research_media"),
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input("captions", "run_captions_media", "review_captions_media"),
            ],
        );
        let scene_plan = self
            .media
            .generate_scene_plan(
                self.scene_plan_options(&format!("{key}-scene-plan"), &captions.captions_input),
            )
            .expect("generate Timeline Scene Plan input");
        self.record(
            "scene_plan",
            "run_scene_plan_media",
            vec![scene_plan.artifact_id.clone()],
        );
        self.approve(
            "scene_plan",
            "run_scene_plan_media",
            "review_scene_plan_media",
        );
        let metadata = self
            .storage
            .get_artifact(&self.project.project_path, &scene_plan.artifact_id)
            .expect("read Timeline Scene Plan metadata")
            .artifact;
        ApprovedTimelineChain {
            audio_input: captions.audio_input,
            captions_input: captions.captions_input,
            scene_plan_input: FrozenArtifactInputData {
                stage_id: "scene_plan".to_owned(),
                run_id: "run_scene_plan_media".to_owned(),
                artifact_id: scene_plan.artifact_id,
                content_hash: metadata["contentHash"]
                    .as_str()
                    .expect("Scene Plan content hash")
                    .to_owned(),
                review_record_id: "review_scene_plan_media".to_owned(),
                claim_ids: vec!["claim_audio_1".to_owned()],
                evidence_refs: vec!["evidence_audio_1".to_owned()],
            },
        }
    }

    fn timeline_options(
        &self,
        key: &str,
        chain: &ApprovedTimelineChain,
    ) -> GenerateTimelineOptions {
        GenerateTimelineOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: "run_timeline_media".to_owned(),
            audio_input: chain.audio_input.clone(),
            captions_input: chain.captions_input.clone(),
            scene_plan_input: chain.scene_plan_input.clone(),
            canvas: TimelineCanvasData {
                width: 1_920,
                height: 1_080,
                frame_rate_numerator: 30_000,
                frame_rate_denominator: 1_001,
            },
            safe_area: TimelineSafeAreaData {
                x: 96,
                y: 54,
                width: 1_728,
                height: 972,
            },
            idempotency_key: key.to_owned(),
        }
    }

    fn generated_timeline_base(
        &self,
        key: &str,
    ) -> (ApprovedTimelineChain, MediaSaveResultData, Value) {
        let chain = self.install_approved_timeline_chain(key);
        let result = self
            .media
            .generate_timeline(self.timeline_options(&format!("{key}-generate"), &chain))
            .expect("generate editable Timeline base");
        let document = read_artifact_json(self, &result.artifact_id);
        (chain, result, document)
    }

    fn approve_timeline_and_downstream(
        &self,
        chain: &ApprovedTimelineChain,
        timeline_run_id: &str,
        timeline_artifact_id: &str,
    ) {
        self.prepare(
            "timeline",
            timeline_run_id,
            vec![
                self.workflow_input(
                    "audio",
                    &chain.audio_input.run_id,
                    &chain.audio_input.review_record_id,
                ),
                self.workflow_input(
                    "captions",
                    &chain.captions_input.run_id,
                    &chain.captions_input.review_record_id,
                ),
                self.workflow_input(
                    "scene_plan",
                    &chain.scene_plan_input.run_id,
                    &chain.scene_plan_input.review_record_id,
                ),
            ],
        );
        self.record(
            "timeline",
            timeline_run_id,
            vec![timeline_artifact_id.to_owned()],
        );
        self.approve("timeline", timeline_run_id, "review_timeline_before_edit");

        self.prepare(
            "render",
            "run_render_before_edit",
            vec![self.workflow_input("timeline", timeline_run_id, "review_timeline_before_edit")],
        );
        let render_artifact_id = self.create_artifact(
            "render",
            "run_render_before_edit",
            "rendered_video",
            "video/mp4",
            b"rendered fixture",
            json!([]),
        );
        self.record("render", "run_render_before_edit", vec![render_artifact_id]);
        self.approve(
            "render",
            "run_render_before_edit",
            "review_render_before_edit",
        );

        self.prepare(
            "export",
            "run_export_before_edit",
            vec![self.workflow_input(
                "render",
                "run_render_before_edit",
                "review_render_before_edit",
            )],
        );
        let export_artifact_id = self.create_artifact(
            "export",
            "run_export_before_edit",
            "final_video",
            "video/mp4",
            b"exported fixture",
            json!([]),
        );
        self.record("export", "run_export_before_edit", vec![export_artifact_id]);
        self.approve(
            "export",
            "run_export_before_edit",
            "review_export_before_edit",
        );
    }

    fn stage_status(&self, stage_id: &str) -> StageStatusData {
        self.workflow
            .get_project_workflow(&self.project.project_path)
            .expect("read workflow state")
            .stage_states
            .into_iter()
            .find(|state| state.stage_id == stage_id)
            .expect("stage state")
            .status
    }

    fn timeline_save_options(
        &self,
        key: &str,
        run_id: &str,
        base_artifact_id: &str,
        edits: Vec<TimelineEditData>,
        change_summary: &str,
    ) -> SaveTimelineOptions {
        SaveTimelineOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: run_id.to_owned(),
            base_artifact_id: base_artifact_id.to_owned(),
            edits,
            change_summary: change_summary.to_owned(),
            idempotency_key: key.to_owned(),
        }
    }

    fn media_document_options(&self, artifact_id: &str) -> GetMediaDocumentOptions {
        GetMediaDocumentOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            artifact_id: artifact_id.to_owned(),
        }
    }

    fn scene_plan_save_options(
        &self,
        key: &str,
        run_id: &str,
        base_artifact_id: &str,
        edits: Vec<ScenePlanEditData>,
        change_summary: &str,
    ) -> SaveScenePlanOptions {
        SaveScenePlanOptions {
            project_path: self.project.project_path.clone(),
            expected_project_id: self.project.project_id.clone(),
            run_id: run_id.to_owned(),
            base_artifact_id: base_artifact_id.to_owned(),
            edits,
            change_summary: change_summary.to_owned(),
            idempotency_key: key.to_owned(),
        }
    }

    fn research_candidate(
        &self,
        run_id: &str,
        review_id: &str,
        approved: bool,
    ) -> FrozenArtifactInputData {
        let brief = self.workflow_input("brief", "run_brief_media", "review_brief_media");
        self.prepare("research", run_id, vec![brief]);
        let artifact_id = self.create_artifact(
            "research",
            run_id,
            "claim_set",
            "application/json",
            br#"{"claims":[{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}]}"#,
            json!([{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}]),
        );
        self.record("research", run_id, vec![artifact_id.clone()]);
        if approved {
            self.approve("research", run_id, review_id);
        }
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read Research candidate")
            .artifact;
        FrozenArtifactInputData {
            stage_id: "research".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("Research candidate hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn captions_candidate(
        &self,
        run_id: &str,
        review_id: &str,
        key: &str,
        audio_input: &FrozenArtifactInputData,
        approved: bool,
    ) -> FrozenArtifactInputData {
        self.prepare(
            "captions",
            run_id,
            vec![
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input("audio", &audio_input.run_id, &audio_input.review_record_id),
            ],
        );
        let mut options = self.captions_options(key, audio_input, valid_scene_plan_srt());
        options.run_id = run_id.to_owned();
        let result = self
            .media
            .import_captions(options)
            .expect("import Captions candidate");
        self.record("captions", run_id, vec![result.artifact_id.clone()]);
        if approved {
            self.approve("captions", run_id, review_id);
        }
        FrozenArtifactInputData {
            stage_id: "captions".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id: result.artifact_id,
            content_hash: result.content_hash,
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn approved_captions_payload(
        &self,
        run_id: &str,
        review_id: &str,
        audio_input: &FrozenArtifactInputData,
        payload: &[u8],
    ) -> FrozenArtifactInputData {
        self.prepare(
            "captions",
            run_id,
            vec![
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input("audio", &audio_input.run_id, &audio_input.review_record_id),
            ],
        );
        let artifact_id = self.create_artifact(
            "captions",
            run_id,
            "captions",
            "application/vnd.narracut.captions+json",
            payload,
            json!([{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}]),
        );
        self.record("captions", run_id, vec![artifact_id.clone()]);
        self.approve("captions", run_id, review_id);
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read custom Captions artifact")
            .artifact;
        FrozenArtifactInputData {
            stage_id: "captions".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("custom Captions hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn approved_derived_captions_payload(
        &self,
        run_id: &str,
        review_id: &str,
        audio_input: &FrozenArtifactInputData,
        payload: &[u8],
    ) -> FrozenArtifactInputData {
        self.prepare(
            "captions",
            run_id,
            vec![
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input("audio", &audio_input.run_id, &audio_input.review_record_id),
            ],
        );
        let source = self
            .external_dir
            .join(format!("{run_id}-derived-captions-document.json"));
        fs::write(&source, payload).expect("write derived Captions document");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "captions",
            "runId": run_id,
            "kind": "captions",
            "mediaType": "application/vnd.narracut.captions+json",
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "derived",
                "sourceArtifactIds": [
                    self.script_input.artifact_id,
                    audio_input.artifact_id,
                ],
            },
            "provenance": [{"claimId":"claim_audio_1","evidenceRef":"evidence_audio_1"}],
        }))
        .expect("derived Captions Artifact draft");
        let committed = self
            .storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import derived Captions document");
        let artifact_id = committed.artifact["artifactId"]
            .as_str()
            .expect("derived Captions artifact ID")
            .to_owned();
        self.record("captions", run_id, vec![artifact_id.clone()]);
        self.approve("captions", run_id, review_id);
        FrozenArtifactInputData {
            stage_id: "captions".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: committed.artifact["contentHash"]
                .as_str()
                .expect("derived Captions hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn create_derived_scene_plan_artifact(
        &self,
        run_id: &str,
        payload: &[u8],
        source_artifact_ids: &[String],
    ) -> String {
        let source = self
            .external_dir
            .join(format!("{run_id}-scene-plan-payload.json"));
        fs::write(&source, payload).expect("write Scene Plan payload");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "scene_plan",
            "runId": run_id,
            "kind": "scene_plan",
            "mediaType": "application/vnd.narracut.scene-plan+json",
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "derived",
                "sourceArtifactIds": source_artifact_ids,
            },
            "provenance": [],
        }))
        .expect("Scene Plan Artifact draft");
        self.storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import Scene Plan payload")
            .artifact["artifactId"]
            .as_str()
            .expect("Scene Plan payload artifact id")
            .to_owned()
    }

    fn create_derived_timeline_artifact(
        &self,
        run_id: &str,
        payload: &[u8],
        source_artifact_ids: &[String],
    ) -> String {
        let source = self
            .external_dir
            .join(format!("{run_id}-timeline-payload.json"));
        fs::write(&source, payload).expect("write Timeline payload");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": "timeline",
            "runId": run_id,
            "kind": "timeline",
            "mediaType": "application/vnd.narracut.timeline+json",
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "derived",
                "sourceArtifactIds": source_artifact_ids,
            },
            "provenance": [],
        }))
        .expect("Timeline Artifact draft");
        self.storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import Timeline payload")
            .artifact["artifactId"]
            .as_str()
            .expect("Timeline payload artifact id")
            .to_owned()
    }

    fn create_derived_media_document(
        &self,
        stage_id: &str,
        run_id: &str,
        kind: &str,
        media_type: &str,
        payload: &[u8],
        source_artifact_ids: &[String],
    ) -> String {
        let source = self
            .external_dir
            .join(format!("{run_id}-{kind}-query-document.json"));
        fs::write(&source, payload).expect("write media query document payload");
        let draft: ArtifactDraft = serde_json::from_value(json!({
            "stageId": stage_id,
            "runId": run_id,
            "kind": kind,
            "mediaType": media_type,
            "evidenceRole": "non_evidence",
            "source": {
                "origin": "derived",
                "sourceArtifactIds": source_artifact_ids,
            },
            "provenance": [],
        }))
        .expect("media query Artifact draft");
        self.storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import media query document")
            .artifact["artifactId"]
            .as_str()
            .expect("media query document artifact id")
            .to_owned()
    }

    fn approved_scene_plan_payload(
        &self,
        run_id: &str,
        review_id: &str,
        payload: &[u8],
        captions: &ApprovedCaptionChain,
    ) -> FrozenArtifactInputData {
        self.prepare(
            "scene_plan",
            run_id,
            vec![
                self.workflow_input("research", "run_research_media", "review_research_media"),
                self.workflow_input("script", "run_script_media", "review_script_media"),
                self.workflow_input(
                    "captions",
                    &captions.captions_input.run_id,
                    &captions.captions_input.review_record_id,
                ),
            ],
        );
        let source_artifact_ids = vec![
            self.research_input().artifact_id,
            self.script_input.artifact_id.clone(),
            captions.captions_input.artifact_id.clone(),
            captions.audio_input.artifact_id.clone(),
        ];
        let artifact_id =
            self.create_derived_scene_plan_artifact(run_id, payload, &source_artifact_ids);
        self.record("scene_plan", run_id, vec![artifact_id.clone()]);
        self.approve("scene_plan", run_id, review_id);
        let metadata = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read custom Scene Plan metadata")
            .artifact;
        FrozenArtifactInputData {
            stage_id: "scene_plan".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: metadata["contentHash"]
                .as_str()
                .expect("custom Scene Plan hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn script_candidate(
        &self,
        run_id: &str,
        review_id: &str,
        decision: Option<ReviewDecisionData>,
    ) -> FrozenArtifactInputData {
        let research =
            self.workflow_input("research", "run_research_media", "review_research_media");
        self.prepare("script", run_id, vec![research]);
        let artifact_id = self.create_artifact(
            "script",
            run_id,
            "script",
            "application/json",
            br#"{"segments":[{"text":"candidate"}]}"#,
            json!([]),
        );
        self.record("script", run_id, vec![artifact_id.clone()]);
        if let Some(decision) = decision {
            let run = read_json(
                Path::new(&self.project.project_path)
                    .join(format!("runs/script/{run_id}/run.json")),
            );
            self.workflow
                .review_stage_run(ReviewStageRunOptions {
                    project_path: self.project.project_path.clone(),
                    expected_project_id: self.project.project_id.clone(),
                    stage_id: "script".to_owned(),
                    run_id: run_id.to_owned(),
                    review_id: review_id.to_owned(),
                    decision,
                    reviewer: ReviewerReferenceData {
                        kind: "human".to_owned(),
                        reviewer_id: "reviewer_media".to_owned(),
                        display_name: "Media Reviewer".to_owned(),
                    },
                    comments: "candidate review".to_owned(),
                    artifact_ids: run["artifactIds"]
                        .as_array()
                        .expect("artifact ids")
                        .iter()
                        .map(|value| value.as_str().expect("artifact id").to_owned())
                        .collect(),
                })
                .expect("review candidate");
        }
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read candidate")
            .artifact;
        FrozenArtifactInputData {
            stage_id: "script".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("candidate hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        }
    }

    fn install_approved_script_payload(
        &mut self,
        run_id: &str,
        review_id: &str,
        payload: Value,
        provenance: Value,
    ) -> FrozenArtifactInputData {
        let research =
            self.workflow_input("research", "run_research_media", "review_research_media");
        self.prepare("script", run_id, vec![research]);
        let artifact_id = self.create_artifact(
            "script",
            run_id,
            "script",
            "application/json",
            &serde_json::to_vec(&payload).expect("custom script payload"),
            provenance.clone(),
        );
        self.record("script", run_id, vec![artifact_id.clone()]);
        self.approve("script", run_id, review_id);
        let artifact = self
            .storage
            .get_artifact(&self.project.project_path, &artifact_id)
            .expect("read custom script")
            .artifact;
        let mut claim_ids = Vec::new();
        let mut evidence_refs = Vec::new();
        for pair in provenance.as_array().into_iter().flatten() {
            let claim_id = pair["claimId"].as_str().expect("claimId").to_owned();
            let evidence_ref = pair["evidenceRef"]
                .as_str()
                .expect("evidenceRef")
                .to_owned();
            if !claim_ids.contains(&claim_id) {
                claim_ids.push(claim_id);
            }
            if !evidence_refs.contains(&evidence_ref) {
                evidence_refs.push(evidence_ref);
            }
        }
        let input = FrozenArtifactInputData {
            stage_id: "script".to_owned(),
            run_id: run_id.to_owned(),
            artifact_id,
            content_hash: artifact["contentHash"]
                .as_str()
                .expect("custom script hash")
                .to_owned(),
            review_record_id: review_id.to_owned(),
            claim_ids,
            evidence_refs,
        };
        self.script_input = input.clone();
        input
    }

    fn metadata_count(&self) -> usize {
        count_regular_files(
            &Path::new(&self.project.project_path)
                .join("artifacts")
                .join("metadata"),
        )
    }

    fn receipt_count(&self) -> usize {
        count_regular_files(
            &Path::new(&self.project.project_path)
                .join("artifacts")
                .join("media-receipts"),
        )
    }
}

#[test]
fn media_document_query_reads_all_four_verified_types_without_writes_or_path_leaks() {
    let fixture = Fixture::new();
    let (chain, timeline_result, timeline_document) =
        fixture.generated_timeline_base("media-document-query-happy");
    let cases = [
        (chain.audio_input.artifact_id.as_str(), "audio_media"),
        (chain.captions_input.artifact_id.as_str(), "captions_media"),
        (chain.scene_plan_input.artifact_id.as_str(), "scene_plan"),
        (timeline_result.artifact_id.as_str(), "timeline"),
    ];
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let external_path = fixture.external_dir.to_string_lossy();
    for (artifact_id, document_type) in cases {
        let metadata = fixture
            .storage
            .get_artifact(&fixture.project.project_path, artifact_id)
            .expect("read queried media metadata");
        let expected_document = if document_type == "timeline" {
            timeline_document.clone()
        } else {
            read_artifact_json(&fixture, artifact_id)
        };
        let result = fixture
            .media
            .get_media_document(fixture.media_document_options(artifact_id))
            .expect("read verified media document");
        assert_eq!(result.owner_project_id, fixture.project.project_id);
        assert_eq!(result.artifact_id, artifact_id);
        assert_eq!(
            result.content_hash,
            metadata.artifact["contentHash"]
                .as_str()
                .expect("metadata content hash")
        );
        assert_eq!(result.document["documentType"], document_type);
        assert_eq!(result.document, expected_document);
        let serialized = serde_json::to_string(&result).expect("serialize media query result");
        assert!(!serialized.contains(external_path.as_ref()));
        assert!(!serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    }
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

#[test]
fn media_document_query_rejects_invalid_cross_project_metadata_identity_hash_and_entity_damage() {
    let fixture = Fixture::new();
    let (_chain, timeline_result, timeline) =
        fixture.generated_timeline_base("media-document-query-invalid");
    let source_ids = timeline["inputRefs"]
        .as_array()
        .expect("Timeline query inputs")
        .iter()
        .map(|input| input["artifactId"].as_str().expect("input id").to_owned())
        .collect::<Vec<_>>();

    let mut invalid_options = fixture.media_document_options("not-an-artifact");
    invalid_options.artifact_id = "invalid".to_owned();
    assert_media_document_query_error(&fixture, invalid_options, &[MediaErrorCode::InvalidRequest]);

    let foreign = Fixture::new();
    let mut cross_project_options = fixture.media_document_options(&timeline_result.artifact_id);
    cross_project_options.expected_project_id = foreign.project.project_id.clone();
    assert_media_document_query_error(
        &fixture,
        cross_project_options,
        &[MediaErrorCode::CrossProjectReference],
    );

    let arbitrary_id = fixture.create_derived_media_document(
        "timeline",
        "run_query_arbitrary",
        "timeline",
        "application/vnd.narracut.timeline+json",
        br#"{"arbitrary":true}"#,
        &source_ids,
    );
    assert_media_document_query_error(
        &fixture,
        fixture.media_document_options(&arbitrary_id),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let wrong_kind_id = fixture.create_derived_media_document(
        "timeline",
        timeline["runId"].as_str().expect("Timeline run"),
        "script",
        "application/json",
        &serde_json::to_vec(&timeline).expect("serialize wrong-kind Timeline"),
        &source_ids,
    );
    assert_media_document_query_error(
        &fixture,
        fixture.media_document_options(&wrong_kind_id),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let mut wrong_run = timeline.clone();
    wrong_run["runId"] = json!("run_query_document_identity");
    let wrong_run_id = fixture.create_derived_media_document(
        "timeline",
        "run_query_metadata_identity",
        "timeline",
        "application/vnd.narracut.timeline+json",
        &serde_json::to_vec(&wrong_run).expect("serialize wrong-run Timeline"),
        &source_ids,
    );
    assert_media_document_query_error(
        &fixture,
        fixture.media_document_options(&wrong_run_id),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let mut wrong_project = timeline.clone();
    wrong_project["projectId"] = json!(foreign.project.project_id);
    wrong_project["runId"] = json!("run_query_wrong_project");
    let wrong_project_id = fixture.create_derived_media_document(
        "timeline",
        "run_query_wrong_project",
        "timeline",
        "application/vnd.narracut.timeline+json",
        &serde_json::to_vec(&wrong_project).expect("serialize wrong-project Timeline"),
        &source_ids,
    );
    assert_media_document_query_error(
        &fixture,
        fixture.media_document_options(&wrong_project_id),
        &[MediaErrorCode::CrossProjectReference],
    );

    let mut wrong_hash_document = timeline.clone();
    wrong_hash_document["runId"] = json!("run_query_wrong_hash");
    let wrong_hash_id = fixture.create_derived_media_document(
        "timeline",
        "run_query_wrong_hash",
        "timeline",
        "application/vnd.narracut.timeline+json",
        &serde_json::to_vec(&wrong_hash_document).expect("serialize wrong-hash Timeline"),
        &source_ids,
    );
    let metadata_path = Path::new(&fixture.project.project_path)
        .join("artifacts")
        .join("metadata")
        .join(format!("{wrong_hash_id}.json"));
    let mut wrong_hash_metadata = read_json(&metadata_path);
    let digest = "f".repeat(64);
    wrong_hash_metadata["contentHash"] = json!(format!("sha256:{digest}"));
    wrong_hash_metadata["uri"] = json!(format!("artifacts/objects/sha256/ff/{digest}"));
    fs::write(
        &metadata_path,
        serde_json::to_vec(&wrong_hash_metadata).expect("serialize wrong hash metadata"),
    )
    .expect("install wrong hash metadata");
    assert_media_document_query_error(
        &fixture,
        fixture.media_document_options(&wrong_hash_id),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );

    let missing = Fixture::new();
    let (_chain, timeline_result, _timeline) =
        missing.generated_timeline_base("media-document-query-missing");
    corrupt_artifact_content(&missing, &timeline_result.artifact_id, false);
    assert_media_document_query_error(
        &missing,
        missing.media_document_options(&timeline_result.artifact_id),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );

    let tampered = Fixture::new();
    let (_chain, timeline_result, _timeline) =
        tampered.generated_timeline_base("media-document-query-tampered");
    corrupt_artifact_content(&tampered, &timeline_result.artifact_id, true);
    assert_media_document_query_error(
        &tampered,
        tampered.media_document_options(&timeline_result.artifact_id),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn media_document_query_rejects_schema_valid_semantic_damage_for_every_document_type() {
    let fixture = Fixture::new();
    let (chain, timeline_result, timeline) =
        fixture.generated_timeline_base("media-document-query-semantic-damage");
    let cases = [
        (
            chain.audio_input.artifact_id.as_str(),
            "audio",
            "voice_audio",
            "application/vnd.narracut.audio+json",
        ),
        (
            chain.captions_input.artifact_id.as_str(),
            "captions",
            "captions",
            "application/vnd.narracut.captions+json",
        ),
        (
            chain.scene_plan_input.artifact_id.as_str(),
            "scene_plan",
            "scene_plan",
            "application/vnd.narracut.scene-plan+json",
        ),
        (
            timeline_result.artifact_id.as_str(),
            "timeline",
            "timeline",
            "application/vnd.narracut.timeline+json",
        ),
    ];
    for (artifact_id, stage_id, kind, media_type) in cases {
        let read = fixture
            .storage
            .get_artifact(&fixture.project.project_path, artifact_id)
            .expect("read semantic damage source metadata");
        let source_ids = read.artifact["source"]["sourceArtifactIds"]
            .as_array()
            .expect("derived source ids")
            .iter()
            .map(|value| value.as_str().expect("source id").to_owned())
            .collect::<Vec<_>>();
        let mut damaged = if kind == "timeline" {
            timeline.clone()
        } else {
            read_artifact_json(&fixture, artifact_id)
        };
        match damaged["documentType"].as_str().expect("document type") {
            "audio_media" => {
                damaged["durationMs"] = json!(99);
            }
            "captions_media" => {
                let word = damaged["mappings"]
                    .as_array_mut()
                    .expect("caption mappings")
                    .iter_mut()
                    .find(|mapping| mapping["level"] == "word")
                    .expect("word mapping");
                let start = word["startMs"].as_u64().expect("word start");
                word["startMs"] = json!(start + 1);
            }
            "scene_plan" => {
                let boundary = damaged["scenes"][0]["suggestedEndMs"]
                    .as_u64()
                    .expect("Scene Plan boundary");
                damaged["scenes"][0]["suggestedEndMs"] = json!(boundary - 1);
            }
            "timeline" => {
                let boundary = damaged["sceneTrack"][0]["endMs"]
                    .as_u64()
                    .expect("Timeline boundary");
                damaged["sceneTrack"][0]["endMs"] = json!(boundary - 1);
            }
            other => panic!("unexpected media document type {other}"),
        }
        validate_media_document(&damaged).expect("semantic damage remains schema-valid");
        let run_id = damaged["runId"].as_str().expect("damaged run").to_owned();
        let damaged_id = fixture.create_derived_media_document(
            stage_id,
            &run_id,
            kind,
            media_type,
            &serde_json::to_vec(&damaged).expect("serialize semantic damage"),
            &source_ids,
        );
        assert_media_document_query_error(
            &fixture,
            fixture.media_document_options(&damaged_id),
            &[MediaErrorCode::ContractViolation],
        );
    }
}

#[test]
fn timeline_generation_persists_schema_valid_three_track_approved_closure() {
    let fixture = Fixture::new();
    let chain = fixture.install_approved_timeline_chain("timeline-happy");
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let result = fixture
        .media
        .generate_timeline(fixture.timeline_options("timeline-happy-generate", &chain))
        .expect("generate Timeline");

    assert_eq!(result.api_version, "1.0.0");
    assert_eq!(result.operation, "generate_timeline");
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.run_id, "run_timeline_media");
    assert_eq!(result.stale_because_stage_ids, ["timeline"]);
    assert!(!result.idempotent_replay);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read Timeline metadata");
    assert_eq!(read.artifact["stageId"], "timeline");
    assert_eq!(read.artifact["runId"], "run_timeline_media");
    assert_eq!(read.artifact["kind"], "timeline");
    assert_eq!(
        read.artifact["mediaType"],
        "application/vnd.narracut.timeline+json"
    );
    assert_eq!(
        read.artifact["source"]["sourceArtifactIds"],
        json!([
            chain.audio_input.artifact_id,
            chain.captions_input.artifact_id,
            chain.scene_plan_input.artifact_id,
        ])
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.artifact_id)
            .expect("verify Timeline Artifact")
            .status,
        ArtifactVerificationStatusData::Verified
    );

    let document = read_artifact_json(&fixture, &result.artifact_id);
    validate_media_document(&document).expect("Timeline media schema");
    parse_media_document(document.clone()).expect("typed Timeline roundtrip");
    validate_timeline_semantics(&document).expect("Timeline semantics");
    assert_eq!(document["documentType"], "timeline");
    assert_eq!(document["projectId"], fixture.project.project_id);
    assert_eq!(document["runId"], "run_timeline_media");
    assert_eq!(document["durationMs"], 100);
    assert_eq!(
        document["inputRefs"]
            .as_array()
            .expect("Timeline inputs")
            .iter()
            .map(|input| input["stageId"].as_str().expect("input stage"))
            .collect::<Vec<_>>(),
        vec!["audio", "captions", "scene_plan"]
    );
    assert_eq!(document["canvas"]["width"], 1_920);
    assert_eq!(document["canvas"]["frameRateNumerator"], 30_000);
    assert_eq!(
        document["safeArea"],
        json!({"x":96,"y":54,"width":1728,"height":972})
    );
    assert_eq!(
        document["audioTrack"],
        json!({
            "audioArtifactId": chain.audio_input.artifact_id,
            "startMs": 0,
            "endMs": 100,
        })
    );
    assert_eq!(
        document["captionTrack"]["captionsArtifactId"],
        chain.captions_input.artifact_id
    );
    let captions_document = read_artifact_json(&fixture, &chain.captions_input.artifact_id);
    let expected_cue_ids = captions_document["cues"]
        .as_array()
        .expect("Captions cues")
        .iter()
        .map(|cue| cue["cueId"].clone())
        .collect::<Vec<_>>();
    assert_eq!(document["captionTrack"]["cueIds"], json!(expected_cue_ids));
    assert_eq!(document["captionTrack"]["visible"], true);
    let scenes = document["sceneTrack"].as_array().expect("Timeline scenes");
    assert_eq!(scenes.len(), 2);
    assert_eq!(scenes[0]["startMs"], 0);
    assert_eq!(scenes.last().expect("last Timeline Scene")["endMs"], 100);
    assert!(scenes
        .windows(2)
        .all(|pair| pair[0]["endMs"] == pair[1]["startMs"]));
    let tracks = serde_json::to_string(&json!({
        "audioTrack": document["audioTrack"],
        "sceneTrack": document["sceneTrack"],
        "captionTrack": document["captionTrack"],
    }))
    .expect("serialize Timeline tracks");
    assert!(!tracks.contains("A traceable narration."));
    assert!(!tracks.contains("claim_audio_1"));
    assert!(!tracks.contains("evidence_audio_1"));
    let mut scene_ids = scenes
        .iter()
        .map(|scene| scene["sceneId"].as_str().expect("Scene ID").to_owned())
        .collect::<Vec<_>>();
    scene_ids.sort();
    assert_eq!(result.changed_scene_ids, scene_ids);
    assert_eq!(
        document["changeSummary"]["changedSceneIds"],
        json!(result.changed_scene_ids)
    );
    assert_eq!(
        document["configSnapshot"]["algorithmId"],
        "narracut.timeline.approved-media-assembly"
    );
    assert_eq!(document["configSnapshot"]["algorithmVersion"], "1.0.0");
}

#[test]
fn timeline_rejects_three_input_approval_identity_and_entity_matrix() {
    let expected = [
        MediaErrorCode::InvalidRequest,
        MediaErrorCode::InputNotApproved,
        MediaErrorCode::InputReferenceMismatch,
        MediaErrorCode::CrossProjectReference,
        MediaErrorCode::ArtifactVerificationFailed,
    ];
    for slot in ["audio", "captions", "scene_plan"] {
        let fixture = Fixture::new();
        let base = fixture.install_approved_timeline_chain(&format!("timeline-matrix-{slot}"));

        let mut wrong_hash = base.clone();
        timeline_input_mut(&mut wrong_hash, slot).content_hash =
            format!("sha256:{}", "9".repeat(64));
        assert_timeline_inputs_rejected(
            &fixture,
            wrong_hash,
            &format!("timeline-{slot}-wrong-hash"),
            &expected,
        );

        let mut wrong_review = base.clone();
        timeline_input_mut(&mut wrong_review, slot).review_record_id =
            format!("review_{slot}_outdated_timeline");
        assert_timeline_inputs_rejected(
            &fixture,
            wrong_review,
            &format!("timeline-{slot}-wrong-review"),
            &expected,
        );

        let mut wrong_run = base.clone();
        timeline_input_mut(&mut wrong_run, slot).run_id = format!("run_{slot}_outdated_timeline");
        assert_timeline_inputs_rejected(
            &fixture,
            wrong_run,
            &format!("timeline-{slot}-wrong-run"),
            &expected,
        );

        let mut wrong_kind = base.clone();
        let mut script_as_media = fixture.script_input.clone();
        script_as_media.stage_id = slot.to_owned();
        *timeline_input_mut(&mut wrong_kind, slot) = script_as_media;
        assert_timeline_inputs_rejected(
            &fixture,
            wrong_kind,
            &format!("timeline-{slot}-wrong-kind"),
            &expected,
        );

        let foreign = Fixture::new();
        let foreign_chain =
            foreign.install_approved_timeline_chain(&format!("timeline-foreign-{slot}"));
        let mut cross_project = base;
        *timeline_input_mut(&mut cross_project, slot) =
            timeline_input(&foreign_chain, slot).clone();
        assert_timeline_inputs_rejected(
            &fixture,
            cross_project,
            &format!("timeline-{slot}-cross-project"),
            &expected,
        );

        for (suffix, tamper) in [("missing", false), ("tampered", true)] {
            let damaged = Fixture::new();
            let chain = damaged
                .install_approved_timeline_chain(&format!("timeline-{slot}-{suffix}-entity"));
            corrupt_artifact_content(&damaged, &timeline_input(&chain, slot).artifact_id, tamper);
            assert_timeline_inputs_rejected(
                &damaged,
                chain,
                &format!("timeline-{slot}-{suffix}"),
                &expected,
            );
        }
    }
}

#[test]
fn timeline_rejects_approved_scene_plan_document_identity_and_semantic_damage() {
    for damage in ["schema", "identity", "semantic"] {
        let fixture = Fixture::new();
        let captions = fixture.install_approved_caption_chain(
            &format!("timeline-document-{damage}-audio"),
            &format!("timeline-document-{damage}-captions"),
        );
        let base_result = fixture
            .media
            .generate_scene_plan(fixture.scene_plan_options(
                &format!("timeline-document-{damage}-base"),
                &captions.captions_input,
            ))
            .expect("generate source Scene Plan document");
        let mut document = read_artifact_json(&fixture, &base_result.artifact_id);
        let run_id = format!("run_scene_plan_{damage}_timeline");
        if damage == "schema" {
            document = json!({"arbitrary":true});
        } else if damage == "identity" {
            document["runId"] = json!("run_scene_plan_wrong_document_timeline");
        } else {
            document["runId"] = json!(run_id);
            let boundary = document["scenes"][0]["suggestedEndMs"]
                .as_u64()
                .expect("Scene boundary");
            document["scenes"][1]["suggestedStartMs"] = json!(boundary - 1);
        }
        let review_id = format!("review_scene_plan_{damage}_timeline");
        let scene_plan_input = fixture.approved_scene_plan_payload(
            &run_id,
            &review_id,
            &serde_json::to_vec(&document).expect("serialize damaged Scene Plan"),
            &captions,
        );
        let chain = ApprovedTimelineChain {
            audio_input: captions.audio_input,
            captions_input: captions.captions_input,
            scene_plan_input,
        };
        assert_timeline_inputs_rejected(
            &fixture,
            chain,
            &format!("timeline-document-{damage}"),
            &[
                MediaErrorCode::InputReferenceMismatch,
                MediaErrorCode::ContractViolation,
            ],
        );
    }
}

#[test]
fn timeline_rejects_approved_caption_semantics_and_audio_duration_drift() {
    let captions_damage = Fixture::new();
    let base = captions_damage.install_approved_caption_chain(
        "timeline-caption-semantic-audio",
        "timeline-caption-semantic-base",
    );
    let scene_result = captions_damage
        .media
        .generate_scene_plan(captions_damage.scene_plan_options(
            "timeline-caption-semantic-scene-source",
            &base.captions_input,
        ))
        .expect("generate Scene Plan source for invalid Captions closure");
    let mut scene_document = read_artifact_json(&captions_damage, &scene_result.artifact_id);
    let mut captions_document =
        read_artifact_json(&captions_damage, &base.captions_input.artifact_id);
    captions_document["runId"] = json!("run_captions_semantic_timeline");
    let first_end = captions_document["cues"][0]["endMs"]
        .as_u64()
        .expect("first cue end");
    captions_document["cues"][1]["startMs"] = json!(first_end - 1);
    let captions_input = captions_damage.approved_derived_captions_payload(
        "run_captions_semantic_timeline",
        "review_captions_semantic_timeline",
        &base.audio_input,
        &serde_json::to_vec(&captions_document).expect("serialize semantic Captions damage"),
    );
    scene_document["runId"] = json!("run_scene_plan_caption_semantic_timeline");
    scene_document["inputRefs"][2] = frozen_input_json(&captions_damage, &captions_input);
    let custom_caption_chain = ApprovedCaptionChain {
        audio_input: base.audio_input.clone(),
        captions_input: captions_input.clone(),
    };
    let scene_plan_input = captions_damage.approved_scene_plan_payload(
        "run_scene_plan_caption_semantic_timeline",
        "review_scene_plan_caption_semantic_timeline",
        &serde_json::to_vec(&scene_document).expect("serialize Scene Plan caption closure"),
        &custom_caption_chain,
    );
    assert_timeline_inputs_rejected(
        &captions_damage,
        ApprovedTimelineChain {
            audio_input: base.audio_input,
            captions_input,
            scene_plan_input,
        },
        "timeline-caption-semantic-damage",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let duration_damage = Fixture::new();
    let base = duration_damage.install_approved_timeline_chain("timeline-duration-base");
    let mut audio_document = read_artifact_json(&duration_damage, &base.audio_input.artifact_id);
    audio_document["runId"] = json!("run_audio_duration_timeline");
    audio_document["durationMs"] = json!(99);
    let audio_input = duration_damage.approved_derived_audio_payload(
        "run_audio_duration_timeline",
        "review_audio_duration_timeline",
        &serde_json::to_vec(&audio_document).expect("serialize duration-drift Audio"),
    );
    let mut captions_document =
        read_artifact_json(&duration_damage, &base.captions_input.artifact_id);
    captions_document["runId"] = json!("run_captions_duration_timeline");
    let audio_ref = frozen_input_json(&duration_damage, &audio_input);
    captions_document["audioInput"] = audio_ref.clone();
    captions_document["inputRefs"][1] = audio_ref;
    let captions_input = duration_damage.approved_derived_captions_payload(
        "run_captions_duration_timeline",
        "review_captions_duration_timeline",
        &audio_input,
        &serde_json::to_vec(&captions_document).expect("serialize duration Captions closure"),
    );
    let mut scene_document =
        read_artifact_json(&duration_damage, &base.scene_plan_input.artifact_id);
    scene_document["runId"] = json!("run_scene_plan_duration_timeline");
    scene_document["inputRefs"][2] = frozen_input_json(&duration_damage, &captions_input);
    let custom_caption_chain = ApprovedCaptionChain {
        audio_input: audio_input.clone(),
        captions_input: captions_input.clone(),
    };
    let scene_plan_input = duration_damage.approved_scene_plan_payload(
        "run_scene_plan_duration_timeline",
        "review_scene_plan_duration_timeline",
        &serde_json::to_vec(&scene_document).expect("serialize duration Scene Plan closure"),
        &custom_caption_chain,
    );
    assert_timeline_inputs_rejected(
        &duration_damage,
        ApprovedTimelineChain {
            audio_input,
            captions_input,
            scene_plan_input,
        },
        "timeline-audio-duration-drift",
        &[
            MediaErrorCode::InputReferenceMismatch,
            MediaErrorCode::ContractViolation,
        ],
    );
}

#[test]
fn timeline_idempotency_replays_conflicts_and_revalidates_stable_artifact() {
    let fixture = Fixture::new();
    let chain = fixture.install_approved_timeline_chain("timeline-idempotency");
    let options = fixture.timeline_options("timeline-idempotent-key", &chain);
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let first = fixture
        .media
        .generate_timeline(options.clone())
        .expect("first Timeline generation");
    let first_document = read_artifact_json(&fixture, &first.artifact_id);
    let replay = fixture
        .media
        .generate_timeline(options.clone())
        .expect("Timeline replay");
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.changed_scene_ids, replay.changed_scene_ids);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    let mut changed_canvas = options.clone();
    changed_canvas.canvas.width = 1_280;
    changed_canvas.safe_area = TimelineSafeAreaData {
        x: 64,
        y: 36,
        width: 1_152,
        height: 648,
    };
    let mut changed_safe_area = options.clone();
    changed_safe_area.safe_area.x = 80;
    let mut changed_run = options.clone();
    changed_run.run_id = "run_timeline_other".to_owned();
    let mut changed_hash = options.clone();
    changed_hash.scene_plan_input.content_hash = format!("sha256:{}", "7".repeat(64));
    for changed in [changed_canvas, changed_safe_area, changed_run, changed_hash] {
        let error = fixture
            .media
            .generate_timeline(changed)
            .expect_err("changed Timeline semantics on one key must conflict");
        assert_eq!(error.code, MediaErrorCode::IdempotencyConflict);
    }
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    assert_eq!(
        first_document["timelineId"],
        read_artifact_json(&fixture, &replay.artifact_id)["timelineId"]
    );
    corrupt_artifact_content(&fixture, &first.artifact_id, true);
    let error = fixture
        .media
        .generate_timeline(options)
        .expect_err("Timeline replay must reverify immutable Artifact");
    assert_eq!(error.code, MediaErrorCode::ArtifactVerificationFailed);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
}

#[test]
fn concurrent_timeline_generation_converges_and_different_keys_preserve_history() {
    let concurrent = Fixture::new();
    let chain = concurrent.install_approved_timeline_chain("timeline-concurrent");
    let before_metadata = concurrent.metadata_count();
    let before_receipts = concurrent.receipt_count();
    let service = concurrent.media.clone();
    let options = concurrent.timeline_options("timeline-concurrent-key", &chain);
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service
                    .generate_timeline(options)
                    .expect("concurrent Timeline generation")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join Timeline generation"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_eq!(results[0].changed_scene_ids, results[1].changed_scene_ids);
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(concurrent.metadata_count(), before_metadata + 1);
    assert_eq!(concurrent.receipt_count(), before_receipts + 1);

    let history = Fixture::new();
    let chain = history.install_approved_timeline_chain("timeline-history");
    let before_metadata = history.metadata_count();
    let before_receipts = history.receipt_count();
    let first = history
        .media
        .generate_timeline(history.timeline_options("timeline-history-first", &chain))
        .expect("first Timeline history entry");
    let first_document = read_artifact_json(&history, &first.artifact_id);
    let second = history
        .media
        .generate_timeline(history.timeline_options("timeline-history-second", &chain))
        .expect("second Timeline history entry");
    let second_document = read_artifact_json(&history, &second.artifact_id);
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_eq!(first.changed_scene_ids, second.changed_scene_ids);
    assert_eq!(first_document, second_document);
    assert_eq!(first_document["timelineId"], second_document["timelineId"]);
    assert_eq!(history.metadata_count(), before_metadata + 2);
    assert_eq!(history.receipt_count(), before_receipts + 2);
    assert_eq!(
        history
            .storage
            .verify_artifact(&history.project.project_path, &first.artifact_id)
            .expect("verify first immutable Timeline")
            .status,
        ArtifactVerificationStatusData::Verified
    );
}

#[test]
fn timeline_redacts_external_paths_and_receipt_failure_projects_no_success() {
    let fixture = Fixture::new();
    let chain = fixture.install_approved_timeline_chain("timeline-path");
    let result = fixture
        .media
        .generate_timeline(fixture.timeline_options("timeline-path-redaction", &chain))
        .expect("generate pathless Timeline");
    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read pathless Timeline metadata");
    let document = read_artifact_json(&fixture, &result.artifact_id);
    let external_path = fixture.external_dir.to_string_lossy();
    for value in [
        serde_json::to_string(&result).expect("serialize Timeline result"),
        read.artifact.to_string(),
        document.to_string(),
    ] {
        assert!(!value.contains(external_path.as_ref()));
        assert!(!value.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    }
    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("read project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(external_path.as_ref()),
            "external source path leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "external path canary leaked in {path:?}"
        );
    }

    let failed = Fixture::new();
    let chain = failed.install_approved_timeline_chain("timeline-receipt-failure");
    let key = "timeline-receipt-failure-key";
    let receipt_id = stable_timeline_receipt_id_for_test(&failed.project.project_id, key);
    let receipt_path = Path::new(&failed.project.project_path)
        .join("artifacts")
        .join("media-receipts")
        .join(format!("{receipt_id}.json"));
    fs::write(&receipt_path, b"{malformed receipt")
        .expect("install malformed Timeline receipt fixture");
    let before_metadata = failed.metadata_count();
    let before_receipts = failed.receipt_count();
    let error = failed
        .media
        .generate_timeline(failed.timeline_options(key, &chain))
        .expect_err("receipt failure must not return Timeline success");
    assert!(matches!(
        error.code,
        MediaErrorCode::StorageUnavailable | MediaErrorCode::InvalidRequest
    ));
    assert_eq!(failed.metadata_count(), before_metadata);
    assert_eq!(failed.receipt_count(), before_receipts);
    let has_timeline_artifact = fs::read_dir(
        Path::new(&failed.project.project_path)
            .join("artifacts")
            .join("metadata"),
    )
    .expect("read Artifact metadata directory")
    .filter_map(Result::ok)
    .map(|entry| read_json(entry.path()))
    .any(|artifact| artifact["stageId"] == "timeline");
    assert!(!has_timeline_artifact);
}

#[test]
fn scene_plan_generation_persists_a_schema_valid_traceable_document() {
    let fixture = Fixture::new();
    let chain = fixture
        .install_approved_caption_chain("scene-plan-happy-audio", "scene-plan-happy-captions");
    let result = fixture
        .media
        .generate_scene_plan(fixture.scene_plan_options("scene-plan-happy", &chain.captions_input))
        .expect("generate Scene Plan");

    assert_eq!(result.api_version, "1.0.0");
    assert_eq!(result.operation, "generate_scene_plan");
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.run_id, "run_scene_plan_media");
    assert_eq!(result.stale_because_stage_ids, ["scene_plan"]);
    assert!(!result.idempotent_replay);

    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read Scene Plan metadata");
    assert_eq!(read.artifact["stageId"], "scene_plan");
    assert_eq!(read.artifact["runId"], "run_scene_plan_media");
    assert_eq!(read.artifact["kind"], "scene_plan");
    assert_eq!(
        read.artifact["mediaType"],
        "application/vnd.narracut.scene-plan+json"
    );
    assert_eq!(
        read.artifact["source"]["sourceArtifactIds"],
        json!([
            fixture.research_input().artifact_id,
            fixture.script_input.artifact_id,
            chain.captions_input.artifact_id,
            chain.audio_input.artifact_id,
        ])
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.artifact_id)
            .expect("verify Scene Plan")
            .status,
        ArtifactVerificationStatusData::Verified
    );

    let bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &result.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read Scene Plan document");
    let document: Value = serde_json::from_slice(&bytes).expect("Scene Plan JSON");
    validate_media_document(&document).expect("Scene Plan media schema");
    parse_media_document(document.clone()).expect("typed Scene Plan roundtrip");
    assert_eq!(document["documentType"], "scene_plan");
    assert_eq!(document["projectId"], fixture.project.project_id);
    assert_eq!(document["runId"], "run_scene_plan_media");
    assert_eq!(document["inputRefs"].as_array().map(Vec::len), Some(3));
    assert_eq!(document["inputRefs"][0]["stageId"], "research");
    assert_eq!(document["inputRefs"][1]["stageId"], "script");
    assert_eq!(document["inputRefs"][2]["stageId"], "captions");
    assert_eq!(
        document["configSnapshot"]["algorithmId"],
        "narracut.scene-plan.caption-cue-grouping"
    );
    assert_eq!(document["configSnapshot"]["algorithmVersion"], "1.0.0");
    let scenes = document["scenes"].as_array().expect("Scene Plan scenes");
    assert_eq!(scenes.len(), 2);
    assert_eq!(scenes[0]["suggestedStartMs"], 0);
    assert_eq!(scenes.last().expect("last scene")["suggestedEndMs"], 100);
    let cue_ids = scenes
        .iter()
        .flat_map(|scene| {
            scene["cueIds"]
                .as_array()
                .expect("scene cue ids")
                .iter()
                .map(|value| value.as_str().expect("cue id").to_owned())
        })
        .collect::<Vec<_>>();
    assert_eq!(cue_ids.len(), 4);
    assert_eq!(
        cue_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert!(scenes.iter().all(|scene| {
        scene["claimIds"] == json!(["claim_audio_1"])
            && scene["evidenceRefs"] == json!(["evidence_audio_1"])
    }));
    let mut scene_ids = scenes
        .iter()
        .map(|scene| scene["sceneId"].as_str().expect("scene id").to_owned())
        .collect::<Vec<_>>();
    scene_ids.sort();
    assert_eq!(result.changed_scene_ids, scene_ids);
    assert_eq!(
        document["changeSummary"]["changedSceneIds"],
        json!(result.changed_scene_ids)
    );
}

#[test]
fn timeline_save_persists_all_edit_types_sequentially_and_preserves_the_base() {
    let fixture = Fixture::new();
    let (chain, base_result, base) =
        fixture.generated_timeline_base("timeline-save-all-edits-base");
    fixture.approve_timeline_and_downstream(&chain, "run_timeline_media", &base_result.artifact_id);
    let base_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &base_result.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read immutable base Timeline");
    let left_id = base["sceneTrack"][0]["sceneId"]
        .as_str()
        .expect("left scene id")
        .to_owned();
    let right_id = base["sceneTrack"][1]["sceneId"]
        .as_str()
        .expect("right scene id")
        .to_owned();
    assert_eq!(base["sceneTrack"][0]["endMs"], 60);
    let safe_area = TimelineSafeAreaData {
        x: 100,
        y: 60,
        width: 1_700,
        height: 900,
    };
    let edits = vec![
        TimelineEditData::MoveSceneBoundary {
            left_scene_id: left_id.clone(),
            right_scene_id: right_id.clone(),
            boundary_ms: 50,
        },
        TimelineEditData::SetSafeArea { safe_area },
        TimelineEditData::SetCaptionVisibility { visible: false },
        TimelineEditData::MoveSceneBoundary {
            left_scene_id: left_id.clone(),
            right_scene_id: right_id.clone(),
            boundary_ms: 55,
        },
    ];
    let summary = "Move the scene boundary twice, then save reviewed framing and captions.";
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let result = fixture
        .media
        .save_timeline(fixture.timeline_save_options(
            "timeline-save-all-edits",
            "run_timeline_saved",
            &base_result.artifact_id,
            edits,
            summary,
        ))
        .expect("save edited Timeline");

    assert_eq!(result.api_version, "1.0.0");
    assert_eq!(result.operation, "save_timeline");
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.run_id, "run_timeline_saved");
    assert!(result.stale_because_stage_ids.is_empty());
    assert!(!result.idempotent_replay);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
    assert_saved_stage_run(
        &fixture,
        "timeline",
        "run_timeline_saved",
        &base_result.artifact_id,
        &result.artifact_id,
    );
    assert_eq!(
        fixture.stage_status("timeline"),
        StageStatusData::NeedsReview
    );
    assert_eq!(fixture.stage_status("render"), StageStatusData::Approved);
    assert_eq!(fixture.stage_status("export"), StageStatusData::Approved);
    fixture.approve("timeline", "run_timeline_saved", "review_timeline_saved");
    assert_eq!(fixture.stage_status("render"), StageStatusData::Stale);
    assert_eq!(fixture.stage_status("export"), StageStatusData::Stale);

    let saved = read_artifact_json(&fixture, &result.artifact_id);
    validate_media_document(&saved).expect("saved Timeline media schema");
    parse_media_document(saved.clone()).expect("saved Timeline typed roundtrip");
    validate_timeline_semantics(&saved).expect("saved Timeline semantics");
    assert_eq!(saved["runId"], "run_timeline_saved");
    assert_ne!(saved["timelineId"], base["timelineId"]);
    assert_eq!(saved["supersedesArtifactId"], base_result.artifact_id);
    assert_eq!(saved["changeSummary"]["summary"], summary);
    assert_eq!(saved["inputRefs"], base["inputRefs"]);
    assert_eq!(saved["configSnapshot"], base["configSnapshot"]);
    assert_eq!(saved["sceneTrack"][0]["endMs"], 55);
    assert_eq!(saved["sceneTrack"][1]["startMs"], 55);
    assert_eq!(saved["safeArea"], json!(safe_area));
    assert_eq!(saved["captionTrack"]["visible"], false);
    let mut expected_changed = vec![left_id, right_id];
    expected_changed.sort();
    assert_eq!(result.changed_scene_ids, expected_changed);
    assert_eq!(
        saved["changeSummary"]["changedSceneIds"],
        json!(result.changed_scene_ids)
    );

    let metadata = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read saved Timeline metadata");
    let mut expected_sources = vec![base_result.artifact_id.clone()];
    expected_sources.extend(
        base["inputRefs"]
            .as_array()
            .expect("base Timeline inputs")
            .iter()
            .map(|input| {
                input["artifactId"]
                    .as_str()
                    .expect("input artifact")
                    .to_owned()
            }),
    );
    assert_eq!(
        metadata.artifact["source"]["sourceArtifactIds"],
        json!(expected_sources)
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.artifact_id)
            .expect("verify saved Timeline")
            .status,
        ArtifactVerificationStatusData::Verified
    );
    assert_eq!(
        fixture
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &base_result.artifact_id,
                16 * 1024 * 1024,
            )
            .expect("base Timeline remains readable"),
        base_bytes
    );
}

#[test]
fn timeline_save_rejects_illegal_edits_and_request_limits_atomically() {
    let fixture = Fixture::new();
    let (_chain, _generated, mut three_scene) =
        fixture.generated_timeline_base("timeline-save-invalid-edits-base");
    three_scene["timelineId"] = json!("timeline_three_scene_base");
    three_scene["runId"] = json!("run_timeline_three_scene_base");
    three_scene["sceneTrack"] = json!([
        {"sceneId":"scene_custom_a","startMs":0,"endMs":30},
        {"sceneId":"scene_custom_b","startMs":30,"endMs":70},
        {"sceneId":"scene_custom_c","startMs":70,"endMs":100}
    ]);
    three_scene["changeSummary"] = json!({
        "summary": "Valid three-scene Timeline base for edit validation.",
        "changedSceneIds": ["scene_custom_a", "scene_custom_b", "scene_custom_c"],
    });
    validate_media_document(&three_scene).expect("three-scene base schema");
    validate_timeline_semantics(&three_scene).expect("three-scene base semantics");
    let source_ids = three_scene["inputRefs"]
        .as_array()
        .expect("Timeline base inputs")
        .iter()
        .map(|input| input["artifactId"].as_str().expect("input id").to_owned())
        .collect::<Vec<_>>();
    let base_artifact_id = fixture.create_derived_timeline_artifact(
        "run_timeline_three_scene_base",
        &serde_json::to_vec(&three_scene).expect("serialize three-scene Timeline"),
        &source_ids,
    );
    let base_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &base_artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read three-scene base bytes");
    let oversized = (0..1_001)
        .map(|index| TimelineEditData::SetCaptionVisibility {
            visible: index % 2 == 0,
        })
        .collect::<Vec<_>>();
    let cases = vec![
        fixture.timeline_save_options(
            "timeline-save-current-boundary",
            "run_timeline_current_boundary",
            &base_artifact_id,
            vec![TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_custom_a".to_owned(),
                right_scene_id: "scene_custom_b".to_owned(),
                boundary_ms: 30,
            }],
            "Reject the unchanged boundary.",
        ),
        fixture.timeline_save_options(
            "timeline-save-endpoint-boundary",
            "run_timeline_endpoint_boundary",
            &base_artifact_id,
            vec![TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_custom_a".to_owned(),
                right_scene_id: "scene_custom_b".to_owned(),
                boundary_ms: 0,
            }],
            "Reject an endpoint boundary.",
        ),
        fixture.timeline_save_options(
            "timeline-save-nonadjacent-boundary",
            "run_timeline_nonadjacent_boundary",
            &base_artifact_id,
            vec![TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_custom_a".to_owned(),
                right_scene_id: "scene_custom_c".to_owned(),
                boundary_ms: 50,
            }],
            "Reject a non-adjacent boundary edit.",
        ),
        fixture.timeline_save_options(
            "timeline-save-overflow-safe-area",
            "run_timeline_overflow_safe_area",
            &base_artifact_id,
            vec![TimelineEditData::SetSafeArea {
                safe_area: TimelineSafeAreaData {
                    x: 1_900,
                    y: 0,
                    width: 100,
                    height: 1_000,
                },
            }],
            "Reject a safe area outside the canvas.",
        ),
        fixture.timeline_save_options(
            "timeline-save-empty-edits",
            "run_timeline_empty_edits",
            &base_artifact_id,
            Vec::new(),
            "Reject empty edits.",
        ),
        fixture.timeline_save_options(
            "timeline-save-too-many-edits",
            "run_timeline_too_many_edits",
            &base_artifact_id,
            oversized,
            "Reject too many edits.",
        ),
        fixture.timeline_save_options(
            "timeline-save-control-summary",
            "run_timeline_control_summary",
            &base_artifact_id,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
            "unsafe\u{0000}summary",
        ),
        fixture.timeline_save_options(
            "timeline-save-late-invalid-edit",
            "run_timeline_late_invalid_edit",
            &base_artifact_id,
            vec![
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: "scene_custom_a".to_owned(),
                    right_scene_id: "scene_custom_b".to_owned(),
                    boundary_ms: 40,
                },
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: "scene_custom_a".to_owned(),
                    right_scene_id: "scene_custom_c".to_owned(),
                    boundary_ms: 60,
                },
            ],
            "A late invalid edit must roll back the whole candidate.",
        ),
        fixture.timeline_save_options(
            "timeline-save-reused-run",
            "run_timeline_three_scene_base",
            &base_artifact_id,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
            "Reject reuse of the base run identity.",
        ),
    ];
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    for options in cases {
        let reservation_path = Path::new(&fixture.project.project_path)
            .join("runs/reservations")
            .join(format!("{}.json", options.run_id));
        let reservation_existed = reservation_path.exists();
        let error = fixture
            .media
            .save_timeline(options)
            .expect_err("invalid Timeline save must fail atomically");
        assert_eq!(error.code, MediaErrorCode::InvalidRequest);
        assert_eq!(fixture.metadata_count(), before_metadata);
        assert_eq!(fixture.receipt_count(), before_receipts);
        assert_eq!(
            reservation_path.exists(),
            reservation_existed,
            "invalid Timeline edit must not create a StageRun reservation"
        );
    }
    assert_eq!(
        fixture
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &base_artifact_id,
                16 * 1024 * 1024,
            )
            .expect("invalid edits preserve base bytes"),
        base_bytes
    );
}

#[test]
fn timeline_save_rejects_cross_project_arbitrary_identity_semantic_lineage_missing_and_tampered_bases(
) {
    let fixture = Fixture::new();
    let (_chain, _base_result, base) =
        fixture.generated_timeline_base("timeline-save-invalid-bases");
    let edit = || vec![TimelineEditData::SetCaptionVisibility { visible: false }];
    let source_ids = base["inputRefs"]
        .as_array()
        .expect("base input refs")
        .iter()
        .map(|input| input["artifactId"].as_str().expect("input id").to_owned())
        .collect::<Vec<_>>();

    let arbitrary_id = fixture.create_derived_timeline_artifact(
        "run_timeline_arbitrary_base",
        br#"{"arbitrary":true}"#,
        &source_ids,
    );
    assert_timeline_save_error(
        &fixture,
        fixture.timeline_save_options(
            "timeline-save-arbitrary-base",
            "run_timeline_from_arbitrary",
            &arbitrary_id,
            edit(),
            "Reject arbitrary JSON pretending to be a Timeline.",
        ),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let foreign = Fixture::new();
    let mut cross_project = base.clone();
    cross_project["projectId"] = json!(foreign.project.project_id);
    cross_project["runId"] = json!("run_timeline_cross_base");
    let cross_id = fixture.create_derived_timeline_artifact(
        "run_timeline_cross_base",
        &serde_json::to_vec(&cross_project).expect("serialize cross-project Timeline"),
        &source_ids,
    );
    assert_timeline_save_error(
        &fixture,
        fixture.timeline_save_options(
            "timeline-save-cross-project-base",
            "run_timeline_from_cross_project",
            &cross_id,
            edit(),
            "Reject a cross-project Timeline base.",
        ),
        &[MediaErrorCode::CrossProjectReference],
    );

    let mut wrong_identity = base.clone();
    wrong_identity["runId"] = json!("run_timeline_identity_document");
    let identity_id = fixture.create_derived_timeline_artifact(
        "run_timeline_identity_metadata",
        &serde_json::to_vec(&wrong_identity).expect("serialize wrong identity Timeline"),
        &source_ids,
    );
    assert_timeline_save_error(
        &fixture,
        fixture.timeline_save_options(
            "timeline-save-wrong-base-identity",
            "run_timeline_from_wrong_identity",
            &identity_id,
            edit(),
            "Reject a run identity mismatch.",
        ),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let mut invalid_semantics = base.clone();
    invalid_semantics["runId"] = json!("run_timeline_semantic_base");
    invalid_semantics["sceneTrack"][0]["endMs"] = json!(59);
    let semantic_id = fixture.create_derived_timeline_artifact(
        "run_timeline_semantic_base",
        &serde_json::to_vec(&invalid_semantics).expect("serialize invalid semantic Timeline"),
        &source_ids,
    );
    assert_timeline_save_error(
        &fixture,
        fixture.timeline_save_options(
            "timeline-save-invalid-semantic-base",
            "run_timeline_from_invalid_semantics",
            &semantic_id,
            edit(),
            "Reject invalid Timeline coverage semantics.",
        ),
        &[MediaErrorCode::ContractViolation],
    );

    let mut valid_lineage_document = base.clone();
    valid_lineage_document["runId"] = json!("run_timeline_lineage_base");
    let lineage_id = fixture.create_derived_timeline_artifact(
        "run_timeline_lineage_base",
        &serde_json::to_vec(&valid_lineage_document).expect("serialize lineage Timeline"),
        &source_ids[..2],
    );
    assert_timeline_save_error(
        &fixture,
        fixture.timeline_save_options(
            "timeline-save-invalid-lineage-base",
            "run_timeline_from_invalid_lineage",
            &lineage_id,
            edit(),
            "Reject Timeline metadata that does not close over inputRefs.",
        ),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let missing = Fixture::new();
    let (_chain, base_result, _base) =
        missing.generated_timeline_base("timeline-save-missing-base");
    corrupt_artifact_content(&missing, &base_result.artifact_id, false);
    assert_timeline_save_error(
        &missing,
        missing.timeline_save_options(
            "timeline-save-missing-content",
            "run_timeline_from_missing",
            &base_result.artifact_id,
            edit(),
            "Reject missing base content.",
        ),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );

    let tampered = Fixture::new();
    let (_chain, base_result, _base) =
        tampered.generated_timeline_base("timeline-save-tampered-base");
    corrupt_artifact_content(&tampered, &base_result.artifact_id, true);
    assert_timeline_save_error(
        &tampered,
        tampered.timeline_save_options(
            "timeline-save-tampered-content",
            "run_timeline_from_tampered",
            &base_result.artifact_id,
            edit(),
            "Reject tampered base content.",
        ),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn timeline_save_idempotency_replays_conflicts_and_revalidates_the_saved_artifact() {
    let fixture = Fixture::new();
    let (chain, base_result, base) =
        fixture.generated_timeline_base("timeline-save-idempotency-base");
    let second_base = fixture
        .media
        .generate_timeline(fixture.timeline_options("timeline-save-idempotency-other-base", &chain))
        .expect("generate alternate immutable Timeline base");
    let left_id = base["sceneTrack"][0]["sceneId"]
        .as_str()
        .expect("left scene")
        .to_owned();
    let right_id = base["sceneTrack"][1]["sceneId"]
        .as_str()
        .expect("right scene")
        .to_owned();
    let edits = vec![
        TimelineEditData::MoveSceneBoundary {
            left_scene_id: left_id.clone(),
            right_scene_id: right_id.clone(),
            boundary_ms: 50,
        },
        TimelineEditData::SetCaptionVisibility { visible: false },
    ];
    let options = fixture.timeline_save_options(
        "timeline-save-idempotent",
        "run_timeline_save_idempotent",
        &base_result.artifact_id,
        edits.clone(),
        "Save exactly once and replay by immutable receipt.",
    );
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let first = fixture
        .media
        .save_timeline(options.clone())
        .expect("first Timeline save");
    let replay = fixture
        .media
        .save_timeline(options.clone())
        .expect("replay Timeline save");
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.changed_scene_ids, replay.changed_scene_ids);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    let mut changed_edit = options.clone();
    changed_edit.edits = vec![TimelineEditData::MoveSceneBoundary {
        left_scene_id: left_id,
        right_scene_id: right_id,
        boundary_ms: 55,
    }];
    let mut changed_summary = options.clone();
    changed_summary.change_summary = "Different semantic summary.".to_owned();
    let mut changed_order = options.clone();
    changed_order.edits.reverse();
    let mut changed_run = options.clone();
    changed_run.run_id = "run_timeline_save_other".to_owned();
    let mut changed_base = options.clone();
    changed_base.base_artifact_id = second_base.artifact_id;
    for changed in [
        changed_edit,
        changed_summary,
        changed_order,
        changed_run,
        changed_base,
    ] {
        let error = fixture
            .media
            .save_timeline(changed)
            .expect_err("same key with changed save semantics must conflict");
        assert_eq!(error.code, MediaErrorCode::IdempotencyConflict);
    }
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    corrupt_artifact_content(&fixture, &first.artifact_id, true);
    let error = fixture
        .media
        .save_timeline(options)
        .expect_err("Timeline save replay must reverify persisted Artifact content");
    assert_eq!(error.code, MediaErrorCode::ArtifactVerificationFailed);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
}

#[test]
fn concurrent_timeline_saves_converge_and_different_keys_preserve_non_scene_edit_history() {
    let concurrent = Fixture::new();
    let (_chain, base_result, _base) =
        concurrent.generated_timeline_base("timeline-save-concurrent-base");
    let options = concurrent.timeline_save_options(
        "timeline-save-concurrent",
        "run_timeline_save_concurrent",
        &base_result.artifact_id,
        vec![TimelineEditData::SetSafeArea {
            safe_area: TimelineSafeAreaData {
                x: 100,
                y: 60,
                width: 1_700,
                height: 900,
            },
        }],
        "Concurrent safe-area saves converge on one immutable Artifact.",
    );
    let before_metadata = concurrent.metadata_count();
    let before_receipts = concurrent.receipt_count();
    let service = concurrent.media.clone();
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service
                    .save_timeline(options)
                    .expect("concurrent Timeline save")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join concurrent Timeline save"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_eq!(results[0].changed_scene_ids, Vec::<String>::new());
    assert_eq!(results[1].changed_scene_ids, Vec::<String>::new());
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(concurrent.metadata_count(), before_metadata + 1);
    assert_eq!(concurrent.receipt_count(), before_receipts + 1);

    let history = Fixture::new();
    let (_chain, base_result, _base) =
        history.generated_timeline_base("timeline-save-history-base");
    let edits = vec![TimelineEditData::SetCaptionVisibility { visible: false }];
    let before_metadata = history.metadata_count();
    let before_receipts = history.receipt_count();
    let first = history
        .media
        .save_timeline(history.timeline_save_options(
            "timeline-save-history-first",
            "run_timeline_save_history_first",
            &base_result.artifact_id,
            edits.clone(),
            "Preserve each immutable caption-visibility edit history entry.",
        ))
        .expect("first Timeline edit history");
    let first_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &first.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read first Timeline edit history");
    let second = history
        .media
        .save_timeline(history.timeline_save_options(
            "timeline-save-history-second",
            "run_timeline_save_history_second",
            &base_result.artifact_id,
            edits,
            "Preserve each immutable caption-visibility edit history entry.",
        ))
        .expect("second Timeline edit history");
    let second_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &second.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read second Timeline edit history");
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_eq!(first.changed_scene_ids, Vec::<String>::new());
    assert_eq!(second.changed_scene_ids, Vec::<String>::new());
    assert_ne!(first_bytes, second_bytes);
    assert_eq!(history.metadata_count(), before_metadata + 2);
    assert_eq!(history.receipt_count(), before_receipts + 2);
    let first_run_path = Path::new(&history.project.project_path)
        .join("runs/timeline/run_timeline_save_history_first/run.json");
    assert!(first_run_path.is_file());
    let second_run = read_json(
        Path::new(&history.project.project_path)
            .join("runs/timeline/run_timeline_save_history_second/run.json"),
    );
    assert_eq!(
        second_run["supersedesRunId"],
        "run_timeline_save_history_first"
    );
    assert_eq!(
        history
            .storage
            .read_artifact_content_bounded(
                &history.project.project_path,
                &history.project.project_id,
                &first.artifact_id,
                16 * 1024 * 1024,
            )
            .expect("first Timeline edit history remains readable"),
        first_bytes
    );
}

#[test]
fn timeline_save_redacts_paths_and_receipt_failure_returns_no_success_projection() {
    let fixture = Fixture::new();
    let (_chain, base_result, _base) = fixture.generated_timeline_base("timeline-save-path-base");
    let result = fixture
        .media
        .save_timeline(fixture.timeline_save_options(
            "timeline-save-path-redaction",
            "run_timeline_save_path",
            &base_result.artifact_id,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
            "Save without persisting any external path.",
        ))
        .expect("save pathless Timeline");
    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read pathless saved Timeline metadata");
    let document = read_artifact_json(&fixture, &result.artifact_id);
    let external_path = fixture.external_dir.to_string_lossy();
    for value in [
        serde_json::to_string(&result).expect("serialize Timeline save result"),
        read.artifact.to_string(),
        document.to_string(),
    ] {
        assert!(!value.contains(external_path.as_ref()));
        assert!(!value.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    }
    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("read saved project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(external_path.as_ref()),
            "external Timeline save path leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "external Timeline save canary leaked in {path:?}"
        );
    }

    let failed = Fixture::new();
    let (_chain, base_result, _base) =
        failed.generated_timeline_base("timeline-save-receipt-failure-base");
    let key = "timeline-save-receipt-failure";
    let receipt_id = stable_timeline_save_receipt_id_for_test(&failed.project.project_id, key);
    let receipt_path = Path::new(&failed.project.project_path)
        .join("artifacts")
        .join("media-receipts")
        .join(format!("{receipt_id}.json"));
    fs::write(&receipt_path, b"{malformed Timeline save receipt")
        .expect("install malformed Timeline save receipt");
    let before_metadata = failed.metadata_count();
    let before_receipts = failed.receipt_count();
    let error = failed
        .media
        .save_timeline(failed.timeline_save_options(
            key,
            "run_timeline_save_receipt_failure",
            &base_result.artifact_id,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
            "Receipt failure cannot project success.",
        ))
        .expect_err("receipt failure must not return Timeline save success");
    assert!(matches!(
        error.code,
        MediaErrorCode::StorageUnavailable | MediaErrorCode::InvalidRequest
    ));
    assert_eq!(failed.metadata_count(), before_metadata);
    assert_eq!(failed.receipt_count(), before_receipts);
}

#[test]
fn scene_plan_save_persists_all_four_edits_sequentially_and_preserves_the_base() {
    let fixture = Fixture::new();
    let (caption_chain, base_result, base) =
        fixture.generated_scene_plan_base("scene-save-four-edits-base");
    fixture.prepare(
        "scene_plan",
        "run_scene_plan_media",
        vec![
            fixture.workflow_input("research", "run_research_media", "review_research_media"),
            fixture.workflow_input("script", "run_script_media", "review_script_media"),
            fixture.workflow_input(
                "captions",
                &caption_chain.captions_input.run_id,
                &caption_chain.captions_input.review_record_id,
            ),
        ],
    );
    fixture.record(
        "scene_plan",
        "run_scene_plan_media",
        vec![base_result.artifact_id.clone()],
    );
    fixture.approve(
        "scene_plan",
        "run_scene_plan_media",
        "review_scene_plan_before_edit",
    );
    let scene_metadata = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &base_result.artifact_id)
        .expect("read approved base Scene Plan")
        .artifact;
    let timeline_chain = ApprovedTimelineChain {
        audio_input: caption_chain.audio_input.clone(),
        captions_input: caption_chain.captions_input.clone(),
        scene_plan_input: FrozenArtifactInputData {
            stage_id: "scene_plan".to_owned(),
            run_id: "run_scene_plan_media".to_owned(),
            artifact_id: base_result.artifact_id.clone(),
            content_hash: scene_metadata["contentHash"]
                .as_str()
                .expect("base Scene Plan hash")
                .to_owned(),
            review_record_id: "review_scene_plan_before_edit".to_owned(),
            claim_ids: vec!["claim_audio_1".to_owned()],
            evidence_refs: vec!["evidence_audio_1".to_owned()],
        },
    };
    let timeline_result = fixture
        .media
        .generate_timeline(
            fixture.timeline_options("scene-save-downstream-timeline", &timeline_chain),
        )
        .expect("generate approved downstream Timeline");
    fixture.approve_timeline_and_downstream(
        &timeline_chain,
        "run_timeline_media",
        &timeline_result.artifact_id,
    );
    let base_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &base_result.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read immutable base Scene Plan");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("first scene id")
        .to_owned();
    let second_id = base["scenes"][1]["sceneId"]
        .as_str()
        .expect("second scene id")
        .to_owned();
    let first_cues = base["scenes"][0]["cueIds"]
        .as_array()
        .expect("first scene cues");
    let split_boundary = first_cues[1].as_str().expect("split cue").to_owned();
    let move_boundary = first_cues[2].as_str().expect("move cue").to_owned();
    let split_preview = apply_scene_plan_edits(
        &base,
        &[ScenePlanEditData::Split {
            scene_id: first_id.clone(),
            boundary_cue_id: split_boundary.clone(),
        }],
        "Preview deterministic split identity.",
        "run_scene_preview",
        "sceneplan_preview",
        "2026-07-18T08:00:00Z",
        &base_result.artifact_id,
    )
    .expect("preview split identity");
    let split_right_id = split_preview["scenes"][1]["sceneId"]
        .as_str()
        .expect("split right id")
        .to_owned();
    let edits = vec![
        ScenePlanEditData::Split {
            scene_id: first_id.clone(),
            boundary_cue_id: split_boundary,
        },
        ScenePlanEditData::Update {
            scene_id: split_right_id.clone(),
            title: Some("Sequentially updated split scene".to_owned()),
            narrative_role: Some("reviewed_caption_sequence".to_owned()),
        },
        ScenePlanEditData::MoveBoundary {
            left_scene_id: split_right_id.clone(),
            right_scene_id: second_id.clone(),
            boundary_cue_id: move_boundary,
        },
        ScenePlanEditData::Merge {
            first_scene_id: first_id.clone(),
            second_scene_id: split_right_id.clone(),
        },
    ];
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let summary = "Apply split, update, boundary move, and merge in sequence.";
    let result = fixture
        .media
        .save_scene_plan(fixture.scene_plan_save_options(
            "scene-save-four-edits",
            "run_scene_plan_saved",
            &base_result.artifact_id,
            edits,
            summary,
        ))
        .expect("save edited Scene Plan");

    assert_eq!(result.operation, "save_scene_plan");
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.run_id, "run_scene_plan_saved");
    assert!(result.stale_because_stage_ids.is_empty());
    assert!(!result.idempotent_replay);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
    assert_saved_stage_run(
        &fixture,
        "scene_plan",
        "run_scene_plan_saved",
        &base_result.artifact_id,
        &result.artifact_id,
    );
    assert_eq!(
        fixture.stage_status("scene_plan"),
        StageStatusData::NeedsReview
    );
    assert_eq!(fixture.stage_status("timeline"), StageStatusData::Approved);
    fixture.approve(
        "scene_plan",
        "run_scene_plan_saved",
        "review_scene_plan_saved",
    );
    assert_eq!(fixture.stage_status("timeline"), StageStatusData::Stale);
    assert_eq!(fixture.stage_status("render"), StageStatusData::Stale);
    assert_eq!(fixture.stage_status("export"), StageStatusData::Stale);
    let saved = read_artifact_json(&fixture, &result.artifact_id);
    validate_media_document(&saved).expect("saved Scene Plan media schema");
    parse_media_document(saved.clone()).expect("saved Scene Plan typed roundtrip");
    assert_eq!(saved["runId"], "run_scene_plan_saved");
    assert_eq!(saved["supersedesArtifactId"], base_result.artifact_id);
    assert_eq!(saved["changeSummary"]["summary"], summary);
    assert_eq!(saved["inputRefs"], base["inputRefs"]);
    assert_eq!(saved["configSnapshot"], base["configSnapshot"]);
    assert_eq!(saved["scenes"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        saved["scenes"][0]["cueIds"],
        json!([
            base["scenes"][0]["cueIds"][0].clone(),
            base["scenes"][0]["cueIds"][1].clone()
        ])
    );
    assert_eq!(
        saved["scenes"][1]["cueIds"],
        json!([
            base["scenes"][0]["cueIds"][2].clone(),
            base["scenes"][1]["cueIds"][0].clone()
        ])
    );
    let mut expected_changed = vec![first_id, split_right_id, second_id];
    expected_changed.sort();
    expected_changed.dedup();
    assert_eq!(result.changed_scene_ids, expected_changed);
    assert_eq!(
        saved["changeSummary"]["changedSceneIds"],
        json!(result.changed_scene_ids)
    );
    let metadata = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read saved Scene Plan metadata");
    let mut expected_sources = vec![base_result.artifact_id.clone()];
    expected_sources.extend(
        base["inputRefs"]
            .as_array()
            .expect("base inputs")
            .iter()
            .map(|input| {
                input["artifactId"]
                    .as_str()
                    .expect("input artifact")
                    .to_owned()
            }),
    );
    assert_eq!(
        metadata.artifact["source"]["sourceArtifactIds"],
        json!(expected_sources)
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.artifact_id)
            .expect("verify saved Scene Plan")
            .status,
        ArtifactVerificationStatusData::Verified
    );
    assert_eq!(
        fixture
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &base_result.artifact_id,
                16 * 1024 * 1024,
            )
            .expect("base remains readable"),
        base_bytes
    );
}

#[test]
fn scene_plan_save_rejects_illegal_edits_and_request_limits_atomically() {
    let fixture = Fixture::new();
    let chain = fixture.install_approved_caption_chain_with_srt(
        "scene-save-invalid-audio",
        "scene-save-invalid-captions",
        valid_seven_cue_scene_plan_srt(),
    );
    let base_result = fixture
        .media
        .generate_scene_plan(
            fixture.scene_plan_options("scene-save-invalid-base", &chain.captions_input),
        )
        .expect("generate three-scene base");
    let base = read_artifact_json(&fixture, &base_result.artifact_id);
    assert_eq!(base["scenes"].as_array().map(Vec::len), Some(3));
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("first scene")
        .to_owned();
    let second_id = base["scenes"][1]["sceneId"]
        .as_str()
        .expect("second scene")
        .to_owned();
    let third_id = base["scenes"][2]["sceneId"]
        .as_str()
        .expect("third scene")
        .to_owned();
    let first_cue = base["scenes"][0]["cueIds"][0]
        .as_str()
        .expect("first cue")
        .to_owned();
    let oversized = (0..1_001)
        .map(|index| ScenePlanEditData::Update {
            scene_id: first_id.clone(),
            title: Some(format!("Edit {index}")),
            narrative_role: None,
        })
        .collect::<Vec<_>>();
    let cases = vec![
        fixture.scene_plan_save_options(
            "scene-save-invalid-boundary",
            "run_scene_invalid_boundary",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Split {
                scene_id: first_id.clone(),
                boundary_cue_id: first_cue,
            }],
            "Reject an illegal split boundary.",
        ),
        fixture.scene_plan_save_options(
            "scene-save-nonadjacent-merge",
            "run_scene_nonadjacent_merge",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Merge {
                first_scene_id: first_id.clone(),
                second_scene_id: third_id,
            }],
            "Reject a non-adjacent merge.",
        ),
        fixture.scene_plan_save_options(
            "scene-save-nonadjacent-boundary",
            "run_scene_nonadjacent_boundary",
            &base_result.artifact_id,
            vec![ScenePlanEditData::MoveBoundary {
                left_scene_id: first_id.clone(),
                right_scene_id: second_id,
                boundary_cue_id: "cue_that_does_not_exist".to_owned(),
            }],
            "Reject an unknown boundary cue.",
        ),
        fixture.scene_plan_save_options(
            "scene-save-empty-edits",
            "run_scene_empty_edits",
            &base_result.artifact_id,
            Vec::new(),
            "Reject empty edits.",
        ),
        fixture.scene_plan_save_options(
            "scene-save-too-many-edits",
            "run_scene_too_many_edits",
            &base_result.artifact_id,
            oversized,
            "Reject too many edits.",
        ),
        fixture.scene_plan_save_options(
            "scene-save-control-summary",
            "run_scene_control_summary",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id.clone(),
                title: Some("Valid title".to_owned()),
                narrative_role: None,
            }],
            "unsafe\u{0000}summary",
        ),
        fixture.scene_plan_save_options(
            "scene-save-control-edit",
            "run_scene_control_edit",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("unsafe\u{0000}title".to_owned()),
                narrative_role: None,
            }],
            "Reject control characters in edits.",
        ),
    ];
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    for options in cases {
        let reservation_path = Path::new(&fixture.project.project_path)
            .join("runs/reservations")
            .join(format!("{}.json", options.run_id));
        let reservation_existed = reservation_path.exists();
        let error = fixture
            .media
            .save_scene_plan(options)
            .expect_err("invalid Scene Plan save must fail atomically");
        assert_eq!(error.code, MediaErrorCode::InvalidRequest);
        assert_eq!(fixture.metadata_count(), before_metadata);
        assert_eq!(fixture.receipt_count(), before_receipts);
        assert_eq!(
            reservation_path.exists(),
            reservation_existed,
            "invalid Scene Plan edit must not create a StageRun reservation"
        );
    }
}

#[test]
fn scene_plan_save_rejects_cross_project_arbitrary_identity_semantic_missing_and_tampered_bases() {
    let fixture = Fixture::new();
    let (_chain, _base_result, base) =
        fixture.generated_scene_plan_base("scene-save-invalid-base-closure");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("base first scene")
        .to_owned();
    let edit = || {
        vec![ScenePlanEditData::Update {
            scene_id: first_id.clone(),
            title: Some("Valid edit".to_owned()),
            narrative_role: None,
        }]
    };
    let source_ids = base["inputRefs"]
        .as_array()
        .expect("base input refs")
        .iter()
        .map(|input| input["artifactId"].as_str().expect("input id").to_owned())
        .collect::<Vec<_>>();

    let arbitrary_id = fixture.create_derived_scene_plan_artifact(
        "run_scene_arbitrary_base",
        br#"{"arbitrary":true}"#,
        &source_ids,
    );
    assert_scene_plan_save_error(
        &fixture,
        fixture.scene_plan_save_options(
            "scene-save-arbitrary-base",
            "run_scene_from_arbitrary",
            &arbitrary_id,
            edit(),
            "Reject arbitrary JSON pretending to be a Scene Plan.",
        ),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let foreign = Fixture::new();
    let mut cross_project = base.clone();
    cross_project["projectId"] = json!(foreign.project.project_id);
    cross_project["runId"] = json!("run_scene_cross_base");
    let cross_id = fixture.create_derived_scene_plan_artifact(
        "run_scene_cross_base",
        &serde_json::to_vec(&cross_project).expect("serialize cross-project Scene Plan"),
        &source_ids,
    );
    assert_scene_plan_save_error(
        &fixture,
        fixture.scene_plan_save_options(
            "scene-save-cross-project-base",
            "run_scene_from_cross_project",
            &cross_id,
            edit(),
            "Reject a cross-project Scene Plan base.",
        ),
        &[MediaErrorCode::CrossProjectReference],
    );

    let mut wrong_identity = base.clone();
    wrong_identity["runId"] = json!("run_scene_identity_document");
    let identity_id = fixture.create_derived_scene_plan_artifact(
        "run_scene_identity_metadata",
        &serde_json::to_vec(&wrong_identity).expect("serialize wrong identity Scene Plan"),
        &source_ids,
    );
    assert_scene_plan_save_error(
        &fixture,
        fixture.scene_plan_save_options(
            "scene-save-wrong-base-identity",
            "run_scene_from_wrong_identity",
            &identity_id,
            edit(),
            "Reject a run identity mismatch.",
        ),
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let mut invalid_semantics = base.clone();
    invalid_semantics["runId"] = json!("run_scene_semantic_base");
    let boundary = invalid_semantics["scenes"][0]["suggestedEndMs"]
        .as_u64()
        .expect("scene boundary");
    invalid_semantics["scenes"][1]["suggestedStartMs"] = json!(boundary - 1);
    let semantic_id = fixture.create_derived_scene_plan_artifact(
        "run_scene_semantic_base",
        &serde_json::to_vec(&invalid_semantics).expect("serialize invalid semantic Scene Plan"),
        &source_ids,
    );
    assert_scene_plan_save_error(
        &fixture,
        fixture.scene_plan_save_options(
            "scene-save-invalid-semantic-base",
            "run_scene_from_invalid_semantics",
            &semantic_id,
            edit(),
            "Reject invalid Scene Plan semantics.",
        ),
        &[MediaErrorCode::ContractViolation],
    );

    let missing = Fixture::new();
    let (_chain, base_result, base) = missing.generated_scene_plan_base("scene-save-missing-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("missing base first scene")
        .to_owned();
    corrupt_artifact_content(&missing, &base_result.artifact_id, false);
    assert_scene_plan_save_error(
        &missing,
        missing.scene_plan_save_options(
            "scene-save-missing-content",
            "run_scene_from_missing",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("Cannot save".to_owned()),
                narrative_role: None,
            }],
            "Reject missing base content.",
        ),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );

    let tampered = Fixture::new();
    let (_chain, base_result, base) =
        tampered.generated_scene_plan_base("scene-save-tampered-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("tampered base first scene")
        .to_owned();
    corrupt_artifact_content(&tampered, &base_result.artifact_id, true);
    assert_scene_plan_save_error(
        &tampered,
        tampered.scene_plan_save_options(
            "scene-save-tampered-content",
            "run_scene_from_tampered",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("Cannot save".to_owned()),
                narrative_role: None,
            }],
            "Reject tampered base content.",
        ),
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn scene_plan_save_idempotency_replays_conflicts_and_revalidates_the_saved_artifact() {
    let fixture = Fixture::new();
    let (chain, base_result, base) =
        fixture.generated_scene_plan_base("scene-save-idempotency-base");
    let second_base = fixture
        .media
        .generate_scene_plan(
            fixture.scene_plan_options("scene-save-idempotency-other-base", &chain.captions_input),
        )
        .expect("generate alternate immutable base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("first scene")
        .to_owned();
    let edits = vec![
        ScenePlanEditData::Update {
            scene_id: first_id.clone(),
            title: Some("Idempotent saved title".to_owned()),
            narrative_role: None,
        },
        ScenePlanEditData::Update {
            scene_id: first_id.clone(),
            title: None,
            narrative_role: Some("idempotent_reviewed_sequence".to_owned()),
        },
    ];
    let options = fixture.scene_plan_save_options(
        "scene-save-idempotent",
        "run_scene_save_idempotent",
        &base_result.artifact_id,
        edits.clone(),
        "Save exactly once and replay by immutable receipt.",
    );
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let first = fixture
        .media
        .save_scene_plan(options.clone())
        .expect("first Scene Plan save");
    let replay = fixture
        .media
        .save_scene_plan(options.clone())
        .expect("replay Scene Plan save");
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.changed_scene_ids, replay.changed_scene_ids);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    let mut changed_edit = options.clone();
    changed_edit.edits = vec![ScenePlanEditData::Update {
        scene_id: first_id,
        title: Some("Different title".to_owned()),
        narrative_role: None,
    }];
    let mut changed_summary = options.clone();
    changed_summary.change_summary = "Different semantic summary.".to_owned();
    let mut changed_order = options.clone();
    changed_order.edits.reverse();
    let mut changed_run = options.clone();
    changed_run.run_id = "run_scene_save_other".to_owned();
    let mut changed_base = options.clone();
    changed_base.base_artifact_id = second_base.artifact_id;
    for changed in [
        changed_edit,
        changed_summary,
        changed_order,
        changed_run,
        changed_base,
    ] {
        let error = fixture
            .media
            .save_scene_plan(changed)
            .expect_err("same key with changed save semantics must conflict");
        assert_eq!(error.code, MediaErrorCode::IdempotencyConflict);
    }
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    corrupt_artifact_content(&fixture, &first.artifact_id, true);
    let error = fixture
        .media
        .save_scene_plan(options)
        .expect_err("save replay must reverify persisted Artifact content");
    assert_eq!(error.code, MediaErrorCode::ArtifactVerificationFailed);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
}

#[test]
fn concurrent_scene_plan_saves_converge_and_different_keys_preserve_edit_history() {
    let concurrent = Fixture::new();
    let (_chain, base_result, base) =
        concurrent.generated_scene_plan_base("scene-save-concurrent-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("concurrent first scene")
        .to_owned();
    let options = concurrent.scene_plan_save_options(
        "scene-save-concurrent",
        "run_scene_save_concurrent",
        &base_result.artifact_id,
        vec![ScenePlanEditData::Update {
            scene_id: first_id,
            title: Some("Concurrent saved title".to_owned()),
            narrative_role: None,
        }],
        "Concurrent requests converge on one saved Artifact.",
    );
    let before_metadata = concurrent.metadata_count();
    let before_receipts = concurrent.receipt_count();
    let service = concurrent.media.clone();
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service
                    .save_scene_plan(options)
                    .expect("concurrent Scene Plan save")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join concurrent Scene Plan save"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_eq!(results[0].changed_scene_ids, results[1].changed_scene_ids);
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(concurrent.metadata_count(), before_metadata + 1);
    assert_eq!(concurrent.receipt_count(), before_receipts + 1);

    let history = Fixture::new();
    let (_chain, base_result, base) = history.generated_scene_plan_base("scene-save-history-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("history first scene")
        .to_owned();
    let edits = vec![ScenePlanEditData::Update {
        scene_id: first_id,
        title: Some("Immutable history title".to_owned()),
        narrative_role: None,
    }];
    let before_metadata = history.metadata_count();
    let before_receipts = history.receipt_count();
    let first = history
        .media
        .save_scene_plan(history.scene_plan_save_options(
            "scene-save-history-first",
            "run_scene_save_history_first",
            &base_result.artifact_id,
            edits.clone(),
            "Preserve every immutable edit history entry.",
        ))
        .expect("first Scene Plan edit history");
    let first_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &first.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read first Scene Plan edit history");
    let second = history
        .media
        .save_scene_plan(history.scene_plan_save_options(
            "scene-save-history-second",
            "run_scene_save_history_second",
            &base_result.artifact_id,
            edits,
            "Preserve every immutable edit history entry.",
        ))
        .expect("second Scene Plan edit history");
    let second_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &second.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read second Scene Plan edit history");
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_eq!(first.changed_scene_ids, second.changed_scene_ids);
    assert_ne!(first_bytes, second_bytes);
    assert_eq!(history.metadata_count(), before_metadata + 2);
    assert_eq!(history.receipt_count(), before_receipts + 2);
    assert_eq!(
        history
            .storage
            .read_artifact_content_bounded(
                &history.project.project_path,
                &history.project.project_id,
                &first.artifact_id,
                16 * 1024 * 1024,
            )
            .expect("first edit history remains readable"),
        first_bytes
    );
}

#[test]
fn scene_plan_save_redacts_paths_and_receipt_failure_returns_no_success_projection() {
    let fixture = Fixture::new();
    let (_chain, base_result, base) = fixture.generated_scene_plan_base("scene-save-path-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("path first scene")
        .to_owned();
    let result = fixture
        .media
        .save_scene_plan(fixture.scene_plan_save_options(
            "scene-save-path-redaction",
            "run_scene_save_path",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("Pathless saved title".to_owned()),
                narrative_role: None,
            }],
            "Save without persisting any external path.",
        ))
        .expect("save pathless Scene Plan");
    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read pathless saved Scene Plan metadata");
    let document = read_artifact_json(&fixture, &result.artifact_id);
    let external_path = fixture.external_dir.to_string_lossy();
    for value in [
        serde_json::to_string(&result).expect("serialize Scene Plan save result"),
        read.artifact.to_string(),
        document.to_string(),
    ] {
        assert!(!value.contains(external_path.as_ref()));
        assert!(!value.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    }
    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("read saved project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(external_path.as_ref()),
            "external Scene Plan save path leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "external Scene Plan save canary leaked in {path:?}"
        );
    }

    let failed = Fixture::new();
    let (_chain, base_result, base) =
        failed.generated_scene_plan_base("scene-save-receipt-failure-base");
    let first_id = base["scenes"][0]["sceneId"]
        .as_str()
        .expect("receipt failure first scene")
        .to_owned();
    let key = "scene-save-receipt-failure";
    let receipt_id = stable_scene_plan_save_receipt_id_for_test(&failed.project.project_id, key);
    let receipt_path = Path::new(&failed.project.project_path)
        .join("artifacts")
        .join("media-receipts")
        .join(format!("{receipt_id}.json"));
    fs::write(&receipt_path, b"{malformed save receipt").expect("install malformed save receipt");
    let before_metadata = failed.metadata_count();
    let before_receipts = failed.receipt_count();
    let error = failed
        .media
        .save_scene_plan(failed.scene_plan_save_options(
            key,
            "run_scene_save_receipt_failure",
            &base_result.artifact_id,
            vec![ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("Must not persist".to_owned()),
                narrative_role: None,
            }],
            "Receipt failure cannot project success.",
        ))
        .expect_err("receipt failure must not return save success");
    assert!(matches!(
        error.code,
        MediaErrorCode::StorageUnavailable | MediaErrorCode::InvalidRequest
    ));
    assert_eq!(failed.metadata_count(), before_metadata);
    assert_eq!(failed.receipt_count(), before_receipts);
}

#[test]
fn scene_plan_rejects_invalid_research_script_and_captions_approval_closures() {
    // Research: unapproved, stale, replaced, old review, cross-project, missing, tampered.
    let unapproved = Fixture::new();
    let chain = unapproved.install_approved_caption_chain(
        "scene-research-unapproved-audio",
        "scene-research-unapproved-captions",
    );
    let research = unapproved.research_candidate(
        "run_research_unapproved_scene",
        "review_research_unapproved_scene",
        false,
    );
    assert_scene_plan_inputs_rejected(
        &unapproved,
        research,
        unapproved.script_input.clone(),
        chain.captions_input,
        "scene-research-unapproved",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    let stale = Fixture::new();
    let chain = stale.install_approved_caption_chain(
        "scene-research-stale-audio",
        "scene-research-stale-captions",
    );
    stale.record_and_approve(
        "brief",
        "run_brief_new_scene",
        "review_brief_new_scene",
        Vec::new(),
    );
    assert_scene_plan_inputs_rejected(
        &stale,
        stale.research_input(),
        stale.script_input.clone(),
        chain.captions_input,
        "scene-research-stale",
        &[MediaErrorCode::InputNotApproved],
    );

    let replaced = Fixture::new();
    let chain = replaced.install_approved_caption_chain(
        "scene-research-replaced-audio",
        "scene-research-replaced-captions",
    );
    replaced.research_candidate(
        "run_research_replacement_scene",
        "review_research_replacement_scene",
        true,
    );
    assert_scene_plan_inputs_rejected(
        &replaced,
        replaced.research_input(),
        replaced.script_input.clone(),
        chain.captions_input,
        "scene-research-replaced",
        &[MediaErrorCode::InputNotApproved],
    );

    let old_review = Fixture::new();
    let chain = old_review.install_approved_caption_chain(
        "scene-research-review-audio",
        "scene-research-review-captions",
    );
    let mut research = old_review.research_input();
    research.review_record_id = "review_research_outdated_scene".to_owned();
    assert_scene_plan_inputs_rejected(
        &old_review,
        research,
        old_review.script_input.clone(),
        chain.captions_input,
        "scene-research-old-review",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let local = Fixture::new();
    let chain = local.install_approved_caption_chain(
        "scene-research-cross-audio",
        "scene-research-cross-captions",
    );
    let foreign = Fixture::new();
    assert_scene_plan_inputs_rejected(
        &local,
        foreign.research_input(),
        local.script_input.clone(),
        chain.captions_input,
        "scene-research-cross-project",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    for (suffix, tamper, expected) in [
        ("missing", false, MediaErrorCode::InputReferenceMismatch),
        ("tampered", true, MediaErrorCode::ArtifactVerificationFailed),
    ] {
        let fixture = Fixture::new();
        let chain = fixture.install_approved_caption_chain(
            &format!("scene-research-{suffix}-audio"),
            &format!("scene-research-{suffix}-captions"),
        );
        corrupt_artifact_content(&fixture, &fixture.research_input().artifact_id, tamper);
        assert_scene_plan_inputs_rejected(
            &fixture,
            fixture.research_input(),
            fixture.script_input.clone(),
            chain.captions_input,
            &format!("scene-research-{suffix}"),
            &[expected],
        );
    }

    // Script: unapproved, stale, replaced, old review, cross-project, missing, tampered.
    let unapproved = Fixture::new();
    let chain = unapproved.install_approved_caption_chain(
        "scene-script-unapproved-audio",
        "scene-script-unapproved-captions",
    );
    let script = unapproved.script_candidate(
        "run_script_unapproved_scene",
        "review_script_unapproved_scene",
        None,
    );
    assert_scene_plan_inputs_rejected(
        &unapproved,
        unapproved.research_input(),
        script,
        chain.captions_input,
        "scene-script-unapproved",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    let stale = Fixture::new();
    let chain = stale
        .install_approved_caption_chain("scene-script-stale-audio", "scene-script-stale-captions");
    let research = stale.research_candidate(
        "run_research_after_script_scene",
        "review_research_after_script_scene",
        true,
    );
    assert_scene_plan_inputs_rejected(
        &stale,
        research,
        stale.script_input.clone(),
        chain.captions_input,
        "scene-script-stale",
        &[MediaErrorCode::InputNotApproved],
    );

    let replaced = Fixture::new();
    let chain = replaced.install_approved_caption_chain(
        "scene-script-replaced-audio",
        "scene-script-replaced-captions",
    );
    replaced.script_candidate(
        "run_script_replacement_scene",
        "review_script_replacement_scene",
        Some(ReviewDecisionData::Approved),
    );
    assert_scene_plan_inputs_rejected(
        &replaced,
        replaced.research_input(),
        replaced.script_input.clone(),
        chain.captions_input,
        "scene-script-replaced",
        &[MediaErrorCode::InputNotApproved],
    );

    let old_review = Fixture::new();
    let chain = old_review.install_approved_caption_chain(
        "scene-script-review-audio",
        "scene-script-review-captions",
    );
    let mut script = old_review.script_input.clone();
    script.review_record_id = "review_script_outdated_scene".to_owned();
    assert_scene_plan_inputs_rejected(
        &old_review,
        old_review.research_input(),
        script,
        chain.captions_input,
        "scene-script-old-review",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let local = Fixture::new();
    let chain = local
        .install_approved_caption_chain("scene-script-cross-audio", "scene-script-cross-captions");
    let foreign = Fixture::new();
    assert_scene_plan_inputs_rejected(
        &local,
        local.research_input(),
        foreign.script_input,
        chain.captions_input,
        "scene-script-cross-project",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    for (suffix, tamper, expected) in [
        ("missing", false, MediaErrorCode::InputReferenceMismatch),
        ("tampered", true, MediaErrorCode::ArtifactVerificationFailed),
    ] {
        let fixture = Fixture::new();
        let chain = fixture.install_approved_caption_chain(
            &format!("scene-script-{suffix}-audio"),
            &format!("scene-script-{suffix}-captions"),
        );
        corrupt_artifact_content(&fixture, &fixture.script_input.artifact_id, tamper);
        assert_scene_plan_inputs_rejected(
            &fixture,
            fixture.research_input(),
            fixture.script_input.clone(),
            chain.captions_input,
            &format!("scene-script-{suffix}"),
            &[expected],
        );
    }

    // Captions: unapproved, stale, replaced, old review, cross-project, missing, tampered.
    let unapproved = Fixture::new();
    let chain = unapproved.install_approved_caption_chain(
        "scene-captions-unapproved-audio",
        "scene-captions-unapproved-base",
    );
    let captions = unapproved.captions_candidate(
        "run_captions_unapproved_scene",
        "review_captions_unapproved_scene",
        "scene-captions-unapproved-candidate",
        &chain.audio_input,
        false,
    );
    assert_scene_plan_inputs_rejected(
        &unapproved,
        unapproved.research_input(),
        unapproved.script_input.clone(),
        captions,
        "scene-captions-unapproved",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    let stale = Fixture::new();
    let chain = stale
        .install_approved_caption_chain("scene-captions-stale-audio", "scene-captions-stale-base");
    stale.audio_candidate(
        "run_audio_after_captions_scene",
        "review_audio_after_captions_scene",
        "scene-captions-stale-replacement-audio",
        true,
    );
    assert_scene_plan_inputs_rejected(
        &stale,
        stale.research_input(),
        stale.script_input.clone(),
        chain.captions_input,
        "scene-captions-stale",
        &[MediaErrorCode::InputNotApproved],
    );

    let replaced = Fixture::new();
    let chain = replaced.install_approved_caption_chain(
        "scene-captions-replaced-audio",
        "scene-captions-replaced-base",
    );
    replaced.captions_candidate(
        "run_captions_replacement_scene",
        "review_captions_replacement_scene",
        "scene-captions-replacement-candidate",
        &chain.audio_input,
        true,
    );
    assert_scene_plan_inputs_rejected(
        &replaced,
        replaced.research_input(),
        replaced.script_input.clone(),
        chain.captions_input,
        "scene-captions-replaced",
        &[MediaErrorCode::InputNotApproved],
    );

    let old_review = Fixture::new();
    let chain = old_review.install_approved_caption_chain(
        "scene-captions-review-audio",
        "scene-captions-review-base",
    );
    let mut captions = chain.captions_input;
    captions.review_record_id = "review_captions_outdated_scene".to_owned();
    assert_scene_plan_inputs_rejected(
        &old_review,
        old_review.research_input(),
        old_review.script_input.clone(),
        captions,
        "scene-captions-old-review",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let local = Fixture::new();
    local.install_approved_caption_chain(
        "scene-captions-cross-local-audio",
        "scene-captions-cross-local-base",
    );
    let foreign = Fixture::new();
    let foreign_chain = foreign.install_approved_caption_chain(
        "scene-captions-cross-foreign-audio",
        "scene-captions-cross-foreign-base",
    );
    assert_scene_plan_inputs_rejected(
        &local,
        local.research_input(),
        local.script_input.clone(),
        foreign_chain.captions_input,
        "scene-captions-cross-project",
        &[
            MediaErrorCode::InputNotApproved,
            MediaErrorCode::InputReferenceMismatch,
        ],
    );

    for (suffix, tamper, expected) in [
        ("missing", false, MediaErrorCode::InputReferenceMismatch),
        ("tampered", true, MediaErrorCode::ArtifactVerificationFailed),
    ] {
        let fixture = Fixture::new();
        let chain = fixture.install_approved_caption_chain(
            &format!("scene-captions-{suffix}-audio"),
            &format!("scene-captions-{suffix}-base"),
        );
        corrupt_artifact_content(&fixture, &chain.captions_input.artifact_id, tamper);
        assert_scene_plan_inputs_rejected(
            &fixture,
            fixture.research_input(),
            fixture.script_input.clone(),
            chain.captions_input,
            &format!("scene-captions-{suffix}"),
            &[expected],
        );
    }
}

#[test]
fn scene_plan_rejects_arbitrary_identity_mismatched_and_corrupt_captions_or_audio_documents() {
    let arbitrary_captions = Fixture::new();
    let audio = arbitrary_captions.install_approved_audio("scene-arbitrary-captions-audio");
    let captions = arbitrary_captions.approved_captions_payload(
        "run_captions_arbitrary_scene",
        "review_captions_arbitrary_scene",
        &audio,
        br#"{"arbitrary":true}"#,
    );
    assert_scene_plan_inputs_rejected(
        &arbitrary_captions,
        arbitrary_captions.research_input(),
        arbitrary_captions.script_input.clone(),
        captions,
        "scene-arbitrary-captions",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let wrong_captions_identity = Fixture::new();
    let chain = wrong_captions_identity.install_approved_caption_chain(
        "scene-wrong-captions-identity-audio",
        "scene-wrong-captions-identity-base",
    );
    let mut document =
        read_artifact_json(&wrong_captions_identity, &chain.captions_input.artifact_id);
    document["runId"] = json!("run_captions_wrong_document_scene");
    let captions = wrong_captions_identity.approved_captions_payload(
        "run_captions_identity_scene",
        "review_captions_identity_scene",
        &chain.audio_input,
        &serde_json::to_vec(&document).expect("serialize wrong Captions identity"),
    );
    assert_scene_plan_inputs_rejected(
        &wrong_captions_identity,
        wrong_captions_identity.research_input(),
        wrong_captions_identity.script_input.clone(),
        captions,
        "scene-wrong-captions-identity",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    for suffix in ["arbitrary", "wrong-identity"] {
        let fixture = Fixture::new();
        let base = fixture.install_approved_caption_chain(
            &format!("scene-{suffix}-audio-base"),
            &format!("scene-{suffix}-captions-base"),
        );
        let payload = if suffix == "wrong-identity" {
            let mut document = valid_audio_document(&fixture);
            document["runId"] = json!("run_audio_wrong_document_scene");
            serde_json::to_vec(&document).expect("serialize local wrong Audio identity")
        } else {
            br#"{"arbitrary":true}"#.to_vec()
        };
        let invalid_audio = fixture.approved_audio_payload(&payload);
        let mut captions_document = read_artifact_json(&fixture, &base.captions_input.artifact_id);
        captions_document["runId"] = json!(format!("run_captions_{suffix}_audio_scene"));
        let audio_ref = frozen_input_json(&fixture, &invalid_audio);
        captions_document["audioInput"] = audio_ref.clone();
        captions_document["inputRefs"][1] = audio_ref;
        let captions = fixture.approved_captions_payload(
            &format!("run_captions_{suffix}_audio_scene"),
            &format!("review_captions_{suffix}_audio_scene"),
            &invalid_audio,
            &serde_json::to_vec(&captions_document).expect("serialize custom Captions document"),
        );
        assert_scene_plan_inputs_rejected(
            &fixture,
            fixture.research_input(),
            fixture.script_input.clone(),
            captions,
            &format!("scene-{suffix}-audio-document"),
            &[MediaErrorCode::InputReferenceMismatch],
        );
    }

    let corrupt_audio = Fixture::new();
    let chain = corrupt_audio
        .install_approved_caption_chain("scene-corrupt-audio-base", "scene-corrupt-audio-captions");
    corrupt_artifact_content(&corrupt_audio, &chain.audio_input.artifact_id, true);
    assert_scene_plan_inputs_rejected(
        &corrupt_audio,
        corrupt_audio.research_input(),
        corrupt_audio.script_input.clone(),
        chain.captions_input,
        "scene-corrupt-audio-document",
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn scene_plan_idempotency_replays_conflicts_and_revalidates_the_immutable_artifact() {
    let fixture = Fixture::new();
    let chain = fixture
        .install_approved_caption_chain("scene-idempotency-audio", "scene-idempotency-captions");
    let options = fixture.scene_plan_options("scene-plan-idempotent", &chain.captions_input);
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let first = fixture
        .media
        .generate_scene_plan(options.clone())
        .expect("first Scene Plan generation");
    let replay = fixture
        .media
        .generate_scene_plan(options.clone())
        .expect("Scene Plan replay");
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.changed_scene_ids, replay.changed_scene_ids);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    let mut changed_review = options.clone();
    changed_review.research_input.review_record_id = "review_research_other_scene".to_owned();
    let mut changed_run = options.clone();
    changed_run.run_id = "run_scene_plan_other".to_owned();
    let mut changed_hash = options.clone();
    changed_hash.captions_input.content_hash = format!("sha256:{}", "c".repeat(64));
    for changed in [changed_review, changed_run, changed_hash] {
        let error = fixture
            .media
            .generate_scene_plan(changed)
            .expect_err("changed semantics on one key must conflict");
        assert_eq!(error.code, MediaErrorCode::IdempotencyConflict);
    }
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);

    corrupt_artifact_content(&fixture, &first.artifact_id, true);
    let error = fixture
        .media
        .generate_scene_plan(options)
        .expect_err("replay must reverify the immutable Scene Plan Artifact");
    assert_eq!(error.code, MediaErrorCode::ArtifactVerificationFailed);
    assert_eq!(fixture.metadata_count(), before_metadata + 1);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
}

#[test]
fn concurrent_scene_plan_generation_converges_and_different_keys_preserve_history() {
    let concurrent = Fixture::new();
    let chain = concurrent
        .install_approved_caption_chain("scene-concurrent-audio", "scene-concurrent-captions");
    let before_metadata = concurrent.metadata_count();
    let before_receipts = concurrent.receipt_count();
    let service = concurrent.media.clone();
    let options = concurrent.scene_plan_options("scene-plan-concurrent", &chain.captions_input);
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service
                    .generate_scene_plan(options)
                    .expect("concurrent Scene Plan generation")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join Scene Plan generation"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_eq!(results[0].changed_scene_ids, results[1].changed_scene_ids);
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(concurrent.metadata_count(), before_metadata + 1);
    assert_eq!(concurrent.receipt_count(), before_receipts + 1);

    let history = Fixture::new();
    let chain =
        history.install_approved_caption_chain("scene-history-audio", "scene-history-captions");
    let before_metadata = history.metadata_count();
    let before_receipts = history.receipt_count();
    let first = history
        .media
        .generate_scene_plan(
            history.scene_plan_options("scene-plan-history-first", &chain.captions_input),
        )
        .expect("first Scene Plan history entry");
    let first_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &first.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read first Scene Plan history entry");
    let second = history
        .media
        .generate_scene_plan(
            history.scene_plan_options("scene-plan-history-second", &chain.captions_input),
        )
        .expect("second Scene Plan history entry");
    let second_bytes = history
        .storage
        .read_artifact_content_bounded(
            &history.project.project_path,
            &history.project.project_id,
            &second.artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read second Scene Plan history entry");
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_eq!(first.changed_scene_ids, second.changed_scene_ids);
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(history.metadata_count(), before_metadata + 2);
    assert_eq!(history.receipt_count(), before_receipts + 2);
    assert_eq!(
        history
            .storage
            .verify_artifact(&history.project.project_path, &first.artifact_id)
            .expect("verify first immutable Scene Plan")
            .status,
        ArtifactVerificationStatusData::Verified
    );
}

#[test]
fn scene_plan_redacts_external_paths_and_receipt_failure_returns_no_success_projection() {
    let fixture = Fixture::new();
    let chain = fixture.install_approved_caption_chain("scene-path-audio", "scene-path-captions");
    let result = fixture
        .media
        .generate_scene_plan(
            fixture.scene_plan_options("scene-plan-path-redaction", &chain.captions_input),
        )
        .expect("generate pathless Scene Plan");
    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read pathless Scene Plan metadata");
    let document = read_artifact_json(&fixture, &result.artifact_id);
    let external_path = fixture.external_dir.to_string_lossy();
    for value in [
        serde_json::to_string(&result).expect("serialize Scene Plan result"),
        read.artifact.to_string(),
        document.to_string(),
    ] {
        assert!(!value.contains(external_path.as_ref()));
        assert!(!value.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    }
    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("read project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(external_path.as_ref()),
            "external source path leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "external path canary leaked in {path:?}"
        );
    }

    let failed = Fixture::new();
    let chain = failed.install_approved_caption_chain(
        "scene-receipt-failure-audio",
        "scene-receipt-failure-captions",
    );
    let key = "scene-plan-receipt-failure";
    let receipt_id = stable_scene_plan_receipt_id_for_test(&failed.project.project_id, key);
    let receipt_path = Path::new(&failed.project.project_path)
        .join("artifacts")
        .join("media-receipts")
        .join(format!("{receipt_id}.json"));
    fs::write(&receipt_path, b"{malformed receipt")
        .expect("install malformed receipt failure fixture");
    let before_metadata = failed.metadata_count();
    let before_receipts = failed.receipt_count();
    let error = failed
        .media
        .generate_scene_plan(failed.scene_plan_options(key, &chain.captions_input))
        .expect_err("receipt read failure must not project success");
    assert!(matches!(
        error.code,
        MediaErrorCode::StorageUnavailable | MediaErrorCode::InvalidRequest
    ));
    assert_eq!(failed.metadata_count(), before_metadata);
    assert_eq!(failed.receipt_count(), before_receipts);
    let has_scene_plan_artifact = fs::read_dir(
        Path::new(&failed.project.project_path)
            .join("artifacts")
            .join("metadata"),
    )
    .expect("read Artifact metadata directory")
    .filter_map(Result::ok)
    .map(|entry| read_json(entry.path()))
    .any(|artifact| artifact["stageId"] == "scene_plan");
    assert!(!has_scene_plan_artifact);
}

#[test]
fn audio_import_persists_a_schema_valid_traceable_document() {
    let fixture = Fixture::new();
    let result = fixture
        .media
        .import_audio(fixture.audio_options("audio-happy"))
        .expect("import Audio");

    validate_media_document(&result.document).expect("media schema");
    parse_media_document(result.document.clone()).expect("typed media roundtrip");
    assert!(!result.idempotent_replay);
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.document["documentType"], "audio_media");
    assert_eq!(
        result.document["source"]["sourceFileName"],
        "private-narration.wav"
    );
    assert_eq!(result.document["durationMs"], 100);
    assert_eq!(result.document["sampleRateHz"], 16_000);
    assert_eq!(result.document["bitsPerSample"], 16);
    assert_eq!(result.document["channels"], 1);
    assert_eq!(
        result.document["rights"]["licenseId"],
        "fixture-owned-audio"
    );
    assert_eq!(
        result.document["configSnapshot"],
        json!({"normalize":false})
    );
    assert_eq!(
        result.document["inputRefs"][0]["claimIds"],
        json!(["claim_audio_1"])
    );
    assert_eq!(
        result.document["inputRefs"][0]["evidenceRefs"],
        json!(["evidence_audio_1"])
    );
    let raw = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.raw_artifact_id)
        .expect("read raw");
    assert_eq!(result.document["artifactUri"], raw.content_uri);
    assert_eq!(
        result.document["source"]["sourceContentHash"],
        raw.artifact["contentHash"]
    );
    assert!(raw.artifact["source"]["sourceUri"]
        .as_str()
        .expect("internal URI")
        .starts_with("narracut:sha256/"));
    assert!(raw.artifact["source"]["sourceUri"]
        .as_str()
        .expect("internal URI")
        .ends_with("private-narration.wav"));
    assert_eq!(raw.artifact["source"]["author"], "Fixture Author");
    assert_eq!(raw.artifact["source"]["license"], "fixture-owned-audio");
    assert_eq!(
        raw.artifact["source"]["authorizationRecordIds"],
        json!(["fixture-owned-audio"])
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.raw_artifact_id)
            .expect("verify raw")
            .status,
        ArtifactVerificationStatusData::Verified
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &result.artifact_id)
            .expect("verify document")
            .status,
        ArtifactVerificationStatusData::Verified
    );
    let document_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &result.artifact_id,
            4 * 1024 * 1024,
        )
        .expect("read document content");
    assert_eq!(
        serde_json::from_slice::<Value>(&document_bytes).expect("document JSON"),
        result.document
    );
}

#[test]
fn captions_import_persists_a_schema_valid_traceable_document() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-happy-audio");
    fixture.prepare_captions();
    let options = fixture.captions_options(
        "captions-happy",
        &audio_input,
        b"1\n00:00:00,000 --> 00:00:00,050\nHello world!\n\n2\n00:00:00,050 --> 00:00:00,100\nTraceable captions.\n",
    );
    let result = fixture
        .media
        .import_captions(options)
        .expect("import Captions");

    validate_media_document(&result.document).expect("media schema");
    parse_media_document(result.document.clone()).expect("typed media roundtrip");
    assert!(!result.idempotent_replay);
    assert_eq!(result.owner_project_id, fixture.project.project_id);
    assert_eq!(result.document["documentType"], "captions_media");
    assert_eq!(
        result.document["source"]["sourceFileName"],
        "private-captions.srt"
    );
    assert_eq!(result.document["rawArtifactId"], result.raw_artifact_id);
    assert_eq!(
        result.document["audioInput"]["artifactId"],
        audio_input.artifact_id
    );
    assert_eq!(
        result.document["audioInput"],
        result.document["inputRefs"][1]
    );
    assert_eq!(
        result.document["inputRefs"][0]["artifactId"],
        fixture.script_input.artifact_id
    );
    assert_eq!(
        result.document["inputRefs"][0]["claimIds"],
        json!(["claim_audio_1"])
    );
    assert_eq!(
        result.document["inputRefs"][1]["evidenceRefs"],
        json!(["evidence_audio_1"])
    );
    assert_eq!(
        result.document["inputRefs"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(result.document["cues"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        result.document["cues"][0]["claimIds"],
        json!(["claim_audio_1"])
    );
    assert_eq!(
        result.document["cues"][0]["evidenceRefs"],
        json!(["evidence_audio_1"])
    );
    assert!(result.document["mappings"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(result.document["diagnostics"][0]["blocking"], false);
    assert_eq!(
        result.document["configSnapshot"],
        json!({"language":"zh-CN","interpolation":"deterministic-v1"})
    );

    for artifact_id in [&result.raw_artifact_id, &result.artifact_id] {
        assert_eq!(
            fixture
                .storage
                .verify_artifact(&fixture.project.project_path, artifact_id)
                .expect("verify Captions artifact")
                .status,
            ArtifactVerificationStatusData::Verified
        );
    }
    let raw = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.raw_artifact_id)
        .expect("read Captions raw");
    let derived = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &result.artifact_id)
        .expect("read Captions document metadata");
    assert_eq!(raw.artifact["kind"], "captions_source");
    assert_eq!(raw.artifact["mediaType"], "application/x-subrip");
    assert_eq!(raw.artifact["source"]["author"], "Fixture Captioner");
    assert_eq!(raw.artifact["source"]["license"], "fixture-owned-captions");
    assert!(raw.artifact["source"]["sourceUri"]
        .as_str()
        .expect("internal SRT URI")
        .starts_with("narracut:sha256/"));
    assert_eq!(
        result.document["rawContentHash"],
        raw.artifact["contentHash"]
    );
    assert_eq!(
        result.document["source"]["sourceContentHash"],
        raw.artifact["contentHash"]
    );
    assert_eq!(derived.artifact["kind"], "captions");
    assert_eq!(derived.artifact["contentHash"], result.content_hash);
    assert_eq!(
        derived.artifact["source"]["sourceArtifactIds"],
        json!([
            result.raw_artifact_id,
            fixture.script_input.artifact_id,
            audio_input.artifact_id
        ])
    );
    let document_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &result.artifact_id,
            4 * 1024 * 1024,
        )
        .expect("read Captions document");
    assert_eq!(
        serde_json::from_slice::<Value>(&document_bytes).expect("Captions JSON"),
        result.document
    );
}

#[test]
fn captions_mapping_is_normalization_stable_and_exactly_partitions_each_cue() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-mapping-audio");
    fixture.prepare_captions();
    let canonical = "1\n00:00:00,000 --> 00:00:00,050\n你好，OpenAI world! 第二句？\n\n2\n00:00:00,050 --> 00:00:00,100\nLine one. Line two!\n";
    let first = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-mapping-lf",
            &audio_input,
            canonical.as_bytes(),
        ))
        .expect("import LF SRT");
    let crlf_bom = format!("\u{feff}{}", canonical.replace('\n', "\r\n"));
    let second = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-mapping-crlf-bom",
            &audio_input,
            crlf_bom.as_bytes(),
        ))
        .expect("import BOM/CRLF SRT");

    assert_eq!(first.document["cues"], second.document["cues"]);
    assert_eq!(first.document["mappings"], second.document["mappings"]);
    assert_ne!(
        first.document["source"]["sourceContentHash"],
        second.document["source"]["sourceContentHash"]
    );
    let cues = first.document["cues"].as_array().expect("cues");
    let mappings = first.document["mappings"].as_array().expect("mappings");
    assert!(mappings.iter().any(|item| item["level"] == "sentence"));
    assert!(mappings.iter().any(|item| item["level"] == "word"));
    for cue in cues {
        let cue_id = cue["cueId"].as_str().expect("cue id");
        let cue_start = cue["startMs"].as_u64().expect("cue start");
        let cue_end = cue["endMs"].as_u64().expect("cue end");
        for (level, precision, basis) in [
            ("cue", "cue_exact", "srt_cue"),
            ("sentence", "estimated", "sentence_interpolation"),
            ("word", "estimated", "word_interpolation"),
        ] {
            let level_mappings = mappings
                .iter()
                .filter(|item| item["sourceCueId"] == cue_id && item["level"] == level)
                .collect::<Vec<_>>();
            assert!(!level_mappings.is_empty(), "missing {level} mapping");
            assert_eq!(level_mappings[0]["startMs"], cue_start);
            assert_eq!(
                level_mappings.last().expect("last mapping")["endMs"],
                cue_end
            );
            for (index, mapping) in level_mappings.iter().enumerate() {
                let start = mapping["startMs"].as_u64().expect("mapping start");
                let end = mapping["endMs"].as_u64().expect("mapping end");
                assert!(cue_start <= start && start < end && end <= cue_end);
                assert_eq!(mapping["timingPrecision"], precision);
                assert_eq!(mapping["timingBasis"], basis);
                if let Some(previous) = index.checked_sub(1).map(|i| level_mappings[i]) {
                    assert_eq!(previous["endMs"], mapping["startMs"]);
                }
            }
            if level == "cue" {
                assert_eq!(level_mappings.len(), 1);
                assert_eq!(level_mappings[0]["text"], cue["text"]);
            }
        }
    }
}

#[test]
fn captions_import_maps_each_cue_to_exact_reviewed_script_pairs() {
    let provenance = json!([
        {"claimId":"claim_1","evidenceRef":"evidence_1"},
        {"claimId":"claim_1","evidenceRef":"evidence_2"},
        {"claimId":"claim_2","evidenceRef":"evidence_1"}
    ]);
    let fixture = Fixture::new_with_initial_script(Some((
        json!({
            "schemaVersion": "narracut.script/v1",
            "title": "Pair mapping",
            "language": "en",
            "summary": "Pair mapping fixture.",
            "estimatedDurationSeconds": 1,
            "segments": [
                {
                    "segmentId": "segment_alpha",
                    "order": 0,
                    "title": "Alpha",
                    "narration": "Alpha cue.",
                    "provenance": [
                        {"claimId":"claim_1","evidenceRef":"evidence_1"},
                        {"claimId":"claim_1","evidenceRef":"evidence_2"}
                    ]
                },
                {
                    "segmentId": "segment_beta",
                    "order": 1,
                    "title": "Beta",
                    "narration": "Beta cue.",
                    "provenance": [
                        {"claimId":"claim_2","evidenceRef":"evidence_1"}
                    ]
                }
            ]
        }),
        provenance.clone(),
    )));
    let audio_input = fixture.install_approved_audio("captions-pair-audio");
    fixture.prepare_captions();
    let srt = b"1\n00:00:00,000 --> 00:00:00,050\nAlpha cue.\n\n2\n00:00:00,050 --> 00:00:00,100\nBeta cue.\n";
    let result = fixture
        .media
        .import_captions(fixture.captions_options("captions-pair-map", &audio_input, srt))
        .expect("import pair-mapped captions");

    assert_eq!(
        result.document["cues"][0]["provenance"],
        json!([
            {"claimId":"claim_1","evidenceRef":"evidence_1"},
            {"claimId":"claim_1","evidenceRef":"evidence_2"}
        ])
    );
    assert_eq!(
        result.document["cues"][1]["provenance"],
        json!([{"claimId":"claim_2","evidenceRef":"evidence_1"}])
    );
    for artifact_id in [&result.raw_artifact_id, &result.artifact_id] {
        let artifact = fixture
            .storage
            .get_artifact(&fixture.project.project_path, artifact_id)
            .expect("read Captions artifact")
            .artifact;
        assert_eq!(artifact["provenance"], provenance);
        assert!(!artifact["provenance"]
            .as_array()
            .expect("pairs")
            .iter()
            .any(|pair| pair["claimId"] == "claim_2" && pair["evidenceRef"] == "evidence_2"));
    }
}

#[test]
fn captions_import_blocks_unmappable_factual_cue_before_writing_artifacts() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-unmappable-audio");
    fixture.prepare_captions();
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let secret = "PRIVATE_UNMAPPABLE_CAPTION";
    let srt = format!("1\n00:00:00,000 --> 00:00:00,100\n{secret}\n");
    let error = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-unmappable",
            &audio_input,
            srt.as_bytes(),
        ))
        .expect_err("unmappable factual cue must block");
    assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
    assert!(error.message.contains("sourceIndex=1"));
    assert!(error.message.contains("cueId="));
    assert!(!error.message.contains(secret));
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

#[test]
fn captions_mapping_fails_closed_when_tokens_outnumber_cue_milliseconds() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-token-limit-audio");
    fixture.prepare_captions();
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let text = "字".repeat(60);
    let srt = format!("1\n00:00:00,000 --> 00:00:00,050\n{text}\n");
    let error = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-token-limit",
            &audio_input,
            srt.as_bytes(),
        ))
        .expect_err("mapping must fail closed");
    assert_eq!(error.code, MediaErrorCode::ResourceLimitExceeded);
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

#[test]
fn captions_rejects_raw_or_invalid_audio_documents_and_identity_mismatches() {
    assert_audio_document_rejected(|_| pcm_wave(16_000, 1, 16, 1_600));
    assert_audio_document_rejected(|_| serde_json::to_vec(&json!({})).expect("invalid JSON doc"));
    assert_audio_document_rejected(|fixture| {
        let mut document = valid_audio_document(fixture);
        document["projectId"] = json!("project_other");
        serde_json::to_vec(&document).expect("wrong-project Audio document")
    });
    assert_audio_document_rejected(|fixture| {
        let mut document = valid_audio_document(fixture);
        document["runId"] = json!("run_audio_other");
        serde_json::to_vec(&document).expect("wrong-run Audio document")
    });
    assert_audio_document_rejected(|fixture| {
        let mut document = valid_audio_document(fixture);
        document["durationMs"] = json!(99);
        serde_json::to_vec(&document).expect("wrong-duration Audio document")
    });

    let wrong_kind = Fixture::new();
    let research =
        wrong_kind.workflow_input("research", "run_research_media", "review_research_media");
    let forged_audio = FrozenArtifactInputData {
        stage_id: "audio".to_owned(),
        run_id: research["sourceRunId"]
            .as_str()
            .expect("research run")
            .to_owned(),
        artifact_id: research["artifactId"]
            .as_str()
            .expect("research artifact")
            .to_owned(),
        content_hash: research["contentHash"]
            .as_str()
            .expect("research hash")
            .to_owned(),
        review_record_id: research["reviewRecordId"]
            .as_str()
            .expect("research review")
            .to_owned(),
        claim_ids: vec![],
        evidence_refs: vec![],
    };
    let before = wrong_kind.metadata_count();
    let error = wrong_kind
        .media
        .import_captions(wrong_kind.captions_options(
            "captions-audio-wrong-kind",
            &forged_audio,
            valid_short_srt(),
        ))
        .expect_err("Script artifact cannot masquerade as voice_audio");
    assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
    assert_eq!(wrong_kind.metadata_count(), before);

    let wrong_hash = Fixture::new();
    let mut audio_input = wrong_hash.install_approved_audio("captions-wrong-hash-audio");
    audio_input.content_hash = format!("sha256:{}", "d".repeat(64));
    let before = wrong_hash.metadata_count();
    let error = wrong_hash
        .media
        .import_captions(wrong_hash.captions_options(
            "captions-audio-wrong-hash",
            &audio_input,
            valid_short_srt(),
        ))
        .expect_err("forged Audio hash must fail");
    assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
    assert_eq!(wrong_hash.metadata_count(), before);
}

#[test]
fn captions_rights_hash_limits_and_parser_failures_propagate_without_writes() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-validation-audio");
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();

    let mut wrong_hash = fixture.captions_options(
        "captions-wrong-source-hash",
        &audio_input,
        valid_short_srt(),
    );
    wrong_hash.expected_source_content_hash = Some(format!("sha256:{}", "f".repeat(64)));
    assert_caption_error(&fixture, wrong_hash, MediaErrorCode::SourceHashMismatch);

    let mut missing_rights =
        fixture.captions_options("captions-missing-rights", &audio_input, valid_short_srt());
    missing_rights.rights.author.clear();
    assert_caption_error(&fixture, missing_rights, MediaErrorCode::RightsRequired);

    let mut voice_clone =
        fixture.captions_options("captions-voice-clone", &audio_input, valid_short_srt());
    voice_clone.rights.voice_authorization = "voice_clone".to_owned();
    assert_caption_error(&fixture, voice_clone, MediaErrorCode::VoiceCloneNotAllowed);

    for (key, change) in [
        ("captions-zero-byte-limit", 0_u8),
        ("captions-zero-cue-limit", 1_u8),
        ("captions-large-text-limit", 2_u8),
    ] {
        let mut options = fixture.captions_options(key, &audio_input, valid_short_srt());
        match change {
            0 => options.limits.max_bytes = 0,
            1 => options.limits.max_cue_count = 0,
            2 => options.limits.max_cue_text_bytes = 8_001,
            _ => unreachable!(),
        }
        assert_caption_error(&fixture, options, MediaErrorCode::InvalidRequest);
    }

    let malformed =
        fixture.captions_options("captions-malformed-srt", &audio_input, b"this is not SRT");
    assert_caption_error(&fixture, malformed, MediaErrorCode::InvalidMedia);

    let mut too_small =
        fixture.captions_options("captions-source-too-large", &audio_input, valid_short_srt());
    too_small.limits.max_bytes = 8;
    assert_caption_error(&fixture, too_small, MediaErrorCode::ResourceLimitExceeded);

    let out_of_audio = fixture.captions_options(
        "captions-outside-audio",
        &audio_input,
        b"1\n00:00:00,090 --> 00:00:00,101\nToo late.\n",
    );
    assert_caption_error(&fixture, out_of_audio, MediaErrorCode::InvalidMedia);
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

#[test]
fn captions_never_persists_or_returns_the_external_absolute_path() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-path-audio");
    fixture.prepare_captions();
    let options =
        fixture.captions_options("captions-path-redaction", &audio_input, valid_short_srt());
    let source_path = options.source_path.clone();
    let external_dir = fixture.external_dir.to_string_lossy().into_owned();
    let result = fixture
        .media
        .import_captions(options)
        .expect("import Captions");
    let result_json = serde_json::to_string(&result).expect("serialize Captions result");
    assert!(!result_json.contains("sourcePath"));
    assert!(!result_json.contains(&source_path));
    assert!(!result_json.contains(&external_dir));
    assert!(!result_json.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));

    let broken_path = fixture.external_dir.join("private-broken-captions.srt");
    let mut broken =
        fixture.captions_options("captions-path-error", &audio_input, b"not valid SRT");
    broken.source_path = broken_path.to_string_lossy().into_owned();
    fs::write(&broken_path, b"not valid SRT").expect("write broken SRT");
    let error = fixture
        .media
        .import_captions(broken)
        .expect_err("broken Captions must fail");
    let error_json = serde_json::to_string(&error).expect("serialize Captions error");
    assert!(!error_json.contains("sourcePath"));
    assert!(!error_json.contains(&external_dir));
    assert!(!error_json.contains("private-broken-captions.srt"));
    assert!(!error.to_string().contains(&external_dir));

    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("scan project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(&source_path), "path leaked in {path:?}");
        assert!(
            !text.contains(&external_dir),
            "directory leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "canary leaked in {path:?}"
        );
    }
}

#[test]
fn captions_idempotency_replays_and_conflicts_on_every_semantic_change() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-idempotency-audio");
    fixture.prepare_captions();
    let srt = b"1\n00:00:00,000 --> 00:00:00,090\nStable captions.\n";
    let options = fixture.captions_options("captions-idempotent", &audio_input, srt);
    let before = fixture.metadata_count();
    let first = fixture
        .media
        .import_captions(options.clone())
        .expect("first Captions import");
    let after_first = fixture.metadata_count();
    let replay = fixture
        .media
        .import_captions(options)
        .expect("Captions replay");
    assert_eq!(after_first, before + 2);
    assert_eq!(fixture.metadata_count(), after_first);
    assert_eq!(first.raw_artifact_id, replay.raw_artifact_id);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.content_hash, replay.content_hash);
    assert_eq!(first.document, replay.document);
    assert_eq!(first.document["createdAt"], replay.document["createdAt"]);
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);

    let changed_srt = fixture.captions_options(
        "captions-idempotent",
        &audio_input,
        b"1\n00:00:00,000 --> 00:00:00,090\nChanged captions.\n",
    );
    assert_caption_error(&fixture, changed_srt, MediaErrorCode::IdempotencyConflict);

    let mut changed_rights = fixture.captions_options("captions-idempotent", &audio_input, srt);
    changed_rights.rights.author = "Different Captioner".to_owned();
    assert_caption_error(
        &fixture,
        changed_rights,
        MediaErrorCode::IdempotencyConflict,
    );

    let mut changed_duration = fixture.captions_options("captions-idempotent", &audio_input, srt);
    changed_duration.audio_duration_ms = 99;
    assert_caption_error(
        &fixture,
        changed_duration,
        MediaErrorCode::IdempotencyConflict,
    );

    let mut changed_config = fixture.captions_options("captions-idempotent", &audio_input, srt);
    changed_config.config_snapshot = json!({"language":"en","interpolation":"other"});
    assert_caption_error(
        &fixture,
        changed_config,
        MediaErrorCode::IdempotencyConflict,
    );

    let mut changed_input = fixture.captions_options("captions-idempotent", &audio_input, srt);
    changed_input.audio_input.review_record_id = "review_audio_other".to_owned();
    assert_caption_error(&fixture, changed_input, MediaErrorCode::IdempotencyConflict);
    assert_eq!(fixture.metadata_count(), after_first);
    assert_eq!(fixture.receipt_count(), 2);
}

#[test]
fn concurrent_same_key_captions_imports_converge_without_duplicate_metadata() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-concurrent-audio");
    fixture.prepare_captions();
    let before = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let service = fixture.media.clone();
    let options = fixture.captions_options("captions-concurrent", &audio_input, valid_short_srt());
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service
                    .import_captions(options)
                    .expect("concurrent Captions import")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join Captions import"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].raw_artifact_id, results[1].raw_artifact_id);
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_eq!(results[0].document, results[1].document);
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(fixture.metadata_count(), before + 2);
    assert_eq!(fixture.receipt_count(), before_receipts + 1);
}

#[test]
fn different_captions_keys_preserve_prior_immutable_history() {
    let fixture = Fixture::new();
    let audio_input = fixture.install_approved_audio("captions-history-audio");
    fixture.prepare_captions();
    let first = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-history-1",
            &audio_input,
            b"1\n00:00:00,000 --> 00:00:00,100\nFirst version.\n",
        ))
        .expect("first Captions history");
    let old_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &first.artifact_id,
            4 * 1024 * 1024,
        )
        .expect("read old Captions document");
    let second = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-history-2",
            &audio_input,
            b"1\n00:00:00,000 --> 00:00:00,100\nSecond version.\n",
        ))
        .expect("second Captions history");
    assert_ne!(first.raw_artifact_id, second.raw_artifact_id);
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_ne!(first.document["captionsId"], second.document["captionsId"]);
    assert_eq!(fixture.receipt_count(), 3);
    assert_eq!(
        fixture
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &first.artifact_id,
                4 * 1024 * 1024,
            )
            .expect("old Captions document remains readable"),
        old_bytes
    );
    assert_eq!(
        fixture
            .storage
            .verify_artifact(&fixture.project.project_path, &first.artifact_id)
            .expect("verify old Captions document")
            .status,
        ArtifactVerificationStatusData::Verified
    );
}

#[test]
fn captions_rejects_script_unapproved_stale_replaced_old_review_cross_project_missing_and_tampered()
{
    let unapproved = Fixture::new();
    let audio = unapproved.install_approved_audio("captions-script-unapproved-audio");
    let script = unapproved.script_candidate(
        "run_script_unapproved_captions",
        "review_script_unapproved_captions",
        None,
    );
    assert_caption_inputs_rejected(
        &unapproved,
        &script,
        &audio,
        "captions-script-unapproved",
        &[MediaErrorCode::InputNotApproved],
    );

    let stale = Fixture::new();
    let audio = stale.install_approved_audio("captions-script-stale-audio");
    let brief = stale.workflow_input("brief", "run_brief_media", "review_brief_media");
    stale.record_and_approve(
        "research",
        "run_research_stale_captions",
        "review_research_stale_captions",
        vec![brief],
    );
    assert_caption_inputs_rejected(
        &stale,
        &stale.script_input,
        &audio,
        "captions-script-stale",
        &[MediaErrorCode::InputNotApproved],
    );

    let replaced = Fixture::new();
    let audio = replaced.install_approved_audio("captions-script-replaced-audio");
    replaced.script_candidate(
        "run_script_replacement_captions",
        "review_script_replacement_captions",
        Some(ReviewDecisionData::Approved),
    );
    assert_caption_inputs_rejected(
        &replaced,
        &replaced.script_input,
        &audio,
        "captions-script-replaced",
        &[MediaErrorCode::InputNotApproved],
    );

    let old_review = Fixture::new();
    let audio = old_review.install_approved_audio("captions-script-old-review-audio");
    let mut script = old_review.script_input.clone();
    script.review_record_id = "review_script_outdated".to_owned();
    assert_caption_inputs_rejected(
        &old_review,
        &script,
        &audio,
        "captions-script-old-review",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let local = Fixture::new();
    let audio = local.install_approved_audio("captions-script-cross-audio");
    let foreign = Fixture::new();
    assert_caption_inputs_rejected(
        &local,
        &foreign.script_input,
        &audio,
        "captions-script-cross-project",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let missing = Fixture::new();
    let audio = missing.install_approved_audio("captions-script-missing-audio");
    let script_read = missing
        .storage
        .get_artifact(
            &missing.project.project_path,
            &missing.script_input.artifact_id,
        )
        .expect("read Script for removal");
    fs::remove_file(Path::new(&missing.project.project_path).join(script_read.content_uri))
        .expect("remove Script content");
    assert_caption_inputs_rejected(
        &missing,
        &missing.script_input,
        &audio,
        "captions-script-missing",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let tampered = Fixture::new();
    let audio = tampered.install_approved_audio("captions-script-tampered-audio");
    let script_read = tampered
        .storage
        .get_artifact(
            &tampered.project.project_path,
            &tampered.script_input.artifact_id,
        )
        .expect("read Script for tamper");
    fs::write(
        Path::new(&tampered.project.project_path).join(script_read.content_uri),
        b"tampered Script",
    )
    .expect("tamper Script content");
    assert_caption_inputs_rejected(
        &tampered,
        &tampered.script_input,
        &audio,
        "captions-script-tampered",
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn captions_rejects_audio_unapproved_stale_replaced_old_review_cross_project_missing_and_tampered()
{
    let unapproved = Fixture::new();
    let audio = unapproved.audio_candidate(
        "run_audio_unapproved_captions",
        "review_audio_unapproved_captions",
        "captions-audio-unapproved-source",
        false,
    );
    assert_caption_inputs_rejected(
        &unapproved,
        &unapproved.script_input,
        &audio,
        "captions-audio-unapproved",
        &[MediaErrorCode::InputNotApproved],
    );

    let stale = Fixture::new();
    let audio = stale.install_approved_audio("captions-audio-stale-source");
    let script = stale.script_candidate(
        "run_script_after_audio",
        "review_script_after_audio",
        Some(ReviewDecisionData::Approved),
    );
    assert_caption_inputs_rejected(
        &stale,
        &script,
        &audio,
        "captions-audio-stale",
        &[MediaErrorCode::InputNotApproved],
    );

    let replaced = Fixture::new();
    let old_audio = replaced.install_approved_audio("captions-audio-old-source");
    replaced.audio_candidate(
        "run_audio_replacement_captions",
        "review_audio_replacement_captions",
        "captions-audio-replacement-source",
        true,
    );
    assert_caption_inputs_rejected(
        &replaced,
        &replaced.script_input,
        &old_audio,
        "captions-audio-replaced",
        &[MediaErrorCode::InputNotApproved],
    );

    let old_review = Fixture::new();
    let mut audio = old_review.install_approved_audio("captions-audio-review-source");
    audio.review_record_id = "review_audio_outdated".to_owned();
    assert_caption_inputs_rejected(
        &old_review,
        &old_review.script_input,
        &audio,
        "captions-audio-old-review",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let local = Fixture::new();
    local.install_approved_audio("captions-audio-local-source");
    let foreign = Fixture::new();
    let foreign_audio = foreign.install_approved_audio("captions-audio-foreign-source");
    assert_caption_inputs_rejected(
        &local,
        &local.script_input,
        &foreign_audio,
        "captions-audio-cross-project",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let missing = Fixture::new();
    let audio = missing.install_approved_audio("captions-audio-missing-source");
    let audio_read = missing
        .storage
        .get_artifact(&missing.project.project_path, &audio.artifact_id)
        .expect("read Audio for removal");
    fs::remove_file(Path::new(&missing.project.project_path).join(audio_read.content_uri))
        .expect("remove Audio content");
    assert_caption_inputs_rejected(
        &missing,
        &missing.script_input,
        &audio,
        "captions-audio-missing",
        &[MediaErrorCode::InputReferenceMismatch],
    );

    let tampered = Fixture::new();
    let audio = tampered.install_approved_audio("captions-audio-tampered-source");
    let audio_read = tampered
        .storage
        .get_artifact(&tampered.project.project_path, &audio.artifact_id)
        .expect("read Audio for tamper");
    fs::write(
        Path::new(&tampered.project.project_path).join(audio_read.content_uri),
        b"tampered Audio document",
    )
    .expect("tamper Audio content");
    assert_caption_inputs_rejected(
        &tampered,
        &tampered.script_input,
        &audio,
        "captions-audio-tampered",
        &[MediaErrorCode::ArtifactVerificationFailed],
    );
}

#[test]
fn audio_import_never_persists_or_returns_the_external_absolute_path() {
    let fixture = Fixture::new();
    let options = fixture.audio_options("audio-path-redaction");
    let source_path = options.source_path.clone();
    let external_dir = fixture.external_dir.to_string_lossy().into_owned();
    let result = fixture.media.import_audio(options).expect("import Audio");
    let result_json = serde_json::to_string(&result).expect("serialize result");
    assert!(!result_json.contains(&source_path));
    assert!(!result_json.contains(&external_dir));
    assert!(!result_json.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));

    let broken_path = fixture.external_dir.join("private-broken.wav");
    fs::write(&broken_path, b"not a wave").expect("write broken source");
    let mut broken = fixture.audio_options("audio-path-error");
    broken.source_path = broken_path.to_string_lossy().into_owned();
    let error = fixture
        .media
        .import_audio(broken)
        .expect_err("broken media must fail");
    let error_json = serde_json::to_string(&error).expect("serialize error");
    assert!(!error.to_string().contains(&external_dir));
    assert!(!error_json.contains(&external_dir));
    assert!(!error_json.contains("private-broken.wav"));

    for path in regular_files_recursive(Path::new(&fixture.project.project_path)) {
        let bytes = fs::read(&path).expect("scan project file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(&source_path), "path leaked in {path:?}");
        assert!(
            !text.contains(&external_dir),
            "directory leaked in {path:?}"
        );
        assert!(
            !text.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"),
            "canary leaked in {path:?}"
        );
    }
}

#[test]
fn audio_idempotency_replays_without_new_metadata_and_conflicts_on_changed_semantics() {
    let fixture = Fixture::new();
    let options = fixture.audio_options("audio-idempotent");
    let before = fixture.metadata_count();
    let first = fixture
        .media
        .import_audio(options.clone())
        .expect("first import");
    let after_first = fixture.metadata_count();
    let replay = fixture
        .media
        .import_audio(options.clone())
        .expect("idempotent replay");
    assert_eq!(after_first, before + 2);
    assert_eq!(fixture.metadata_count(), after_first);
    assert_eq!(first.raw_artifact_id, replay.raw_artifact_id);
    assert_eq!(first.artifact_id, replay.artifact_id);
    assert_eq!(first.document, replay.document);
    assert_eq!(first.document["createdAt"], replay.document["createdAt"]);
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);

    let mut changed_rights = options.clone();
    changed_rights.rights.author = "Different Author".to_owned();
    let mut changed_hash = options.clone();
    changed_hash.expected_source_content_hash = Some(format!("sha256:{}", "a".repeat(64)));
    let mut changed_config = options.clone();
    changed_config.config_snapshot = json!({"normalize":true});
    let mut changed_run = options.clone();
    changed_run.run_id = "run_audio_changed".to_owned();
    let mut changed_input = options;
    changed_input.script_input.review_record_id = "review_script_other".to_owned();
    for changed in [
        changed_rights,
        changed_hash,
        changed_config,
        changed_run,
        changed_input,
    ] {
        let error = fixture
            .media
            .import_audio(changed)
            .expect_err("same key with changed semantics must conflict");
        assert_eq!(error.code, MediaErrorCode::IdempotencyConflict);
    }
    assert_eq!(fixture.metadata_count(), after_first);
    assert_eq!(fixture.receipt_count(), 1);
}

#[test]
fn expected_hash_mismatch_creates_no_artifact_or_receipt() {
    let fixture = Fixture::new();
    let before = fixture.metadata_count();
    let mut options = fixture.audio_options("audio-wrong-hash");
    options.expected_source_content_hash = Some(format!("sha256:{}", "f".repeat(64)));
    let error = fixture
        .media
        .import_audio(options)
        .expect_err("wrong expected hash must fail");
    assert_eq!(error.code, MediaErrorCode::SourceHashMismatch);
    assert_eq!(fixture.metadata_count(), before);
    assert_eq!(fixture.receipt_count(), 0);
}

#[test]
fn audio_import_rejects_unapproved_stale_old_cross_project_and_corrupt_inputs() {
    let unapproved = Fixture::new();
    let candidate =
        unapproved.script_candidate("run_script_unapproved", "review_script_unapproved", None);
    let mut options = unapproved.audio_options("audio-unapproved");
    options.script_input = candidate;
    assert!(matches!(
        unapproved
            .media
            .import_audio(options)
            .expect_err("unapproved")
            .code,
        MediaErrorCode::InputNotApproved | MediaErrorCode::InputReferenceMismatch
    ));

    let stale = Fixture::new();
    let brief = stale.workflow_input("brief", "run_brief_media", "review_brief_media");
    stale.record_and_approve(
        "research",
        "run_research_new",
        "review_research_new",
        vec![brief],
    );
    let error = stale
        .media
        .import_audio(stale.audio_options("audio-stale"))
        .expect_err("stale Script must fail");
    assert_eq!(error.code, MediaErrorCode::InputNotApproved);

    let old = Fixture::new();
    old.script_candidate(
        "run_script_new",
        "review_script_new",
        Some(ReviewDecisionData::Approved),
    );
    let error = old
        .media
        .import_audio(old.audio_options("audio-old-review"))
        .expect_err("old approved Script must fail");
    assert_eq!(error.code, MediaErrorCode::InputNotApproved);

    let first_project = Fixture::new();
    let second_project = Fixture::new();
    let mut cross = first_project.audio_options("audio-cross-project");
    cross.script_input = second_project.script_input.clone();
    let error = first_project
        .media
        .import_audio(cross)
        .expect_err("cross-project artifact must fail");
    assert!(matches!(
        error.code,
        MediaErrorCode::InputReferenceMismatch | MediaErrorCode::InputNotApproved
    ));

    let forged = Fixture::new();
    let mut forged_options = forged.audio_options("audio-forged-hash");
    forged_options.script_input.content_hash = format!("sha256:{}", "b".repeat(64));
    let error = forged
        .media
        .import_audio(forged_options)
        .expect_err("forged metadata hash must fail");
    assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);

    let missing = Fixture::new();
    let missing_artifact = missing
        .storage
        .get_artifact(
            &missing.project.project_path,
            &missing.script_input.artifact_id,
        )
        .expect("read missing candidate");
    fs::remove_file(Path::new(&missing.project.project_path).join(missing_artifact.content_uri))
        .expect("remove script content");
    let error = missing
        .media
        .import_audio(missing.audio_options("audio-missing-content"))
        .expect_err("missing content must fail");
    assert!(matches!(
        error.code,
        MediaErrorCode::InputReferenceMismatch | MediaErrorCode::ArtifactVerificationFailed
    ));

    let tampered = Fixture::new();
    let tampered_artifact = tampered
        .storage
        .get_artifact(
            &tampered.project.project_path,
            &tampered.script_input.artifact_id,
        )
        .expect("read tamper candidate");
    fs::write(
        Path::new(&tampered.project.project_path).join(tampered_artifact.content_uri),
        b"tampered script content",
    )
    .expect("tamper script content");
    let error = tampered
        .media
        .import_audio(tampered.audio_options("audio-tampered-content"))
        .expect_err("tampered content must fail");
    assert_eq!(error.code, MediaErrorCode::ArtifactVerificationFailed);
}

#[test]
fn different_keys_create_new_immutable_history_without_overwriting_old_documents() {
    let fixture = Fixture::new();
    let first = fixture
        .media
        .import_audio(fixture.audio_options("audio-history-1"))
        .expect("first history");
    let old_bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            &first.artifact_id,
            4 * 1024 * 1024,
        )
        .expect("read old document");
    let second = fixture
        .media
        .import_audio(fixture.audio_options("audio-history-2"))
        .expect("second history");
    assert_ne!(first.raw_artifact_id, second.raw_artifact_id);
    assert_ne!(first.artifact_id, second.artifact_id);
    assert_eq!(fixture.receipt_count(), 2);
    assert_eq!(
        fixture
            .storage
            .read_artifact_content_bounded(
                &fixture.project.project_path,
                &fixture.project.project_id,
                &first.artifact_id,
                4 * 1024 * 1024,
            )
            .expect("old document remains readable"),
        old_bytes
    );
}

#[test]
fn invalid_rights_basename_idempotency_run_config_and_limits_fail_before_receipt() {
    let fixture = Fixture::new();
    let mut voice_clone = fixture.audio_options("invalid-rights");
    voice_clone.rights.voice_authorization = "voice_clone".to_owned();
    let mut missing_rights = fixture.audio_options("missing-rights");
    missing_rights.rights.author.clear();
    let mut unsafe_name = fixture.audio_options("unsafe-name");
    unsafe_name.source_path = fixture
        .external_dir
        .join("CON.wav")
        .to_string_lossy()
        .into_owned();
    let mut empty_key = fixture.audio_options("unused");
    empty_key.idempotency_key.clear();
    let mut bad_run = fixture.audio_options("bad-run");
    bad_run.run_id = "bad".to_owned();
    let mut bad_config = fixture.audio_options("bad-config");
    bad_config.config_snapshot = json!([]);
    let mut bad_limit = fixture.audio_options("bad-limit");
    bad_limit.limits.max_bytes = 0;

    let cases = [
        (voice_clone, MediaErrorCode::VoiceCloneNotAllowed),
        (missing_rights, MediaErrorCode::RightsRequired),
        (unsafe_name, MediaErrorCode::InvalidSourceName),
        (empty_key, MediaErrorCode::InvalidRequest),
        (bad_run, MediaErrorCode::InvalidRequest),
        (bad_config, MediaErrorCode::InvalidRequest),
        (bad_limit, MediaErrorCode::InvalidRequest),
    ];
    for (options, expected_code) in cases {
        let error = fixture
            .media
            .import_audio(options)
            .expect_err("invalid request must fail");
        assert_eq!(error.code, expected_code);
    }
    assert_eq!(fixture.receipt_count(), 0);
}

#[test]
fn concurrent_same_key_imports_converge_without_duplicate_metadata() {
    let fixture = Fixture::new();
    let before = fixture.metadata_count();
    let service = fixture.media.clone();
    let options = fixture.audio_options("audio-concurrent");
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let options = options.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                service.import_audio(options).expect("concurrent import")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join import"))
        .collect::<Vec<_>>();
    assert_eq!(results[0].raw_artifact_id, results[1].raw_artifact_id);
    assert_eq!(results[0].artifact_id, results[1].artifact_id);
    assert_ne!(results[0].idempotent_replay, results[1].idempotent_replay);
    assert_eq!(fixture.metadata_count(), before + 2);
    assert_eq!(fixture.receipt_count(), 1);
}

fn assert_caption_error(
    fixture: &Fixture,
    options: ImportCaptionsOptions,
    expected: MediaErrorCode,
) {
    let error = fixture
        .media
        .import_captions(options)
        .expect_err("Captions request must fail");
    assert_eq!(error.code, expected);
}

fn assert_caption_inputs_rejected(
    fixture: &Fixture,
    script_input: &FrozenArtifactInputData,
    audio_input: &FrozenArtifactInputData,
    key: &str,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let mut options = fixture.captions_options(key, audio_input, valid_short_srt());
    options.script_input = script_input.clone();
    let error = fixture
        .media
        .import_captions(options)
        .expect_err("invalid approved-input closure must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected Captions closure error: {:?}",
        error.code
    );
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn assert_scene_plan_inputs_rejected(
    fixture: &Fixture,
    research_input: FrozenArtifactInputData,
    script_input: FrozenArtifactInputData,
    captions_input: FrozenArtifactInputData,
    key: &str,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let mut options = fixture.scene_plan_options(key, &captions_input);
    options.research_input = research_input;
    options.script_input = script_input;
    let error = fixture
        .media
        .generate_scene_plan(options)
        .expect_err("invalid Scene Plan approval closure must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected Scene Plan closure error: {:?}",
        error.code
    );
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn assert_timeline_inputs_rejected(
    fixture: &Fixture,
    chain: ApprovedTimelineChain,
    key: &str,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let error = fixture
        .media
        .generate_timeline(fixture.timeline_options(key, &chain))
        .expect_err("invalid Timeline approval or document closure must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected Timeline closure error: {:?}",
        error.code
    );
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn timeline_input<'a>(
    chain: &'a ApprovedTimelineChain,
    stage_id: &str,
) -> &'a FrozenArtifactInputData {
    match stage_id {
        "audio" => &chain.audio_input,
        "captions" => &chain.captions_input,
        "scene_plan" => &chain.scene_plan_input,
        _ => panic!("unsupported Timeline input stage {stage_id}"),
    }
}

fn timeline_input_mut<'a>(
    chain: &'a mut ApprovedTimelineChain,
    stage_id: &str,
) -> &'a mut FrozenArtifactInputData {
    match stage_id {
        "audio" => &mut chain.audio_input,
        "captions" => &mut chain.captions_input,
        "scene_plan" => &mut chain.scene_plan_input,
        _ => panic!("unsupported Timeline input stage {stage_id}"),
    }
}

fn assert_scene_plan_save_error(
    fixture: &Fixture,
    options: SaveScenePlanOptions,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let error = fixture
        .media
        .save_scene_plan(options)
        .expect_err("invalid Scene Plan base must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected Scene Plan save error: {:?}",
        error.code
    );
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn assert_timeline_save_error(
    fixture: &Fixture,
    options: SaveTimelineOptions,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let error = fixture
        .media
        .save_timeline(options)
        .expect_err("invalid Timeline save must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected Timeline save error: {:?}",
        error.code
    );
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn assert_media_document_query_error(
    fixture: &Fixture,
    options: GetMediaDocumentOptions,
    expected: &[MediaErrorCode],
) {
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let error = fixture
        .media
        .get_media_document(options)
        .expect_err("invalid media document query must fail");
    assert!(
        expected.contains(&error.code),
        "unexpected media document query error: {:?}",
        error.code
    );
    let serialized = serde_json::to_string(&error).expect("serialize media query error");
    assert!(!serialized.contains(&fixture.project.project_path));
    assert!(!serialized.contains(fixture.external_dir.to_string_lossy().as_ref()));
    assert!(!serialized.contains("EXTERNAL_ABSOLUTE_PATH_CANARY"));
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn corrupt_artifact_content(fixture: &Fixture, artifact_id: &str, tamper: bool) {
    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, artifact_id)
        .expect("read Artifact to corrupt");
    let path = Path::new(&fixture.project.project_path).join(read.content_uri);
    if tamper {
        fs::write(path, b"tampered immutable Artifact content").expect("tamper Artifact content");
    } else {
        fs::remove_file(path).expect("remove Artifact content");
    }
}

fn read_artifact_json(fixture: &Fixture, artifact_id: &str) -> Value {
    let bytes = fixture
        .storage
        .read_artifact_content_bounded(
            &fixture.project.project_path,
            &fixture.project.project_id,
            artifact_id,
            16 * 1024 * 1024,
        )
        .expect("read Artifact JSON");
    serde_json::from_slice(&bytes).expect("parse Artifact JSON")
}

fn frozen_input_json(fixture: &Fixture, input: &FrozenArtifactInputData) -> Value {
    json!({
        "projectId": fixture.project.project_id,
        "stageId": input.stage_id,
        "runId": input.run_id,
        "artifactId": input.artifact_id,
        "contentHash": input.content_hash,
        "reviewRecordId": input.review_record_id,
        "claimIds": input.claim_ids,
        "evidenceRefs": input.evidence_refs,
    })
}

fn stable_scene_plan_receipt_id_for_test(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0generate_scene_plan\0");
    hasher.update(idempotency_key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_timeline_receipt_id_for_test(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0generate_timeline\0");
    hasher.update(idempotency_key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_scene_plan_save_receipt_id_for_test(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0save_scene_plan\0");
    hasher.update(idempotency_key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_timeline_save_receipt_id_for_test(project_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:media-receipt:v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\0save_timeline\0");
    hasher.update(idempotency_key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_short_srt() -> &'static [u8] {
    b"1\n00:00:00,000 --> 00:00:00,100\nTraceable captions.\n"
}

fn valid_scene_plan_srt() -> &'static [u8] {
    b"1\n00:00:00,000 --> 00:00:00,020\nFirst.\n\n2\n00:00:00,020 --> 00:00:00,040\nSecond.\n\n3\n00:00:00,040 --> 00:00:00,060\nThird.\n\n4\n00:00:00,060 --> 00:00:00,100\nFourth.\n"
}

fn valid_seven_cue_scene_plan_srt() -> &'static [u8] {
    b"1\n00:00:00,000 --> 00:00:00,010\nOne.\n\n2\n00:00:00,010 --> 00:00:00,020\nTwo.\n\n3\n00:00:00,020 --> 00:00:00,030\nThree.\n\n4\n00:00:00,030 --> 00:00:00,040\nFour.\n\n5\n00:00:00,040 --> 00:00:00,050\nFive.\n\n6\n00:00:00,050 --> 00:00:00,060\nSix.\n\n7\n00:00:00,060 --> 00:00:00,100\nSeven.\n"
}

fn valid_audio_document(fixture: &Fixture) -> Value {
    let object_hash = "a".repeat(64);
    json!({
        "schemaVersion": "1.0.0",
        "documentType": "audio_media",
        "mediaId": "media_audio_payload",
        "projectId": fixture.project.project_id,
        "runId": "run_audio_payload",
        "artifactUri": format!("artifacts/objects/sha256/aa/{object_hash}"),
        "source": {
            "sourceFileName": "payload.wav",
            "sourceContentHash": format!("sha256:{}", "b".repeat(64)),
            "byteLength": 3244,
        },
        "rights": {
            "ownership": "self_recorded",
            "author": "Fixture Author",
            "rightsStatement": "Fixture owns this recording.",
            "licenseId": "fixture-owned-audio",
            "attributionText": "",
            "voiceAuthorization": "not_voice_clone",
        },
        "durationMs": 100,
        "sampleRateHz": 16000,
        "bitsPerSample": 16,
        "channels": 1,
        "blockAlign": 2,
        "byteRate": 32000,
        "dataBytes": 3200,
        "inputRefs": [{
            "projectId": fixture.project.project_id,
            "stageId": fixture.script_input.stage_id,
            "runId": fixture.script_input.run_id,
            "artifactId": fixture.script_input.artifact_id,
            "contentHash": fixture.script_input.content_hash,
            "reviewRecordId": fixture.script_input.review_record_id,
            "claimIds": fixture.script_input.claim_ids,
            "evidenceRefs": fixture.script_input.evidence_refs,
        }],
        "configSnapshot": {"fixture":true},
        "createdAt": "2026-07-18T08:00:00Z",
    })
}

fn assert_audio_document_rejected(payload: impl FnOnce(&Fixture) -> Vec<u8>) {
    let fixture = Fixture::new();
    let bytes = payload(&fixture);
    let audio_input = fixture.approved_audio_payload(&bytes);
    let before_metadata = fixture.metadata_count();
    let before_receipts = fixture.receipt_count();
    let error = fixture
        .media
        .import_captions(fixture.captions_options(
            "captions-invalid-audio-document",
            &audio_input,
            valid_short_srt(),
        ))
        .expect_err("invalid Audio document must fail");
    assert_eq!(error.code, MediaErrorCode::InputReferenceMismatch);
    assert_eq!(fixture.metadata_count(), before_metadata);
    assert_eq!(fixture.receipt_count(), before_receipts);
}

fn output_kind(stage_id: &str) -> &'static str {
    match stage_id {
        "brief" => "brief",
        "research" => "claim_set",
        _ => panic!("unsupported fixture stage {stage_id}"),
    }
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
    let riff_size = u32::try_from(bytes.len() - 8).expect("small WAV");
    bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
    bytes
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read JSON")).expect("parse JSON")
}

fn assert_saved_stage_run(
    fixture: &Fixture,
    stage_id: &str,
    run_id: &str,
    base_artifact_id: &str,
    artifact_id: &str,
) {
    let run_dir = Path::new(&fixture.project.project_path)
        .join("runs")
        .join(stage_id)
        .join(run_id);
    let execution = read_json(run_dir.join("execution.json"));
    let run = read_json(run_dir.join("run.json"));
    assert_eq!(execution["documentType"], "stage_execution_snapshot");
    assert_eq!(execution["runId"], run_id);
    assert_eq!(execution["stageId"], stage_id);
    assert_eq!(execution["jobId"], run["jobId"]);
    assert_eq!(execution["inputRefs"], run["inputRefs"]);
    assert_eq!(execution["configSnapshot"], run["configSnapshot"]);
    assert_eq!(execution["executor"], run["executor"]);
    assert_eq!(execution["executor"]["providerId"], "narracut_media_editor");
    assert_eq!(execution["executor"]["executionMode"], "local");
    let input_refs = execution["inputRefs"]
        .as_array()
        .expect("saved execution input refs");
    assert!(!input_refs.is_empty());
    let expected_base_uri = format!("project://artifacts/metadata/{base_artifact_id}.json");
    let base_ref = input_refs
        .iter()
        .find(|input| input["uri"] == expected_base_uri)
        .expect("execution freezes the edited base Artifact metadata");
    assert_eq!(base_ref["referenceType"], "project_document");
    assert_eq!(base_ref["kind"], stage_id);
    assert!(base_ref["contentHash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:") && hash.len() == 71));
    assert_eq!(run["documentType"], "stage_run");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["artifactIds"], json!([artifact_id]));
    assert_eq!(run["logSummary"]["warnings"], json!([]));
    assert_eq!(run["logSummary"]["errors"], json!([]));

    let job_id = run["jobId"].as_str().expect("saved run job id");
    let jobs = JobService::new(
        ProjectService::default(),
        fixture.storage.clone(),
        fixture.workflow.clone(),
    );
    let snapshot = jobs
        .get_job(GetJobOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id.to_owned(),
        })
        .expect("saved run points to a queryable Job");
    assert_eq!(snapshot.status, JobStatusData::Succeeded);
    assert_eq!(snapshot.artifact_ids, vec![artifact_id.to_owned()]);
    let events = jobs
        .list_job_events(ListJobEventsOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            job_id: job_id.to_owned(),
            after_sequence: None,
            limit: 32,
        })
        .expect("saved run Job events");
    let event_types = events
        .events
        .iter()
        .filter_map(|event| event["eventType"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            "queued",
            "started",
            "artifact_created",
            "completion_requested",
            "completed"
        ]
    );
}

fn count_regular_files(directory: &Path) -> usize {
    match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read {directory:?}: {error}"),
    }
}

fn regular_files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("scan project directory") {
            let entry = entry.expect("project directory entry");
            let kind = entry.file_type().expect("project entry type");
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    files
}
