mod project_commands;

use narracut_core::ProjectService;
use project_commands::{
    copy_project, create_project, inspect_project, migrate_project, move_project_to_trash,
    open_project, rename_project, set_project_archived,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ProjectService::default())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running NarraCut desktop application");
}
