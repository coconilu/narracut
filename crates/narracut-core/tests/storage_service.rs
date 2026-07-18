use std::{fs, path::Path};

use narracut_contracts::{validate_storage_command_message, ArtifactDraft};
use narracut_core::{
    ArtifactVerificationStatusData, CopyProjectOptions, CreateProjectOptions, IndexedJobStatusData,
    IndexedJobUpsertData, ListIndexedJobsOptions, ProjectDescriptorData, ProjectService,
    ResolveStagedMediaSourceOptions, StageMediaSourceFileOptions, StorageErrorCode,
    StorageIndexStatusData, StorageService, StoreArtifactFileOptions,
};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    imports: TempDir,
    project_service: ProjectService,
    storage: StorageService,
    index_path: std::path::PathBuf,
    project: ProjectDescriptorData,
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
                name: "示例项目".to_owned(),
                workflow_definition_id: "workflow_standard_v1".to_owned(),
                default_locale: Some("zh-CN".to_owned()),
            })
            .expect("create project");
        let index_path = temp.path().join("app-data").join("narracut-index.sqlite3");
        let storage = StorageService::new(&index_path, project_service.clone());
        Self {
            _temp: temp,
            imports,
            project_service,
            storage,
            index_path,
            project,
        }
    }

    fn write_source(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.imports.path().join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }

    fn import(
        &self,
        source: &Path,
        draft: ArtifactDraft,
    ) -> narracut_core::ArtifactCommitResultData {
        self.storage
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: self.project.project_path.clone(),
                expected_project_id: self.project.project_id.clone(),
                source_path: source.to_string_lossy().into_owned(),
                artifact: draft,
            })
            .expect("import artifact")
    }
}

#[test]
fn media_source_staging_is_content_addressed_deduplicated_and_resolved() {
    let fixture = Fixture::new();
    let external_canary = "EXTERNAL_MEDIA_SOURCE_PATH_MUST_NOT_PERSIST";
    let bytes = b"immutable staged narration";
    let source = fixture.write_source(&format!("{external_canary} 旁白.wav"), bytes);
    let expected_hash = test_sha256(bytes);
    let options = StageMediaSourceFileOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        source_path: source.to_string_lossy().into_owned(),
        expected_content_hash: Some(expected_hash.clone()),
        max_bytes: 1024,
    };

    let first = fixture
        .storage
        .stage_media_source_file(options.clone())
        .expect("stage media source");
    assert_eq!(first.owner_project_id, fixture.project.project_id);
    assert_eq!(first.content_hash, expected_hash);
    assert_eq!(first.byte_length, bytes.len() as u64);
    assert!(!first.deduplicated);
    assert!(first.source_file_name.starts_with("source-"));
    assert!(!first.source_file_name.contains(' '));
    let uri_parts = first.staged_source_uri.split('/').collect::<Vec<_>>();
    assert_eq!(&uri_parts[..3], &["requests", "media-sources", "sha256"]);
    assert_eq!(uri_parts.len(), 6);
    assert_eq!(uri_parts[3], &first.content_hash[7..9]);
    assert_eq!(uri_parts[4], &first.content_hash[7..]);
    assert_eq!(uri_parts[5], first.source_file_name);
    let serialized = serde_json::to_string(&first).expect("serialize staged source");
    assert!(!serialized.contains(&fixture.imports.path().to_string_lossy().into_owned()));

    let second = fixture
        .storage
        .stage_media_source_file(options)
        .expect("deduplicate staged source");
    assert!(second.deduplicated);
    assert_eq!(second.staged_source_uri, first.staged_source_uri);

    let resolved = fixture
        .storage
        .resolve_staged_media_source(ResolveStagedMediaSourceOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            staged_source_uri: first.staged_source_uri.clone(),
            expected_content_hash: first.content_hash.clone(),
            expected_byte_length: first.byte_length,
            max_bytes: 1024,
        })
        .expect("resolve staged media source");
    assert_eq!(resolved.staged_source_uri, first.staged_source_uri);
    assert_eq!(resolved.source_file_name, first.source_file_name);
    assert!(Path::new(&resolved.source_path).starts_with(&fixture.project.project_path));
    assert_eq!(
        fs::read(&resolved.source_path).expect("read staged source"),
        bytes
    );
}

#[test]
fn media_source_staging_rejects_hash_size_non_file_and_links_without_path_leaks() {
    let fixture = Fixture::new();
    let external_canary = "EXTERNAL_MEDIA_ERROR_PATH_CANARY";
    let source = fixture.write_source(&format!("{external_canary}.srt"), b"12345678");
    let base = StageMediaSourceFileOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        source_path: source.to_string_lossy().into_owned(),
        expected_content_hash: None,
        max_bytes: 1024,
    };

    let mut wrong_hash = base.clone();
    wrong_hash.expected_content_hash = Some(format!("sha256:{}", "0".repeat(64)));
    assert_redacted_media_source_error(
        fixture
            .storage
            .stage_media_source_file(wrong_hash)
            .expect_err("expected hash mismatch must fail"),
        StorageErrorCode::SourceChanged,
        external_canary,
    );

    let mut oversized = base.clone();
    oversized.max_bytes = 4;
    assert_redacted_media_source_error(
        fixture
            .storage
            .stage_media_source_file(oversized)
            .expect_err("oversized source must fail before copy"),
        StorageErrorCode::SourceTooLarge,
        external_canary,
    );

    let mut directory = base.clone();
    directory.source_path = fixture.imports.path().to_string_lossy().into_owned();
    assert_redacted_media_source_error(
        fixture
            .storage
            .stage_media_source_file(directory)
            .expect_err("directory is not a source file"),
        StorageErrorCode::InvalidPath,
        external_canary,
    );

    let link = fixture.imports.path().join("linked-media-source.srt");
    match create_file_symlink(&source, &link) {
        Ok(()) => {
            let mut linked = base;
            linked.source_path = link.to_string_lossy().into_owned();
            assert_redacted_media_source_error(
                fixture
                    .storage
                    .stage_media_source_file(linked)
                    .expect_err("linked source must fail"),
                StorageErrorCode::PathContainsSymlink,
                external_canary,
            );
        }
        Err(error)
            if cfg!(windows)
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                ) => {}
        Err(error) => panic!("create media source symlink fixture: {error}"),
    }
}

