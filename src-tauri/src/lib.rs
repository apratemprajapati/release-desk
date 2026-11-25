pub mod ai;
pub mod config;
pub mod error;
pub mod exec;
pub mod git;
pub mod github;
pub mod notes;
pub mod pipeline;
pub mod redact;
pub mod runner;
pub mod store;
pub mod version;

#[cfg(feature = "app")]
pub mod commands;

#[cfg(feature = "app")]
pub fn run() {
    use std::sync::Arc;
    use tauri::Manager;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("no app data dir");
            let store = store::Store::open(&dir.join("release-desk.sqlite3"))?;
            app.manage(pipeline::Desk {
                store,
                secrets: Arc::new(store::KeyringSecrets),
                api_base: None,
                ai_base: None,
                runners: Default::default(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_project,
            commands::add_project,
            commands::update_project,
            commands::remove_project,
            commands::list_projects,
            commands::dashboard,
            commands::git_status,
            commands::git_diff,
            commands::release_plan,
            commands::start_run,
            commands::finish_run,
            commands::run_commit,
            commands::run_push,
            commands::run_version,
            commands::run_notes,
            commands::run_tag,
            commands::run_release,
            commands::run_publish,
            commands::history,
            commands::list_accounts,
            commands::add_account,
            commands::remove_account,
            commands::get_settings,
            commands::set_setting,
            commands::open_pr,
            commands::pr_status,
            commands::merge_pr,
            commands::merge_ff,
            commands::list_workflows,
            commands::dispatch,
            commands::actions_runs,
            commands::actions_jobs,
            commands::actions_control,
            commands::attach,
            commands::rollback,
            commands::resume_info,
            commands::device_start,
            commands::device_poll,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Release Desk");
}
