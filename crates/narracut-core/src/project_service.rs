use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use atomic_write_file::AtomicWriteFile;
use narracut_contracts::validate_contract_document;
use serde_json::{json, Map, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::{
    CopyProjectOptions, CreateProjectOptions, ProjectCopyResultData, ProjectDescriptorData,
    ProjectErrorCode, ProjectInspectionData, ProjectMigrationResultData,
    ProjectMigrationStatusData, ProjectOperation, ProjectServiceError, ProjectTrashResultData,
    CURRENT_PROJECT_FORMAT_VERSION, PROJECT_COMMAND_API_VERSION, PROJECT_MARKER_FILE,
};

const MAX_MARKER_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_SYNCHRONOUS_COPY_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_SYNCHRONOUS_COPY_FILES: u64 = 2048;
const DEFAULT_MAX_SYNCHRONOUS_COPY_ENTRIES: u64 = 4096;
const DEFAULT_MAX_SYNCHRONOUS_COPY_DEPTH: usize = 64;
const MAX_REWRITABLE_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MIGRATION_V0_TO_V1: &str = "project-v0-to-v1";
const COPY_HISTORY_POLICY: &str = "preserve_immutable_source_identity";
const PROJECT_DIRECTORIES: &[&str] = &[
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
];
pub trait TrashBackend: Send + Sync {
    fn move_to_trash(&self, path: &Path) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct OsTrashBackend;

impl TrashBackend for OsTrashBackend {
    fn move_to_trash(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct ProjectService {
    inner: Arc<ProjectServiceInner>,
}

struct ProjectServiceInner {
    operation_lock: Mutex<()>,
    trash_backend: Arc<dyn TrashBackend>,
    max_synchronous_copy_bytes: u64,
    max_synchronous_copy_files: u64,
    max_synchronous_copy_entries: u64,
    max_synchronous_copy_depth: usize,
}

impl Default for ProjectService {
    fn default() -> Self {
        Self::new(Arc::new(OsTrashBackend))
    }
}

impl ProjectService {
    pub fn new(trash_backend: Arc<dyn TrashBackend>) -> Self {
        Self {
            inner: Arc::new(ProjectServiceInner {
                operation_lock: Mutex::new(()),
                trash_backend,
                max_synchronous_copy_bytes: DEFAULT_MAX_SYNCHRONOUS_COPY_BYTES,
                max_synchronous_copy_files: DEFAULT_MAX_SYNCHRONOUS_COPY_FILES,
                max_synchronous_copy_entries: DEFAULT_MAX_SYNCHRONOUS_COPY_ENTRIES,
                max_synchronous_copy_depth: DEFAULT_MAX_SYNCHRONOUS_COPY_DEPTH,
            }),
        }
    }

    pub fn with_copy_limit(
        trash_backend: Arc<dyn TrashBackend>,
        max_synchronous_copy_bytes: u64,
    ) -> Self {
        Self {
            inner: Arc::new(ProjectServiceInner {
                operation_lock: Mutex::new(()),
                trash_backend,
                max_synchronous_copy_bytes,
                max_synchronous_copy_files: DEFAULT_MAX_SYNCHRONOUS_COPY_FILES,
                max_synchronous_copy_entries: DEFAULT_MAX_SYNCHRONOUS_COPY_ENTRIES,
                max_synchronous_copy_depth: DEFAULT_MAX_SYNCHRONOUS_COPY_DEPTH,
            }),
        }
    }

    pub fn with_copy_limits(
        trash_backend: Arc<dyn TrashBackend>,
        max_synchronous_copy_bytes: u64,
        max_synchronous_copy_files: u64,
        max_synchronous_copy_entries: u64,
        max_synchronous_copy_depth: usize,
    ) -> Self {
        Self {
            inner: Arc::new(ProjectServiceInner {
                operation_lock: Mutex::new(()),
                trash_backend,
                max_synchronous_copy_bytes,
                max_synchronous_copy_files,
                max_synchronous_copy_entries,
                max_synchronous_copy_depth,
            }),
        }
    }

    pub fn inspect_project(
        &self,
        project_path: impl AsRef<Path>,
    ) -> Result<ProjectInspectionData, ProjectServiceError> {
        let _guard = self.operation_guard();
        self.inspect_project_unlocked(project_path.as_ref(), ProjectOperation::Inspect)
    }

    pub fn open_project(
        &self,
        project_path: impl AsRef<Path>,
    ) -> Result<ProjectDescriptorData, ProjectServiceError> {
        let _guard = self.operation_guard();
        self.open_project_unlocked(project_path.as_ref(), ProjectOperation::Open)
    }

    pub fn create_project(
        &self,
        options: CreateProjectOptions,
    ) -> Result<ProjectDescriptorData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::Create;
        let parent = canonical_existing_directory(Path::new(&options.parent_path), operation)?;
        validate_directory_name(&options.directory_name, operation)?;
        let name = validate_text(&options.name, "项目名称", 160, operation)?;
        let workflow_definition_id = validate_text(
            &options.workflow_definition_id,
            "工作流定义 ID",
            160,
            operation,
        )?;
        let default_locale = options
            .default_locale
            .as_deref()
            .map(|value| validate_text(value, "默认语言", 64, operation))
            .transpose()?;
        let destination = parent.join(&options.directory_name);
        ensure_destination_available(&destination, operation)?;

        let temporary = parent.join(format!(".narracut-create-{}", Uuid::new_v4().simple()));
        let mut pending = PendingDirectory::create(&parent, temporary, operation)?;
        for relative in PROJECT_DIRECTORIES {
            let path = pending.path().join(relative);
            fs::create_dir_all(&path).map_err(|error| {
                ProjectServiceError::io(operation, &path, "创建项目目录失败", &error)
            })?;
        }

        let now = current_timestamp(operation)?;
        let project_id = new_project_id();
        let mut project = json!({
            "schemaVersion": narracut_contracts::NARRACUT_CONTRACT_VERSION,
            "documentType": "project",
            "projectFormatVersion": CURRENT_PROJECT_FORMAT_VERSION,
            "projectId": project_id,
            "name": name,
            "workflowDefinitionId": workflow_definition_id,
            "stages": [],
            "createdAt": now,
            "updatedAt": now,
            "metadata": {
                "archived": false
            }
        });
        if let Some(locale) = default_locale {
            project
                .as_object_mut()
                .expect("project literal is an object")
                .insert("defaultLocale".to_owned(), Value::String(locale));
        }
        validate_current_project(&project, operation, pending.path())?;
        write_json_atomic(
            &pending.path().join(PROJECT_MARKER_FILE),
            &project,
            operation,
        )?;

        fs::rename(pending.path(), &destination).map_err(|error| {
            ProjectServiceError::io(operation, &destination, "提交新项目目录失败", &error)
        })?;
        pending.commit();
        self.open_project_unlocked(&destination, operation)
    }

    pub fn migrate_project(
        &self,
        project_path: impl AsRef<Path>,
        expected_source_format_version: u32,
    ) -> Result<ProjectMigrationResultData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::Migrate;
        let project_dir = canonical_project_directory(project_path.as_ref(), operation)?;
        let marker = read_marker(&project_dir, operation)?;

        if marker.format_version != expected_source_format_version {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::MigrationConflict,
                operation,
                "项目格式在确认迁移后发生了变化，请重新检查。",
            )
            .at_path(&marker.marker_path)
            .with_versions(expected_source_format_version, marker.format_version));
        }

        if marker.format_version > CURRENT_PROJECT_FORMAT_VERSION {
            return Err(unsupported_version_error(
                operation,
                &marker.marker_path,
                marker.format_version,
            ));
        }
        if marker.format_version == CURRENT_PROJECT_FORMAT_VERSION {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::MigrationConflict,
                operation,
                "项目已经是当前格式，不需要重复迁移。",
            )
            .at_path(&marker.marker_path)
            .with_versions(CURRENT_PROJECT_FORMAT_VERSION, marker.format_version));
        }
        if marker.format_version != 0 {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::MigrationFailed,
                operation,
                format!("没有从项目格式 {} 开始的迁移路径。", marker.format_version),
            )
            .at_path(&marker.marker_path)
            .with_versions(CURRENT_PROJECT_FORMAT_VERSION, marker.format_version));
        }

        let migrated = migrate_v0_to_v1(marker.value.clone(), operation, &marker.marker_path)?;
        let backup_dir =
            ensure_safe_project_subdirectory(&project_dir, &["backups", "migrations"], operation)?;
        let backup_path = backup_dir.join(format!(
            "narracut.project.v0.{}.json",
            Uuid::new_v4().simple()
        ));
        write_backup(&backup_path, &marker.raw_bytes, operation)?;
        write_json_atomic(&marker.marker_path, &migrated, operation)?;

        let project = self.open_project_unlocked(&project_dir, operation)?;
        Ok(ProjectMigrationResultData {
            api_version: PROJECT_COMMAND_API_VERSION.to_owned(),
            project,
            from_version: 0,
            to_version: CURRENT_PROJECT_FORMAT_VERSION,
            applied_steps: vec![MIGRATION_V0_TO_V1.to_owned()],
            backup_path: path_to_string(&backup_path),
        })
    }

    pub fn rename_project(
        &self,
        project_path: impl AsRef<Path>,
        new_name: &str,
    ) -> Result<ProjectDescriptorData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::Rename;
        let project_dir = canonical_project_directory(project_path.as_ref(), operation)?;
        let mut marker = current_marker(&project_dir, operation)?;
        let name = validate_text(new_name, "项目名称", 160, operation)?;
        let object = marker
            .value
            .as_object_mut()
            .expect("validated project marker is an object");
        object.insert("name".to_owned(), Value::String(name));
        object.insert(
            "updatedAt".to_owned(),
            Value::String(current_timestamp(operation)?),
        );
        validate_current_project(&marker.value, operation, &marker.marker_path)?;
        write_json_atomic(&marker.marker_path, &marker.value, operation)?;
        self.open_project_unlocked(&project_dir, operation)
    }

    pub fn set_project_archived(
        &self,
        project_path: impl AsRef<Path>,
        archived: bool,
    ) -> Result<ProjectDescriptorData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::SetArchived;
        let project_dir = canonical_project_directory(project_path.as_ref(), operation)?;
        let mut marker = current_marker(&project_dir, operation)?;
        let now = current_timestamp(operation)?;
        let object = marker
            .value
            .as_object_mut()
            .expect("validated project marker is an object");
        let metadata = object
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .expect("validated project metadata is an object");
        metadata.insert("archived".to_owned(), Value::Bool(archived));
        if archived {
            metadata.insert("archivedAt".to_owned(), Value::String(now.clone()));
        } else {
            metadata.remove("archivedAt");
        }
        object.insert("updatedAt".to_owned(), Value::String(now));
        validate_current_project(&marker.value, operation, &marker.marker_path)?;
        write_json_atomic(&marker.marker_path, &marker.value, operation)?;
        self.open_project_unlocked(&project_dir, operation)
    }

    pub fn copy_project(
        &self,
        options: CopyProjectOptions,
    ) -> Result<ProjectCopyResultData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::Copy;
        let source =
            canonical_project_directory(Path::new(&options.source_project_path), operation)?;
        let source_marker = current_marker(&source, operation)?;
        let source_descriptor = descriptor_from_current_marker(&source_marker, operation)?;
        let destination_parent =
            canonical_existing_directory(Path::new(&options.destination_parent_path), operation)?;
        validate_directory_name(&options.directory_name, operation)?;
        let name = validate_text(&options.name, "项目名称", 160, operation)?;
        if destination_parent.starts_with(&source) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "项目副本不能创建在源项目目录内部。",
            )
            .at_path(&destination_parent));
        }

        let destination = destination_parent.join(&options.directory_name);
        ensure_destination_available(&destination, operation)?;
        let scan = scan_copy_entries(
            &source,
            self.inner.max_synchronous_copy_bytes,
            self.inner.max_synchronous_copy_files,
            self.inner.max_synchronous_copy_entries,
            self.inner.max_synchronous_copy_depth,
            operation,
        )?;

        let temporary =
            destination_parent.join(format!(".narracut-copy-{}", Uuid::new_v4().simple()));
        let mut pending = PendingDirectory::create(&destination_parent, temporary, operation)?;
        copy_entries(&source, pending.path(), &scan.entries, operation)?;

        let new_project_id = new_project_id();
        rebind_mutable_stage_configs(
            pending.path(),
            &source_descriptor.project_id,
            &new_project_id,
            operation,
        )?;
        rewrite_copied_marker(
            pending.path(),
            &source_descriptor.project_id,
            &new_project_id,
            &name,
            operation,
        )?;

        fs::rename(pending.path(), &destination).map_err(|error| {
            ProjectServiceError::io(operation, &destination, "提交项目副本失败", &error)
        })?;
        pending.commit();
        let project = self.open_project_unlocked(&destination, operation)?;

        Ok(ProjectCopyResultData {
            api_version: PROJECT_COMMAND_API_VERSION.to_owned(),
            project,
            source_project_id: source_descriptor.project_id,
            history_policy: COPY_HISTORY_POLICY.to_owned(),
            files_copied: scan.files,
            bytes_copied: scan.bytes,
        })
    }

    pub fn move_project_to_trash(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
    ) -> Result<ProjectTrashResultData, ProjectServiceError> {
        let _guard = self.operation_guard();
        let operation = ProjectOperation::MoveToTrash;
        let project_dir = canonical_project_directory(project_path.as_ref(), operation)?;
        if project_dir.parent().is_none() {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "文件系统根目录不能作为项目移入回收站。",
            )
            .at_path(&project_dir));
        }
        let descriptor = self.open_project_unlocked(&project_dir, operation)?;
        if descriptor.project_id != expected_project_id {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidRequest,
                operation,
                "项目身份确认失败，已拒绝移动到回收站。",
            )
            .at_path(&project_dir));
        }

        self.inner
            .trash_backend
            .move_to_trash(&project_dir)
            .map_err(|message| {
                ProjectServiceError::new(
                    ProjectErrorCode::TrashFailed,
                    operation,
                    format!("移动项目到系统回收站失败：{message}"),
                )
                .at_path(&project_dir)
            })?;

        Ok(ProjectTrashResultData {
            api_version: PROJECT_COMMAND_API_VERSION.to_owned(),
            project_id: descriptor.project_id,
            trashed_path: path_to_string(&project_dir),
        })
    }

    fn inspect_project_unlocked(
        &self,
        project_path: &Path,
        operation: ProjectOperation,
    ) -> Result<ProjectInspectionData, ProjectServiceError> {
        let project_dir = canonical_project_directory(project_path, operation)?;
        let marker = read_marker(&project_dir, operation)?;
        let (migration, project) = match marker.format_version {
            CURRENT_PROJECT_FORMAT_VERSION => {
                validate_current_project(&marker.value, operation, &marker.marker_path)?;
                let descriptor = descriptor_from_current_marker(&marker, operation)?;
                (
                    ProjectMigrationStatusData::Current {
                        format_version: CURRENT_PROJECT_FORMAT_VERSION,
                    },
                    Some(descriptor),
                )
            }
            0 => {
                validate_legacy_v0(&marker.value, operation, &marker.marker_path)?;
                (
                    ProjectMigrationStatusData::Required {
                        from_version: 0,
                        to_version: CURRENT_PROJECT_FORMAT_VERSION,
                        steps: vec![MIGRATION_V0_TO_V1.to_owned()],
                    },
                    None,
                )
            }
            version => (
                ProjectMigrationStatusData::UnsupportedNewer {
                    detected_version: version,
                    supported_version: CURRENT_PROJECT_FORMAT_VERSION,
                },
                None,
            ),
        };

        Ok(ProjectInspectionData {
            api_version: PROJECT_COMMAND_API_VERSION.to_owned(),
            project_path: path_to_string(&project_dir),
            marker_path: path_to_string(&marker.marker_path),
            detected_format_version: marker.format_version,
            current_format_version: CURRENT_PROJECT_FORMAT_VERSION,
            migration,
            project,
        })
    }

    fn open_project_unlocked(
        &self,
        project_path: &Path,
        operation: ProjectOperation,
    ) -> Result<ProjectDescriptorData, ProjectServiceError> {
        let inspection = self.inspect_project_unlocked(project_path, operation)?;
        match inspection.migration {
            ProjectMigrationStatusData::Current { .. } => inspection.project.ok_or_else(|| {
                ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    "当前格式项目没有可读取的项目描述。",
                )
                .at_path(Path::new(&inspection.project_path))
            }),
            ProjectMigrationStatusData::Required {
                from_version,
                to_version,
                ..
            } => Err(ProjectServiceError::new(
                ProjectErrorCode::MigrationRequired,
                operation,
                "项目格式需要迁移后才能打开。",
            )
            .at_path(Path::new(&inspection.marker_path))
            .with_versions(to_version, from_version)),
            ProjectMigrationStatusData::UnsupportedNewer {
                detected_version,
                supported_version,
            } => Err(ProjectServiceError::new(
                ProjectErrorCode::UnsupportedNewerVersion,
                operation,
                "项目由更新版本的 NarraCut 创建，当前版本拒绝降级打开。",
            )
            .at_path(Path::new(&inspection.marker_path))
            .with_versions(supported_version, detected_version)),
        }
    }

    fn operation_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct MarkerDocument {
    project_dir: PathBuf,
    marker_path: PathBuf,
    raw_bytes: Vec<u8>,
    value: Value,
    format_version: u32,
}