#[test]
fn media_source_resolver_rejects_invalid_uris_tampering_and_linked_intermediates() {
    let fixture = Fixture::new();
    let source = fixture.write_source("captions.srt", b"staged captions bytes");
    let staged = fixture
        .storage
        .stage_media_source_file(StageMediaSourceFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: source.to_string_lossy().into_owned(),
            expected_content_hash: None,
            max_bytes: 1024,
        })
        .expect("stage resolver fixture");
    let resolve = |uri: String| ResolveStagedMediaSourceOptions {
        project_path: fixture.project.project_path.clone(),
        expected_project_id: fixture.project.project_id.clone(),
        staged_source_uri: uri,
        expected_content_hash: staged.content_hash.clone(),
        expected_byte_length: staged.byte_length,
        max_bytes: 1024,
    };

    for invalid_uri in [
        "requests/media-sources/.tmp/source.srt".to_owned(),
        format!("{}/extra", staged.staged_source_uri),
        staged.staged_source_uri.replace('/', "\\"),
        format!(
            "requests/media-sources/sha256/{}/{}/source-%2E%2E",
            &staged.content_hash[7..9],
            &staged.content_hash[7..]
        ),
        format!(
            "requests/media-sources/sha256/{}/{}/source-%2Fescape",
            &staged.content_hash[7..9],
            &staged.content_hash[7..]
        ),
    ] {
        let error = fixture
            .storage
            .resolve_staged_media_source(resolve(invalid_uri))
            .expect_err("invalid staged source URI must fail");
        assert_eq!(error.code, StorageErrorCode::InvalidPath);
        assert!(error.path.is_none());
    }

    let staged_path = portable_path(
        Path::new(&fixture.project.project_path),
        &staged.staged_source_uri,
    );
    fs::write(&staged_path, b"tampered staged captions").expect("tamper staged source");
    let error = fixture
        .storage
        .resolve_staged_media_source(resolve(staged.staged_source_uri.clone()))
        .expect_err("tampered staged source must fail resolver");
    assert_eq!(error.code, StorageErrorCode::ContentCorrupt);
    assert!(error.path.is_none());

    let dedupe_error = fixture
        .storage
        .stage_media_source_file(StageMediaSourceFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: source.to_string_lossy().into_owned(),
            expected_content_hash: Some(staged.content_hash.clone()),
            max_bytes: 1024,
        })
        .expect_err("dedupe must reverify an existing entity");
    assert_eq!(dedupe_error.code, StorageErrorCode::ContentCorrupt);
    assert!(dedupe_error.path.is_none());

    fs::write(&staged_path, b"staged captions bytes").expect("restore staged source");
    let sha_dir = Path::new(&fixture.project.project_path).join("requests/media-sources/sha256");
    let outside = fixture.imports.path().join("outside-staged-sha256");
    fs::rename(&sha_dir, &outside).expect("move staged source tree outside");
    match create_dir_symlink(&outside, &sha_dir) {
        Ok(()) => {
            let error = fixture
                .storage
                .resolve_staged_media_source(resolve(staged.staged_source_uri))
                .expect_err("linked staged intermediate must fail");
            assert_eq!(error.code, StorageErrorCode::PathContainsSymlink);
            assert!(error.path.is_none());
            remove_dir_symlink(&sha_dir).expect("remove staged intermediate link");
            fs::rename(&outside, &sha_dir).expect("restore staged source tree");
        }
        Err(error)
            if cfg!(windows)
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                ) =>
        {
            fs::rename(&outside, &sha_dir).expect("restore skipped staged source tree");
        }
        Err(error) => panic!("create staged source directory link fixture: {error}"),
    }
}

