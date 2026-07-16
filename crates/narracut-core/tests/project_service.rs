use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use narracut_contracts::{validate_contract_document, validate_project_command_message};
use narracut_core::{
    CopyProjectOptions, CreateProjectOptions, ProjectErrorCode, ProjectMigrationStatusData,
    ProjectService, TrashBackend, PROJECT_MARKER_FILE,
};
use serde::Serialize;
use serde_json::{json, Value};
use tempfile::TempDir;

#[test]
fn create_open_rename_and_archive_keep_a_valid_marker() {
    let temp = tempfile::tempdir().expect("temp directory");
    let service = ProjectService::default();
    let created = create_demo_project(&service, &temp, "demo", "演示项目");

    assert_eq!(created.name, "演示项目");
    assert!(!created.archived);
    assert!(created.archived_at.is_none());
    assert!(Path::new(&created.marker_path).is_file());
    for directory in [
        "sources",
        "contracts",
        "stages",
        "runs",
        "artifacts",
        "assets",
        "cache",
        "exports",
        "manifests",
        "logs",
        "backups/migrations",
    ] {
        assert!(Path::new(&created.project_path).join(directory).is_dir());
    }
    assert_command_contract(&created);
    assert_marker_contract(Path::new(&created.marker_path));

    let opened = service
        .open_project(&created.project_path)
        .expect("created project opens");
    assert_eq!(opened.project_id, created.project_id);

    let renamed = service
        .rename_project(&created.project_path, "  新名称  ")
        .expect("project renames");
    assert_eq!(renamed.name, "新名称");
    assert_eq!(renamed.project_path, created.project_path);
    assert_command_contract(&renamed);

    let archived = service
        .set_project_archived(&created.project_path, true)
        .expect("project archives");
    assert!(archived.archived);
    assert!(archived.archived_at.is_some());
    assert_command_contract(&archived);

    let restored = service
        .set_project_archived(&created.project_path, false)
        .expect("project restores");
    assert!(!restored.archived);
    assert!(restored.archived_at.is_none());
    assert_marker_contract(Path::new(&restored.marker_path));
}

#[test]
fn inspect_then_migrate_v0_creates_backup_and_is_conflict_safe() {
    let temp = tempfile::tempdir().expect("temp directory");
    let legacy_dir = temp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).expect("legacy directory");
    let legacy = json!({
        "projectFormatVersion": 0,
        "projectId": "project_legacy",
        "name": "旧项目",
        "workflowDefinitionId": "workflow_standard_v1",
        "stages": [],
        "createdAt": "2026-07-16T08:00:00Z",
        "updatedAt": "2026-07-16T08:30:00Z",
        "archived": true,
        "archivedAt": "2026-07-16T08:20:00Z"
    });
    let legacy_bytes = write_pretty_json(&legacy_dir.join(PROJECT_MARKER_FILE), &legacy);
    let service = ProjectService::default();

    let inspection = service
        .inspect_project(&legacy_dir)
        .expect("legacy project is inspectable");
    assert_eq!(inspection.detected_format_version, 0);
    assert!(inspection.project.is_none());
    assert_eq!(
        inspection.migration,
        ProjectMigrationStatusData::Required {
            from_version: 0,
            to_version: 1,
            steps: vec!["project-v0-to-v1".to_owned()],
        }
    );
    assert_command_contract(&inspection);

    let open_error = service
        .open_project(&legacy_dir)
        .expect_err("legacy project cannot open before migration");
    assert_eq!(open_error.code, ProjectErrorCode::MigrationRequired);

    let migrated = service
        .migrate_project(&legacy_dir, 0)
        .expect("legacy project migrates");
    assert_eq!(migrated.from_version, 0);
    assert_eq!(migrated.to_version, 1);
    assert!(migrated.project.archived);
    assert!(Path::new(&migrated.backup_path).is_file());
    assert_eq!(
        fs::read(&migrated.backup_path).expect("read migration backup"),
        legacy_bytes
    );
    assert_command_contract(&migrated);
    assert_marker_contract(Path::new(&migrated.project.marker_path));

    let conflict = service
        .migrate_project(&legacy_dir, 0)
        .expect_err("stale migration confirmation is rejected");
    assert_eq!(conflict.code, ProjectErrorCode::MigrationConflict);
    assert_eq!(conflict.expected_version, Some(0));
    assert_eq!(conflict.detected_version, Some(1));
}