fn current_marker(
    project_dir: &Path,
    operation: ProjectOperation,
) -> Result<MarkerDocument, ProjectServiceError> {
    let marker = read_marker(project_dir, operation)?;
    if marker.format_version < CURRENT_PROJECT_FORMAT_VERSION {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::MigrationRequired,
            operation,
            "项目格式需要迁移后才能修改。",
        )
        .at_path(&marker.marker_path)
        .with_versions(CURRENT_PROJECT_FORMAT_VERSION, marker.format_version));
    }
    if marker.format_version > CURRENT_PROJECT_FORMAT_VERSION {
        return Err(unsupported_version_error(
            operation,
            &marker.marker_path,
            marker.format_version,
        ));
    }
    validate_current_project(&marker.value, operation, &marker.marker_path)?;
    Ok(marker)
}

fn read_marker(
    project_dir: &Path,
    operation: ProjectOperation,
) -> Result<MarkerDocument, ProjectServiceError> {
    let marker_path = project_dir.join(PROJECT_MARKER_FILE);
    let metadata = fs::symlink_metadata(&marker_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProjectServiceError::new(
                ProjectErrorCode::MarkerMissing,
                operation,
                format!("目录中缺少 {PROJECT_MARKER_FILE}。"),
            )
            .at_path(&marker_path)
        } else {
            ProjectServiceError::io(operation, &marker_path, "读取项目标识文件失败", &error)
        }
    })?;
    if metadata_is_link(&metadata) {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::PathContainsSymlink,
            operation,
            "项目标识文件不能是符号链接或重解析点。",
        )
        .at_path(&marker_path));
    }
    if !metadata.is_file() {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            "项目标识路径不是普通文件。",
        )
        .at_path(&marker_path));
    }
    if metadata.len() > MAX_MARKER_BYTES {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::MarkerTooLarge,
            operation,
            format!("项目标识文件超过 {MAX_MARKER_BYTES} 字节上限。"),
        )
        .at_path(&marker_path));
    }

    let raw_bytes = fs::read(&marker_path).map_err(|error| {
        ProjectServiceError::io(operation, &marker_path, "读取项目标识文件失败", &error)
    })?;
    let value = serde_json::from_slice::<Value>(&raw_bytes).map_err(|error| {
        ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            format!("项目标识文件不是合法 JSON：{error}"),
        )
        .at_path(&marker_path)
    })?;
    let format_version = value
        .get("projectFormatVersion")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            ProjectServiceError::new(
                ProjectErrorCode::InvalidProject,
                operation,
                "projectFormatVersion 必须是非负整数。",
            )
            .at_path(&marker_path)
        })?;

    Ok(MarkerDocument {
        project_dir: project_dir.to_path_buf(),
        marker_path,
        raw_bytes,
        value,
        format_version,
    })
}

