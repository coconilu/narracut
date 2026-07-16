#![forbid(unsafe_code)]

//! NarraCut 的本地核心服务。
//!
//! 本 crate 不依赖 Tauri。桌面宿主只负责把版本化 command 契约适配到这里，
//! 文件系统边界、迁移和项目不变量由核心统一执行。

mod error;
mod project_service;
mod types;

pub use error::{ProjectErrorCode, ProjectOperation, ProjectServiceError};
pub use project_service::{OsTrashBackend, ProjectService, TrashBackend};
pub use types::{
    CopyProjectOptions, CreateProjectOptions, ProjectCopyResultData, ProjectDescriptorData,
    ProjectInspectionData, ProjectMigrationResultData, ProjectMigrationStatusData,
    ProjectTrashResultData,
};

pub const PROJECT_MARKER_FILE: &str = "narracut.project.json";
pub const CURRENT_PROJECT_FORMAT_VERSION: u32 = 1;
pub const PROJECT_COMMAND_API_VERSION: &str = "1.0.0";
