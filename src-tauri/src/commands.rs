#![allow(non_snake_case)] // camelCase args match the JS invoke keys
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
    Ok(desk.store.projects()?.iter().map(|p| pipeline::overview(&ctx, &desk.project(p.id).unwrap_or_else(|_| p.clone()))).collect())
}

#[tauri::command]
pub fn git_status(desk: D, id: i64) -> Result<crate::git::Status> {
    let p = desk.project(id)?;
    crate::git::status(&desk.ctx(false), &Desk::repo(&p))
}

#[tauri::command]
pub fn git_diff(desk: D, id: i64, path: String, untracked: bool) -> Result<String> {
    let p = desk.project(id)?;
    let d = crate::git::diff_file(&desk.ctx(false), &Desk::repo(&p), &path, untracked)?;
    // Cap very large diffs so the webview stays responsive.
    Ok(if d.len() > 400_000 { format!("{}\n... (diff truncated at 400 KB)", &d[..400_000]) } else { d })
}

#[tauri::command]
pub fn release_plan(desk: D, id: i64, bump: Bump) -> Result<pipeline::Plan> {
    let p = desk.project(id)?;
    pipeline::plan(&desk.ctx(false), &p, &bump)
}

#[derive(Serialize)]
pub struct RunStart {
    pub run_id: i64,
    pub account: Option<Account>,
}

#[tauri::command]
pub fn start_run(desk: D, id: i64, dry_run: bool) -> Result<RunStart> {
    let p = desk.project(id)?;
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
    let p = desk.project(id)?;
    Ok(desk.step_commit(run_id, &p, dry_run, &files, &message))
}

#[tauri::command]
pub fn run_push(desk: D, run_id: Option<i64>, id: i64, dry_run: bool) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_push(run_id, &p, dry_run))
}

#[tauri::command]
pub fn run_version(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, next: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_version(run_id, &p, dry_run, &next))
}

#[tauri::command]
pub fn run_notes(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, notes: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_notes(run_id, &p, dry_run, &notes))
}

#[tauri::command]
pub fn run_tag(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, next: String, notes: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_tag(run_id, &p, dry_run, &next, &notes))
}

#[tauri::command]
pub async fn run_release(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, next: String, notes: String, prerelease: bool) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_release(run_id, &p, dry_run, &next, &notes, prerelease).await)
}

#[tauri::command]
pub async fn run_publish(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, release_id: u64) -> Result<StepOutcome> {
    let p = desk.project(id)?;
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
    save_account(&desk, label, token.trim().to_string()).await
}

async fn save_account(desk: &Desk, label: String, token: String) -> Result<Account> {
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
    pub oauth_client_id: String,
    pub ai_model: String,
    pub ai_key_set: bool,
}

#[tauri::command]
pub fn get_settings(desk: D) -> Result<Settings> {
    Ok(Settings {
        git_path: desk.store.setting("git_path")?.unwrap_or_default(),
        default_tag_format: desk.store.setting("default_tag_format")?.unwrap_or_else(|| "v{version}".into()),
        oauth_client_id: desk.store.setting("oauth_client_id")?.unwrap_or_default(),
        ai_model: desk.store.setting("ai_model")?.unwrap_or_else(|| crate::ai::DEFAULT_MODEL.into()),
        ai_key_set: desk.secrets.get(AI_KEY)?.is_some(),
    })
}

#[tauri::command]
pub fn set_setting(desk: D, key: String, value: String) -> Result<()> {
    if !["git_path", "default_tag_format", "oauth_client_id", "ai_model"].contains(&key.as_str()) {
        return Err(Error::Other(format!("unknown setting {key}")));
    }
    desk.store.set_setting(&key, &value)
}

// ================================================================ Phase 2

#[tauri::command]
pub async fn open_pr(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, title: String, body: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_open_pr(run_id, &p, dry_run, &title, &body).await)
}

#[tauri::command]
pub async fn pr_status(desk: D<'_>, id: i64, number: u64) -> Result<pipeline::PrStatus> {
    let p = desk.project(id)?;
    desk.pr_status(&p, number).await
}

#[tauri::command]
pub async fn merge_pr(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, number: u64, allowFailing: bool) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_merge_pr(run_id, &p, dry_run, number, allowFailing).await)
}

#[tauri::command]
pub fn merge_ff(desk: D, run_id: Option<i64>, id: i64, dry_run: bool) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_merge_ff(run_id, &p, dry_run))
}

#[tauri::command]
pub async fn list_workflows(desk: D<'_>, id: i64) -> Result<Vec<crate::github::Workflow>> {
    let p = desk.project(id)?;
    pipeline::guard_remote(&p, "Actions")?;
    let (_, gh) = desk.client_for(&p)?.ok_or_else(|| Error::GitHub("no GitHub account configured".into()))?;
    gh.workflows(&desk.ctx(false), &p.owner, &p.repo).await
}

#[tauri::command]
pub async fn dispatch(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, next: String, notes: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_dispatch(run_id, &p, dry_run, &next, &notes).await)
}

#[tauri::command]
pub async fn actions_runs(desk: D<'_>, id: i64, gitRef: Option<String>) -> Result<Vec<crate::github::WorkflowRun>> {
    let p = desk.project(id)?;
    desk.actions_runs(&p, gitRef.as_deref()).await
}