#[test]
fn newer_project_format_is_inspectable_but_never_opened_or_mutated() {
    let temp = tempfile::tempdir().expect("temp directory");
    let service = ProjectService::default();
    let project = create_demo_project(&service, &temp, "future", "未来项目");
    let marker_path = Path::new(&project.marker_path);
    let mut marker: Value =
        serde_json::from_slice(&fs::read(marker_path).expect("read marker")).expect("marker JSON");
    marker["projectFormatVersion"] = json!(2);
    write_pretty_json(marker_path, &marker);

    let inspection = service
        .inspect_project(&project.project_path)
        .expect("future project can be inspected");
    assert_eq!(inspection.detected_format_version, 2);
    assert_eq!(
        inspection.migration,
        ProjectMigrationStatusData::UnsupportedNewer {
            detected_version: 2,
            supported_version: 1,
        }
    );
    assert_command_contract(&inspection);

    let open_error = service
        .open_project(&project.project_path)
        .expect_err("future project cannot open");
    assert_eq!(open_error.code, ProjectErrorCode::UnsupportedNewerVersion);
    let rename_error = service
        .rename_project(&project.project_path, "不能修改")
        .expect_err("future project cannot be modified");
    assert_eq!(rename_error.code, ProjectErrorCode::UnsupportedNewerVersion);
}

#[test]
fn create_rejects_traversal_reserved_names_and_existing_destinations() {
    let temp = tempfile::tempdir().expect("temp directory");
    let service = ProjectService::default();

    for invalid in [
        "../outside",
        "CON",
        "name.",
        "a/b",
        "  ",
        "demo ",
        " demo",
        "\tdemo",
    ] {
        let error = service
            .create_project(create_options(&temp, invalid, "无效项目"))
            .expect_err("unsafe directory name must fail");
        assert_eq!(error.code, ProjectErrorCode::InvalidName, "{invalid}");
    }
    assert!(!temp.path().join("outside").exists());

    create_demo_project(&service, &temp, "exists", "已存在");
    let error = service
        .create_project(create_options(&temp, "exists", "重复项目"))
        .expect_err("existing destination must fail");
    assert_eq!(error.code, ProjectErrorCode::DestinationExists);
}

#[test]
fn oversized_marker_is_rejected_before_json_allocation() {
    let temp = tempfile::tempdir().expect("temp directory");
    let project_dir = temp.path().join("oversized");
    fs::create_dir(&project_dir).expect("project directory");
    fs::write(
        project_dir.join(PROJECT_MARKER_FILE),
        vec![b' '; 1024 * 1024 + 1],
    )
    .expect("write oversized marker");

    let error = ProjectService::default()
        .inspect_project(&project_dir)
        .expect_err("oversized marker must fail");
    assert_eq!(error.code, ProjectErrorCode::MarkerTooLarge);
}

#[test]
fn marker_symlink_is_rejected_when_the_platform_allows_the_fixture() {
    let temp = tempfile::tempdir().expect("temp directory");
    let project_dir = temp.path().join("linked");
    fs::create_dir(&project_dir).expect("project directory");
    let target = temp.path().join("real-marker.json");
    write_pretty_json(
        &target,
        &json!({
            "projectFormatVersion": 0,
            "projectId": "project_linked",
            "name": "链接项目",
            "workflowDefinitionId": "workflow_standard_v1",
            "stages": [],
            "createdAt": "2026-07-16T08:00:00Z",
            "updatedAt": "2026-07-16T08:00:00Z"
        }),
    );
    if let Err(error) = create_file_symlink(&target, &project_dir.join(PROJECT_MARKER_FILE)) {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("create symlink fixture: {error}");
    }

    let error = ProjectService::default()
        .inspect_project(&project_dir)
        .expect_err("linked marker must fail");
    assert_eq!(error.code, ProjectErrorCode::PathContainsSymlink);
}

#[test]
fn migration_backup_directory_rejects_links_without_writing_outside_project() {
    let temp = tempfile::tempdir().expect("temp directory");
    let legacy_dir = temp.path().join("legacy-linked-backup");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&legacy_dir).expect("legacy directory");
    fs::create_dir_all(&outside).expect("outside directory");
    write_pretty_json(
        &legacy_dir.join(PROJECT_MARKER_FILE),
        &legacy_project_value("project_linked_backup"),
    );
    if let Err(error) = create_dir_symlink(&outside, &legacy_dir.join("backups")) {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("create directory symlink fixture: {error}");
    }

    let error = ProjectService::default()
        .migrate_project(&legacy_dir, 0)
        .expect_err("linked migration backup directory must fail");
    assert_eq!(error.code, ProjectErrorCode::PathContainsSymlink);
    assert_eq!(
        fs::read_dir(&outside)
            .expect("read outside directory")
            .count(),
        0
    );
    let marker: Value = serde_json::from_slice(
        &fs::read(legacy_dir.join(PROJECT_MARKER_FILE)).expect("read unchanged marker"),
    )
    .expect("marker JSON");
    assert_eq!(marker["projectFormatVersion"], 0);
}