#[test]
fn artifact_import_is_content_addressed_deduplicated_and_verifiable() {
    let fixture = Fixture::new();
    let source = fixture.write_source("script.md", b"hello narracut");
    let first = fixture.import(&source, generated_draft("non_evidence"));

    assert_eq!(first.owner_project_id, fixture.project.project_id);
    assert!(!first.deduplicated);
    assert_eq!(first.index_status, StorageIndexStatusData::UpToDate);
    assert!(first.content_uri.starts_with("artifacts/objects/sha256/"));
    assert_storage_contract(&first);

    let first_artifact_id = artifact_id(&first.artifact);
    let metadata_path = portable_path(
        Path::new(&fixture.project.project_path),
        &first.metadata_uri,
    );
    let content_path = portable_path(Path::new(&fixture.project.project_path), &first.content_uri);
    assert!(metadata_path.is_file());
    assert_eq!(
        fs::read(&content_path).expect("read stored content"),
        b"hello narracut"
    );

    let read = fixture
        .storage
        .get_artifact(&fixture.project.project_path, &first_artifact_id)
        .expect("get artifact");
    assert!(read.content_available);
    assert_eq!(read.artifact, first.artifact);
    assert_storage_contract(&read);

    let verified = fixture
        .storage
        .verify_artifact(&fixture.project.project_path, &first_artifact_id)
        .expect("verify artifact");
    assert_eq!(verified.status, ArtifactVerificationStatusData::Verified);
    assert_eq!(
        verified.actual_content_hash,
        Some(verified.expected_content_hash.clone())
    );
    assert_storage_contract(&verified);

    let second = fixture.import(&source, generated_draft("non_evidence"));
    assert!(second.deduplicated);
    assert_eq!(second.content_uri, first.content_uri);
    assert_ne!(artifact_id(&second.artifact), first_artifact_id);
    assert_storage_contract(&second);

    let prefix_dir = content_path.parent().expect("content prefix directory");
    assert_eq!(
        fs::read_dir(prefix_dir)
            .expect("read content prefix")
            .count(),
        1
    );

    let recent = fixture
        .storage
        .list_recent_projects(20, false)
        .expect("list recent projects");
    assert_eq!(recent.projects.len(), 1);
    assert_eq!(recent.projects[0].project_id, fixture.project.project_id);
    assert_storage_contract(&recent);
}

#[test]
fn imported_source_hash_is_computed_by_the_store() {
    let fixture = Fixture::new();
    let source = fixture.write_source("licensed-source.png", b"licensed source bytes");
    let committed = fixture.import(&source, imported_draft());

    assert_eq!(
        committed.artifact["source"]["sourceContentHash"],
        committed.artifact["contentHash"]
    );
    assert_storage_contract(&committed);

    let forged = serde_json::from_value::<ArtifactDraft>(json!({
        "stageId": "research",
        "runId": "run_research_001",
        "kind": "source_image",
        "mediaType": "image/png",
        "evidenceRole": "factual_evidence",
        "source": {
            "origin": "imported",
            "sourceUri": "https://example.com/source.png",
            "author": "Example Author",
            "license": "CC-BY-4.0",
            "attributionText": "Example Author / CC-BY-4.0",
            "sourceContentHash": "sha256:forged",
            "authorizationRecordIds": ["authorization_001"]
        },
        "provenance": [{
            "claimId": "claim_001",
            "evidenceRef": "evidence_001"
        }]
    }));
    assert!(
        forged.is_err(),
        "an imported draft must not be able to declare its own source hash"
    );

    let mut tampered = committed.artifact.clone();
    tampered["source"]["sourceContentHash"] = Value::String("sha256:forged".to_owned());
    let metadata_path = portable_path(
        Path::new(&fixture.project.project_path),
        &committed.metadata_uri,
    );
    fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&tampered).expect("serialize tampered metadata"),
    )
    .expect("tamper imported source hash");
    let error = fixture
        .storage
        .get_artifact(
            &fixture.project.project_path,
            &artifact_id(&committed.artifact),
        )
        .expect_err("tampered source hash must fail on read");
    assert_eq!(error.code, StorageErrorCode::InvalidArtifact);
}

#[test]
fn artifact_source_references_must_resolve_before_writes() {
    let fixture = Fixture::new();
    let source = fixture.write_source("source.md", b"source artifact");
    let source_artifact = fixture.import(&source, generated_draft("non_evidence"));
    let source_artifact_id = artifact_id(&source_artifact.artifact);

    let derived_source = fixture.write_source("derived.md", b"derived artifact");
    let derived = fixture.import(&derived_source, derived_draft(&source_artifact_id));
    assert_eq!(derived.artifact["source"]["origin"], "derived");
    assert_eq!(
        derived.artifact["source"]["sourceArtifactIds"][0],
        source_artifact_id
    );
    assert_storage_contract(&derived);

    let derived_artifact_id = artifact_id(&derived.artifact);
    let derived_metadata_path = portable_path(
        Path::new(&fixture.project.project_path),
        &derived.metadata_uri,
    );
    let mut broken_reference = derived.artifact.clone();
    broken_reference["source"]["sourceArtifactIds"][0] =
        Value::String("artifact_missing".to_owned());
    write_json_fixture(&derived_metadata_path, &broken_reference);
    let broken_error = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect_err("rebuild must reject missing source references");
    assert_eq!(broken_error.code, StorageErrorCode::ArtifactNotFound);
    write_json_fixture(&derived_metadata_path, &derived.artifact);

    let source_metadata_path = portable_path(
        Path::new(&fixture.project.project_path),
        &source_artifact.metadata_uri,
    );
    let mut cyclic_source = source_artifact.artifact.clone();
    cyclic_source["source"]["promptArtifactId"] = Value::String(derived_artifact_id);
    write_json_fixture(&source_metadata_path, &cyclic_source);
    let cycle_error = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect_err("rebuild must reject cyclic source references");
    assert_eq!(cycle_error.code, StorageErrorCode::ArtifactConflict);
    write_json_fixture(&source_metadata_path, &source_artifact.artifact);
    assert_eq!(
        fixture
            .storage
            .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
            .expect("rebuild repaired reference graph")
            .artifacts_indexed,
        2
    );

    let metadata_dir = Path::new(&fixture.project.project_path)
        .join("artifacts")
        .join("metadata");
    let metadata_count = fs::read_dir(&metadata_dir)
        .expect("read metadata before rejected import")
        .count();
    let missing_error = fixture
        .storage
        .import_artifact_file(StoreArtifactFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: derived_source.to_string_lossy().into_owned(),
            artifact: derived_draft("artifact_missing"),
        })
        .expect_err("missing source Artifact must reject the draft");
    assert_eq!(missing_error.code, StorageErrorCode::ArtifactNotFound);
    assert_eq!(
        fs::read_dir(&metadata_dir)
            .expect("read metadata after rejected import")
            .count(),
        metadata_count
    );

    fs::remove_file(portable_path(
        Path::new(&fixture.project.project_path),
        &source_artifact.content_uri,
    ))
    .expect("remove referenced content");
    let unavailable_error = fixture
        .storage
        .import_artifact_file(StoreArtifactFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: derived_source.to_string_lossy().into_owned(),
            artifact: derived_draft(&source_artifact_id),
        })
        .expect_err("missing referenced content must reject the draft");
    assert_eq!(unavailable_error.code, StorageErrorCode::ContentCorrupt);
}

