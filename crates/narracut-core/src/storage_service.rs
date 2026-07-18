use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write as FmtWrite,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use atomic_write_file::AtomicWriteFile;
use narracut_contracts::{validate_contract_document, ArtifactDraft};
use rusqlite::{params, params_from_iter, Connection, OpenFlags, Transaction};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ArtifactCommitResultData, ArtifactReadResultData, ArtifactVerificationResultData,
    ArtifactVerificationStatusData, CacheCleanupResultData, ForgetProjectResultData,
    IndexedJobData, IndexedJobStatusData, IndexedJobUpsertData, IndexedJobsResultData,
    ListIndexedJobsOptions, ProjectDescriptorData, ProjectErrorCode, ProjectIndexRebuildResultData,
    ProjectOperation, ProjectService, ProjectServiceError, RecentProjectData,
    RecentProjectsResultData, ResolveStagedMediaSourceOptions, ResolvedStagedMediaSourceData,
    StageMediaSourceFileOptions, StagedMediaSourceData, StorageErrorCode, StorageIndexStatusData,
    StorageOperation, StorageServiceError, StoreArtifactFileOptions, STORAGE_COMMAND_API_VERSION,
};

const INDEX_SCHEMA_VERSION: i64 = 2;
const MAX_SYNCHRONOUS_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARTIFACT_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_INDEXED_ARTIFACTS_PER_PROJECT: usize = 4096;
const MAX_INDEXED_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INDEXED_REFERENCE_EDGES: usize = 65_536;
const MAX_CACHE_ENTRIES: usize = 4096;
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CACHE_DEPTH: usize = 64;
const HASH_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct StorageService {
    inner: Arc<StorageServiceInner>,
}

struct StorageServiceInner {
    index_path: PathBuf,
    project_service: ProjectService,
    index_lock: Mutex<()>,
    max_artifact_bytes: u64,
    max_indexed_artifacts: usize,
    max_indexed_metadata_bytes: u64,
    max_cache_entries: usize,
    max_cache_bytes: u64,
    max_cache_depth: usize,
}

impl StorageService {
    pub fn new(index_path: impl Into<PathBuf>, project_service: ProjectService) -> Self {
        Self {
            inner: Arc::new(StorageServiceInner {
                index_path: index_path.into(),
                project_service,
                index_lock: Mutex::new(()),
                max_artifact_bytes: MAX_SYNCHRONOUS_ARTIFACT_BYTES,
                max_indexed_artifacts: MAX_INDEXED_ARTIFACTS_PER_PROJECT,
                max_indexed_metadata_bytes: MAX_INDEXED_METADATA_BYTES,
                max_cache_entries: MAX_CACHE_ENTRIES,
                max_cache_bytes: MAX_CACHE_BYTES,
                max_cache_depth: MAX_CACHE_DEPTH,
            }),
        }
    }

    pub(crate) fn read_media_receipt(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
        receipt_id: &str,
    ) -> Result<Option<Value>, StorageServiceError> {
        let operation = StorageOperation::ManageMediaReceipt;
        validate_media_receipt_id(receipt_id, operation)?;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        read_media_receipt_unlocked(&descriptor, receipt_id, operation)
    }