#[test]
fn copy_rebinds_only_mutable_config_and_preserves_immutable_history_bytes() {
    let temp = tempfile::tempdir().expect("temp directory");
    let destination_root = tempfile::tempdir().expect("destination directory");
    let service = ProjectService::default();
    let source = create_demo_project(&service, &temp, "source", "源项目");
    let source_dir = Path::new(&source.project_path);
    let mut source_marker: Value =
        serde_json::from_slice(&fs::read(&source.marker_path).expect("read source marker"))
            .expect("source marker JSON");
    source_marker["stages"] = json!([{
        "stageId": "stage_script",
        "status": "approved",
        "approvedRunId": "run_script",
        "latestRunId": "run_script",
        "staleBecauseStageIds": []
    }]);
    write_pretty_json(Path::new(&source.marker_path), &source_marker);
    let mut stage_config = contract_fixture("stage_config");
    stage_config["projectId"] = json!(source.project_id);
    stage_config["values"] = json!({
        "projectId": source.project_id,
        "tone": "calm"
    });
    write_pretty_json(&source_dir.join("stages/config.json"), &stage_config);

    let mut immutable_documents = Vec::new();
    for (document_type, relative_path) in [
        ("stage_run", "runs/run.json"),
        ("artifact", "artifacts/artifact.json"),
        ("render_manifest", "manifests/render.json"),
    ] {
        let mut document = contract_fixture(document_type);
        document["projectId"] = json!(source.project_id);
        if document_type == "stage_run" {
            document["configSnapshot"]["projectId"] = json!(source.project_id);
            document["configSnapshot"]["values"] = json!({
                "projectId": source.project_id,
                "tone": "snapshot"
            });
        }
        let bytes = write_pretty_json(&source_dir.join(relative_path), &document);
        immutable_documents.push((relative_path, bytes));
    }
    fs::write(source_dir.join("cache/proxy.bin"), b"derived cache").expect("write cache fixture");
    fs::create_dir_all(source_dir.join("artifacts/.tmp/nested"))
        .expect("create artifact temp fixture");
    let crash_leftover = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(source_dir.join("artifacts/.tmp/nested/crash-leftover.blob"))
        .expect("create artifact temp crash leftover");
    crash_leftover
        .set_len(64 * 1024 * 1024 + 1)
        .expect("make ignored temp fixture exceed the synchronous copy byte limit");
    drop(crash_leftover);
    service
        .set_project_archived(&source.project_path, true)
        .expect("archive source");

    let copied = service
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path.clone(),
            destination_parent_path: destination_root.path().to_string_lossy().into_owned(),
            directory_name: "copy".to_owned(),
            name: "项目副本".to_owned(),
        })
        .expect("project copies");

    assert_ne!(copied.project.project_id, source.project_id);
    assert_eq!(copied.source_project_id, source.project_id);
    assert_eq!(copied.history_policy, "preserve_immutable_source_identity");
    assert_eq!(
        copied.project.copied_from_project_id.as_deref(),
        Some(source.project_id.as_str())
    );
    assert!(copied.project.copied_at.is_some());
    assert_eq!(copied.project.name, "项目副本");
    assert!(!copied.project.archived);
    assert!(copied.files_copied >= 2);
    assert!(copied.bytes_copied > 0);
    assert_command_contract(&copied);
    assert!(!Path::new(&copied.project.project_path)
        .join("cache/proxy.bin")
        .exists());
    assert!(Path::new(&copied.project.project_path)
        .join("cache")
        .is_dir());
    let copied_artifact_tmp = Path::new(&copied.project.project_path).join("artifacts/.tmp");
    assert!(copied_artifact_tmp.is_dir());
    assert!(fs::read_dir(&copied_artifact_tmp)
        .expect("read empty copied artifact temp directory")
        .next()
        .is_none());
    assert!(source_dir
        .join("artifacts/.tmp/nested/crash-leftover.blob")
        .exists());

    let copied_config: Value = serde_json::from_slice(
        &fs::read(Path::new(&copied.project.project_path).join("stages/config.json"))
            .expect("read copied stage config"),
    )
    .expect("copied stage config JSON");
    assert_eq!(copied_config["projectId"], json!(copied.project.project_id));
    assert_eq!(
        copied_config["values"]["projectId"],
        json!(source.project_id)
    );
    for (relative_path, expected_bytes) in immutable_documents {
        assert_eq!(
            fs::read(Path::new(&copied.project.project_path).join(relative_path))
                .expect("read immutable copied document"),
            expected_bytes,
            "immutable history changed at {relative_path}"
        );
    }
    let copied_marker: Value =
        serde_json::from_slice(&fs::read(&copied.project.marker_path).expect("read copied marker"))
            .expect("copied marker JSON");
    assert_eq!(
        copied_marker["stages"],
        json!([{
            "stageId": "stage_script",
            "status": "draft",
            "staleBecauseStageIds": []
        }])
    );

    let source_after = service
        .open_project(&source.project_path)
        .expect("source remains readable");
    assert_eq!(source_after.project_id, source.project_id);
    assert!(source_after.archived);
    let source_marker_after: Value = serde_json::from_slice(
        &fs::read(&source.marker_path).expect("read source marker after copy"),
    )
    .expect("source marker JSON after copy");
    assert_eq!(source_marker_after["stages"][0]["status"], "approved");
}

