#![forbid(unsafe_code)]

//! NarraCut 的本地核心服务。
//!
//! 本 crate 不依赖 Tauri。桌面宿主只负责把版本化 command 契约适配到这里，
//! 文件系统边界、迁移和项目不变量由核心统一执行。

mod error;
mod export_error;
mod export_service;
mod export_types;
mod job_error;
mod job_service;
mod job_types;
mod media_error;
mod media_parser;
mod media_plan;
mod media_service;
mod media_timeline;
mod media_types;
mod project_service;
mod renderer_error;
mod renderer_service;
mod renderer_types;
mod storage_error;
mod storage_service;
mod storage_types;
mod types;
mod workflow_error;
mod workflow_service;
mod workflow_types;

pub use error::{ProjectErrorCode, ProjectOperation, ProjectServiceError};
pub use export_error::{ExportErrorCode, ExportOperation, ExportServiceError};
pub use export_service::ExportService;
pub use export_types::{
    EnqueueExportOptions, ExportCommitResultData, ExportEnqueueResultData, ExportRenderInputData,
    ExportTransferAbort, ExportTransferObserver, NoopExportTransferObserver, PreparedExportData,
    RetryExportOptions, RunExportQaOptions,
};
pub use job_error::{JobErrorCode, JobOperation, JobServiceError};
pub use job_service::{JobClock, JobService, SystemJobClock};
pub use job_types::{
    AcknowledgeCancellationOptions, BeginJobCompletionOptions, CancelJobOptions, ClaimJobOptions,
    ClaimNextJobOptions, ClaimStageJobRequestOptions, CompleteJobOptions, EnqueueStageJobOptions,
    FailJobOptions, GetJobOptions, GetStageJobRequestOptions, JobEventsResultData, JobFailureData,
    JobFinalizationModeData, JobLeaseData, JobListResultData, JobRecoveryResultData,
    JobSnapshotData, JobStatusData, ListJobEventsOptions, ListJobsOptions,
    RecordJobArtifactOptions, RecoverJobsOptions, RenewJobLeaseOptions, ReportJobProgressOptions,
    RetryPolicyData, RetryStageJobOptions, StageJobRequestClaimData, StageJobRequestData,
};
pub use media_error::{MediaErrorCode, MediaOperation, MediaServiceError};
pub use media_parser::{
    parse_pcm_wav_file, parse_srt_file, MediaParseError, MediaParseErrorCode, ParsedCaptionCue,
    ParsedPcmWav, ParsedSrt, PcmWavParseLimits, SrtParseLimits,
};
pub use media_plan::{
    apply_scene_plan_edits, build_scene_plan_document, validate_scene_plan_semantics,
    ScenePlanError, ScenePlanErrorCode,
};
pub use media_service::MediaService;
pub use media_timeline::{
    apply_timeline_edits, build_timeline_document, validate_timeline_semantics,
    TimelineDomainError, TimelineDomainErrorCode,
};
pub use media_types::{
    ApplyTimelineEditsOptions, AuthorizationRecordInputData, BuildScenePlanOptions,
    BuildTimelineOptions, FrozenArtifactInputData, GenerateScenePlanOptions,
    GenerateTimelineOptions, GetMediaDocumentOptions, ImportAudioOptions, ImportCaptionsOptions,
    MediaClock, MediaDocumentReadResultData, MediaImportResultData, MediaRightsData,
    MediaSaveResultData, ReauthorizeMediaOptions, SaveScenePlanOptions, SaveTimelineOptions,
    ScenePlanEditData, SystemMediaClock, TimelineCanvasData, TimelineEditData,
    TimelineSafeAreaData, VoiceAuthorizationApplicabilityData,
};
pub use project_service::{OsTrashBackend, ProjectService, TrashBackend};
pub use renderer_error::{RendererOperation, RendererServiceError, RendererServiceErrorCode};
pub use renderer_service::RendererService;
pub use renderer_types::{
    CommitRenderOptions, CreateSceneSnapshotOptions, EnqueueRenderOptions, PreparedRenderData,
    ProvenanceReferenceData, RenderCommitResultData, RenderConfigData, RenderEnqueueResultData,
    RenderTargetData, RendererTimelineInputData, SceneSnapshotData,
};
pub use storage_error::{StorageErrorCode, StorageOperation, StorageServiceError};
pub use storage_service::StorageService;
pub use storage_types::{
    ArtifactCommitJournalData, ArtifactCommitJournalStatusData, ArtifactCommitPlanEntryData,
    ArtifactCommitResultData, ArtifactReadResultData, ArtifactTransferAbort,
    ArtifactTransferObserver, ArtifactVerificationResultData, ArtifactVerificationStatusData,
    AuthorizationRecordData, CacheCleanupResultData, ForgetProjectResultData, IndexedJobData,
    IndexedJobStatusData, IndexedJobUpsertData, IndexedJobsResultData, ListIndexedJobsOptions,
    NoopArtifactTransferObserver, ProjectIndexRebuildResultData, RecentProjectData,
    RecentProjectsResultData, ResolveStagedMediaSourceOptions, ResolvedStagedMediaSourceData,
    StageMediaSourceFileOptions, StagedMediaSourceData, StorageIndexStatusData,
    StoreArtifactFileOptions, StoreAuthorizationRecordOptions,
};
pub use types::{
    CopyProjectOptions, CreateProjectOptions, ProjectCopyResultData, ProjectDescriptorData,
    ProjectInspectionData, ProjectMigrationResultData, ProjectMigrationStatusData,
    ProjectTrashResultData,
};
pub use workflow_error::{WorkflowErrorCode, WorkflowOperation, WorkflowServiceError};
pub use workflow_service::WorkflowService;
pub use workflow_types::{
    AffectedStageData, ApprovedArtifactInputData, InitializeWorkflowOptions,
    PrepareStageRunOptions, RecordStageRunOptions, RegenerationImpactResultData,
    ReviewDecisionData, ReviewStageRunOptions, ReviewerReferenceData, StageConfigUpdateResultData,
    StageHistoryResultData, StageReviewResultData, StageRunCommitResultData,
    StageRunPreparationResultData, StageStateData, StageStatusData, TerminalRunStatusData,
    UpdateStageConfigOptions, ValidateApprovedMediaInputsOptions,
    ValidateCurrentApprovedStageArtifactOptions, WorkflowSnapshotData,
};

pub const PROJECT_MARKER_FILE: &str = "narracut.project.json";
pub const CURRENT_PROJECT_FORMAT_VERSION: u32 = 1;
pub const PROJECT_COMMAND_API_VERSION: &str = "1.0.0";
pub const STORAGE_COMMAND_API_VERSION: &str = "1.0.0";
pub const WORKFLOW_COMMAND_API_VERSION: &str = "1.0.0";
pub const JOB_COMMAND_API_VERSION: &str = "1.0.0";
pub const RENDERER_COMMAND_API_VERSION: &str = "1.0.0";
pub const EXPORT_COMMAND_API_VERSION: &str = "1.0.0";
pub const EXPORT_MANIFEST_VERSION: &str = "1.0.0";