    pub(crate) fn commit_media_receipt(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
        receipt_id: &str,
        receipt: &Value,
    ) -> Result<(Value, bool), StorageServiceError> {
        const MAX_MEDIA_RECEIPT_BYTES: usize = 64 * 1024;

        let operation = StorageOperation::ManageMediaReceipt;
        validate_media_receipt_id(receipt_id, operation)?;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        if let Some(existing) = read_media_receipt_unlocked(&descriptor, receipt_id, operation)? {
            return Ok((existing, false));
        }
        if receipt.get("documentType").and_then(Value::as_str) != Some("media_import_receipt")
            || receipt.get("projectId").and_then(Value::as_str)
                != Some(descriptor.project_id.as_str())
            || receipt.get("receiptId").and_then(Value::as_str) != Some(receipt_id)
        {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "媒体导入 receipt 必须绑定当前项目与派生 receiptId。",
            ));
        }
        let mut bytes = serde_json::to_vec_pretty(receipt).map_err(|_| {
            StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "媒体导入 receipt 无法序列化。",
            )
        })?;
        bytes.push(b'\n');
        if bytes.len() > MAX_MEDIA_RECEIPT_BYTES {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "媒体导入 receipt 超过 64 KiB 上限。",
            ));
        }

        let project_dir = PathBuf::from(&descriptor.project_path);
        let receipt_dir =
            ensure_project_directories(&project_dir, &["artifacts", "media-receipts"], operation)?;
        let destination = receipt_dir.join(format!("{receipt_id}.json"));
        let mut temporary = NamedTempFile::new_in(&receipt_dir).map_err(|error| {
            StorageServiceError::io(
                operation,
                &receipt_dir,
                "创建媒体导入 receipt 临时文件失败",
                &error,
            )
        })?;
        temporary.write_all(&bytes).map_err(|error| {
            StorageServiceError::io(
                operation,
                temporary.path(),
                "写入媒体导入 receipt 临时文件失败",
                &error,
            )
        })?;
        temporary.as_file().sync_all().map_err(|error| {
            StorageServiceError::io(
                operation,
                temporary.path(),
                "同步媒体导入 receipt 临时文件失败",
                &error,
            )
        })?;
        match temporary.persist_noclobber(&destination) {
            Ok(_) => Ok((receipt.clone(), true)),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_media_receipt_unlocked(&descriptor, receipt_id, operation)?
                    .ok_or_else(|| {
                        StorageServiceError::new(
                            StorageErrorCode::ArtifactConflict,
                            operation,
                            "媒体导入 receipt 发生不可调和的并发冲突。",
                        )
                    })?;
                Ok((existing, false))
            }
            Err(error) => Err(StorageServiceError::io(
                operation,
                &destination,
                "提交媒体导入 receipt 失败",
                &error.error,
            )),
        }
    }

    pub fn stage_media_source_file(
        &self,
        options: StageMediaSourceFileOptions,
    ) -> Result<StagedMediaSourceData, StorageServiceError> {
        self.stage_media_source_file_inner(options)
            .map_err(redact_media_source_error)
    }

    fn stage_media_source_file_inner(
        &self,
        options: StageMediaSourceFileOptions,
    ) -> Result<StagedMediaSourceData, StorageServiceError> {
        let operation = StorageOperation::ManageMediaSource;
        validate_media_source_limit(options.max_bytes, self.inner.max_artifact_bytes, operation)?;
        if let Some(expected_hash) = options.expected_content_hash.as_deref() {
            validate_sha256(expected_hash, operation)?;
        }
        let source_path = Path::new(&options.source_path);
        reject_source_path_links(source_path, operation)?;
        let source_file_name = safe_media_source_file_name(source_path, operation)?;
        let source = inspect_source_file(source_path, operation)?;
        if source.byte_length > options.max_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::SourceTooLarge,
                operation,
                format!("媒体源文件超过 {} 字节 staging 上限。", options.max_bytes),
            ));
        }

        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let temporary_dir = ensure_project_directories(
            &project_dir,
            &["requests", "media-sources", ".tmp"],
            operation,
        )?;
        let temporary_path = temporary_dir.join(format!("{}.source", Uuid::new_v4().simple()));
        let mut temporary = PendingFile::new(temporary_path);
        let (content_hash, byte_length) =
            copy_and_hash_source(&source, temporary.path(), options.max_bytes, operation)?;
        if options
            .expected_content_hash
            .as_deref()
            .is_some_and(|expected| expected != content_hash)
        {
            return Err(StorageServiceError::new(
                StorageErrorCode::SourceChanged,
                operation,
                "媒体源文件 SHA-256 与调用方确认值不一致。",
            ));
        }

        let staged_source_uri = media_source_uri(&content_hash, &source_file_name, operation)?;
        let destination =
            portable_uri_to_project_path(&project_dir, &staged_source_uri, operation)?;
        let parent = destination.parent().ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "媒体源 staging 路径缺少父目录。",
            )
        })?;
        ensure_project_directories_from_path(&project_dir, parent, operation)?;

        let deduplicated = match inspect_project_path(&project_dir, &destination, operation)? {
            Some(_) => {
                verify_existing_content(
                    &project_dir,
                    &destination,
                    &content_hash,
                    byte_length,
                    operation,
                )?;
                fs::remove_file(temporary.path()).map_err(|error| {
                    StorageServiceError::io(
                        operation,
                        temporary.path(),
                        "移除已去重的媒体源临时文件失败",
                        &error,
                    )
                })?;
                temporary.commit();
                true
            }
            None => match persist_content_noclobber(temporary.path(), &destination) {
                Ok(()) => {
                    temporary.commit();
                    false
                }
                Err(error) => match inspect_project_path(&project_dir, &destination, operation)? {
                    Some(_) => {
                        verify_existing_content(
                            &project_dir,
                            &destination,
                            &content_hash,
                            byte_length,
                            operation,
                        )?;
                        fs::remove_file(temporary.path()).map_err(|remove_error| {
                            StorageServiceError::io(
                                operation,
                                temporary.path(),
                                "移除竞态去重后的媒体源临时文件失败",
                                &remove_error,
                            )
                        })?;
                        temporary.commit();
                        true
                    }
                    None => {
                        return Err(StorageServiceError::io(
                            operation,
                            &destination,
                            "提交媒体源 staging 文件失败",
                            &error,
                        ));
                    }
                },
            },
        };

        Ok(StagedMediaSourceData {
            owner_project_id: descriptor.project_id,
            staged_source_uri,
            source_file_name,
            content_hash,
            byte_length,
            deduplicated,
        })
    }

    pub fn resolve_staged_media_source(
        &self,
        options: ResolveStagedMediaSourceOptions,
    ) -> Result<ResolvedStagedMediaSourceData, StorageServiceError> {
        self.resolve_staged_media_source_inner(options)
            .map_err(redact_media_source_error)
    }

    fn resolve_staged_media_source_inner(
        &self,
        options: ResolveStagedMediaSourceOptions,
    ) -> Result<ResolvedStagedMediaSourceData, StorageServiceError> {
        let operation = StorageOperation::ManageMediaSource;
        validate_media_source_limit(options.max_bytes, self.inner.max_artifact_bytes, operation)?;
        let expected_hex = validate_sha256(&options.expected_content_hash, operation)?;
        if options.expected_byte_length > options.max_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::SourceTooLarge,
                operation,
                "媒体源声明长度超过 resolver 上限。",
            ));
        }
        let (uri_hex, source_file_name) =
            parse_media_source_uri(&options.staged_source_uri, operation)?;
        if uri_hex != expected_hex {
            return Err(StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "媒体源 URI 与确认的 SHA-256 不一致。",
            ));
        }

        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let source_path =
            portable_uri_to_project_path(&project_dir, &options.staged_source_uri, operation)?;
        let (actual_hash, actual_length) =
            hash_file(&project_dir, &source_path, options.max_bytes, operation)?;
        if actual_hash != options.expected_content_hash
            || actual_length != options.expected_byte_length
        {
            return Err(StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "staged 媒体源实体未通过 SHA-256 与字节数复验。",
            ));
        }

        Ok(ResolvedStagedMediaSourceData {
            owner_project_id: descriptor.project_id,
            staged_source_uri: options.staged_source_uri,
            source_path: source_path.to_string_lossy().into_owned(),
            source_file_name,
            content_hash: actual_hash,
            byte_length: actual_length,
        })
    }

    pub fn import_artifact_file(
        &self,
        options: StoreArtifactFileOptions,
    ) -> Result<ArtifactCommitResultData, StorageServiceError> {
        self.import_artifact_file_internal(options, None)
    }

    pub(crate) fn import_artifact_file_idempotent(
        &self,
        options: StoreArtifactFileOptions,
        artifact_id: &str,
        created_at: &str,
    ) -> Result<ArtifactCommitResultData, StorageServiceError> {
        validate_artifact_id(artifact_id, StorageOperation::ImportArtifact)?;
        self.import_artifact_file_internal(options, Some((artifact_id, created_at)))
    }

    fn import_artifact_file_internal(
        &self,
        options: StoreArtifactFileOptions,
        requested_identity: Option<(&str, &str)>,
    ) -> Result<ArtifactCommitResultData, StorageServiceError> {
        let operation = StorageOperation::ImportArtifact;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        preflight_artifact_draft(&descriptor, &options.artifact, operation)?;
        self.validate_draft_references_unlocked(&descriptor, &options.artifact, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let source = inspect_source_file(Path::new(&options.source_path), operation)?;
        if source.byte_length > self.inner.max_artifact_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::SourceTooLarge,
                operation,
                format!(
                    "同步导入上限为 {} 字节；更大的 Artifact 必须交给持久化任务队列。",
                    self.inner.max_artifact_bytes
                ),
            )
            .at_path(&source.path));
        }

        ensure_project_directories(&project_dir, &["artifacts"], operation)?;
        let temp_dir = ensure_project_directories(&project_dir, &["artifacts", ".tmp"], operation)?;
        let metadata_dir =
            ensure_project_directories(&project_dir, &["artifacts", "metadata"], operation)?;

        let temporary_path = temp_dir.join(format!("{}.blob", Uuid::new_v4().simple()));
        let mut temporary = PendingFile::new(temporary_path.clone());
        let (content_hash, byte_length) = copy_and_hash_source(
            &source,
            temporary.path(),
            self.inner.max_artifact_bytes,
            operation,
        )?;
        let content_uri = content_uri_for_hash(&content_hash, operation)?;
        let content_path = portable_uri_to_project_path(&project_dir, &content_uri, operation)?;
        let content_parent = content_path.parent().ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::InvalidArtifact,
                operation,
                "内容寻址路径缺少父目录。",
            )
        })?;
        ensure_project_directories_from_path(&project_dir, content_parent, operation)?;

        let deduplicated = match inspect_project_path(&project_dir, &content_path, operation)? {
            Some(_) => {
                verify_existing_content(
                    &project_dir,
                    &content_path,
                    &content_hash,
                    byte_length,
                    operation,
                )?;
                fs::remove_file(temporary.path()).map_err(|error| {
                    StorageServiceError::io(
                        operation,
                        temporary.path(),
                        "移除已去重的临时 Artifact 失败",
                        &error,
                    )
                })?;
                temporary.commit();
                true
            }
            None => match persist_content_noclobber(temporary.path(), &content_path) {
                Ok(()) => {
                    temporary.commit();
                    false
                }
                Err(persist_error) => {
                    match inspect_project_path(&project_dir, &content_path, operation)? {
                        Some(_) => {
                            verify_existing_content(
                                &project_dir,
                                &content_path,
                                &content_hash,
                                byte_length,
                                operation,
                            )?;
                            fs::remove_file(temporary.path()).map_err(|error| {
                                StorageServiceError::io(
                                    operation,
                                    temporary.path(),
                                    "移除竞态去重后的临时 Artifact 失败",
                                    &error,
                                )
                            })?;
                            temporary.commit();
                            true
                        }
                        None => {
                            return Err(StorageServiceError::io(
                                operation,
                                &content_path,
                                "提交内容寻址 Artifact 失败",
                                &persist_error,
                            ));
                        }
                    }
                }
            },
        };

        let artifact_id = requested_identity
            .map(|(artifact_id, _)| artifact_id.to_owned())
            .unwrap_or_else(|| format!("artifact_{}", Uuid::new_v4().simple()));
        let artifact = build_artifact_document(
            &descriptor,
            ArtifactDocumentIdentity {
                artifact_id: &artifact_id,
                content_uri: &content_uri,
                content_hash: &content_hash,
                byte_length,
                created_at: requested_identity.map(|(_, created_at)| created_at),
            },
            &options.artifact,
            operation,
        )?;
        let metadata_uri = format!("artifacts/metadata/{artifact_id}.json");
        let metadata_path = metadata_dir.join(format!("{artifact_id}.json"));
        let metadata_replay = if requested_identity.is_some()
            && inspect_project_path(&project_dir, &metadata_path, operation)?.is_some()
        {
            let existing = self.read_artifact_unlocked(&descriptor, &artifact_id, operation)?;
            if existing.artifact != artifact || !existing.content_available {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ArtifactConflict,
                    operation,
                    "相同确定性 Artifact 身份已绑定不同内容、元数据或缺失内容。",
                )
                .at_path(&metadata_path)
                .for_artifact(&artifact_id));
            }
            true
        } else {
            ensure_destination_absent(&metadata_path, operation)?;
            write_json_atomic(&metadata_path, &artifact, operation)?;
            false
        };

        let indexed = self
            .index_artifact(&descriptor, &metadata_uri, &content_uri, &artifact, true)
            .is_ok();

        Ok(ArtifactCommitResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            artifact,
            metadata_uri,
            content_uri,
            deduplicated: deduplicated || metadata_replay,
            index_status: if indexed {
                StorageIndexStatusData::UpToDate
            } else {
                StorageIndexStatusData::RebuildRequired
            },
        })
    }

    pub fn get_artifact(
        &self,
        project_path: impl AsRef<Path>,
        artifact_id: &str,
    ) -> Result<ArtifactReadResultData, StorageServiceError> {
        let operation = StorageOperation::GetArtifact;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        self.read_artifact_unlocked(&descriptor, artifact_id, operation)
    }

    /// 读取并校验一个 Artifact 的内容，且在分配内存前执行调用方给定的字节上限。
    ///
    /// 此接口仅供 Rust 内部有界执行器使用，不注册为 Tauri command。
    pub fn read_artifact_content_bounded(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
        artifact_id: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, StorageServiceError> {
        let operation = StorageOperation::GetArtifact;
        if max_bytes == 0 || max_bytes > self.inner.max_artifact_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                format!(
                    "内容读取上限必须位于 1..={} 字节。",
                    self.inner.max_artifact_bytes
                ),
            ));
        }
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        let read = self.read_artifact_unlocked(&descriptor, artifact_id, operation)?;
        if !read.content_available {
            return Err(StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "Artifact 内容对象不存在。",
            )
            .for_artifact(artifact_id));
        }
        let project_dir = PathBuf::from(&descriptor.project_path);
        let content_path =
            portable_uri_to_project_path(&project_dir, &read.content_uri, operation)?;
        let (bytes, actual_hash, actual_length) =
            read_file_bounded(&project_dir, &content_path, max_bytes, operation)?;
        let expected_hash = required_string(&read.artifact, "contentHash", operation)?;
        let expected_length = required_u64(&read.artifact, "byteLength", operation)?;
        if actual_hash != expected_hash || actual_length != expected_length {
            return Err(StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "Artifact 内容哈希或字节数与不可变元数据不一致。",
            )
            .at_path(&content_path)
            .for_artifact(artifact_id));
        }
        Ok(bytes)
    }

    pub fn verify_artifact(
        &self,
        project_path: impl AsRef<Path>,
        artifact_id: &str,
    ) -> Result<ArtifactVerificationResultData, StorageServiceError> {
        let operation = StorageOperation::VerifyArtifact;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        let read = self.read_artifact_unlocked(&descriptor, artifact_id, operation)?;
        let expected_hash = required_string(&read.artifact, "contentHash", operation)?;
        let expected_length = required_u64(&read.artifact, "byteLength", operation)?;
        let content_path = portable_uri_to_project_path(
            Path::new(&descriptor.project_path),
            &read.content_uri,
            operation,
        )?;

        if !read.content_available {
            return Ok(ArtifactVerificationResultData {
                api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
                owner_project_id: descriptor.project_id,
                artifact_id: artifact_id.to_owned(),
                status: ArtifactVerificationStatusData::MissingContent,
                expected_content_hash: expected_hash,
                actual_content_hash: None,
                expected_byte_length: expected_length,
                actual_byte_length: None,
            });
        }

        let (actual_hash, actual_length) = hash_file(
            Path::new(&descriptor.project_path),
            &content_path,
            self.inner.max_artifact_bytes,
            operation,
        )?;
        let status = if actual_length != expected_length {
            ArtifactVerificationStatusData::ByteLengthMismatch
        } else if actual_hash != expected_hash {
            ArtifactVerificationStatusData::HashMismatch
        } else {
            ArtifactVerificationStatusData::Verified
        };

        Ok(ArtifactVerificationResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            artifact_id: artifact_id.to_owned(),
            status,
            expected_content_hash: expected_hash,
            actual_content_hash: Some(actual_hash),
            expected_byte_length: expected_length,
            actual_byte_length: Some(actual_length),
        })
    }

    pub fn rebuild_project_index(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
    ) -> Result<ProjectIndexRebuildResultData, StorageServiceError> {
        let operation = StorageOperation::RebuildProjectIndex;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        let artifacts = self.scan_artifacts_unlocked(&descriptor, operation)?;
        let missing_content_count = artifacts
            .iter()
            .filter(|artifact| !artifact.content_available)
            .count() as u64;

        let _index_guard = self.index_guard();
        let mut connection = self.open_index(operation)?;
        let transaction = connection.transaction().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "开始索引重建事务失败",
                &error,
            )
        })?;
        record_project_tx(&transaction, &descriptor, operation, &self.inner.index_path)?;
        transaction
            .execute(
                "DELETE FROM artifacts WHERE owner_project_id = ?1",
                params![descriptor.project_id],
            )
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "清理旧 Artifact 索引失败",
                    &error,
                )
            })?;
        for artifact in &artifacts {
            insert_artifact_tx(
                &transaction,
                &descriptor.project_id,
                artifact,
                operation,
                &self.inner.index_path,
            )?;
        }
        transaction.commit().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "提交索引重建事务失败",
                &error,
            )
        })?;

        Ok(ProjectIndexRebuildResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            artifacts_indexed: artifacts.len() as u64,
            missing_content_count,
            index_status: StorageIndexStatusData::UpToDate,
        })
    }

    pub fn record_recent_project(
        &self,
        descriptor: &ProjectDescriptorData,
    ) -> Result<(), StorageServiceError> {
        let operation = StorageOperation::ListRecentProjects;
        let _index_guard = self.index_guard();
        let mut connection = self.open_index(operation)?;
        let transaction = connection.transaction().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "开始最近项目索引事务失败",
                &error,
            )
        })?;
        record_project_tx(&transaction, descriptor, operation, &self.inner.index_path)?;
        transaction.commit().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "提交最近项目索引事务失败",
                &error,
            )
        })
    }

    pub fn list_recent_projects(
        &self,
        limit: u32,
        include_missing: bool,
    ) -> Result<RecentProjectsResultData, StorageServiceError> {
        let operation = StorageOperation::ListRecentProjects;
        if !(1..=100).contains(&limit) {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "最近项目 limit 必须位于 1..=100。",
            ));
        }
        let _index_guard = self.index_guard();
        let connection = self.open_index(operation)?;
        let mut statement = connection
            .prepare(
                "SELECT project_id, project_path, name, workflow_definition_id, \
                        project_format_version, archived, last_opened_at, marker_updated_at \
                 FROM recent_projects ORDER BY last_opened_at DESC, project_id ASC",
            )
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "准备最近项目查询失败",
                    &error,
                )
            })?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "执行最近项目查询失败",
                    &error,
                )
            })?;

        let mut projects = Vec::new();
        for row in rows {
            let (
                project_id,
                project_path,
                name,
                workflow_definition_id,
                project_format_version,
                archived,
                last_opened_at,
                marker_updated_at,
            ) = row.map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "读取最近项目记录失败",
                    &error,
                )
            })?;
            let format_version = u32::try_from(project_format_version).map_err(|_| {
                StorageServiceError::new(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    "最近项目索引包含超出 u32 的格式版本。",
                )
                .at_path(&self.inner.index_path)
            })?;
            let path_available = recent_project_path_available(Path::new(&project_path));
            if include_missing || path_available {
                projects.push(RecentProjectData {
                    project_id,
                    project_path,
                    name,
                    workflow_definition_id,
                    project_format_version: format_version,
                    archived: archived != 0,
                    last_opened_at,
                    marker_updated_at,
                    path_available,
                });
            }
            if projects.len() == limit as usize {
                break;
            }
        }

        Ok(RecentProjectsResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            projects,
        })
    }

    pub fn upsert_job_summary(
        &self,
        descriptor: &ProjectDescriptorData,
        job: IndexedJobUpsertData,
    ) -> Result<(), StorageServiceError> {
        let operation = StorageOperation::ListIndexedJobs;
        validate_job_upsert(&job, operation)?;
        let created_at = canonical_job_timestamp(&job.created_at, "createdAt", operation)?;
        let updated_at = canonical_job_timestamp(&job.updated_at, "updatedAt", operation)?;
        let _index_guard = self.index_guard();
        let mut connection = self.open_index(operation)?;
        let transaction = connection.transaction().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "开始任务索引事务失败",
                &error,
            )
        })?;
        record_project_tx(&transaction, descriptor, operation, &self.inner.index_path)?;
        transaction
            .execute(
                "INSERT INTO job_summaries (\
                    owner_project_id, job_id, stage_run_id, stage_id, status, attempt, progress, \
                    message, created_at, updated_at\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(owner_project_id, job_id) DO UPDATE SET \
                    stage_run_id = excluded.stage_run_id, \
                    stage_id = excluded.stage_id, \
                    status = excluded.status, \
                    attempt = excluded.attempt, \
                    progress = excluded.progress, \
                    message = excluded.message, \
                    updated_at = excluded.updated_at",
                params![
                    descriptor.project_id,
                    job.job_id,
                    job.stage_run_id,
                    job.stage_id,
                    job.status.as_str(),
                    job.attempt,
                    job.progress,
                    job.message,
                    created_at,
                    updated_at,
                ],
            )
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "更新任务摘要索引失败",
                    &error,
                )
            })?;
        transaction.commit().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "提交任务索引事务失败",
                &error,
            )
        })
    }

    pub fn list_indexed_jobs(
        &self,
        options: ListIndexedJobsOptions,
    ) -> Result<IndexedJobsResultData, StorageServiceError> {
        let operation = StorageOperation::ListIndexedJobs;
        if !(1..=200).contains(&options.limit) {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "任务摘要 limit 必须位于 1..=200。",
            ));
        }
        if options
            .owner_project_id
            .as_deref()
            .is_some_and(|project_id| project_id.trim().is_empty())
        {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "ownerProjectId 过滤器不能为空。",
            ));
        }
        let unique_statuses = options.statuses.iter().copied().collect::<HashSet<_>>();
        if unique_statuses.len() != options.statuses.len() {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "任务状态过滤器不能包含重复值。",
            ));
        }

        let (sql, values) = build_job_query(&options);
        let _index_guard = self.index_guard();
        let connection = self.open_index(operation)?;
        let mut statement = connection.prepare(&sql).map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "准备任务摘要查询失败",
                &error,
            )
        })?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "执行任务摘要查询失败",
                    &error,
                )
            })?;
        let mut jobs = Vec::new();
        for row in rows {
            let (
                owner_project_id,
                job_id,
                stage_run_id,
                stage_id,
                status,
                attempt,
                progress,
                message,
                created_at,
                updated_at,
            ) = row.map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "读取任务摘要失败",
                    &error,
                )
            })?;
            let status = IndexedJobStatusData::parse(&status).ok_or_else(|| {
                StorageServiceError::new(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    "任务索引包含未知状态。",
                )
                .at_path(&self.inner.index_path)
            })?;
            let attempt = u32::try_from(attempt).map_err(|_| {
                StorageServiceError::new(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    "任务索引包含非法 attempt。",
                )
                .at_path(&self.inner.index_path)
            })?;
            jobs.push(IndexedJobData {
                owner_project_id,
                job_id,
                stage_run_id,
                stage_id,
                status,
                attempt,
                progress,
                message,
                created_at,
                updated_at,
            });
        }

        Ok(IndexedJobsResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            jobs,
        })
    }

    pub fn forget_project(
        &self,
        owner_project_id: &str,
    ) -> Result<ForgetProjectResultData, StorageServiceError> {
        let operation = StorageOperation::ForgetProject;
        if owner_project_id.trim().is_empty() {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidRequest,
                operation,
                "ownerProjectId 不能为空。",
            ));
        }
        let _index_guard = self.index_guard();
        let connection = self.open_index(operation)?;
        let removed = connection
            .execute(
                "DELETE FROM recent_projects WHERE project_id = ?1",
                params![owner_project_id],
            )
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "移除项目索引失败",
                    &error,
                )
            })?
            > 0;
        Ok(ForgetProjectResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: owner_project_id.to_owned(),
            removed,
        })
    }

    pub fn clean_project_cache(
        &self,
        project_path: impl AsRef<Path>,
        expected_project_id: &str,
    ) -> Result<CacheCleanupResultData, StorageServiceError> {
        let operation = StorageOperation::CleanProjectCache;
        let _project_guard = self.inner.project_service.operation_guard();
        let descriptor = self.open_project_unlocked(project_path.as_ref(), operation)?;
        require_project_identity(&descriptor, expected_project_id, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let cache_dir = project_dir.join("cache");
        let entries = scan_cache(
            &cache_dir,
            self.inner.max_cache_entries,
            self.inner.max_cache_bytes,
            self.inner.max_cache_depth,
            operation,
        )?;
        let entries_removed = entries.len() as u64;
        let bytes_removed = entries.iter().map(|entry| entry.byte_length).sum();
        remove_cache_entries(entries, operation)?;
        Ok(CacheCleanupResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            entries_removed,
            bytes_removed,
        })
    }

    fn read_artifact_unlocked(
        &self,
        descriptor: &ProjectDescriptorData,
        artifact_id: &str,
        operation: StorageOperation,
    ) -> Result<ArtifactReadResultData, StorageServiceError> {
        validate_artifact_id(artifact_id, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        let metadata_uri = format!("artifacts/metadata/{artifact_id}.json");
        let metadata_path = portable_uri_to_project_path(&project_dir, &metadata_uri, operation)?;
        let artifact = read_artifact_metadata(&project_dir, &metadata_path, operation)?;
        if required_string(&artifact, "artifactId", operation)? != artifact_id {
            return Err(StorageServiceError::new(
                StorageErrorCode::ArtifactConflict,
                operation,
                "Artifact 元数据文件名与 artifactId 不一致。",
            )
            .at_path(&metadata_path)
            .for_artifact(artifact_id));
        }
        let content_hash = required_string(&artifact, "contentHash", operation)?;
        let content_uri = content_uri_for_hash(&content_hash, operation)?;
        if required_string(&artifact, "uri", operation)? != content_uri {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidArtifact,
                operation,
                "Artifact uri 必须与 SHA-256 内容寻址路径完全一致。",
            )
            .at_path(&metadata_path)
            .for_artifact(artifact_id));
        }
        let content_path = portable_uri_to_project_path(&project_dir, &content_uri, operation)?;
        let content_available = regular_file_available(&project_dir, &content_path, operation)?;

        Ok(ArtifactReadResultData {
            api_version: STORAGE_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id.clone(),
            artifact,
            metadata_uri,
            content_uri,
            content_available,
        })
    }

    pub(crate) fn read_artifact_for_workflow_unlocked(
        &self,
        descriptor: &ProjectDescriptorData,
        artifact_id: &str,
    ) -> Result<ArtifactReadResultData, StorageServiceError> {
        self.read_artifact_unlocked(descriptor, artifact_id, StorageOperation::GetArtifact)
    }

    fn validate_draft_references_unlocked(
        &self,
        descriptor: &ProjectDescriptorData,
        draft: &ArtifactDraft,
        operation: StorageOperation,
    ) -> Result<(), StorageServiceError> {
        for artifact_id in draft_artifact_references(draft, operation)? {
            let referenced = self.read_artifact_unlocked(descriptor, &artifact_id, operation)?;
            if !referenced.content_available {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ContentCorrupt,
                    operation,
                    "来源 Artifact 的内容对象缺失，不能创建新的派生产物。",
                )
                .for_artifact(artifact_id));
            }
        }
        Ok(())
    }

    fn scan_artifacts_unlocked(
        &self,
        descriptor: &ProjectDescriptorData,
        operation: StorageOperation,
    ) -> Result<Vec<ArtifactIndexRow>, StorageServiceError> {
        let project_dir = PathBuf::from(&descriptor.project_path);
        let metadata_dir = project_dir.join("artifacts").join("metadata");
        match inspect_project_path(&project_dir, &metadata_dir, operation)? {
            Some(metadata) if metadata.is_dir() => {}
            Some(_) => {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "Artifact 元数据路径不是目录。",
                )
                .at_path(&metadata_dir));
            }
            None => return Ok(Vec::new()),
        }
        let paths = collect_bounded_directory_paths(
            &metadata_dir,
            self.inner.max_indexed_artifacts,
            operation,
        )?;

        let mut rows = Vec::with_capacity(paths.len());
        let mut total_metadata_bytes = 0_u64;
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidArtifact,
                    operation,
                    "Artifact 元数据目录只能包含 .json 文件。",
                )
                .at_path(&path));
            }
            let metadata =
                inspect_project_path(&project_dir, &path, operation)?.ok_or_else(|| {
                    StorageServiceError::new(
                        StorageErrorCode::ArtifactNotFound,
                        operation,
                        "Artifact 元数据在扫描过程中消失。",
                    )
                    .at_path(&path)
                })?;
            total_metadata_bytes = total_metadata_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| {
                    StorageServiceError::new(
                        StorageErrorCode::ScanLimitExceeded,
                        operation,
                        "Artifact 元数据扫描字节数溢出。",
                    )
                    .at_path(&metadata_dir)
                })?;
            if total_metadata_bytes > self.inner.max_indexed_metadata_bytes {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ScanLimitExceeded,
                    operation,
                    format!(
                        "同步索引最多扫描 {} 字节 Artifact 元数据。",
                        self.inner.max_indexed_metadata_bytes
                    ),
                )
                .at_path(&metadata_dir));
            }
            let artifact = read_artifact_metadata(&project_dir, &path, operation)?;
            let artifact_id = required_string(&artifact, "artifactId", operation)?;
            if !artifact_id_is_valid(&artifact_id) {
                return Err(invalid_artifact(
                    operation,
                    "Artifact 元数据中的 artifactId 不是安全、可移植的 artifact_ 文件身份。",
                )
                .at_path(&path));
            }
            if path.file_stem().and_then(|value| value.to_str()) != Some(artifact_id.as_str()) {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ArtifactConflict,
                    operation,
                    "Artifact 元数据文件名与 artifactId 不一致。",
                )
                .at_path(&path)
                .for_artifact(artifact_id));
            }
            let content_hash = required_string(&artifact, "contentHash", operation)?;
            let content_uri = content_uri_for_hash(&content_hash, operation)?;
            if required_string(&artifact, "uri", operation)? != content_uri {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidArtifact,
                    operation,
                    "Artifact uri 与内容哈希不一致。",
                )
                .at_path(&path)
                .for_artifact(artifact_id));
            }
            let content_path = portable_uri_to_project_path(&project_dir, &content_uri, operation)?;
            let content_available = regular_file_available(&project_dir, &content_path, operation)?;
            rows.push(ArtifactIndexRow::from_document(
                &artifact,
                path_to_portable_uri(&project_dir, &path, operation)?,
                content_uri,
                content_available,
                operation,
            )?);
        }
        validate_indexed_artifact_references(&rows, operation, &metadata_dir)?;
        Ok(rows)
    }

    fn index_artifact(
        &self,
        descriptor: &ProjectDescriptorData,
        metadata_uri: &str,
        content_uri: &str,
        artifact: &Value,
        content_available: bool,
    ) -> Result<(), StorageServiceError> {
        let operation = StorageOperation::ImportArtifact;
        let row = ArtifactIndexRow::from_document(
            artifact,
            metadata_uri.to_owned(),
            content_uri.to_owned(),
            content_available,
            operation,
        )?;
        let _index_guard = self.index_guard();
        let mut connection = self.open_index(operation)?;
        let transaction = connection.transaction().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "开始 Artifact 索引事务失败",
                &error,
            )
        })?;
        record_project_tx(&transaction, descriptor, operation, &self.inner.index_path)?;
        insert_artifact_tx(
            &transaction,
            &descriptor.project_id,
            &row,
            operation,
            &self.inner.index_path,
        )?;
        transaction.commit().map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "提交 Artifact 索引事务失败",
                &error,
            )
        })
    }

    fn open_project_unlocked(
        &self,
        project_path: impl AsRef<Path>,
        operation: StorageOperation,
    ) -> Result<ProjectDescriptorData, StorageServiceError> {
        self.inner
            .project_service
            .open_project_unlocked(project_path.as_ref(), ProjectOperation::Open)
            .map_err(|error| map_project_error(error, operation))
    }

    fn index_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner
            .index_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn open_index(&self, operation: StorageOperation) -> Result<Connection, StorageServiceError> {
        let parent = self.inner.index_path.parent().ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "SQLite 索引路径缺少父目录。",
            )
            .at_path(&self.inner.index_path)
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            StorageServiceError::io(operation, parent, "创建 SQLite 索引目录失败", &error)
        })?;
        require_safe_directory(parent, operation)?;
        if let Ok(metadata) = fs::symlink_metadata(&self.inner.index_path) {
            if metadata_is_link(&metadata) {
                return Err(StorageServiceError::new(
                    StorageErrorCode::PathContainsSymlink,
                    operation,
                    "SQLite 索引文件不能是符号链接或重解析点。",
                )
                .at_path(&self.inner.index_path));
            }
            if !metadata.is_file() {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "SQLite 索引路径不是普通文件。",
                )
                .at_path(&self.inner.index_path));
            }
        }

        let connection = Connection::open_with_flags(
            &self.inner.index_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                &self.inner.index_path,
                "打开 SQLite 索引失败",
                &error,
            )
        })?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "设置 SQLite busy timeout 失败",
                    &error,
                )
            })?;
        let schema_version =
            read_index_schema_version(&connection, operation, &self.inner.index_path)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "启用 SQLite 外键失败",
                    &error,
                )
            })?;
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "启用 SQLite WAL 失败",
                    &error,
                )
            })?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StorageServiceError::new(
                StorageErrorCode::IndexUnavailable,
                operation,
                format!("SQLite 未进入 WAL 模式，实际模式为 {journal_mode}。"),
            )
            .at_path(&self.inner.index_path));
        }
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|error| {
                StorageServiceError::index(
                    StorageErrorCode::IndexUnavailable,
                    operation,
                    &self.inner.index_path,
                    "设置 SQLite synchronous 失败",
                    &error,
                )
            })?;
        initialize_index_schema(
            &connection,
            schema_version,
            operation,
            &self.inner.index_path,
        )?;
        Ok(connection)
    }
}