#[test]
fn invalid_draft_and_project_identity_are_rejected_before_artifact_writes() {
    let fixture = Fixture::new();
    let source = fixture.write_source("evidence.txt", b"not evidence");

    let identity_error = fixture
        .storage
        .import_artifact_file(StoreArtifactFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: "project_wrong".to_owned(),
            source_path: source.to_string_lossy().into_owned(),
            artifact: generated_draft("non_evidence"),
        })
        .expect_err("wrong project identity must fail");
    assert_eq!(
        identity_error.code,
        StorageErrorCode::ProjectIdentityMismatch
    );

    let invalid_draft = serde_json::from_value::<ArtifactDraft>(json!({
        "stageId": "script",
        "runId": "run_script_001",
        "kind": "script",
        "mediaType": "text/markdown",
        "evidenceRole": "factual_evidence",
        "source": {
            "origin": "generated",
            "providerId": "openai",
            "model": "example-model"
        },
        "provenance": []
    }));
    assert!(
        invalid_draft.is_err(),
        "generated material must be rejected before entering the store"
    );
    let invalid_id_error = fixture
        .storage
        .get_artifact(&fixture.project.project_path, "CON")
        .expect_err("artifact IDs must remain portable across Windows filesystems");
    assert_eq!(invalid_id_error.code, StorageErrorCode::InvalidRequest);

    let oversized_source = fixture.write_source("oversized.bin", b"");
    let oversized_file = fs::OpenOptions::new()
        .write(true)
        .open(&oversized_source)
        .expect("open oversized source fixture");
    oversized_file
        .set_len(64 * 1024 * 1024 + 1)
        .expect("create oversized sparse source");
    drop(oversized_file);
    let oversized_error = fixture
        .storage
        .import_artifact_file(StoreArtifactFileOptions {
            project_path: fixture.project.project_path.clone(),
            expected_project_id: fixture.project.project_id.clone(),
            source_path: oversized_source.to_string_lossy().into_owned(),
            artifact: generated_draft("non_evidence"),
        })
        .expect_err("large imports must wait for the job queue");
    assert_eq!(oversized_error.code, StorageErrorCode::SourceTooLarge);

    let project_dir = Path::new(&fixture.project.project_path);
    assert!(!project_dir.join("artifacts/metadata").exists());
    assert!(!project_dir.join("artifacts/.tmp").exists());
    assert!(fs::read_dir(project_dir.join("artifacts"))
        .expect("read artifacts directory")
        .next()
        .is_none());
}

#[test]
fn artifact_truth_commits_when_the_disposable_index_is_unavailable() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.index_path).expect("block SQLite path with a directory");
    let source = fixture.write_source("offline-index.md", b"project truth survives");

    let committed = fixture.import(&source, generated_draft("non_evidence"));
    assert_eq!(
        committed.index_status,
        StorageIndexStatusData::RebuildRequired
    );
    assert_storage_contract(&committed);

    let artifact_id = artifact_id(&committed.artifact);
    let metadata_path = portable_path(
        Path::new(&fixture.project.project_path),
        &committed.metadata_uri,
    );
    let content_path = portable_path(
        Path::new(&fixture.project.project_path),
        &committed.content_uri,
    );
    assert!(metadata_path.is_file());
    assert_eq!(
        fs::read(&content_path).expect("read committed content"),
        b"project truth survives"
    );
    assert_eq!(
        fixture
            .storage
            .get_artifact(&fixture.project.project_path, &artifact_id)
            .expect("read artifact without index")
            .artifact,
        committed.artifact
    );

    fs::remove_dir_all(&fixture.index_path).expect("unblock SQLite path");
    let rebuilt = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect("rebuild disposable index from project truth");
    assert_eq!(rebuilt.artifacts_indexed, 1);
    assert_eq!(rebuilt.index_status, StorageIndexStatusData::UpToDate);
    assert_storage_contract(&rebuilt);
}