#[tauri::command]
pub async fn actions_jobs(desk: D<'_>, id: i64, runId: u64) -> Result<Vec<crate::github::Job>> {
    let p = desk.project(id)?;
    desk.actions_jobs(&p, runId).await
}

#[tauri::command]
pub async fn actions_control(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, actionsRun: u64, action: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.actions_control(run_id, &p, dry_run, actionsRun, &action).await)
}

#[tauri::command]
pub async fn attach(desk: D<'_>, run_id: Option<i64>, id: i64, dry_run: bool, releaseId: u64, actionsRun: u64) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.step_attach(run_id, &p, dry_run, releaseId, actionsRun).await)
}

#[tauri::command]
pub fn rollback(desk: D, run_id: Option<i64>, id: i64, dry_run: bool, next: String) -> Result<StepOutcome> {
    let p = desk.project(id)?;
    Ok(desk.rollback(run_id, &p, dry_run, &next))
}

#[tauri::command]
pub fn resume_info(desk: D, runId: i64) -> Result<pipeline::ResumeInfo> {
    desk.resume_info(runId)
}

fn client_id(desk: &Desk) -> Result<String> {
    desk.store.setting("oauth_client_id")?.filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::Other("set the OAuth App client ID in Settings first".into()))
}

#[tauri::command]
pub async fn device_start(desk: D<'_>) -> Result<crate::github::DeviceCode> {
    crate::github::device_start(None, &client_id(&desk)?, "repo workflow").await
}

#[derive(Serialize)]
pub struct DeviceResult {
    pub state: String,
    pub interval: u64,
    pub account: Option<Account>,
}

/// Poll once. The token goes straight from GitHub into the keychain; only the
/// account record comes back to the UI.
#[tauri::command]
pub async fn device_poll(desk: D<'_>, deviceCode: String, interval: u64, label: String) -> Result<DeviceResult> {
    match crate::github::device_poll(None, &client_id(&desk)?, &deviceCode, interval).await? {
        crate::github::DevicePoll::Pending { interval } => Ok(DeviceResult { state: "pending".into(), interval, account: None }),
        crate::github::DevicePoll::Done { token } => {
            let a = save_account(&desk, label, token).await?;
            Ok(DeviceResult { state: "done".into(), interval, account: Some(a) })
        }
    }
}

// ================================================================ Phase 3

#[derive(Serialize)]
pub struct Effective {
    pub project: Project,
    /// Keys set by the repo's release-desk.json (empty when there is none).
    pub repo_config: Vec<String>,
    pub repo_config_error: Option<String>,
}

#[tauri::command]
pub fn effective_project(desk: D, id: i64) -> Result<Effective> {
    let mut p = desk.store.project(id)?;
    let (mut set, mut err) = (vec![], None);
    match crate::config::load(&Desk::repo(&p)) {
        Ok(Some(c)) => match crate::config::apply(&mut p, &c) {
            Ok(s) => set = s,
            Err(e) => err = Some(e.to_string()),
        },
        Ok(None) => {}
        Err(e) => err = Some(e.to_string()),
    }
    Ok(Effective { project: p, repo_config: set, repo_config_error: err })
}

fn runner_dir(p: &Project) -> Result<String> {
    p.options.runner_dir.clone().filter(|d| !d.trim().is_empty())
        .ok_or_else(|| Error::Other("set the runner folder for this project first".into()))
}

#[tauri::command]
pub fn runner_status(desk: D, id: i64) -> Result<crate::runner::LocalRunner> {
    Ok(desk.runners.status(&runner_dir(&desk.project(id)?)?))
}

#[tauri::command]
pub fn runner_start(desk: D, id: i64) -> Result<crate::runner::LocalRunner> {
    desk.runners.start(&runner_dir(&desk.project(id)?)?)
}

#[tauri::command]
pub fn runner_stop(desk: D, id: i64) -> Result<crate::runner::LocalRunner> {
    desk.runners.stop(&runner_dir(&desk.project(id)?)?)
}

#[tauri::command]
pub fn runner_log(desk: D, id: i64) -> Result<String> {
    Ok(desk.runners.tail_log(&runner_dir(&desk.project(id)?)?, 200))
}

#[tauri::command]
pub async fn github_runners(desk: D<'_>, id: i64) -> Result<serde_json::Value> {
    let p = desk.project(id)?;
    pipeline::guard_remote(&p, "runners")?;
    let (_, gh) = desk.client_for(&p)?.ok_or_else(|| Error::GitHub("no GitHub account configured".into()))?;
    gh.runners(&desk.ctx(false), &p.owner, &p.repo).await
}

const AI_KEY: &str = "anthropic_api_key";

#[tauri::command]
pub fn set_ai_key(desk: D, key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return desk.secrets.delete(AI_KEY);
    }
    desk.secrets.set(AI_KEY, key)
}

/// Sends only the notes text to the Anthropic API, with the user's own key.
#[tauri::command]
pub async fn polish_notes(desk: D<'_>, notes: String) -> Result<String> {
    let key = desk.secrets.get(AI_KEY)?.ok_or_else(|| Error::Other("add an Anthropic API key in Settings to use this".into()))?;
    let model = desk.store.setting("ai_model")?.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| crate::ai::DEFAULT_MODEL.into());
    crate::ai::polish(desk.ai_base.as_deref(), &key, &model, &notes).await
}