struct SourceFileSnapshot {
    path: PathBuf,
    byte_length: u64,
    modified_at: Option<SystemTime>,
}

struct PendingFile {
    path: PathBuf,
    committed: bool,
}

impl PendingFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug)]
struct ArtifactIndexRow {
    artifact_id: String,
    document_project_id: String,
    stage_id: String,
    run_id: String,
    kind: String,
    media_type: Option<String>,
    evidence_role: String,
    origin: String,
    content_hash: String,
    byte_length: u64,
    metadata_uri: String,
    content_uri: String,
    content_available: bool,
    created_at: String,
    reference_ids: Vec<String>,
}

impl ArtifactIndexRow {
    fn from_document(
        artifact: &Value,
        metadata_uri: String,
        content_uri: String,
        content_available: bool,
        operation: StorageOperation,
    ) -> Result<Self, StorageServiceError> {
        let source = artifact
            .get("source")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_artifact(operation, "Artifact 缺少 source 对象。"))?;
        let origin = source
            .get("origin")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_artifact(operation, "Artifact source 缺少 origin。"))?;
        Ok(Self {
            artifact_id: required_string(artifact, "artifactId", operation)?,
            document_project_id: required_string(artifact, "projectId", operation)?,
            stage_id: required_string(artifact, "stageId", operation)?,
            run_id: required_string(artifact, "runId", operation)?,
            kind: required_string(artifact, "kind", operation)?,
            media_type: artifact
                .get("mediaType")
                .and_then(Value::as_str)
                .map(str::to_owned),
            evidence_role: required_string(artifact, "evidenceRole", operation)?,
            origin: origin.to_owned(),
            content_hash: required_string(artifact, "contentHash", operation)?,
            byte_length: required_u64(artifact, "byteLength", operation)?,
            metadata_uri,
            content_uri,
            content_available,
            created_at: required_string(artifact, "createdAt", operation)?,
            reference_ids: artifact_references_from_value(artifact, operation)?,
        })
    }
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    is_directory: bool,
    depth: usize,
    byte_length: u64,
}