#[test]
fn failed_rebuild_keeps_the_previous_sqlite_snapshot() {
    let fixture = Fixture::new();
    let source = fixture.write_source("stable-index.md", b"stable index");
    fixture.import(&source, generated_draft("non_evidence"));

    let metadata_dir = Path::new(&fixture.project.project_path)
        .join("artifacts")
        .join("metadata");
    let malformed_path = metadata_dir.join("artifact_malformed.json");
    fs::write(&malformed_path, b"{").expect("write malformed metadata");

    let error = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect_err("malformed metadata must abort rebuild");
    assert_eq!(error.code, StorageErrorCode::InvalidArtifact);

    let connection = Connection::open(&fixture.index_path).expect("open existing index");
    let indexed_artifacts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM artifacts WHERE owner_project_id = ?1",
            rusqlite::params![fixture.project.project_id],
            |row| row.get(0),
        )
        .expect("count preserved artifact rows");
    assert_eq!(indexed_artifacts, 1);
    drop(connection);

    fs::remove_file(&malformed_path).expect("remove malformed metadata");
    let rebuilt = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect("rebuild after repairing project truth");
    assert_eq!(rebuilt.artifacts_indexed, 1);
}

#[test]
fn sqlite_index_can_be_deleted_and_rebuilt_from_project_truth() {
    let fixture = Fixture::new();
    let source = fixture.write_source("script.md", b"alpha");
    let committed = fixture.import(&source, generated_draft("non_evidence"));
    let artifact_id = artifact_id(&committed.artifact);
    let content_path = portable_path(
        Path::new(&fixture.project.project_path),
        &committed.content_uri,
    );

    remove_sqlite_files(&fixture.index_path);
    let rebuilt = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect("rebuild index");
    assert_eq!(rebuilt.artifacts_indexed, 1);
    assert_eq!(rebuilt.missing_content_count, 0);
    assert_storage_contract(&rebuilt);

    fs::remove_file(&content_path).expect("remove content");
    let missing = fixture
        .storage
        .verify_artifact(&fixture.project.project_path, &artifact_id)
        .expect("verify missing content");
    assert_eq!(
        missing.status,
        ArtifactVerificationStatusData::MissingContent
    );
    assert_storage_contract(&missing);
    let rebuilt_missing = fixture
        .storage
        .rebuild_project_index(&fixture.project.project_path, &fixture.project.project_id)
        .expect("rebuild with missing content");
    assert_eq!(rebuilt_missing.missing_content_count, 1);

    fs::create_dir_all(content_path.parent().expect("content parent")).expect("create parent");
    fs::write(&content_path, b"omega").expect("write corrupted same-length content");
    let corrupt = fixture
        .storage
        .verify_artifact(&fixture.project.project_path, &artifact_id)
        .expect("verify corrupt content");
    assert_eq!(corrupt.status, ArtifactVerificationStatusData::HashMismatch);
    assert_eq!(
        corrupt.actual_byte_length,
        corrupt.expected_byte_length.into()
    );
    assert_storage_contract(&corrupt);

    let oversized = fs::OpenOptions::new()
        .write(true)
        .open(&content_path)
        .expect("open content for sparse resize");
    oversized
        .set_len(64 * 1024 * 1024 + 1)
        .expect("create oversized sparse content");
    drop(oversized);
    let oversized_error = fixture
        .storage
        .verify_artifact(&fixture.project.project_path, &artifact_id)
        .expect_err("large verification must wait for the job queue");
    assert_eq!(oversized_error.code, StorageErrorCode::ArtifactTooLarge);
}

#[test]
fn recent_project_and_job_indexes_are_queryable_and_forget_cascades() {
    let fixture = Fixture::new();
    fixture
        .storage
        .record_recent_project(&fixture.project)
        .expect("record recent project");
    fixture
        .storage
        .upsert_job_summary(
            &fixture.project,
            IndexedJobUpsertData {
                job_id: "job_script_001".to_owned(),
                stage_run_id: "run_script_001".to_owned(),
                stage_id: "script".to_owned(),
                status: IndexedJobStatusData::Running,
                attempt: 1,
                progress: 0.5,
                message: Some("正在生成脚本".to_owned()),
                created_at: "2026-07-16T08:20:00.0001Z".to_owned(),
                updated_at: "2026-07-16T08:20:05.0001Z".to_owned(),
            },
        )
        .expect("upsert job");
    fixture
        .storage
        .upsert_job_summary(
            &fixture.project,
            IndexedJobUpsertData {
                job_id: "job_script_002".to_owned(),
                stage_run_id: "run_script_002".to_owned(),
                stage_id: "script".to_owned(),
                status: IndexedJobStatusData::Running,
                attempt: 1,
                progress: 0.25,
                message: Some("same-second newer job".to_owned()),
                created_at: "2026-07-16T08:20:00.0002Z".to_owned(),
                updated_at: "2026-07-16T08:20:05.0002Z".to_owned(),
            },
        )
        .expect("upsert same-second newer job");

    let running = fixture
        .storage
        .list_indexed_jobs(ListIndexedJobsOptions {
            owner_project_id: Some(fixture.project.project_id.clone()),
            statuses: vec![IndexedJobStatusData::Running],
            limit: 20,
        })
        .expect("list jobs");
    assert_eq!(running.jobs.len(), 2);
    assert_eq!(running.jobs[0].job_id, "job_script_002");
    assert_eq!(running.jobs[1].job_id, "job_script_001");
    assert_eq!(running.jobs[1].progress, 0.5);
    assert_eq!(running.jobs[0].updated_at, "2026-07-16T08:20:05.000200000Z");
    assert_storage_contract(&running);

    let connection = Connection::open(&fixture.index_path).expect("open index for query plan");
    let mut plan = connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT owner_project_id, job_id FROM job_summaries \
             ORDER BY updated_at DESC, job_id ASC LIMIT 20",
        )
        .expect("prepare indexed ordering plan");
    let details = plan
        .query_map([], |row| row.get::<_, String>(3))
        .expect("query ordering plan")
        .collect::<Result<Vec<_>, _>>()
        .expect("read ordering plan");
    assert!(details
        .iter()
        .any(|detail| detail.contains("job_summaries_updated_idx")));
    assert!(!details
        .iter()
        .any(|detail| detail.contains("USE TEMP B-TREE")));

    fixture
        .storage
        .upsert_job_summary(
            &fixture.project,
            IndexedJobUpsertData {
                job_id: "job_script_001".to_owned(),
                stage_run_id: "run_script_001".to_owned(),
                stage_id: "script".to_owned(),
                status: IndexedJobStatusData::Succeeded,
                attempt: 1,
                progress: 1.0,
                message: Some("完成".to_owned()),
                created_at: "2026-07-16T08:20:00Z".to_owned(),
                updated_at: "2026-07-16T08:20:12Z".to_owned(),
            },
        )
        .expect("finish job");
    let finished = fixture
        .storage
        .list_indexed_jobs(ListIndexedJobsOptions {
            owner_project_id: None,
            statuses: vec![IndexedJobStatusData::Succeeded],
            limit: 20,
        })
        .expect("list completed jobs");
    assert_eq!(finished.jobs.len(), 1);

    let forgotten = fixture
        .storage
        .forget_project(&fixture.project.project_id)
        .expect("forget project");
    assert!(forgotten.removed);
    assert_storage_contract(&forgotten);
    assert!(fixture
        .storage
        .list_recent_projects(20, true)
        .expect("list recents")
        .projects
        .is_empty());
    assert!(fixture
        .storage
        .list_indexed_jobs(ListIndexedJobsOptions {
            owner_project_id: None,
            statuses: Vec::new(),
            limit: 20,
        })
        .expect("list jobs after forget")
        .jobs
        .is_empty());
}