#[test]
fn oversized_copy_requires_the_future_background_job_boundary() {
    let temp = tempfile::tempdir().expect("temp directory");
    let destination_root = tempfile::tempdir().expect("destination directory");
    let service = ProjectService::with_copy_limit(Arc::new(RecordingTrash::default()), 8);
    let source = create_demo_project(&service, &temp, "source", "源项目");

    let error = service
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path,
            destination_parent_path: destination_root.path().to_string_lossy().into_owned(),
            directory_name: "copy".to_owned(),
            name: "项目副本".to_owned(),
        })
        .expect_err("oversized synchronous copy must fail");
    assert_eq!(error.code, ProjectErrorCode::CopyTooLarge);
    assert!(!destination_root.path().join("copy").exists());
}

#[test]
fn bounded_copy_stops_on_entry_and_depth_limits_during_scan() {
    let temp = tempfile::tempdir().expect("temp directory");
    let destination_root = tempfile::tempdir().expect("destination directory");
    let source_service = ProjectService::default();
    let source = create_demo_project(&source_service, &temp, "source", "源项目");
    let source_dir = Path::new(&source.project_path);
    for index in 0..8 {
        fs::create_dir(source_dir.join(format!("sources/empty-{index}")))
            .expect("create empty directory fixture");
    }

    let entry_limited = ProjectService::with_copy_limits(
        Arc::new(RecordingTrash::default()),
        64 * 1024 * 1024,
        2048,
        16,
        64,
    );
    let error = entry_limited
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path.clone(),
            destination_parent_path: destination_root.path().to_string_lossy().into_owned(),
            directory_name: "entry-limited".to_owned(),
            name: "条目超限".to_owned(),
        })
        .expect_err("entry limit must fail during scan");
    assert_eq!(error.code, ProjectErrorCode::CopyTooLarge);
    assert!(!destination_root.path().join("entry-limited").exists());

    fs::write(source_dir.join("sources/file.txt"), b"bounded file")
        .expect("create file-count fixture");
    let file_limited = ProjectService::with_copy_limits(
        Arc::new(RecordingTrash::default()),
        64 * 1024 * 1024,
        1,
        4096,
        64,
    );
    let error = file_limited
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path.clone(),
            destination_parent_path: destination_root.path().to_string_lossy().into_owned(),
            directory_name: "file-limited".to_owned(),
            name: "文件超限".to_owned(),
        })
        .expect_err("file limit must fail during scan");
    assert_eq!(error.code, ProjectErrorCode::CopyTooLarge);
    assert!(!destination_root.path().join("file-limited").exists());

    let deep_path = source_dir.join("sources/deep/a/b/c");
    fs::create_dir_all(&deep_path).expect("create deep fixture");
    let depth_limited = ProjectService::with_copy_limits(
        Arc::new(RecordingTrash::default()),
        64 * 1024 * 1024,
        2048,
        4096,
        2,
    );
    let error = depth_limited
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path,
            destination_parent_path: destination_root.path().to_string_lossy().into_owned(),
            directory_name: "depth-limited".to_owned(),
            name: "深度超限".to_owned(),
        })
        .expect_err("depth limit must fail during scan");
    assert_eq!(error.code, ProjectErrorCode::CopyTooLarge);
    assert!(!destination_root.path().join("depth-limited").exists());
}