fn map_project_error(
    error: ProjectServiceError,
    operation: StorageOperation,
) -> StorageServiceError {
    let code = match error.code {
        ProjectErrorCode::InvalidRequest => StorageErrorCode::InvalidRequest,
        ProjectErrorCode::PathContainsSymlink => StorageErrorCode::PathContainsSymlink,
        ProjectErrorCode::ProjectNotFound | ProjectErrorCode::MarkerMissing => {
            StorageErrorCode::ProjectNotFound
        }
        ProjectErrorCode::InvalidPath => StorageErrorCode::InvalidPath,
        ProjectErrorCode::MarkerTooLarge | ProjectErrorCode::InvalidProject => {
            StorageErrorCode::InvalidProject
        }
        ProjectErrorCode::MigrationRequired => StorageErrorCode::MigrationRequired,
        ProjectErrorCode::UnsupportedNewerVersion => StorageErrorCode::UnsupportedNewerVersion,
        ProjectErrorCode::IoError => StorageErrorCode::IoError,
        _ => StorageErrorCode::InvalidPath,
    };
    let mut mapped = StorageServiceError::new(code, operation, error.message);
    mapped.path = error.path;
    mapped
}

fn require_project_identity(
    descriptor: &ProjectDescriptorData,
    expected_project_id: &str,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if descriptor.project_id == expected_project_id {
        Ok(())
    } else {
        Err(StorageServiceError::new(
            StorageErrorCode::ProjectIdentityMismatch,
            operation,
            "项目身份与调用方确认值不一致，拒绝写入。",
        )
        .at_path(Path::new(&descriptor.project_path)))
    }
}

fn redact_media_source_error(mut error: StorageServiceError) -> StorageServiceError {
    error.path = None;
    error
}

fn validate_media_source_limit(
    max_bytes: u64,
    service_max_bytes: u64,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if max_bytes == 0 || max_bytes > service_max_bytes {
        Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            format!("媒体源同步上限必须位于 1..={service_max_bytes} 字节。"),
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    content_hash: &str,
    operation: StorageOperation,
) -> Result<&str, StorageServiceError> {
    let Some(hex) = content_hash.strip_prefix("sha256:") else {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            "媒体源 SHA-256 必须使用 sha256:<hex> 格式。",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            "媒体源 SHA-256 必须是 64 位小写十六进制。",
        ));
    }
    Ok(hex)
}

fn reject_source_path_links(
    source_path: &Path,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if source_path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "媒体源路径不能包含当前目录或父目录组件。",
        ));
    }
    for ancestor in source_path
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
    {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata_is_link(&metadata) => {
                return Err(StorageServiceError::new(
                    StorageErrorCode::PathContainsSymlink,
                    operation,
                    "媒体源路径不能经过符号链接或重解析点。",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(StorageServiceError::io(
                    operation,
                    ancestor,
                    "检查媒体源路径失败",
                    &error,
                ));
            }
        }
    }
    Ok(())
}

fn safe_media_source_file_name(
    source_path: &Path,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    const MAX_STAGED_FILE_NAME_BYTES: usize = 200;
    let original = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "媒体源必须包含有效的 Unicode 文件名。",
            )
        })?;
    let encoded = percent_encode_file_name(original.as_bytes());
    let full = format!("source-{encoded}");
    if full.len() <= MAX_STAGED_FILE_NAME_BYTES {
        return Ok(full);
    }

    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| percent_encode_file_name(value.as_bytes()))
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if extension.len() > 32 {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "媒体源扩展名编码后超过安全上限。",
        ));
    }
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(original);
    let name_hash = Sha256::digest(original.as_bytes());
    let suffix = format!("-{}", hex_prefix(&name_hash, 16));
    let extension_suffix = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };
    let prefix = "source-";
    let stem_budget = MAX_STAGED_FILE_NAME_BYTES
        .checked_sub(prefix.len() + suffix.len() + extension_suffix.len())
        .ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "媒体源文件名无法编码到安全上限内。",
            )
        })?;
    let encoded_stem = percent_encode_file_name_bounded(stem.as_bytes(), stem_budget);
    if encoded_stem.is_empty() {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "媒体源文件名编码后为空。",
        ));
    }
    Ok(format!("{prefix}{encoded_stem}{suffix}{extension_suffix}"))
}

fn percent_encode_file_name(bytes: &[u8]) -> String {
    percent_encode_file_name_bounded(bytes, usize::MAX)
}

fn percent_encode_file_name_bounded(bytes: &[u8], max_length: usize) -> String {
    let mut encoded = String::new();
    for byte in bytes {
        let token_length = if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            1
        } else {
            3
        };
        if encoded.len().saturating_add(token_length) > max_length {
            break;
        }
        if token_length == 1 {
            encoded.push(char::from(*byte));
        } else {
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    let mut output = String::with_capacity(length);
    for byte in bytes {
        if output.len() >= length {
            break;
        }
        let _ = write!(&mut output, "{byte:02x}");
    }
    output.truncate(length);
    output
}

fn media_source_uri(
    content_hash: &str,
    source_file_name: &str,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    let hex = validate_sha256(content_hash, operation)?;
    if !valid_staged_media_file_name(source_file_name) {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "媒体源 staging 文件名不安全。",
        ));
    }
    Ok(format!(
        "requests/media-sources/sha256/{}/{hex}/{source_file_name}",
        &hex[..2]
    ))
}

fn parse_media_source_uri(
    uri: &str,
    operation: StorageOperation,
) -> Result<(&str, String), StorageServiceError> {
    let parts = uri.split('/').collect::<Vec<_>>();
    if parts.len() != 6
        || parts[..3] != ["requests", "media-sources", "sha256"]
        || parts[3].len() != 2
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "staged 媒体源 URI 不符合版本化 content-addressed 布局。",
        ));
    }
    let content_hash = format!("sha256:{}", parts[4]);
    let hex = validate_sha256(&content_hash, operation)?;
    if parts[3] != &hex[..2] || !valid_staged_media_file_name(parts[5]) {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "staged 媒体源 URI 的哈希前缀或安全文件名无效。",
        ));
    }
    Ok((parts[4], parts[5].to_owned()))
}

fn valid_staged_media_file_name(value: &str) -> bool {
    if !value.starts_with("source-") || value.len() > 200 {
        return false;
    }
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            index += 1;
            continue;
        }
        if byte != b'%'
            || index + 2 >= bytes.len()
            || !bytes[index + 1].is_ascii_hexdigit()
            || !bytes[index + 2].is_ascii_hexdigit()
            || bytes[index + 1].is_ascii_lowercase()
            || bytes[index + 2].is_ascii_lowercase()
        {
            return false;
        }
        let decoded = (hex_value(bytes[index + 1]) << 4) | hex_value(bytes[index + 2]);
        if decoded.is_ascii_alphanumeric()
            || matches!(decoded, b'.' | b'_' | b'-' | b'/' | b'\\')
            || decoded < 0x20
            || decoded == 0x7f
        {
            return false;
        }
        index += 3;
    }
    true
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'A'..=b'F' => byte - b'A' + 10,
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

fn inspect_source_file(
    source_path: &Path,
    operation: StorageOperation,
) -> Result<SourceFileSnapshot, StorageServiceError> {
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StorageServiceError::new(
                StorageErrorCode::SourceNotFound,
                operation,
                "待导入文件不存在。",
            )
            .at_path(source_path)
        } else {
            StorageServiceError::io(operation, source_path, "读取待导入文件失败", &error)
        }
    })?;
    if metadata_is_link(&metadata) {
        return Err(StorageServiceError::new(
            StorageErrorCode::PathContainsSymlink,
            operation,
            "待导入文件不能是符号链接或重解析点。",
        )
        .at_path(source_path));
    }
    if !metadata.is_file() {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "待导入路径不是普通文件。",
        )
        .at_path(source_path));
    }
    let path = fs::canonicalize(source_path).map_err(|error| {
        StorageServiceError::io(operation, source_path, "规范化待导入文件失败", &error)
    })?;
    Ok(SourceFileSnapshot {
        path,
        byte_length: metadata.len(),
        modified_at: metadata.modified().ok(),
    })
}