#[test]
fn recent_projects_scan_past_any_number_of_missing_paths() {
    let fixture = Fixture::new();
    fixture
        .storage
        .record_recent_project(&fixture.project)
        .expect("record available recent project");

    let connection = Connection::open(&fixture.index_path).expect("open recent project index");
    connection
        .execute(
            "UPDATE recent_projects SET last_opened_at = '2020-01-01T00:00:00Z' \
             WHERE project_id = ?1",
            rusqlite::params![fixture.project.project_id],
        )
        .expect("make available project older than missing fixtures");
    for index in 0..6 {
        let missing_path = fixture
            .imports
            .path()
            .join(format!("missing-project-{index}"));
        connection
            .execute(
                "INSERT INTO recent_projects (\
                    project_id, project_path, name, workflow_definition_id, \
                    project_format_version, archived, last_opened_at, marker_updated_at\
                 ) VALUES (?1, ?2, ?3, ?4, 1, 0, ?5, ?6)",
                rusqlite::params![
                    format!("project_missing_{index}"),
                    missing_path.to_string_lossy().into_owned(),
                    format!("缺失项目 {index}"),
                    "workflow_standard_v1",
                    format!("2030-01-01T00:00:{index:02}Z"),
                    "2030-01-01T00:00:00Z",
                ],
            )
            .expect("insert missing recent project fixture");
    }
    drop(connection);

    let result = fixture
        .storage
        .list_recent_projects(1, false)
        .expect("filter all missing rows before the available project");
    assert_eq!(result.projects.len(), 1);
    assert_eq!(result.projects[0].project_id, fixture.project.project_id);
    assert!(result.projects[0].path_available);
    assert_storage_contract(&result);
}

