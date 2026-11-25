//! `#[tauri::command]` surface. Thin: every rule (local-only, dry-run,
//! redaction) lives in the core so it holds no matter what the UI sends.
use crate::error::{Error, Result};
use crate::github::Client;
use crate::pipeline::{self, Desk, StepOutcome};
use crate::store::{Account, Project, Run};
use crate::version::Bump;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

type D<'a> = State<'a, Desk>;

#[tauri::command]
pub fn detect_project(desk: D, path: String) -> Result<pipeline::Detected> {
    pipeline::detect(&desk.ctx(false), &PathBuf::from(path))
}

#[tauri::command]
pub fn add_project(desk: D, project: Project) -> Result<i64> {
    if !PathBuf::from(&project.path).join(".git").exists() {
        return Err(Error::Git("folder is not a git repository root".into()));
    }
    desk.store.add_project(&project)
}

#[tauri::command]
pub fn update_project(desk: D, project: Project) -> Result<()> {
    desk.store.update_project(&project)
}

#[tauri::command]
pub fn remove_project(desk: D, id: i64) -> Result<()> {
    desk.store.remove_project(id)
}

#[tauri::command]
pub fn list_projects(desk: D) -> Result<Vec<Project>> {
    desk.store.projects()
}

#[tauri::command]
pub async fn dashboard(desk: D<'_>) -> Result<Vec<pipeline::Overview>> {
    let ctx = desk.ctx(false);
    Ok(desk.store.projects()?.iter().map(|p| pipeline::overview(&ctx, p)).collect())
}

#[tauri::command]
pub fn git_status(desk: D, id: i64) -> Result<crate::git::Status> {
    let p = desk.store.project(id)?;
    crate::git::status(&desk.ctx(false), &Desk::repo(&p))
}

#[tauri::command]
pub fn git_diff(desk: D, id: i64, path: String, untracked: bool) -> Result<String> {
    let p = desk.store.project(id)?;
    let d = crate::git::diff_file(&desk.ctx(false), &Desk::repo(&p), &path, untracked)?;
    // Cap very large diffs so the webview stays responsive.
    Ok(if d.len() > 400_000 { format!("{}\n... (diff truncated at 400 KB)", &d[..400_000]) } else { d })
}

#[tauri::command]
pub fn release_plan(desk: D, id: i64, bump: Bump) -> Result<pipeline::Plan> {
    let p = desk.store.project(id)?;
    pipeline::plan(&desk.ctx(false), &p, &bump)
}

#[derive(Serialize)]
pub struct RunStart {
    pub run_id: i64,
    pub account: Option<Account>,
}

#[tauri::command]
pub fn start_run(desk: D, id: i64, dry_run: bool) -> Result<RunStart> {
    let p = desk.store.project(id)?;
    let from = pipeline::current_version(&desk.ctx(false), &p).map(|v| v.to_string()).unwrap_or_default();
    let run_id = desk.store.start_run(id, &from, &from, dry_run)?;
    Ok(RunStart { run_id, account: desk.account_for(&p)? })
}

#[tauri::command]
pub fn finish_run(desk: D, run_id: i64, status: String) -> Result<()> {
    desk.finish_run(run_id, &status)
}

#[tauri::command]
pub fn run_commit(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, files: Vec<String>, message: String) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_commit(run_id, &p, dry_run, &files, &message))
}

#[tauri::command]
pub fn run_push(desk: D, run_id: Option<i64>, id: i64, dry_run: bool) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_push(run_id, &p, dry_run))
}

#[tauri::command]
pub fn run_version(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, next: String) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_version(run_id, &p, dry_run, &next))
}

#[tauri::command]
pub fn run_notes(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, notes: String) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_notes(run_id, &p, dry_run, &notes))
}

#[tauri::command]
pub fn run_tag(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, next: String, notes: String) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_tag(run_id, &p, dry_run, &next, &notes))
}

#[tauri::command]
pub async fn run_release(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, next: String, notes: String, prerelease: bool) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_release(run_id, &p, dry_run, &next, &notes, prerelease).await)
}

#[tauri::command]
pub async fn run_publish(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, release_id: u64) -> Result<StepOutcome> {
    let p = desk.store.project(id)?;
    Ok(desk.step_publish(run_id, &p, dry_run, release_id).await)
}

#[tauri::command]
pub fn history(desk: D, project_id: Option<i64>) -> Result<Vec<Run>> {
    desk.store.runs(project_id)
}

#[tauri::command]
pub fn list_accounts(desk: D) -> Result<Vec<Account>> {
    desk.store.accounts()
}

/// Verify the token against /user, store it in the OS keychain, and save only
/// a reference in SQLite. The token never comes back to the UI.
#[tauri::command]
pub async fn add_account(desk: D<'_>, label: String, token: String) -> Result<Account> {
    let token = token.trim().to_string();
    let gh = Client::new(desk.api_base.as_deref(), &token)?;
    let login = gh.whoami(&desk.ctx(false)).await?;
    let key = format!("account:{}:{}", login, chrono::Utc::now().timestamp());
    desk.secrets.set(&key, &token)?;
    let label = if label.trim().is_empty() { login.clone() } else { label };
    let id = desk.store.add_account(&label, &login, &key)?;
    desk.store.account(id)
}

#[tauri::command]
pub fn remove_account(desk: D, id: i64) -> Result<()> {
    let a = desk.store.account(id)?;
    desk.secrets.delete(&a.keyring_ref)?;
    desk.store.remove_account(id)
}

#[derive(Serialize)]
pub struct Settings {
    pub git_path: String,
    pub default_tag_format: String,
}

#[tauri::command]
pub fn get_settings(desk: D) -> Result<Settings> {
    Ok(Settings {
        git_path: desk.store.setting("git_path")?.unwrap_or_default(),
        default_tag_format: desk.store.setting("default_tag_format")?.unwrap_or_else(|| "v{version}".into()),
    })
}

#[tauri::command]
pub fn set_setting(desk: D, key: String, value: String) -> Result<()> {
    if !["git_path", "default_tag_format"].contains(&key.as_str()) {
        return Err(Error::Other(format!("unknown setting {key}")));
    }
    desk.store.set_setting(&key, &value)
}