#[test]
fn copy_refuses_a_destination_inside_the_source_project() {
    let temp = tempfile::tempdir().expect("temp directory");
    let service = ProjectService::default();
    let source = create_demo_project(&service, &temp, "source", "源项目");

    let error = service
        .copy_project(CopyProjectOptions {
            source_project_path: source.project_path.clone(),
            destination_parent_path: source.project_path.clone(),
            directory_name: "nested-copy".to_owned(),
            name: "嵌套副本".to_owned(),
        })
        .expect_err("nested copy must fail");
    assert_eq!(error.code, ProjectErrorCode::InvalidPath);
    assert!(!Path::new(&source.project_path).join("nested-copy").exists());
}

#[test]
fn move_to_trash_requires_matching_project_identity() {
    let temp = tempfile::tempdir().expect("temp directory");
    let trash = Arc::new(RecordingTrash::default());
    let service = ProjectService::new(trash.clone());
    let project = create_demo_project(&service, &temp, "trash-me", "待删除");

    let mismatch = service
        .move_project_to_trash(&project.project_path, "wrong_project")
        .expect_err("identity mismatch must fail");
    assert_eq!(mismatch.code, ProjectErrorCode::InvalidRequest);
    assert!(trash.paths().is_empty());

    let result = service
        .move_project_to_trash(&project.project_path, &project.project_id)
        .expect("matching project moves to trash backend");
    assert_eq!(result.project_id, project.project_id);
    assert_eq!(trash.paths(), vec![PathBuf::from(&project.project_path)]);
    assert_command_contract(&result);
}

fn create_demo_project(
    service: &ProjectService,
    temp: &TempDir,
    directory_name: &str,
    name: &str,
) -> narracut_core::ProjectDescriptorData {
    service
        .create_project(create_options(temp, directory_name, name))
        .expect("create demo project")
}

fn create_options(temp: &TempDir, directory_name: &str, name: &str) -> CreateProjectOptions {
    CreateProjectOptions {
        parent_path: temp.path().to_string_lossy().into_owned(),
        directory_name: directory_name.to_owned(),
        name: name.to_owned(),
        workflow_definition_id: "workflow_standard_v1".to_owned(),
        default_locale: Some("zh-CN".to_owned()),
    }
}

fn legacy_project_value(project_id: &str) -> Value {
    json!({
        "projectFormatVersion": 0,
        "projectId": project_id,
        "name": "旧项目",
        "workflowDefinitionId": "workflow_standard_v1",
        "stages": [],
        "createdAt": "2026-07-16T08:00:00Z",
        "updatedAt": "2026-07-16T08:30:00Z"
    })
}

fn contract_fixture(document_type: &str) -> Value {
    let documents: Vec<Value> = serde_json::from_str(include_str!(
        "../../../packages/contracts/fixtures/valid-documents.json"
    ))
    .expect("valid contract fixture file");
    documents
        .into_iter()
        .find(|document| {
            document.get("documentType").and_then(Value::as_str) == Some(document_type)
        })
        .unwrap_or_else(|| panic!("missing contract fixture for {document_type}"))
}

fn assert_marker_contract(path: &Path) {
    let value: Value =
        serde_json::from_slice(&fs::read(path).expect("read marker")).expect("marker JSON");
    validate_contract_document(&value).expect("marker follows persistent contract");
}

fn assert_command_contract(value: &impl Serialize) {
    let value = serde_json::to_value(value).expect("serialize command value");
    validate_project_command_message(&value).expect("value follows project command contract");
}

fn write_pretty_json(path: &Path, value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("serialize JSON");
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, &bytes).expect("write JSON fixture");
    bytes
}

#[derive(Default)]
struct RecordingTrash {
    paths: Mutex<Vec<PathBuf>>,
}

impl RecordingTrash {
    fn paths(&self) -> Vec<PathBuf> {
        self.paths.lock().expect("trash lock").clone()
    }
}

impl TrashBackend for RecordingTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), String> {
        self.paths
            .lock()
            .map_err(|error| error.to_string())?
            .push(path.to_path_buf());
        Ok(())
    }
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
