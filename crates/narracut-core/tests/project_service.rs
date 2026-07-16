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

    for invalid in ["../outside", "CON", "name.", "a/b", "  "] {
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
fn copy_creates_new_identity_rewrites_contracts_and_drops_cache_contents() {
    let temp = tempfile::tempdir().expect("temp directory");
    let destination_root = tempfile::tempdir().expect("destination directory");
    let service = ProjectService::default();
    let source = create_demo_project(&service, &temp, "source", "源项目");
    let source_dir = Path::new(&source.project_path);
    let run_path = source_dir.join("runs/run.json");
    let nested_contract = json!({
        "projectId": source.project_id,
        "nested": {
            "projectId": source.project_id
        },
        "external": {
            "projectId": "another_project"
        }
    });
    write_pretty_json(&run_path, &nested_contract);
    fs::write(source_dir.join("cache/proxy.bin"), b"derived cache").expect("write cache fixture");
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

    let copied_contract: Value = serde_json::from_slice(
        &fs::read(Path::new(&copied.project.project_path).join("runs/run.json"))
            .expect("read copied contract"),
    )
    .expect("copied contract JSON");
    assert_eq!(
        copied_contract["projectId"],
        json!(copied.project.project_id)
    );
    assert_eq!(
        copied_contract["nested"]["projectId"],
        json!(copied.project.project_id)
    );
    assert_eq!(
        copied_contract["external"]["projectId"],
        json!("another_project")
    );

    let source_after = service
        .open_project(&source.project_path)
        .expect("source remains readable");
    assert_eq!(source_after.project_id, source.project_id);
    assert!(source_after.archived);
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

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