fn copy_and_hash_source(
    source: &SourceFileSnapshot,
    temporary_path: &Path,
    max_bytes: u64,
    operation: StorageOperation,
) -> Result<(String, u64), StorageServiceError> {
    let source_file = File::open(&source.path).map_err(|error| {
        StorageServiceError::io(operation, &source.path, "打开待导入文件失败", &error)
    })?;
    let temporary_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary_path)
        .map_err(|error| {
            StorageServiceError::io(
                operation,
                temporary_path,
                "创建 Artifact 临时文件失败",
                &error,
            )
        })?;
    let mut reader = BufReader::new(source_file);
    let mut writer = BufWriter::new(temporary_file);
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            StorageServiceError::io(operation, &source.path, "读取待导入文件失败", &error)
        })?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::SourceTooLarge,
                operation,
                "待导入文件长度溢出。",
            )
            .at_path(&source.path)
        })?;
        if total > max_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::SourceTooLarge,
                operation,
                format!("同步导入上限为 {max_bytes} 字节。"),
            )
            .at_path(&source.path));
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read]).map_err(|error| {
            StorageServiceError::io(
                operation,
                temporary_path,
                "写入 Artifact 临时文件失败",
                &error,
            )
        })?;
    }
    writer.flush().map_err(|error| {
        StorageServiceError::io(
            operation,
            temporary_path,
            "刷新 Artifact 临时文件失败",
            &error,
        )
    })?;
    writer.get_ref().sync_all().map_err(|error| {
        StorageServiceError::io(
            operation,
            temporary_path,
            "同步 Artifact 临时文件失败",
            &error,
        )
    })?;

    let after = fs::symlink_metadata(&source.path).map_err(|error| {
        StorageServiceError::io(operation, &source.path, "复核待导入文件失败", &error)
    })?;
    if metadata_is_link(&after)
        || !after.is_file()
        || after.len() != source.byte_length
        || after.modified().ok() != source.modified_at
        || total != source.byte_length
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::SourceChanged,
            operation,
            "待导入文件在读取过程中发生变化，请重试。",
        )
        .at_path(&source.path));
    }

    let digest = hasher.finalize();
    Ok((format_sha256(&digest), total))
}

fn persist_content_noclobber(source: &Path, destination: &Path) -> std::io::Result<()> {
    let temporary = tempfile::TempPath::try_from_path(source.to_path_buf())?;
    match temporary.persist_noclobber(destination) {
        Ok(()) => match fs::remove_file(source) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
        Err(mut persist_error) => {
            // The caller owns cleanup and must retain the source for collision verification.
            // Disable TempPath's drop cleanup before returning the original no-clobber error.
            persist_error.path.disable_cleanup(true);
            Err(persist_error.error)
        }
    }
}

fn hash_file(
    project_dir: &Path,
    path: &Path,
    max_bytes: u64,
    operation: StorageOperation,
) -> Result<(String, u64), StorageServiceError> {
    let metadata = inspect_project_path(project_dir, path, operation)?.ok_or_else(|| {
        StorageServiceError::new(
            StorageErrorCode::ContentCorrupt,
            operation,
            "Artifact 内容不存在。",
        )
        .at_path(path)
    })?;
    if !metadata.is_file() {
        return Err(StorageServiceError::new(
            StorageErrorCode::ContentCorrupt,
            operation,
            "Artifact 内容不是普通文件。",
        )
        .at_path(path));
    }
    if metadata.len() > max_bytes {
        return Err(StorageServiceError::new(
            StorageErrorCode::ArtifactTooLarge,
            operation,
            format!(
                "同步 Artifact 校验上限为 {max_bytes} 字节；更大的校验必须交给持久化任务队列。"
            ),
        )
        .at_path(path));
    }
    let file = File::open(path).map_err(|error| {
        StorageServiceError::io(operation, path, "打开 Artifact 内容失败", &error)
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            StorageServiceError::io(operation, path, "读取 Artifact 内容失败", &error)
        })?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "Artifact 内容长度溢出。",
            )
            .at_path(path)
        })?;
        if total > max_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::ArtifactTooLarge,
                operation,
                format!("同步 Artifact 校验上限为 {max_bytes} 字节。"),
            )
            .at_path(path));
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok((format_sha256(&digest), total))
}

fn read_file_bounded(
    project_dir: &Path,
    path: &Path,
    max_bytes: u64,
    operation: StorageOperation,
) -> Result<(Vec<u8>, String, u64), StorageServiceError> {
    let metadata = inspect_project_path(project_dir, path, operation)?.ok_or_else(|| {
        StorageServiceError::new(
            StorageErrorCode::ContentCorrupt,
            operation,
            "Artifact 内容不存在。",
        )
        .at_path(path)
    })?;
    if !metadata.is_file() {
        return Err(StorageServiceError::new(
            StorageErrorCode::ContentCorrupt,
            operation,
            "Artifact 内容不是普通文件。",
        )
        .at_path(path));
    }
    if metadata.len() > max_bytes {
        return Err(StorageServiceError::new(
            StorageErrorCode::ArtifactTooLarge,
            operation,
            format!("同步 Artifact 读取上限为 {max_bytes} 字节。"),
        )
        .at_path(path));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::ArtifactTooLarge,
            operation,
            "Artifact 字节数超出当前平台可分配范围。",
        )
        .at_path(path)
    })?;
    let file = File::open(path).map_err(|error| {
        StorageServiceError::io(operation, path, "打开 Artifact 内容失败", &error)
    })?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::with_capacity(capacity);
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            StorageServiceError::io(operation, path, "读取 Artifact 内容失败", &error)
        })?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                operation,
                "Artifact 内容长度溢出。",
            )
            .at_path(path)
        })?;
        if total > max_bytes {
            return Err(StorageServiceError::new(
                StorageErrorCode::ArtifactTooLarge,
                operation,
                format!("同步 Artifact 读取上限为 {max_bytes} 字节。"),
            )
            .at_path(path));
        }
        hasher.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok((bytes, format_sha256(&digest), total))
}

fn verify_existing_content(
    project_dir: &Path,
    path: &Path,
    expected_hash: &str,
    expected_length: u64,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    let (actual_hash, actual_length) =
        hash_file(project_dir, path, MAX_SYNCHRONOUS_ARTIFACT_BYTES, operation)?;
    if actual_hash == expected_hash && actual_length == expected_length {
        Ok(())
    } else {
        Err(StorageServiceError::new(
            StorageErrorCode::ContentCorrupt,
            operation,
            "内容寻址路径已存在，但字节数或 SHA-256 与路径不一致。",
        )
        .at_path(path))
    }
}

fn format_sha256(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn content_uri_for_hash(
    content_hash: &str,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    let hex = content_hash.strip_prefix("sha256:").ok_or_else(|| {
        invalid_artifact(
            operation,
            "Artifact contentHash 必须使用 sha256:<hex> 格式。",
        )
    })?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid_artifact(
            operation,
            "Artifact SHA-256 必须是 64 位小写十六进制。",
        ));
    }
    Ok(format!("artifacts/objects/sha256/{}/{}", &hex[..2], hex))
}

struct ArtifactDocumentIdentity<'a> {
    artifact_id: &'a str,
    content_uri: &'a str,
    content_hash: &'a str,
    byte_length: u64,
    created_at: Option<&'a str>,
}

fn build_artifact_document(
    descriptor: &ProjectDescriptorData,
    identity: ArtifactDocumentIdentity<'_>,
    draft: &ArtifactDraft,
    operation: StorageOperation,
) -> Result<Value, StorageServiceError> {
    let draft_value = serde_json::to_value(draft).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            format!("序列化 Artifact 草稿失败：{error}"),
        )
    })?;
    let mut object = draft_value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid_artifact(operation, "Artifact 草稿序列化后不是对象。"))?;
    if let Some(source) = object.get_mut("source").and_then(Value::as_object_mut) {
        if source.get("origin").and_then(Value::as_str) == Some("imported") {
            source.insert(
                "sourceContentHash".to_owned(),
                Value::String(identity.content_hash.to_owned()),
            );
        }
    }
    object.insert(
        "schemaVersion".to_owned(),
        Value::String(narracut_contracts::NARRACUT_CONTRACT_VERSION.to_owned()),
    );
    object.insert(
        "documentType".to_owned(),
        Value::String("artifact".to_owned()),
    );
    object.insert(
        "artifactId".to_owned(),
        Value::String(identity.artifact_id.to_owned()),
    );
    object.insert(
        "projectId".to_owned(),
        Value::String(descriptor.project_id.clone()),
    );
    object.insert(
        "uri".to_owned(),
        Value::String(identity.content_uri.to_owned()),
    );
    object.insert(
        "contentHash".to_owned(),
        Value::String(identity.content_hash.to_owned()),
    );
    object.insert("byteLength".to_owned(), Value::from(identity.byte_length));
    object.insert(
        "createdAt".to_owned(),
        Value::String(match identity.created_at {
            Some(created_at) => created_at.to_owned(),
            None => current_timestamp(operation)?,
        }),
    );
    let artifact = Value::Object(object);
    validate_contract_document(&artifact).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            format!("Artifact 未通过 v1 持久化契约：{error}"),
        )
        .for_artifact(identity.artifact_id)
    })?;
    validate_imported_source_hash(&artifact, operation)?;
    Ok(artifact)
}

fn preflight_artifact_draft(
    descriptor: &ProjectDescriptorData,
    draft: &ArtifactDraft,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    build_artifact_document(
        descriptor,
        ArtifactDocumentIdentity {
            artifact_id: "artifact_00000000000000000000000000000000",
            content_uri: "artifacts/objects/sha256/00/0000000000000000000000000000000000000000000000000000000000000000",
            content_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            byte_length: 0,
            created_at: None,
        },
        draft,
        operation,
    )?;
    Ok(())
}

fn draft_artifact_references(
    draft: &ArtifactDraft,
    operation: StorageOperation,
) -> Result<Vec<String>, StorageServiceError> {
    let value = serde_json::to_value(draft).map_err(|error| {
        invalid_artifact(operation, format!("序列化 Artifact 草稿引用失败：{error}"))
    })?;
    artifact_references_from_value(&value, operation)
}

fn artifact_references_from_value(
    value: &Value,
    operation: StorageOperation,
) -> Result<Vec<String>, StorageServiceError> {
    let source = value
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_artifact(operation, "Artifact 缺少 source 对象。"))?;
    match source.get("origin").and_then(Value::as_str) {
        Some("generated") => Ok(source
            .get("promptArtifactId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect()),
        Some("derived") => source
            .get("sourceArtifactIds")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_artifact(operation, "派生 Artifact 缺少来源 Artifact 列表。"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid_artifact(operation, "来源 Artifact 身份必须是字符串。"))
            })
            .collect(),
        Some("imported") => Ok(Vec::new()),
        _ => Err(invalid_artifact(
            operation,
            "Artifact 包含未知 source.origin。",
        )),
    }
}