fn descriptor_from_current_marker(
    marker: &MarkerDocument,
    operation: ProjectOperation,
) -> Result<ProjectDescriptorData, ProjectServiceError> {
    validate_current_project(&marker.value, operation, &marker.marker_path)?;
    let object = marker
        .value
        .as_object()
        .expect("validated project is an object");
    let metadata = object
        .get("metadata")
        .and_then(Value::as_object)
        .expect("validated metadata is an object");

    Ok(ProjectDescriptorData {
        api_version: PROJECT_COMMAND_API_VERSION.to_owned(),
        project_path: path_to_string(&marker.project_dir),
        marker_path: path_to_string(&marker.marker_path),
        project_id: required_string(object, "projectId", operation, &marker.marker_path)?,
        name: required_string(object, "name", operation, &marker.marker_path)?,
        workflow_definition_id: required_string(
            object,
            "workflowDefinitionId",
            operation,
            &marker.marker_path,
        )?,
        project_format_version: CURRENT_PROJECT_FORMAT_VERSION,
        default_locale: optional_string(object, "defaultLocale"),
        archived: metadata
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        archived_at: metadata
            .get("archivedAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
        copied_from_project_id: metadata
            .get("copiedFromProjectId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        copied_at: metadata
            .get("copiedAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
        created_at: required_string(object, "createdAt", operation, &marker.marker_path)?,
        updated_at: required_string(object, "updatedAt", operation, &marker.marker_path)?,
    })
}

fn validate_current_project(
    value: &Value,
    operation: ProjectOperation,
    path: &Path,
) -> Result<(), ProjectServiceError> {
    if value.get("documentType").and_then(Value::as_str) != Some("project") {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            "标识文件的 documentType 必须为 project。",
        )
        .at_path(path));
    }
    validate_contract_document(value).map_err(|error| {
        ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            format!("项目标识文件不满足 v1 Schema：{error}"),
        )
        .at_path(path)
    })
}