#[test]
fn copied_project_rebuild_keeps_document_provenance_but_uses_new_owner_identity() {
    let fixture = Fixture::new();
    let source = fixture.write_source("script.md", b"copy me");
    let committed = fixture.import(&source, generated_draft("non_evidence"));
    let source_artifact_id = artifact_id(&committed.artifact);
    let copy = fixture
        .project_service
        .copy_project(CopyProjectOptions {
            source_project_path: fixture.project.project_path.clone(),
            destination_parent_path: fixture._temp.path().to_string_lossy().into_owned(),
            directory_name: "copy".to_owned(),
            name: "副本".to_owned(),
        })
        .expect("copy project");

    let rebuilt = fixture
        .storage
        .rebuild_project_index(&copy.project.project_path, &copy.project.project_id)
        .expect("rebuild copied project");
    assert_eq!(rebuilt.artifacts_indexed, 1);
    let inherited = fixture
        .storage
        .get_artifact(&copy.project.project_path, &source_artifact_id)
        .expect("read inherited artifact");
    assert_eq!(inherited.owner_project_id, copy.project.project_id);
    assert_eq!(
        inherited.artifact["projectId"].as_str(),
        Some(fixture.project.project_id.as_str())
    );

    let connection = Connection::open(&fixture.index_path).expect("open index");
    let (owner, document): (String, String) = connection
        .query_row(
            "SELECT owner_project_id, document_project_id FROM artifacts \
             WHERE owner_project_id = ?1 AND artifact_id = ?2",
            rusqlite::params![copy.project.project_id, source_artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query copied artifact index");
    assert_eq!(owner, copy.project.project_id);
    assert_eq!(document, fixture.project.project_id);
}

#[test]
fn cache_cleanup_is_bounded_to_the_cache_tree_and_rejects_links() {
    let fixture = Fixture::new();
    let cache = Path::new(&fixture.project.project_path).join("cache");
    fs::create_dir_all(cache.join("nested")).expect("create cache tree");
    fs::write(cache.join("a.bin"), b"abc").expect("write cache file");
    fs::write(cache.join("nested/b.bin"), b"12345").expect("write nested cache file");

    let cleaned = fixture
        .storage
        .clean_project_cache(&fixture.project.project_path, &fixture.project.project_id)
        .expect("clean cache");
    assert_eq!(cleaned.entries_removed, 3);
    assert_eq!(cleaned.bytes_removed, 8);
    assert!(fs::read_dir(&cache)
        .expect("read empty cache")
        .next()
        .is_none());
    assert_storage_contract(&cleaned);

    let outside = fixture.imports.path().join("outside.txt");
    fs::write(&outside, b"outside").expect("write outside file");
    let link = cache.join("outside-link");
    match create_file_symlink(&outside, &link) {
        Ok(()) => {
            let error = fixture
                .storage
                .clean_project_cache(&fixture.project.project_path, &fixture.project.project_id)
                .expect_err("cache symlink must fail");
            assert_eq!(error.code, StorageErrorCode::PathContainsSymlink);
            assert_eq!(fs::read(&outside).expect("outside remains"), b"outside");
        }
        Err(error)
            if cfg!(windows)
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                ) => {}
        Err(error) => panic!("create cache symlink fixture: {error}"),
    }
}

#[test]
fn content_address_target_link_is_never_followed_or_replaced() {
    let fixture = Fixture::new();
    let source = fixture.write_source("linked-content.bin", b"same bytes");
    let committed = fixture.import(&source, generated_draft("non_evidence"));
    let content_path = portable_path(
        Path::new(&fixture.project.project_path),
        &committed.content_uri,
    );
    fs::remove_file(&content_path).expect("remove original content");
    let outside = fixture.imports.path().join("outside-content.bin");
    fs::write(&outside, b"same bytes").expect("write outside content");

    match create_file_symlink(&outside, &content_path) {
        Ok(()) => {
            let error = fixture
                .storage
                .import_artifact_file(StoreArtifactFileOptions {
                    project_path: fixture.project.project_path.clone(),
                    expected_project_id: fixture.project.project_id.clone(),
                    source_path: source.to_string_lossy().into_owned(),
                    artifact: generated_draft("non_evidence"),
                })
                .expect_err("content-address link must fail");
            assert_eq!(error.code, StorageErrorCode::PathContainsSymlink);
            assert_eq!(fs::read(&outside).expect("outside remains"), b"same bytes");
        }
        Err(error)
            if cfg!(windows)
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                ) => {}
        Err(error) => panic!("create content symlink fixture: {error}"),
    }
}

#[test]
fn artifact_reads_reject_every_linked_intermediate_directory() {
    let mut exercised = 0_usize;
    for case in ["artifacts", "metadata", "objects", "sha256", "hash-prefix"] {
        let fixture = Fixture::new();
        let source = fixture.write_source("linked-intermediate.bin", b"safe project bytes");
        let committed = fixture.import(&source, generated_draft("non_evidence"));
        let committed_artifact_id = artifact_id(&committed.artifact);
        let project_dir = Path::new(&fixture.project.project_path);
        let selected = match case {
            "artifacts" => project_dir.join("artifacts"),
            "metadata" => project_dir.join("artifacts/metadata"),
            "objects" => project_dir.join("artifacts/objects"),
            "sha256" => project_dir.join("artifacts/objects/sha256"),
            "hash-prefix" => portable_path(project_dir, &committed.content_uri)
                .parent()
                .expect("content hash prefix")
                .to_path_buf(),
            _ => unreachable!("known intermediate-link case"),
        };
        let outside = fixture.imports.path().join(format!("outside-{case}"));
        fs::rename(&selected, &outside).expect("move real directory outside the project path");

        match create_dir_symlink(&outside, &selected) {
            Ok(()) => {
                exercised += 1;
                let error = if matches!(case, "artifacts" | "metadata") {
                    fixture
                        .storage
                        .get_artifact(&fixture.project.project_path, &committed_artifact_id)
                        .expect_err("metadata path must reject an intermediate directory link")
                } else {
                    fixture
                        .storage
                        .verify_artifact(&fixture.project.project_path, &committed_artifact_id)
                        .expect_err("content path must reject an intermediate directory link")
                };
                assert_eq!(
                    error.code,
                    StorageErrorCode::PathContainsSymlink,
                    "unexpected error for linked {case} directory"
                );
                remove_dir_symlink(&selected).expect("remove intermediate directory link");
                fs::rename(&outside, &selected).expect("restore real project directory");
            }
            Err(error)
                if cfg!(windows)
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                    ) =>
            {
                fs::rename(&outside, &selected).expect("restore skipped symlink fixture");
            }
            Err(error) => panic!("create {case} directory symlink fixture: {error}"),
        }
    }
    if !cfg!(windows) {
        assert_eq!(exercised, 5);
    }
}

#[test]
fn future_sqlite_index_version_is_rejected_without_downgrade() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.index_path.parent().expect("index parent"))
        .expect("create index parent");
    let connection = Connection::open(&fixture.index_path).expect("open index");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
        .expect("set delete journal mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "delete");
    connection
        .pragma_update(None, "user_version", 3)
        .expect("set future version");
    drop(connection);

    let error = fixture
        .storage
        .list_recent_projects(20, true)
        .expect_err("future index must fail");
    assert_eq!(error.code, StorageErrorCode::IndexMigrationFailed);

    let connection = Connection::open(&fixture.index_path).expect("reopen future index");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read unchanged journal mode");
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read unchanged user version");
    assert_eq!(journal_mode.to_ascii_lowercase(), "delete");
    assert_eq!(user_version, 3);
}