fn validate_indexed_artifact_references(
    rows: &[ArtifactIndexRow],
    operation: StorageOperation,
    metadata_dir: &Path,
) -> Result<(), StorageServiceError> {
    let reference_edges = rows.iter().try_fold(0_usize, |total, row| {
        total.checked_add(row.reference_ids.len()).ok_or_else(|| {
            StorageServiceError::new(
                StorageErrorCode::ScanLimitExceeded,
                operation,
                "Artifact 来源引用计数溢出。",
            )
            .at_path(metadata_dir)
        })
    })?;
    if reference_edges > MAX_INDEXED_REFERENCE_EDGES {
        return Err(StorageServiceError::new(
            StorageErrorCode::ScanLimitExceeded,
            operation,
            format!("同步索引最多校验 {MAX_INDEXED_REFERENCE_EDGES} 条 Artifact 来源引用。"),
        )
        .at_path(metadata_dir));
    }
    let by_id = rows
        .iter()
        .map(|row| (row.artifact_id.as_str(), row))
        .collect::<HashMap<_, _>>();
    let mut remaining_dependencies = HashMap::<&str, usize>::new();
    let mut dependents = HashMap::<&str, Vec<&str>>::new();

    for row in rows {
        remaining_dependencies.insert(row.artifact_id.as_str(), row.reference_ids.len());
        for reference_id in &row.reference_ids {
            if !artifact_id_is_valid(reference_id) {
                return Err(invalid_artifact(
                    operation,
                    "Artifact 来源引用不是安全、可移植的 artifact_ 身份。",
                )
                .at_path(metadata_dir));
            }
            let referenced = by_id.get(reference_id.as_str()).ok_or_else(|| {
                StorageServiceError::new(
                    StorageErrorCode::ArtifactNotFound,
                    operation,
                    "Artifact 来源引用未在当前项目中找到。",
                )
                .at_path(metadata_dir)
                .for_artifact(reference_id)
            })?;
            if !referenced.content_available {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ContentCorrupt,
                    operation,
                    "Artifact 来源引用的内容对象缺失。",
                )
                .at_path(metadata_dir)
                .for_artifact(reference_id));
            }
            dependents
                .entry(reference_id.as_str())
                .or_default()
                .push(row.artifact_id.as_str());
        }
    }

    let mut queue = remaining_dependencies
        .iter()
        .filter_map(|(artifact_id, count)| (*count == 0).then_some(*artifact_id))
        .collect::<VecDeque<_>>();
    let mut resolved = 0_usize;
    while let Some(artifact_id) = queue.pop_front() {
        resolved += 1;
        if let Some(children) = dependents.get(artifact_id) {
            for child in children {
                let remaining = remaining_dependencies
                    .get_mut(child)
                    .ok_or_else(|| invalid_artifact(operation, "Artifact 来源图包含未知节点。"))?;
                *remaining = remaining
                    .checked_sub(1)
                    .ok_or_else(|| invalid_artifact(operation, "Artifact 来源图依赖计数下溢。"))?;
                if *remaining == 0 {
                    queue.push_back(child);
                }
            }
        }
    }
    if resolved != rows.len() {
        return Err(StorageServiceError::new(
            StorageErrorCode::ArtifactConflict,
            operation,
            "Artifact 来源图包含循环引用。",
        )
        .at_path(metadata_dir));
    }
    Ok(())
}

fn collect_bounded_directory_paths(
    directory: &Path,
    max_entries: usize,
    operation: StorageOperation,
) -> Result<Vec<PathBuf>, StorageServiceError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        StorageServiceError::io(operation, directory, "读取 Artifact 元数据目录失败", &error)
    })?;
    let mut paths = Vec::with_capacity(max_entries.min(256));
    for entry in entries {
        let entry = entry.map_err(|error| {
            StorageServiceError::io(
                operation,
                directory,
                "读取 Artifact 元数据目录项失败",
                &error,
            )
        })?;
        if paths.len() >= max_entries {
            return Err(StorageServiceError::new(
                StorageErrorCode::ScanLimitExceeded,
                operation,
                format!("单项目同步索引最多处理 {max_entries} 个 Artifact。"),
            )
            .at_path(directory));
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn read_artifact_metadata(
    project_dir: &Path,
    path: &Path,
    operation: StorageOperation,
) -> Result<Value, StorageServiceError> {
    let metadata = inspect_project_path(project_dir, path, operation)?.ok_or_else(|| {
        StorageServiceError::new(
            StorageErrorCode::ArtifactNotFound,
            operation,
            "Artifact 元数据不存在。",
        )
        .at_path(path)
    })?;
    if !metadata.is_file() {
        return Err(invalid_artifact(operation, "Artifact 元数据不是普通文件。").at_path(path));
    }
    if metadata.len() > MAX_ARTIFACT_METADATA_BYTES {
        return Err(invalid_artifact(operation, "Artifact 元数据超过 1 MiB 上限。").at_path(path));
    }
    let bytes = fs::read(path).map_err(|error| {
        StorageServiceError::io(operation, path, "读取 Artifact 元数据失败", &error)
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            format!("Artifact 元数据不是合法 JSON：{error}"),
        )
        .at_path(path)
    })?;
    validate_contract_document(&value).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            format!("Artifact 元数据未通过 v1 契约：{error}"),
        )
        .at_path(path)
    })?;
    if value.get("documentType").and_then(Value::as_str) != Some("artifact") {
        return Err(invalid_artifact(operation, "元数据文档类型不是 artifact。").at_path(path));
    }
    validate_imported_source_hash(&value, operation).map_err(|error| error.at_path(path))?;
    Ok(value)
}

fn validate_imported_source_hash(
    artifact: &Value,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    let source = artifact
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_artifact(operation, "Artifact 缺少 source 对象。"))?;
    if source.get("origin").and_then(Value::as_str) != Some("imported") {
        return Ok(());
    }
    let stored_hash = required_string(artifact, "contentHash", operation)?;
    let source_hash = source
        .get("sourceContentHash")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_artifact(operation, "导入 Artifact 缺少 sourceContentHash。"))?;
    if source_hash != stored_hash {
        return Err(invalid_artifact(
            operation,
            "导入 Artifact 的 sourceContentHash 与实际存储内容哈希不一致。",
        ));
    }
    Ok(())
}

fn validate_media_receipt_id(
    receipt_id: &str,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if receipt_id.len() == 64
        && receipt_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            "媒体导入 receiptId 必须是 64 位小写十六进制摘要。",
        ))
    }
}

fn read_media_receipt_unlocked(
    descriptor: &ProjectDescriptorData,
    receipt_id: &str,
    operation: StorageOperation,
) -> Result<Option<Value>, StorageServiceError> {
    const MAX_MEDIA_RECEIPT_BYTES: u64 = 64 * 1024;

    let project_dir = PathBuf::from(&descriptor.project_path);
    let path = project_dir
        .join("artifacts")
        .join("media-receipts")
        .join(format!("{receipt_id}.json"));
    let Some(metadata) = inspect_project_path(&project_dir, &path, operation)? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.len() > MAX_MEDIA_RECEIPT_BYTES {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidProject,
            operation,
            "媒体导入 receipt 不是有界普通文件。",
        )
        .at_path(&path));
    }
    let file = File::open(&path).map_err(|error| {
        StorageServiceError::io(operation, &path, "打开媒体导入 receipt 失败", &error)
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_MEDIA_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            StorageServiceError::io(operation, &path, "读取媒体导入 receipt 失败", &error)
        })?;
    if bytes.len() as u64 > MAX_MEDIA_RECEIPT_BYTES {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidProject,
            operation,
            "媒体导入 receipt 在读取期间超过上限。",
        )
        .at_path(&path));
    }
    let receipt: Value = serde_json::from_slice(&bytes).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::InvalidProject,
            operation,
            "媒体导入 receipt 不是合法 JSON。",
        )
        .at_path(&path)
    })?;
    if receipt.get("documentType").and_then(Value::as_str) != Some("media_import_receipt")
        || receipt.get("projectId").and_then(Value::as_str) != Some(descriptor.project_id.as_str())
        || receipt.get("receiptId").and_then(Value::as_str) != Some(receipt_id)
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidProject,
            operation,
            "媒体导入 receipt 身份与当前项目不一致。",
        )
        .at_path(&path));
    }
    Ok(Some(receipt))
}

fn write_json_atomic(
    path: &Path,
    value: &Value,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            format!("序列化 Artifact 元数据失败：{error}"),
        )
        .at_path(path)
    })?;
    bytes.push(b'\n');
    let mut file = AtomicWriteFile::options().open(path).map_err(|error| {
        StorageServiceError::io(operation, path, "创建 Artifact 原子写入文件失败", &error)
    })?;
    file.write_all(&bytes).map_err(|error| {
        StorageServiceError::io(operation, path, "写入 Artifact 原子临时文件失败", &error)
    })?;
    file.commit().map_err(|error| {
        StorageServiceError::io(operation, path, "提交 Artifact 元数据失败", &error)
    })
}

fn required_string(
    value: &Value,
    field: &str,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_artifact(operation, format!("Artifact 缺少字符串字段 {field}。")))
}

fn required_u64(
    value: &Value,
    field: &str,
    operation: StorageOperation,
) -> Result<u64, StorageServiceError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_artifact(operation, format!("Artifact 缺少整数数字段 {field}。")))
}

fn invalid_artifact(
    operation: StorageOperation,
    message: impl Into<String>,
) -> StorageServiceError {
    StorageServiceError::new(StorageErrorCode::InvalidArtifact, operation, message)
}

fn validate_artifact_id(
    artifact_id: &str,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if artifact_id_is_valid(artifact_id) {
        Ok(())
    } else {
        Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            "artifactId 必须以 artifact_ 开头，后续只能包含 ASCII 字母、数字、点、下划线和连字符。",
        ))
    }
}

fn artifact_id_is_valid(artifact_id: &str) -> bool {
    let Some(suffix) = artifact_id.strip_prefix("artifact_") else {
        return false;
    };
    let valid_length = (1..=151).contains(&suffix.len());
    let mut bytes = suffix.bytes();
    let first_valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric());
    let rest_valid =
        bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    valid_length && first_valid && rest_valid
}

fn ensure_destination_absent(
    path: &Path,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(StorageServiceError::new(
            StorageErrorCode::ArtifactConflict,
            operation,
            "Artifact 元数据目标已存在。",
        )
        .at_path(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StorageServiceError::io(
            operation,
            path,
            "检查 Artifact 元数据目标失败",
            &error,
        )),
    }
}

fn ensure_project_directories(
    project_dir: &Path,
    components: &[&str],
    operation: StorageOperation,
) -> Result<PathBuf, StorageServiceError> {
    let mut current = project_dir.to_path_buf();
    for component in components {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata_is_link(&metadata) {
                    return Err(StorageServiceError::new(
                        StorageErrorCode::PathContainsSymlink,
                        operation,
                        "项目存储目录不能是符号链接或重解析点。",
                    )
                    .at_path(&current));
                }
                if !metadata.is_dir() {
                    return Err(StorageServiceError::new(
                        StorageErrorCode::InvalidPath,
                        operation,
                        "项目存储路径不是目录。",
                    )
                    .at_path(&current));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    StorageServiceError::io(operation, &current, "创建项目存储目录失败", &error)
                })?;
                require_safe_directory(&current, operation)?;
            }
            Err(error) => {
                return Err(StorageServiceError::io(
                    operation,
                    &current,
                    "检查项目存储目录失败",
                    &error,
                ));
            }
        }
    }
    let canonical = fs::canonicalize(&current).map_err(|error| {
        StorageServiceError::io(operation, &current, "规范化项目存储目录失败", &error)
    })?;
    if !canonical.starts_with(project_dir) {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "项目存储目录逃逸出项目根目录。",
        )
        .at_path(&canonical));
    }
    Ok(canonical)
}

fn ensure_project_directories_from_path(
    project_dir: &Path,
    directory: &Path,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    let relative = directory.strip_prefix(project_dir).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "Artifact 目录不在项目根目录内。",
        )
        .at_path(directory)
    })?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().ok_or_else(|| {
                StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "Artifact 目录包含非 Unicode 路径组件。",
                )
                .at_path(directory)
            }),
            _ => Err(StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "Artifact 目录包含非法路径组件。",
            )
            .at_path(directory)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    ensure_project_directories(project_dir, &components, operation)?;
    Ok(())
}

fn portable_uri_to_project_path(
    project_dir: &Path,
    uri: &str,
    operation: StorageOperation,
) -> Result<PathBuf, StorageServiceError> {
    if uri.is_empty() || uri.contains('\\') || uri.starts_with('/') {
        return Err(invalid_artifact(
            operation,
            "Artifact URI 不是安全的项目相对路径。",
        ));
    }
    let mut path = project_dir.to_path_buf();
    for component in uri.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(invalid_artifact(
                operation,
                "Artifact URI 包含空、当前或父级路径组件。",
            ));
        }
        path.push(component);
    }
    if !path.starts_with(project_dir) {
        return Err(invalid_artifact(
            operation,
            "Artifact URI 逃逸出项目根目录。",
        ));
    }
    Ok(path)
}

fn path_to_portable_uri(
    project_dir: &Path,
    path: &Path,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    let relative = path.strip_prefix(project_dir).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "Artifact 元数据路径不在项目根目录内。",
        )
        .at_path(path)
    })?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned).ok_or_else(|| {
                StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "Artifact 路径包含非 Unicode 组件。",
                )
                .at_path(path)
            }),
            _ => Err(StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "Artifact 路径包含非法组件。",
            )
            .at_path(path)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(components.join("/"))
}