fn validate_legacy_v0(
    value: &Value,
    operation: ProjectOperation,
    path: &Path,
) -> Result<(), ProjectServiceError> {
    let object = value.as_object().ok_or_else(|| {
        ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            "旧版项目标识必须是 JSON 对象。",
        )
        .at_path(path)
    })?;
    for field in [
        "projectId",
        "name",
        "workflowDefinitionId",
        "createdAt",
        "updatedAt",
    ] {
        required_string(object, field, operation, path)?;
    }
    if !object.get("stages").is_some_and(Value::is_array) {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidProject,
            operation,
            "旧版项目的 stages 必须是数组。",
        )
        .at_path(path));
    }
    Ok(())
}

fn migrate_v0_to_v1(
    mut value: Value,
    operation: ProjectOperation,
    path: &Path,
) -> Result<Value, ProjectServiceError> {
    validate_legacy_v0(&value, operation, path)?;
    let object = value
        .as_object_mut()
        .expect("validated legacy project is an object");
    let archived = object.remove("archived").and_then(|value| value.as_bool());
    let archived_at = object
        .remove("archivedAt")
        .and_then(|value| value.as_str().map(str::to_owned));
    object.insert(
        "schemaVersion".to_owned(),
        Value::String(narracut_contracts::NARRACUT_CONTRACT_VERSION.to_owned()),
    );
    object.insert(
        "documentType".to_owned(),
        Value::String("project".to_owned()),
    );
    object.insert(
        "projectFormatVersion".to_owned(),
        Value::Number(CURRENT_PROJECT_FORMAT_VERSION.into()),
    );
    object.insert(
        "updatedAt".to_owned(),
        Value::String(current_timestamp(operation)?),
    );
    let metadata = object
        .entry("metadata".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            ProjectServiceError::new(
                ProjectErrorCode::MigrationFailed,
                operation,
                "旧版项目 metadata 不是对象，无法安全迁移。",
            )
            .at_path(path)
        })?;
    metadata.insert(
        "archived".to_owned(),
        Value::Bool(archived.unwrap_or(false)),
    );
    if let Some(timestamp) = archived_at {
        metadata.insert("archivedAt".to_owned(), Value::String(timestamp));
    }
    validate_current_project(&value, operation, path).map_err(|error| ProjectServiceError {
        code: ProjectErrorCode::MigrationFailed,
        ..error
    })?;
    Ok(value)
}

