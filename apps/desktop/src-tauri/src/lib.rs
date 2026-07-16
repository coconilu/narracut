mod project_commands;
mod storage_commands;

use narracut_core::{ProjectService, StorageService};
use project_commands::{
    copy_project, create_project, inspect_project, migrate_project, move_project_to_trash,
    open_project, rename_project, set_project_archived,
};
use storage_commands::{
    clean_project_cache, forget_project, get_artifact, import_artifact_file, list_indexed_jobs,
    list_recent_projects, rebuild_project_index, verify_artifact,
};
use tauri::Manager;

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
            app.manage(StorageService::new(
                index_path,
                storage_project_service.clone(),
            ));
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running NarraCut desktop application");
}
