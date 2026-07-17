mod job_commands;
mod project_commands;
mod provider_commands;
mod provider_runtime;
mod storage_commands;
mod workflow_commands;

use job_commands::{
    cancel_job, enqueue_stage_job, get_job, list_job_events, list_jobs, recover_jobs,
    retry_stage_job,
};
use narracut_core::{JobService, ProjectService, StorageService, WorkflowService};
use narracut_provider::{AiProvider, OpenAiProvider, ProviderService, SystemCredentialStore};
use project_commands::{
    copy_project, create_project, inspect_project, migrate_project, move_project_to_trash,
    open_project, rename_project, set_project_archived,
};
use provider_commands::{
    delete_provider_credential, enqueue_script_stage, get_provider_catalog,
    get_provider_credential_status, set_provider_credential,
};
use provider_runtime::ProviderRuntime;
use storage_commands::{
    clean_project_cache, forget_project, get_artifact, import_artifact_file, list_indexed_jobs,
    list_recent_projects, rebuild_project_index, verify_artifact,
};
use tauri::Manager;
use workflow_commands::{
    get_project_workflow, initialize_project_workflow, list_stage_history, prepare_stage_run,
    preview_regeneration, record_stage_run, review_stage_run, update_stage_config,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let project_service = ProjectService::default();
    let storage_project_service = project_service.clone();
    tauri::Builder::default()
        .manage(project_service)
        .setup(move |app| {
            let index_path = app
                .path()
                .app_local_data_dir()?
                .join("narracut-index.sqlite3");
            let storage_service = StorageService::new(index_path, storage_project_service.clone());
            let workflow_service =
                WorkflowService::new(storage_project_service.clone(), storage_service.clone());
            let job_service = JobService::new(
                storage_project_service.clone(),
                storage_service.clone(),
                workflow_service.clone(),
            );
            let openai_provider = OpenAiProvider::production()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let provider_service = ProviderService::new(
                std::sync::Arc::new(SystemCredentialStore),
                [std::sync::Arc::new(openai_provider) as std::sync::Arc<dyn AiProvider>],
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            let provider_runtime = ProviderRuntime::new(
                provider_service,
                job_service.clone(),
                storage_service.clone(),
                workflow_service.clone(),
            );
            let _resumed_projects = provider_runtime.resume_recent_projects();
            app.manage(provider_runtime);
            app.manage(job_service);
            app.manage(workflow_service);
            app.manage(storage_service);
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            inspect_project,
            open_project,
            create_project,
            migrate_project,
            rename_project,
            copy_project,
            set_project_archived,
            move_project_to_trash,
            import_artifact_file,
            get_artifact,
            verify_artifact,
            rebuild_project_index,
            list_recent_projects,
            list_indexed_jobs,
            forget_project,
            clean_project_cache,
            initialize_project_workflow,
            get_project_workflow,
            update_stage_config,
            prepare_stage_run,
            record_stage_run,
            review_stage_run,
            preview_regeneration,
            list_stage_history,
            enqueue_stage_job,
            get_job,
            list_jobs,
            list_job_events,
            cancel_job,
            retry_stage_job,
            recover_jobs,
            get_provider_catalog,
            get_provider_credential_status,
            set_provider_credential,
            delete_provider_credential,
            enqueue_script_stage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running NarraCut desktop application");
}