fn rewrite_copied_marker(
    project_dir: &Path,
    source_project_id: &str,
    project_id: &str,
    name: &str,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let mut marker = current_marker(project_dir, operation)?;
    let now = current_timestamp(operation)?;
    let object = marker
        .value
        .as_object_mut()
        .expect("validated copied project is an object");
    object.insert("projectId".to_owned(), Value::String(project_id.to_owned()));
    object.insert("name".to_owned(), Value::String(name.to_owned()));
    object.insert("createdAt".to_owned(), Value::String(now.clone()));
    object.insert("updatedAt".to_owned(), Value::String(now.clone()));
    let stages = object
        .get_mut("stages")
        .and_then(Value::as_array_mut)
        .expect("validated project stages is an array");
    for stage in stages {
        let stage_id = stage
            .get("stageId")
            .and_then(Value::as_str)
            .expect("validated stage state has a stageId")
            .to_owned();
        *stage = json!({
            "stageId": stage_id,
            "status": "draft",
            "staleBecauseStageIds": []
        });
    }
    let metadata = object
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .expect("validated project metadata is an object");
    metadata.insert("archived".to_owned(), Value::Bool(false));
    metadata.remove("archivedAt");
    metadata.insert(
        "copiedFromProjectId".to_owned(),
        Value::String(source_project_id.to_owned()),
    );
    metadata.insert("copiedAt".to_owned(), Value::String(now));
    validate_current_project(&marker.value, operation, &marker.marker_path)?;
    write_json_atomic(&marker.marker_path, &marker.value, operation)
}

fn rebind_mutable_stage_configs(
    project_dir: &Path,
    old_project_id: &str,
    new_project_id: &str,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let root = project_dir.join("stages");
    if root.exists() {
        rebind_stage_config_directory(&root, old_project_id, new_project_id, operation)?;
    }
    Ok(())
}