fn inspect_project_path(
    project_dir: &Path,
    path: &Path,
    operation: StorageOperation,
) -> Result<Option<fs::Metadata>, StorageServiceError> {
    let relative = path.strip_prefix(project_dir).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "Artifact 路径不在项目根目录内。",
        )
        .at_path(path)
    })?;
    let project_metadata = fs::symlink_metadata(project_dir).map_err(|error| {
        StorageServiceError::io(operation, project_dir, "读取项目根目录失败", &error)
    })?;
    if metadata_is_link(&project_metadata) {
        return Err(StorageServiceError::new(
            StorageErrorCode::PathContainsSymlink,
            operation,
            "项目根目录不能是符号链接或重解析点。",
        )
        .at_path(project_dir));
    }
    if !project_metadata.is_dir() {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "项目根路径不是目录。",
        )
        .at_path(project_dir));
    }

    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Ok(Some(project_metadata));
    }

    let mut current = project_dir.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "Artifact 路径包含非法组件。",
            )
            .at_path(path));
        };
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(StorageServiceError::io(
                    operation,
                    &current,
                    "逐级检查 Artifact 路径失败",
                    &error,
                ));
            }
        };
        if metadata_is_link(&metadata) {
            return Err(StorageServiceError::new(
                StorageErrorCode::PathContainsSymlink,
                operation,
                "Artifact 路径不能经过符号链接或重解析点。",
            )
            .at_path(&current));
        }
        let is_final = index + 1 == components.len();
        if !is_final && !metadata.is_dir() {
            return Err(StorageServiceError::new(
                StorageErrorCode::InvalidPath,
                operation,
                "Artifact 路径的中间组件不是目录。",
            )
            .at_path(&current));
        }
        if is_final {
            return Ok(Some(metadata));
        }
    }

    unreachable!("non-empty component list must return from the loop")
}

fn regular_file_available(
    project_dir: &Path,
    path: &Path,
    operation: StorageOperation,
) -> Result<bool, StorageServiceError> {
    match inspect_project_path(project_dir, path, operation)? {
        Some(metadata) => {
            if metadata.is_file() {
                Ok(true)
            } else {
                Err(StorageServiceError::new(
                    StorageErrorCode::ContentCorrupt,
                    operation,
                    "Artifact 内容路径不是普通文件。",
                )
                .at_path(path))
            }
        }
        None => Ok(false),
    }
}

fn require_safe_directory(
    path: &Path,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| StorageServiceError::io(operation, path, "读取目录元数据失败", &error))?;
    if metadata_is_link(&metadata) {
        return Err(StorageServiceError::new(
            StorageErrorCode::PathContainsSymlink,
            operation,
            "目录不能是符号链接或重解析点。",
        )
        .at_path(path));
    }
    if !metadata.is_dir() {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidPath,
            operation,
            "路径不是目录。",
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

fn current_timestamp(operation: StorageOperation) -> Result<String, StorageServiceError> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::IoError,
            operation,
            format!("生成 RFC 3339 时间失败：{error}"),
        )
    })
}

fn recent_project_path_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata_is_link(&metadata))
        .unwrap_or(false)
        && fs::symlink_metadata(path.join(crate::PROJECT_MARKER_FILE))
            .map(|metadata| metadata.is_file() && !metadata_is_link(&metadata))
            .unwrap_or(false)
}

fn read_index_schema_version(
    connection: &Connection,
    operation: StorageOperation,
    index_path: &Path,
) -> Result<i64, StorageServiceError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                index_path,
                "读取 SQLite 索引版本失败",
                &error,
            )
        })?;
    if version > INDEX_SCHEMA_VERSION {
        return Err(StorageServiceError::new(
            StorageErrorCode::IndexMigrationFailed,
            operation,
            format!("SQLite 索引版本 {version} 高于当前支持版本 {INDEX_SCHEMA_VERSION}。"),
        )
        .at_path(index_path));
    }
    if !matches!(version, 0 | 1 | INDEX_SCHEMA_VERSION) {
        return Err(StorageServiceError::new(
            StorageErrorCode::IndexMigrationFailed,
            operation,
            format!("没有从 SQLite 索引版本 {version} 开始的迁移路径。"),
        )
        .at_path(index_path));
    }
    Ok(version)
}

fn initialize_index_schema(
    connection: &Connection,
    version: i64,
    operation: StorageOperation,
    index_path: &Path,
) -> Result<(), StorageServiceError> {
    match version {
        INDEX_SCHEMA_VERSION => return Ok(()),
        1 => return migrate_index_v1_to_v2(connection, operation, index_path),
        0 => {}
        _ => unreachable!("index version was checked before initialization"),
    }

    connection
        .execute_batch(
            "BEGIN IMMEDIATE;\
             CREATE TABLE recent_projects (\
                project_id TEXT PRIMARY KEY NOT NULL,\
                project_path TEXT UNIQUE NOT NULL,\
                name TEXT NOT NULL,\
                workflow_definition_id TEXT NOT NULL,\
                project_format_version INTEGER NOT NULL CHECK(project_format_version >= 0),\
                archived INTEGER NOT NULL CHECK(archived IN (0, 1)),\
                last_opened_at TEXT NOT NULL,\
                marker_updated_at TEXT NOT NULL\
             );\
             CREATE INDEX recent_projects_last_opened_idx \
                ON recent_projects(last_opened_at DESC);\
             CREATE TABLE artifacts (\
                owner_project_id TEXT NOT NULL,\
                artifact_id TEXT NOT NULL,\
                document_project_id TEXT NOT NULL,\
                stage_id TEXT NOT NULL,\
                run_id TEXT NOT NULL,\
                kind TEXT NOT NULL,\
                media_type TEXT,\
                evidence_role TEXT NOT NULL,\
                origin TEXT NOT NULL,\
                content_hash TEXT NOT NULL,\
                byte_length INTEGER NOT NULL CHECK(byte_length >= 0),\
                metadata_uri TEXT NOT NULL,\
                content_uri TEXT NOT NULL,\
                content_available INTEGER NOT NULL CHECK(content_available IN (0, 1)),\
                created_at TEXT NOT NULL,\
                indexed_at TEXT NOT NULL,\
                PRIMARY KEY(owner_project_id, artifact_id),\
                FOREIGN KEY(owner_project_id) REFERENCES recent_projects(project_id) ON DELETE CASCADE\
             );\
             CREATE INDEX artifacts_content_hash_idx ON artifacts(content_hash);\
             CREATE INDEX artifacts_stage_run_idx \
                ON artifacts(owner_project_id, stage_id, run_id);\
             CREATE TABLE job_summaries (\
                owner_project_id TEXT NOT NULL,\
                job_id TEXT NOT NULL,\
                stage_run_id TEXT NOT NULL,\
                stage_id TEXT NOT NULL,\
                status TEXT NOT NULL CHECK(status IN (\
                    'queued', 'running', 'retrying', 'succeeded', 'failed', 'canceled'\
                )),\
                attempt INTEGER NOT NULL CHECK(attempt >= 1),\
                progress REAL NOT NULL CHECK(progress >= 0 AND progress <= 1),\
                message TEXT,\
                created_at TEXT NOT NULL,\
                updated_at TEXT NOT NULL,\
                PRIMARY KEY(owner_project_id, job_id),\
                FOREIGN KEY(owner_project_id) REFERENCES recent_projects(project_id) ON DELETE CASCADE\
             );\
             CREATE INDEX job_summaries_updated_idx \
                ON job_summaries(updated_at DESC, job_id ASC);\
             PRAGMA user_version = 2;\
             COMMIT;",
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexMigrationFailed,
                operation,
                index_path,
                "创建 SQLite 索引 Schema 失败",
                &error,
            )
        })
}

fn migrate_index_v1_to_v2(
    connection: &Connection,
    operation: StorageOperation,
    index_path: &Path,
) -> Result<(), StorageServiceError> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;\
             DROP INDEX IF EXISTS job_summaries_updated_idx;\
             DELETE FROM job_summaries;\
             CREATE INDEX job_summaries_updated_idx \
                ON job_summaries(updated_at DESC, job_id ASC);\
             PRAGMA user_version = 2;\
             COMMIT;",
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexMigrationFailed,
                operation,
                index_path,
                "迁移 SQLite 索引 v1 → v2 失败",
                &error,
            )
        })
}

fn record_project_tx(
    transaction: &Transaction<'_>,
    descriptor: &ProjectDescriptorData,
    operation: StorageOperation,
    index_path: &Path,
) -> Result<(), StorageServiceError> {
    let now = current_timestamp(operation)?;
    transaction
        .execute(
            "DELETE FROM recent_projects WHERE project_path = ?1 AND project_id <> ?2",
            params![descriptor.project_path, descriptor.project_id],
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                index_path,
                "清理路径冲突的最近项目记录失败",
                &error,
            )
        })?;
    transaction
        .execute(
            "INSERT INTO recent_projects (\
                project_id, project_path, name, workflow_definition_id, project_format_version, \
                archived, last_opened_at, marker_updated_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(project_id) DO UPDATE SET \
                project_path = excluded.project_path, \
                name = excluded.name, \
                workflow_definition_id = excluded.workflow_definition_id, \
                project_format_version = excluded.project_format_version, \
                archived = excluded.archived, \
                last_opened_at = excluded.last_opened_at, \
                marker_updated_at = excluded.marker_updated_at",
            params![
                descriptor.project_id,
                descriptor.project_path,
                descriptor.name,
                descriptor.workflow_definition_id,
                descriptor.project_format_version,
                i64::from(descriptor.archived),
                now,
                descriptor.updated_at,
            ],
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                index_path,
                "写入最近项目索引失败",
                &error,
            )
        })?;
    Ok(())
}

fn insert_artifact_tx(
    transaction: &Transaction<'_>,
    owner_project_id: &str,
    artifact: &ArtifactIndexRow,
    operation: StorageOperation,
    index_path: &Path,
) -> Result<(), StorageServiceError> {
    let indexed_at = current_timestamp(operation)?;
    let byte_length = i64::try_from(artifact.byte_length).map_err(|_| {
        StorageServiceError::new(
            StorageErrorCode::InvalidArtifact,
            operation,
            "Artifact byteLength 超过 SQLite INTEGER 上限。",
        )
        .for_artifact(&artifact.artifact_id)
    })?;
    transaction
        .execute(
            "INSERT INTO artifacts (\
                owner_project_id, artifact_id, document_project_id, stage_id, run_id, kind, \
                media_type, evidence_role, origin, content_hash, byte_length, metadata_uri, \
                content_uri, content_available, created_at, indexed_at\
             ) VALUES (\
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16\
             ) \
             ON CONFLICT(owner_project_id, artifact_id) DO UPDATE SET \
                document_project_id = excluded.document_project_id, \
                stage_id = excluded.stage_id, \
                run_id = excluded.run_id, \
                kind = excluded.kind, \
                media_type = excluded.media_type, \
                evidence_role = excluded.evidence_role, \
                origin = excluded.origin, \
                content_hash = excluded.content_hash, \
                byte_length = excluded.byte_length, \
                metadata_uri = excluded.metadata_uri, \
                content_uri = excluded.content_uri, \
                content_available = excluded.content_available, \
                created_at = excluded.created_at, \
                indexed_at = excluded.indexed_at",
            params![
                owner_project_id,
                artifact.artifact_id,
                artifact.document_project_id,
                artifact.stage_id,
                artifact.run_id,
                artifact.kind,
                artifact.media_type,
                artifact.evidence_role,
                artifact.origin,
                artifact.content_hash,
                byte_length,
                artifact.metadata_uri,
                artifact.content_uri,
                i64::from(artifact.content_available),
                artifact.created_at,
                indexed_at,
            ],
        )
        .map_err(|error| {
            StorageServiceError::index(
                StorageErrorCode::IndexUnavailable,
                operation,
                index_path,
                "写入 Artifact 索引失败",
                &error,
            )
        })?;
    Ok(())
}