#[test]
fn sqlite_v1_migration_discards_only_rebuildable_job_summaries() {
    let fixture = Fixture::new();
    fixture
        .storage
        .record_recent_project(&fixture.project)
        .expect("create current index");
    fixture
        .storage
        .upsert_job_summary(
            &fixture.project,
            IndexedJobUpsertData {
                job_id: "job_legacy_001".to_owned(),
                stage_run_id: "run_legacy_001".to_owned(),
                stage_id: "brief".to_owned(),
                status: IndexedJobStatusData::Running,
                attempt: 1,
                progress: 0.5,
                message: Some("legacy summary".to_owned()),
                created_at: "2026-07-16T08:20:00Z".to_owned(),
                updated_at: "2026-07-16T08:20:05.0001Z".to_owned(),
            },
        )
        .expect("insert legacy summary");

    let connection = Connection::open(&fixture.index_path).expect("open current index");
    connection
        .execute_batch(
            "DROP INDEX job_summaries_updated_idx;\
             CREATE INDEX job_summaries_updated_idx ON job_summaries(updated_at DESC);\
             UPDATE job_summaries SET updated_at = '2026-07-16T08:20:05.0001Z';\
             PRAGMA user_version = 1;",
        )
        .expect("downgrade fixture to the shipped v1 shape");
    drop(connection);

    let migrated = fixture
        .storage
        .list_indexed_jobs(ListIndexedJobsOptions {
            owner_project_id: None,
            statuses: Vec::new(),
            limit: 20,
        })
        .expect("open and migrate v1 index");
    assert!(migrated.jobs.is_empty(), "job summaries are disposable");
    assert_eq!(
        fixture
            .storage
            .list_recent_projects(20, true)
            .expect("recent projects survive migration")
            .projects
            .len(),
        1
    );

    let connection = Connection::open(&fixture.index_path).expect("inspect migrated index");
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read migrated version");
    let index_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
            ["job_summaries_updated_idx"],
            |row| row.get(0),
        )
        .expect("read migrated index definition");
    assert_eq!(user_version, 2);
    assert!(index_sql.contains("updated_at DESC, job_id ASC"));
}

fn generated_draft(evidence_role: &str) -> ArtifactDraft {
    serde_json::from_value(json!({
        "stageId": "script",
        "runId": "run_script_001",
        "kind": "script",
        "mediaType": "text/markdown",
        "evidenceRole": evidence_role,
        "source": {
            "origin": "generated",
            "providerId": "openai",
            "model": "example-model"
        },
        "provenance": []
    }))
    .expect("deserialize artifact draft")
}

fn imported_draft() -> ArtifactDraft {
    serde_json::from_value(json!({
        "stageId": "research",
        "runId": "run_research_001",
        "kind": "source_image",
        "mediaType": "image/png",
        "evidenceRole": "factual_evidence",
        "source": {
            "origin": "imported",
            "sourceUri": "https://example.com/source.png",
            "author": "Example Author",
            "license": "CC-BY-4.0",
            "attributionText": "Example Author / CC-BY-4.0",
            "authorizationRecordIds": ["authorization_001"]
        },
        "provenance": [{
            "claimId": "claim_001",
            "evidenceRef": "evidence_001"
        }]
    }))
    .expect("deserialize imported artifact draft")
}

fn derived_draft(source_artifact_id: &str) -> ArtifactDraft {
    serde_json::from_value(json!({
        "stageId": "scene_plan",
        "runId": "run_scene_plan_001",
        "kind": "scene_plan",
        "mediaType": "application/json",
        "evidenceRole": "non_evidence",
        "source": {
            "origin": "derived",
            "sourceArtifactIds": [source_artifact_id]
        },
        "provenance": []
    }))
    .expect("deserialize derived artifact draft")
}

fn artifact_id(artifact: &Value) -> String {
    artifact["artifactId"]
        .as_str()
        .expect("artifact id")
        .to_owned()
}

fn test_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

fn assert_redacted_media_source_error(
    error: narracut_core::StorageServiceError,
    expected_code: StorageErrorCode,
    external_canary: &str,
) {
    assert_eq!(error.code, expected_code);
    assert!(error.path.is_none());
    assert!(!error.message.contains(external_canary));
    assert!(!format!("{error:?}").contains(external_canary));
}

fn portable_path(root: &Path, uri: &str) -> std::path::PathBuf {
    uri.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn remove_sqlite_files(index_path: &Path) {
    for path in [
        index_path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", index_path.to_string_lossy())),
        std::path::PathBuf::from(format!("{}-shm", index_path.to_string_lossy())),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove SQLite fixture {}: {error}", path.display()),
        }
    }
}

fn assert_storage_contract<T: Serialize>(value: &T) {
    let value = serde_json::to_value(value).expect("serialize storage response");
    validate_storage_command_message(&value).unwrap_or_else(|error| {
        panic!("storage response must follow command schema: {error}; value={value}")
    });
}

fn write_json_fixture(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("serialize JSON fixture"),
    )
    .unwrap_or_else(|error| panic!("write JSON fixture {}: {error}", path.display()));
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn remove_dir_symlink(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}

#[cfg(unix)]
fn remove_dir_symlink(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}