fn rebind_stage_config_directory(
    directory: &Path,
    old_project_id: &str,
    new_project_id: &str,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        ProjectServiceError::io(operation, directory, "读取阶段配置目录失败", &error)
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            ProjectServiceError::io(operation, directory, "枚举阶段配置目录失败", &error)
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ProjectServiceError::io(operation, &path, "读取阶段配置元数据失败", &error)
        })?;
        if metadata_is_link(&metadata) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::PathContainsSymlink,
                operation,
                "阶段配置目录不能包含符号链接或重解析点。",
            )
            .at_path(&path));
        }
        if metadata.is_dir() {
            rebind_stage_config_directory(&path, old_project_id, new_project_id, operation)?;
        } else if metadata.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            if metadata.len() > MAX_REWRITABLE_JSON_BYTES {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::CopyTooLarge,
                    operation,
                    format!("阶段配置 JSON 超过 {MAX_REWRITABLE_JSON_BYTES} 字节改写上限。"),
                )
                .at_path(&path));
            }
            let bytes = fs::read(&path).map_err(|error| {
                ProjectServiceError::io(operation, &path, "读取复制后的阶段配置失败", &error)
            })?;
            let mut value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    format!("阶段配置目录包含无效 JSON：{error}"),
                )
                .at_path(&path)
            })?;
            validate_contract_document(&value).map_err(|error| {
                ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    format!("阶段配置不满足 v1 Schema：{error}"),
                )
                .at_path(&path)
            })?;
            if value.get("documentType").and_then(Value::as_str) != Some("stage_config") {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    "stages 目录中的 JSON 必须是 StageConfig 文档。",
                )
                .at_path(&path));
            }
            let object = value
                .as_object_mut()
                .expect("validated StageConfig is an object");
            if object.get("projectId").and_then(Value::as_str) != Some(old_project_id) {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    "阶段配置的 projectId 与源项目不一致。",
                )
                .at_path(&path));
            }
            object.insert(
                "projectId".to_owned(),
                Value::String(new_project_id.to_owned()),
            );
            object.insert(
                "updatedAt".to_owned(),
                Value::String(current_timestamp(operation)?),
            );
            validate_contract_document(&value).map_err(|error| {
                ProjectServiceError::new(
                    ProjectErrorCode::InvalidProject,
                    operation,
                    format!("重绑定后的阶段配置不满足 v1 Schema：{error}"),
                )
                .at_path(&path)
            })?;
            write_json_atomic(&path, &value, operation)?;
        } else if !metadata.is_file() {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "阶段配置目录包含不支持的文件类型。",
            )
            .at_path(&path));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct CopyEntry {
    relative_path: PathBuf,
    kind: CopyEntryKind,
}

#[derive(Debug)]
struct CopyScan {
    entries: Vec<CopyEntry>,
    files: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum CopyEntryKind {
    Directory,
    File { length: u64 },
}

fn scan_copy_entries(
    source: &Path,
    max_bytes: u64,
    max_files: u64,
    max_entries: u64,
    max_depth: usize,
    operation: ProjectOperation,
) -> Result<CopyScan, ProjectServiceError> {
    let mut entries = Vec::new();
    let mut pending_directories = vec![(source.to_path_buf(), 0_usize)];
    let mut files = 0_u64;
    let mut bytes = 0_u64;

    while let Some((directory, depth)) = pending_directories.pop() {
        let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| {
            ProjectServiceError::io(
                operation,
                &directory,
                "扫描前重新读取项目目录元数据失败",
                &error,
            )
        })?;
        if metadata_is_link(&directory_metadata) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::PathContainsSymlink,
                operation,
                "扫描中的项目目录变成了符号链接或重解析点，已中止复制。",
            )
            .at_path(&directory));
        }
        if !directory_metadata.is_dir() {
            return Err(source_changed_error(operation, &directory));
        }
        let children = fs::read_dir(&directory).map_err(|error| {
            ProjectServiceError::io(operation, &directory, "读取项目目录失败", &error)
        })?;

        for child in children {
            let child = child.map_err(|error| {
                ProjectServiceError::io(operation, &directory, "枚举项目目录失败", &error)
            })?;
            let path = child.path();
            let relative_path = path
                .strip_prefix(source)
                .expect("scanned path remains under source")
                .to_path_buf();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                ProjectServiceError::io(operation, &path, "读取项目文件元数据失败", &error)
            })?;
            if metadata_is_link(&metadata) {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::PathContainsSymlink,
                    operation,
                    "项目副本拒绝跟随符号链接或重解析点。",
                )
                .at_path(&path));
            }

            let next_entry_count = u64::try_from(entries.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            if next_entry_count > max_entries {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::CopyTooLarge,
                    operation,
                    format!("项目条目数超过当前有界扫描上限 {max_entries}；请等待任务队列接管。"),
                )
                .at_path(&source.join(relative_path)));
            }

            if metadata.is_dir() {
                let child_depth = depth.saturating_add(1);
                if child_depth > max_depth {
                    return Err(ProjectServiceError::new(
                        ProjectErrorCode::CopyTooLarge,
                        operation,
                        format!(
                            "项目目录深度超过当前有界扫描上限 {max_depth}；请等待任务队列接管。"
                        ),
                    )
                    .at_path(&path));
                }
                entries.push(CopyEntry {
                    relative_path: relative_path.clone(),
                    kind: CopyEntryKind::Directory,
                });
                if relative_path.components().next() != Some(Component::Normal("cache".as_ref())) {
                    pending_directories.push((path, child_depth));
                }
            } else if metadata.is_file() {
                files = files.checked_add(1).ok_or_else(|| {
                    copy_too_large(operation, &path, "项目文件数超过可表示范围。")
                })?;
                if files > max_files {
                    return Err(copy_too_large(
                        operation,
                        &path,
                        format!("项目文件数超过当前有界扫描上限 {max_files}；请等待任务队列接管。"),
                    ));
                }
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| copy_too_large(operation, &path, "项目大小超过可表示范围。"))?;
                if bytes > max_bytes {
                    return Err(copy_too_large(
                        operation,
                        &path,
                        format!(
                            "项目大小超过当前有界扫描上限 {max_bytes} 字节；请等待任务队列接管。"
                        ),
                    ));
                }
                entries.push(CopyEntry {
                    relative_path,
                    kind: CopyEntryKind::File {
                        length: metadata.len(),
                    },
                });
            } else {
                return Err(ProjectServiceError::new(
                    ProjectErrorCode::InvalidPath,
                    operation,
                    "项目目录包含不支持的特殊文件。",
                )
                .at_path(&path));
            }
        }
    }

    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(CopyScan {
        entries,
        files,
        bytes,
    })
}