fn validate_job_upsert(
    job: &IndexedJobUpsertData,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    if job.job_id.trim().is_empty()
        || job.stage_run_id.trim().is_empty()
        || job.stage_id.trim().is_empty()
        || job.attempt == 0
        || !job.progress.is_finite()
        || !(0.0..=1.0).contains(&job.progress)
        || job.created_at.trim().is_empty()
        || job.updated_at.trim().is_empty()
    {
        return Err(StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            "任务摘要字段不完整或 progress/attempt 越界。",
        ));
    }
    Ok(())
}

fn canonical_job_timestamp(
    value: &str,
    field_name: &str,
    operation: StorageOperation,
) -> Result<String, StorageServiceError> {
    let timestamp = OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        StorageServiceError::new(
            StorageErrorCode::InvalidRequest,
            operation,
            format!("任务摘要 {field_name} 不是 RFC 3339 时间：{error}"),
        )
    })?;
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
        timestamp.nanosecond(),
    ))
}

fn build_job_query(options: &ListIndexedJobsOptions) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::from(
        "SELECT owner_project_id, job_id, stage_run_id, stage_id, status, attempt, progress, \
                message, created_at, updated_at FROM job_summaries",
    );
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(project_id) = &options.owner_project_id {
        clauses.push("owner_project_id = ?".to_owned());
        values.push(rusqlite::types::Value::Text(project_id.clone()));
    }
    if !options.statuses.is_empty() {
        let placeholders = vec!["?"; options.statuses.len()].join(", ");
        clauses.push(format!("status IN ({placeholders})"));
        values.extend(
            options
                .statuses
                .iter()
                .map(|status| rusqlite::types::Value::Text(status.as_str().to_owned())),
        );
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY updated_at DESC, job_id ASC LIMIT ?");
    values.push(rusqlite::types::Value::Integer(i64::from(options.limit)));
    (sql, values)
}

fn scan_cache(
    cache_dir: &Path,
    max_entries: usize,
    max_bytes: u64,
    max_depth: usize,
    operation: StorageOperation,
) -> Result<Vec<CacheEntry>, StorageServiceError> {
    match fs::symlink_metadata(cache_dir) {
        Ok(metadata) => {
            if metadata_is_link(&metadata) {
                return Err(StorageServiceError::new(
                    StorageErrorCode::PathContainsSymlink,
                    operation,
                    "cache 目录不能是符号链接或重解析点。",
                )
                .at_path(cache_dir));
            }
            if !metadata.is_dir() {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "cache 路径不是目录。",
                )
                .at_path(cache_dir));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(StorageServiceError::io(
                operation,
                cache_dir,
                "读取 cache 目录失败",
                &error,
            ));
        }
    }

    let mut stack = vec![(cache_dir.to_path_buf(), 0_usize)];
    let mut entries = Vec::new();
    let mut bytes = 0_u64;
    while let Some((directory, depth)) = stack.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| {
            StorageServiceError::io(operation, &directory, "扫描 cache 目录失败", &error)
        })? {
            let entry = entry.map_err(|error| {
                StorageServiceError::io(operation, &directory, "读取 cache 目录项失败", &error)
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                StorageServiceError::io(operation, &path, "读取 cache 条目失败", &error)
            })?;
            if metadata_is_link(&metadata) {
                return Err(StorageServiceError::new(
                    StorageErrorCode::PathContainsSymlink,
                    operation,
                    "cache 条目不能是符号链接或重解析点。",
                )
                .at_path(&path));
            }
            let entry_depth = depth + 1;
            if entry_depth > max_depth {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ScanLimitExceeded,
                    operation,
                    format!("cache 同步清理最多扫描 {max_depth} 层。"),
                )
                .at_path(&path));
            }
            let (is_directory, byte_length) = if metadata.is_dir() {
                stack.push((path.clone(), entry_depth));
                (true, 0)
            } else if metadata.is_file() {
                (false, metadata.len())
            } else {
                return Err(StorageServiceError::new(
                    StorageErrorCode::InvalidPath,
                    operation,
                    "cache 中包含非普通文件。",
                )
                .at_path(&path));
            };
            bytes = bytes.checked_add(byte_length).ok_or_else(|| {
                StorageServiceError::new(
                    StorageErrorCode::ScanLimitExceeded,
                    operation,
                    "cache 字节数溢出。",
                )
                .at_path(&path)
            })?;
            entries.push(CacheEntry {
                path,
                is_directory,
                depth: entry_depth,
                byte_length,
            });
            if entries.len() > max_entries || bytes > max_bytes {
                return Err(StorageServiceError::new(
                    StorageErrorCode::ScanLimitExceeded,
                    operation,
                    format!("cache 同步清理上限为 {max_entries} 个条目 / {max_bytes} 字节。"),
                )
                .at_path(cache_dir));
            }
        }
    }
    Ok(entries)
}

fn remove_cache_entries(
    mut entries: Vec<CacheEntry>,
    operation: StorageOperation,
) -> Result<(), StorageServiceError> {
    entries.sort_by_key(|entry| (Reverse(entry.depth), entry.is_directory));
    for entry in entries {
        let metadata = fs::symlink_metadata(&entry.path).map_err(|error| {
            StorageServiceError::io(operation, &entry.path, "复核 cache 条目失败", &error)
        })?;
        if metadata_is_link(&metadata)
            || metadata.is_dir() != entry.is_directory
            || (!entry.is_directory && (!metadata.is_file() || metadata.len() != entry.byte_length))
        {
            return Err(StorageServiceError::new(
                StorageErrorCode::CacheCleanupFailed,
                operation,
                "cache 条目在扫描后发生变化，已停止清理。",
            )
            .at_path(&entry.path));
        }
        let result = if entry.is_directory {
            fs::remove_dir(&entry.path)
        } else {
            fs::remove_file(&entry.path)
        };
        result.map_err(|error| {
            StorageServiceError::new(
                StorageErrorCode::CacheCleanupFailed,
                operation,
                format!("移除 cache 条目失败：{error}"),
            )
            .at_path(&entry.path)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        collect_bounded_directory_paths, content_uri_for_hash, copy_and_hash_source,
        inspect_source_file, map_project_error, parse_media_source_uri, persist_content_noclobber,
        scan_cache,
    };
    use crate::{
        ProjectErrorCode, ProjectOperation, ProjectServiceError, StorageErrorCode, StorageOperation,
    };
    use std::fs;

    #[test]
    fn cache_limits_fail_before_any_entry_is_removed() {
        let temp = tempfile::tempdir().expect("temp directory");
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).expect("create cache");
        fs::write(cache.join("one"), b"1").expect("write one");
        fs::write(cache.join("two"), b"22").expect("write two");
        fs::write(cache.join("three"), b"333").expect("write three");

        let entry_error = scan_cache(&cache, 2, 100, 8, StorageOperation::CleanProjectCache)
            .expect_err("entry limit must fail");
        assert_eq!(entry_error.code, StorageErrorCode::ScanLimitExceeded);
        assert!(cache.join("one").exists());
        assert!(cache.join("two").exists());
        assert!(cache.join("three").exists());

        let byte_error = scan_cache(&cache, 10, 5, 8, StorageOperation::CleanProjectCache)
            .expect_err("byte limit must fail");
        assert_eq!(byte_error.code, StorageErrorCode::ScanLimitExceeded);
        assert_eq!(fs::read(cache.join("three")).expect("read three"), b"333");
    }

    #[test]
    fn metadata_scan_stops_at_the_first_entry_over_the_limit() {
        let temp = tempfile::tempdir().expect("temp directory");
        for name in ["one.json", "two.json", "three.json"] {
            fs::write(temp.path().join(name), b"{}").expect("write metadata fixture");
        }

        let error =
            collect_bounded_directory_paths(temp.path(), 2, StorageOperation::RebuildProjectIndex)
                .expect_err("third entry must exceed the bounded scan");
        assert_eq!(error.code, StorageErrorCode::ScanLimitExceeded);
    }

    #[test]
    fn cas_commit_helper_never_replaces_an_existing_destination() {
        let temp = tempfile::tempdir().expect("temp directory");
        let source = temp.path().join("pending.blob");
        let destination = temp.path().join("stored.blob");
        let destination_identity_link = temp.path().join("stored-identity.blob");
        fs::write(&source, b"new bytes").expect("write pending content");
        fs::write(&destination, b"existing bytes").expect("write existing CAS content");
        fs::hard_link(&destination, &destination_identity_link)
            .expect("create a second name for the existing destination identity");

        let error = persist_content_noclobber(&source, &destination)
            .expect_err("CAS commit must not replace an existing destination");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&destination).expect("read preserved destination"),
            b"existing bytes"
        );
        assert_eq!(
            fs::read(&source).expect("read preserved pending source"),
            b"new bytes"
        );
        fs::write(&destination, b"identity probe").expect("write destination identity probe");
        assert_eq!(
            fs::read(&destination_identity_link).expect("read identity through the second name"),
            b"identity probe",
            "the destination path must still name the original file"
        );
    }

    #[test]
    fn content_hash_path_requires_canonical_lowercase_sha256() {
        let valid = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            content_uri_for_hash(&valid, StorageOperation::GetArtifact)
                .expect("valid content hash"),
            format!("artifacts/objects/sha256/ab/{}", "ab".repeat(32))
        );

        let uppercase = format!("sha256:{}", "AB".repeat(32));
        let error = content_uri_for_hash(&uppercase, StorageOperation::GetArtifact)
            .expect_err("uppercase hash must fail");
        assert_eq!(error.code, StorageErrorCode::InvalidArtifact);
    }

    #[test]
    fn staged_source_copy_rejects_changes_after_the_inspected_snapshot() {
        let temp = tempfile::tempdir().expect("temp directory");
        let source = temp.path().join("source.wav");
        let temporary = temp.path().join("pending.source");
        fs::write(&source, b"original").expect("write source snapshot");
        let snapshot = inspect_source_file(&source, StorageOperation::ManageMediaSource)
            .expect("inspect source snapshot");
        fs::write(&source, b"changed after inspection").expect("change source after inspection");

        let error = copy_and_hash_source(
            &snapshot,
            &temporary,
            1024,
            StorageOperation::ManageMediaSource,
        )
        .expect_err("snapshot drift must fail after the final metadata check");
        assert_eq!(error.code, StorageErrorCode::SourceChanged);
    }

    #[test]
    fn staged_source_uri_parser_accepts_only_the_canonical_bounded_layout() {
        let hash = "ab".repeat(32);
        let valid = format!("requests/media-sources/sha256/ab/{hash}/source-caption%20final.srt");
        let (parsed_hash, file_name) =
            parse_media_source_uri(&valid, StorageOperation::ManageMediaSource)
                .expect("canonical staged source URI");
        assert_eq!(parsed_hash, hash);
        assert_eq!(file_name, "source-caption%20final.srt");

        for invalid in [
            format!("requests/media-sources/.tmp/ab/{hash}/source-caption.srt"),
            format!("requests/media-sources/sha256/ab/{hash}/source-caption.srt/extra"),
            format!("requests\\media-sources\\sha256\\ab\\{hash}\\source-caption.srt"),
            format!("requests/media-sources/sha256/ab/{hash}/source-%2E%2E"),
            format!("requests/media-sources/sha256/ab/{hash}/source-%2Fescape"),
            format!("requests/media-sources/sha256/cd/{hash}/source-caption.srt"),
            format!(
                "requests/media-sources/sha256/ab/{}/source-caption.srt",
                hash.to_uppercase()
            ),
        ] {
            assert!(
                parse_media_source_uri(&invalid, StorageOperation::ManageMediaSource).is_err(),
                "invalid staged source URI was accepted: {invalid}"
            );
        }
    }

    #[test]
    fn project_open_failures_keep_actionable_storage_codes() {
        for (project_code, storage_code) in [
            (
                ProjectErrorCode::MigrationRequired,
                StorageErrorCode::MigrationRequired,
            ),
            (
                ProjectErrorCode::UnsupportedNewerVersion,
                StorageErrorCode::UnsupportedNewerVersion,
            ),
            (
                ProjectErrorCode::InvalidProject,
                StorageErrorCode::InvalidProject,
            ),
            (ProjectErrorCode::IoError, StorageErrorCode::IoError),
        ] {
            let mapped = map_project_error(
                ProjectServiceError::new(project_code, ProjectOperation::Open, "project error"),
                StorageOperation::GetArtifact,
            );
            assert_eq!(mapped.code, storage_code);
        }
    }
}
