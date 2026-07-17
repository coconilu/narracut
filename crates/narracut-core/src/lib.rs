#![forbid(unsafe_code)]

//! NarraCut 的本地核心服务。
//!
//! 本 crate 不依赖 Tauri。桌面宿主只负责把版本化 command 契约适配到这里，
//! 文件系统边界、迁移和项目不变量由核心统一执行。

mod error;
mod job_error;
mod job_service;
mod job_types;
mod project_service;
mod storage_error;
mod storage_service;
mod storage_types;
mod types;
mod workflow_error;
mod workflow_service;
mod workflow_types;

pub use error::{ProjectErrorCode, ProjectOperation, ProjectServiceError};
pub use job_error::{JobErrorCode, JobOperation, JobServiceError};
pub use job_service::{JobClock, JobService, SystemJobClock};
pub use job_types::{
    AcknowledgeCancellationOptions, CancelJobOptions, ClaimJobOptions, ClaimNextJobOptions,
    ClaimStageJobRequestOptions, CompleteJobOptions, EnqueueStageJobOptions, FailJobOptions,
    GetJobOptions, JobEventsResultData, JobFailureData, JobLeaseData, JobListResultData,
    JobRecoveryResultData, JobSnapshotData, JobStatusData, ListJobEventsOptions, ListJobsOptions,
    RecordJobArtifactOptions, RecoverJobsOptions, RenewJobLeaseOptions, ReportJobProgressOptions,
    RetryPolicyData, RetryStageJobOptions, StageJobRequestClaimData,
};
pub use project_service::{OsTrashBackend, ProjectService, TrashBackend};
pub use storage_error::{StorageErrorCode, StorageOperation, StorageServiceError};
pub use storage_service::StorageService;
pub use storage_types::{
    ArtifactCommitResultData, ArtifactReadResultData, ArtifactVerificationResultData,
    ArtifactVerificationStatusData, CacheCleanupResultData, ForgetProjectResultData,
    IndexedJobData, IndexedJobStatusData, IndexedJobUpsertData, IndexedJobsResultData,
    ListIndexedJobsOptions, ProjectIndexRebuildResultData, RecentProjectData,
    RecentProjectsResultData, StorageIndexStatusData, StoreArtifactFileOptions,
};
pub use types::{
    CopyProjectOptions, CreateProjectOptions, ProjectCopyResultData, ProjectDescriptorData,
    ProjectInspectionData, ProjectMigrationResultData, ProjectMigrationStatusData,
    ProjectTrashResultData,
};
pub use workflow_error::{WorkflowErrorCode, WorkflowOperation, WorkflowServiceError};
pub use workflow_service::WorkflowService;
pub use workflow_types::{
    AffectedStageData, InitializeWorkflowOptions, PrepareStageRunOptions, RecordStageRunOptions,
    RegenerationImpactResultData, ReviewDecisionData, ReviewStageRunOptions, ReviewerReferenceData,
    StageConfigUpdateResultData, StageHistoryResultData, StageReviewResultData,
    StageRunCommitResultData, StageRunPreparationResultData, StageStateData, StageStatusData,
    TerminalRunStatusData, UpdateStageConfigOptions, WorkflowSnapshotData,
};

pub const PROJECT_MARKER_FILE: &str = "narracut.project.json";
pub const CURRENT_PROJECT_FORMAT_VERSION: u32 = 1;
pub const PROJECT_COMMAND_API_VERSION: &str = "1.0.0";
pub const STORAGE_COMMAND_API_VERSION: &str = "1.0.0";
pub const WORKFLOW_COMMAND_API_VERSION: &str = "1.0.0";
pub const JOB_COMMAND_API_VERSION: &str = "1.0.0";