fn copy_entries(
    source: &Path,
    destination: &Path,
    entries: &[CopyEntry],
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    for entry in entries {
        let source_path = source.join(&entry.relative_path);
        let destination_path = destination.join(&entry.relative_path);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            ProjectServiceError::io(
                operation,
                &source_path,
                "复制前重新读取源文件元数据失败",
                &error,
            )
        })?;
        if metadata_is_link(&metadata) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::PathContainsSymlink,
                operation,
                "扫描后源项目出现符号链接或重解析点，已中止复制。",
            )
            .at_path(&source_path));
        }
        match entry.kind {
            CopyEntryKind::Directory => {
                if !metadata.is_dir() {
                    return Err(source_changed_error(operation, &source_path));
                }
                fs::create_dir_all(&destination_path).map_err(|error| {
                    ProjectServiceError::io(
                        operation,
                        &destination_path,
                        "创建副本目录失败",
                        &error,
                    )
                })?;
            }
            CopyEntryKind::File { length } => {
                if !metadata.is_file() || metadata.len() != length {
                    return Err(source_changed_error(operation, &source_path));
                }
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        ProjectServiceError::io(operation, parent, "创建副本父目录失败", &error)
                    })?;
                }
                let copied = fs::copy(&source_path, &destination_path).map_err(|error| {
                    ProjectServiceError::io(
                        operation,
                        &destination_path,
                        "复制项目文件失败",
                        &error,
                    )
                })?;
                if copied != length {
                    return Err(source_changed_error(operation, &source_path));
                }
            }
        }
    }
    Ok(())
}

fn copy_too_large(
    operation: ProjectOperation,
    path: &Path,
    message: impl Into<String>,
) -> ProjectServiceError {
    ProjectServiceError::new(ProjectErrorCode::CopyTooLarge, operation, message).at_path(path)
}

fn source_changed_error(operation: ProjectOperation, path: &Path) -> ProjectServiceError {
    ProjectServiceError::new(
        ProjectErrorCode::IoError,
        operation,
        "源项目在扫描后发生变化，已中止复制；请重试。",
    )
    .at_path(path)
}

struct PendingDirectory {
    parent: PathBuf,
    path: PathBuf,
    committed: bool,
}

impl PendingDirectory {
    fn create(
        parent: &Path,
        path: PathBuf,
        operation: ProjectOperation,
    ) -> Result<Self, ProjectServiceError> {
        if path.parent() != Some(parent) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "内部临时目录超出目标父目录。",
            )
            .at_path(&path));
        }
        fs::create_dir(&path).map_err(|error| {
            ProjectServiceError::io(operation, &path, "创建临时项目目录失败", &error)
        })?;
        Ok(Self {
            parent: parent.to_path_buf(),
            path,
            committed: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingDirectory {
    fn drop(&mut self) {
        if !self.committed && self.path.parent() == Some(self.parent.as_path()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn canonical_project_directory(
    path: &Path,
    operation: ProjectOperation,
) -> Result<PathBuf, ProjectServiceError> {
    canonical_existing_directory_with_code(path, operation, ProjectErrorCode::ProjectNotFound)
}

fn canonical_existing_directory(
    path: &Path,
    operation: ProjectOperation,
) -> Result<PathBuf, ProjectServiceError> {
    canonical_existing_directory_with_code(path, operation, ProjectErrorCode::InvalidPath)
}

fn canonical_existing_directory_with_code(
    path: &Path,
    operation: ProjectOperation,
    missing_code: ProjectErrorCode,
) -> Result<PathBuf, ProjectServiceError> {
    if path.as_os_str().is_empty() {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidPath,
            operation,
            "路径不能为空。",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProjectServiceError::new(missing_code, operation, "目录不存在。").at_path(path)
        } else {
            ProjectServiceError::io(operation, path, "读取目录元数据失败", &error)
        }
    })?;
    if metadata_is_link(&metadata) {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::PathContainsSymlink,
            operation,
            "项目边界目录不能是符号链接或重解析点。",
        )
        .at_path(path));
    }
    if !metadata.is_dir() {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidPath,
            operation,
            "路径不是目录。",
        )
        .at_path(path));
    }
    fs::canonicalize(path)
        .map_err(|error| ProjectServiceError::io(operation, path, "规范化目录路径失败", &error))
}

fn ensure_safe_project_subdirectory(
    project_root: &Path,
    components: &[&str],
    operation: ProjectOperation,
) -> Result<PathBuf, ProjectServiceError> {
    let mut current = project_root.to_path_buf();

    for component in components {
        let mut path_components = Path::new(component).components();
        let is_safe_component = matches!(path_components.next(), Some(Component::Normal(_)))
            && path_components.next().is_none();
        if component.is_empty() || !is_safe_component {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "内部项目子目录不是安全路径片段。",
            )
            .at_path(&current));
        }
        current.push(component);

        match fs::symlink_metadata(&current) {
            Ok(metadata) => validate_safe_directory_metadata(&current, &metadata, operation)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    ProjectServiceError::io(operation, &current, "创建项目受控子目录失败", &error)
                })?;
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    ProjectServiceError::io(
                        operation,
                        &current,
                        "重新读取项目受控子目录失败",
                        &error,
                    )
                })?;
                validate_safe_directory_metadata(&current, &metadata, operation)?;
            }
            Err(error) => {
                return Err(ProjectServiceError::io(
                    operation,
                    &current,
                    "读取项目受控子目录失败",
                    &error,
                ));
            }
        }

        let canonical = fs::canonicalize(&current).map_err(|error| {
            ProjectServiceError::io(operation, &current, "规范化项目受控子目录失败", &error)
        })?;
        if !canonical.starts_with(project_root) {
            return Err(ProjectServiceError::new(
                ProjectErrorCode::InvalidPath,
                operation,
                "项目受控子目录解析后逃逸项目根目录。",
            )
            .at_path(&canonical));
        }
        current = canonical;
    }

    Ok(current)
}

fn validate_safe_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    if metadata_is_link(metadata) {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::PathContainsSymlink,
            operation,
            "项目受控子目录不能是符号链接或重解析点。",
        )
        .at_path(path));
    }
    if !metadata.is_dir() {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidPath,
            operation,
            "项目受控子目录路径不是目录。",
        )
        .at_path(path));
    }
    Ok(())
}

fn ensure_destination_available(
    path: &Path,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    match path.try_exists() {
        Ok(false) => Ok(()),
        Ok(true) => Err(ProjectServiceError::new(
            ProjectErrorCode::DestinationExists,
            operation,
            "目标目录已经存在。",
        )
        .at_path(path)),
        Err(error) => Err(ProjectServiceError::io(
            operation,
            path,
            "检查目标目录失败",
            &error,
        )),
    }
}

fn validate_directory_name(
    name: &str,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let trimmed = name.trim();
    let invalid_character = name
        .chars()
        .any(|character| character.is_control() || r#"/\:*?"<>|"#.contains(character));
    let is_single_component = {
        let mut components = Path::new(name).components();
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
    };
    let base_name = trimmed
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(base_name.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || (base_name.len() == 4
            && (base_name.starts_with("COM") || base_name.starts_with("LPT"))
            && base_name
                .chars()
                .last()
                .is_some_and(|digit| ('1'..='9').contains(&digit)));

    if trimmed.is_empty()
        || trimmed != name
        || trimmed.chars().count() > 120
        || invalid_character
        || !is_single_component
        || name.ends_with(['.', ' '])
        || reserved
    {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidName,
            operation,
            "目录名必须是无首尾空白的单个安全路径片段，且不能使用 Windows 保留名或结尾点。",
        ));
    }
    Ok(())
}

fn validate_text(
    value: &str,
    label: &str,
    max_characters: usize,
    operation: ProjectOperation,
) -> Result<String, ProjectServiceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_characters {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidName,
            operation,
            format!("{label}不能为空，且不能超过 {max_characters} 个字符。"),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ProjectServiceError::new(
            ProjectErrorCode::InvalidName,
            operation,
            format!("{label}不能包含控制字符。"),
        ));
    }
    Ok(trimmed.to_owned())
}

fn write_json_atomic(
    path: &Path,
    value: &Value,
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        ProjectServiceError::new(
            ProjectErrorCode::IoError,
            operation,
            format!("序列化项目 JSON 失败：{error}"),
        )
        .at_path(path)
    })?;
    bytes.push(b'\n');
    let mut file = AtomicWriteFile::options().open(path).map_err(|error| {
        ProjectServiceError::io(operation, path, "创建原子写入文件失败", &error)
    })?;
    file.write_all(&bytes).map_err(|error| {
        ProjectServiceError::io(operation, path, "写入原子临时文件失败", &error)
    })?;
    file.commit()
        .map_err(|error| ProjectServiceError::io(operation, path, "提交原子文件替换失败", &error))
}

fn write_backup(
    path: &Path,
    bytes: &[u8],
    operation: ProjectOperation,
) -> Result<(), ProjectServiceError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| ProjectServiceError::io(operation, path, "创建迁移备份失败", &error))?;
    file.write_all(bytes)
        .map_err(|error| ProjectServiceError::io(operation, path, "写入迁移备份失败", &error))?;
    file.sync_all()
        .map_err(|error| ProjectServiceError::io(operation, path, "同步迁移备份失败", &error))
}

fn current_timestamp(operation: ProjectOperation) -> Result<String, ProjectServiceError> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|error| {
        ProjectServiceError::new(
            ProjectErrorCode::IoError,
            operation,
            format!("生成 UTC 时间戳失败：{error}"),
        )
    })
}

fn new_project_id() -> String {
    format!("project_{}", Uuid::new_v4().simple())
}

fn unsupported_version_error(
    operation: ProjectOperation,
    path: &Path,
    detected_version: u32,
) -> ProjectServiceError {
    ProjectServiceError::new(
        ProjectErrorCode::UnsupportedNewerVersion,
        operation,
        "项目由更新版本的 NarraCut 创建，当前版本拒绝降级修改。",
    )
    .at_path(path)
    .with_versions(CURRENT_PROJECT_FORMAT_VERSION, detected_version)
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    operation: ProjectOperation,
    path: &Path,
) -> Result<String, ProjectServiceError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ProjectServiceError::new(
                ProjectErrorCode::InvalidProject,
                operation,
                format!("项目字段 {field} 必须是字符串。"),
            )
            .at_path(path)
        })
}

fn optional_string(object: &Map<String, Value>, field: &str) -> Option<String> {
    object.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
