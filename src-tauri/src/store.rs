//! SQLite store (projects, accounts, runs, steps) and the secret store.
use crate::error::{Error, Result};
use crate::version::AdapterConfig;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub owner: String,
    pub repo: String,
    pub account_id: Option<i64>,
    pub default_branch: String,
    pub release_branch: String,
    pub tag_format: String,
    pub local_only: bool,
    pub adapters: Vec<AdapterConfig>,
    #[serde(default)]
    pub pipeline: Vec<String>,
    #[serde(default)]
    pub options: ProjectOptions,
}

/// Phase 2 per-project settings (`options_json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectOptions {
    /// "none" (release from the current branch), "pr" or "ff" (local fast-forward).
    pub merge_mode: String,
    /// "merge", "squash" or "rebase" for PR merges.
    pub merge_method: String,
    /// Workflow file name in .github/workflows, e.g. "release.yml".
    pub workflow: Option<String>,
    /// true: call workflow_dispatch. false: rely on the tag push to trigger it.
    pub dispatch: bool,
    /// Dispatch inputs; values may use {version}, {tag} and {notes}.
    pub dispatch_inputs: std::collections::BTreeMap<String, String>,
    /// Upload the run's artifacts to the release.
    pub attach_artifacts: bool,
    /// "files": upload each file inside the artifact; "zip": upload `<name>.zip`.
    pub attach_mode: String,
    /// Prepend the notes to a changelog file in the release commit.
    pub changelog: bool,
    pub changelog_file: String,
    /// Local self-hosted runner folder (contains run.cmd / run.sh).
    pub runner_dir: Option<String>,
}

impl Default for ProjectOptions {
    fn default() -> Self {
        ProjectOptions {
            merge_mode: "none".into(),
            merge_method: "squash".into(),
            workflow: None,
            dispatch: false,
            dispatch_inputs: [("version".to_string(), "{version}".to_string())].into(),
            attach_artifacts: true,
            attach_mode: "files".into(),
            changelog: false,
            changelog_file: "CHANGELOG.md".into(),
            runner_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub label: String,
    pub github_login: String,
    pub keyring_ref: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: i64,
    pub project_id: i64,
    pub from_version: String,
    pub to_version: String,
    pub status: String,
    pub dry_run: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: i64,
    pub step_key: String,
    pub status: String,
    pub log: String,
    pub meta_json: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// Phase 1 default, upgraded on load so old projects show the new steps.
const PHASE1_PIPELINE: &[&str] = &["status", "commit", "push", "version", "notes", "tag", "release"];

pub const DEFAULT_PIPELINE: &[&str] =
    &["status", "commit", "push", "merge", "version", "notes", "tag", "actions", "release", "publish"];

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Self::init(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS accounts(
              id INTEGER PRIMARY KEY, label TEXT NOT NULL, github_login TEXT NOT NULL,
              keyring_ref TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS projects(
              id INTEGER PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
              owner TEXT NOT NULL, repo TEXT NOT NULL, account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
              default_branch TEXT NOT NULL, release_branch TEXT NOT NULL, tag_format TEXT NOT NULL,
              local_only INTEGER NOT NULL, pipeline_json TEXT NOT NULL, adapters_json TEXT NOT NULL,
              created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS release_runs(
              id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              from_version TEXT NOT NULL, to_version TEXT NOT NULL, status TEXT NOT NULL,
              dry_run INTEGER NOT NULL DEFAULT 0, started_at TEXT NOT NULL, finished_at TEXT);
            CREATE TABLE IF NOT EXISTS run_steps(
              id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL REFERENCES release_runs(id) ON DELETE CASCADE,
              step_key TEXT NOT NULL, status TEXT NOT NULL, log TEXT NOT NULL, meta_json TEXT NOT NULL,
              started_at TEXT NOT NULL, finished_at TEXT);
            CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        // Phase 2 migration: idempotent, the error just means the column exists.
        let _ = conn.execute("ALTER TABLE projects ADD COLUMN options_json TEXT NOT NULL DEFAULT '{}'", []);
        Ok(Store { conn: Mutex::new(conn) })
    }

    // ---------------------------------------------------------- settings
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.lock().unwrap()
            .query_row("SELECT value FROM settings WHERE key=?", [key], |r| r.get(0)).optional()?)
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------- accounts
    pub fn add_account(&self, label: &str, login: &str, keyring_ref: &str) -> Result<i64> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO accounts(label,github_login,keyring_ref,created_at) VALUES(?,?,?,?)",
            params![label, login, keyring_ref, now()],
        )?;
        Ok(c.last_insert_rowid())
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT id,label,github_login,keyring_ref FROM accounts ORDER BY id")?;
        let rows = st.query_map([], |r| {
            Ok(Account { id: r.get(0)?, label: r.get(1)?, github_login: r.get(2)?, keyring_ref: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn account(&self, id: i64) -> Result<Account> {
        self.accounts()?.into_iter().find(|a| a.id == id).ok_or_else(|| Error::Store("account not found".into()))
    }
    pub fn remove_account(&self, id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM accounts WHERE id=?", [id])?;
        Ok(())
    }

    // ---------------------------------------------------------- projects
    pub fn add_project(&self, p: &Project) -> Result<i64> {
        let c = self.conn.lock().unwrap();
        let pipeline = if p.pipeline.is_empty() {
            DEFAULT_PIPELINE.iter().map(|s| s.to_string()).collect()
        } else {
            p.pipeline.clone()
        };
        c.execute(
            "INSERT INTO projects(name,path,owner,repo,account_id,default_branch,release_branch,tag_format,
             local_only,pipeline_json,adapters_json,created_at,options_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![p.name, p.path, p.owner, p.repo, p.account_id, p.default_branch, p.release_branch,
                p.tag_format, p.local_only, serde_json::to_string(&pipeline)?, serde_json::to_string(&p.adapters)?, now(),
                serde_json::to_string(&p.options)?],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                Error::Store("that folder is already added".into())
            }
            e => e.into(),
        })?;
        Ok(c.last_insert_rowid())
    }

    pub fn update_project(&self, p: &Project) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE projects SET name=?,owner=?,repo=?,account_id=?,default_branch=?,release_branch=?,
             tag_format=?,local_only=?,pipeline_json=?,adapters_json=?,options_json=? WHERE id=?",
            params![p.name, p.owner, p.repo, p.account_id, p.default_branch, p.release_branch, p.tag_format,
                p.local_only, serde_json::to_string(&p.pipeline)?, serde_json::to_string(&p.adapters)?,
                serde_json::to_string(&p.options)?, p.id],
        )?;
        Ok(())
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id,name,path,owner,repo,account_id,default_branch,release_branch,tag_format,local_only,
             pipeline_json,adapters_json,options_json FROM projects ORDER BY name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            let pipeline: String = r.get(10)?;
            let adapters: String = r.get(11)?;
            let options: String = r.get(12)?;
            Ok(Project {
                id: r.get(0)?, name: r.get(1)?, path: r.get(2)?, owner: r.get(3)?, repo: r.get(4)?,
                account_id: r.get(5)?, default_branch: r.get(6)?, release_branch: r.get(7)?,
                tag_format: r.get(8)?, local_only: r.get(9)?,
                pipeline: {
                    let v: Vec<String> = serde_json::from_str(&pipeline).unwrap_or_default();
                    if v.is_empty() || v == PHASE1_PIPELINE {
                        DEFAULT_PIPELINE.iter().map(|s| s.to_string()).collect()
                    } else {
                        v
                    }
                },
                adapters: serde_json::from_str(&adapters).unwrap_or_default(),
                options: serde_json::from_str(&options).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn project(&self, id: i64) -> Result<Project> {
        self.projects()?.into_iter().find(|p| p.id == id).ok_or_else(|| Error::Store("project not found".into()))
    }

    pub fn remove_project(&self, id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM projects WHERE id=?", [id])?;
        Ok(())
    }

    // ---------------------------------------------------------- runs
    pub fn start_run(&self, project_id: i64, from: &str, to: &str, dry_run: bool) -> Result<i64> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO release_runs(project_id,from_version,to_version,status,dry_run,started_at) VALUES(?,?,?,?,?,?)",
            params![project_id, from, to, "running", dry_run, now()],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn set_run(&self, id: i64, to: Option<&str>, status: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        let fin = if status == "running" { None } else { Some(now()) };
        c.execute("UPDATE release_runs SET status=?, finished_at=? WHERE id=?", params![status, fin, id])?;
        if let Some(t) = to {
            c.execute("UPDATE release_runs SET to_version=? WHERE id=?", params![t, id])?;
        }
        Ok(())
    }

    pub fn record_step(&self, run_id: i64, key: &str, status: &str, log: &str, meta: &serde_json::Value, started: &str) -> Result<()> {
        let log = crate::redact::redact(log);
        let meta = crate::redact::redact(&meta.to_string());
        self.conn.lock().unwrap().execute(
            "INSERT INTO run_steps(run_id,step_key,status,log,meta_json,started_at,finished_at) VALUES(?,?,?,?,?,?,?)",
            params![run_id, key, status, log, meta, started, now()],
        )?;
        Ok(())
    }

    pub fn run(&self, id: i64) -> Result<Run> {
        self.runs(None)?.into_iter().find(|r| r.id == id).ok_or_else(|| Error::Store("run not found".into()))
    }

    pub fn runs(&self, project_id: Option<i64>) -> Result<Vec<Run>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id,project_id,from_version,to_version,status,dry_run,started_at,finished_at FROM release_runs
             WHERE (?1 IS NULL OR project_id=?1) ORDER BY id DESC LIMIT 200",
        )?;
        let mut runs: Vec<Run> = st
            .query_map([project_id], |r| {
                Ok(Run {
                    id: r.get(0)?, project_id: r.get(1)?, from_version: r.get(2)?, to_version: r.get(3)?,
                    status: r.get(4)?, dry_run: r.get(5)?, started_at: r.get(6)?, finished_at: r.get(7)?, steps: vec![],
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut ss = c.prepare(
            "SELECT id,step_key,status,log,meta_json,started_at,finished_at FROM run_steps WHERE run_id=? ORDER BY id",
        )?;
        for run in runs.iter_mut() {
            run.steps = ss
                .query_map([run.id], |r| {
                    Ok(Step { id: r.get(0)?, step_key: r.get(1)?, status: r.get(2)?, log: r.get(3)?,
                        meta_json: r.get(4)?, started_at: r.get(5)?, finished_at: r.get(6)? })
                })?
                .collect::<std::result::Result<_, _>>()?;
        }
        Ok(runs)
    }
}

// ---------------------------------------------------------- secrets

/// Abstract over the OS keychain so tests use an in-memory store.
pub trait Secrets: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

#[derive(Default)]
pub struct MemorySecrets(Mutex<std::collections::HashMap<String, String>>);
impl Secrets for MemorySecrets {
    fn set(&self, k: &str, v: &str) -> Result<()> {
        self.0.lock().unwrap().insert(k.into(), v.into());
        Ok(())
    }
    fn get(&self, k: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(k).cloned())
    }
    fn delete(&self, k: &str) -> Result<()> {
        self.0.lock().unwrap().remove(k);
        Ok(())
    }
}

#[cfg(feature = "app")]
pub struct KeyringSecrets;
#[cfg(feature = "app")]
const SERVICE: &str = "release-desk";
#[cfg(feature = "app")]
impl Secrets for KeyringSecrets {
    fn set(&self, k: &str, v: &str) -> Result<()> {
        keyring::Entry::new(SERVICE, k).and_then(|e| e.set_password(v)).map_err(|e| Error::Store(e.to_string()))
    }
    fn get(&self, k: &str) -> Result<Option<String>> {
        match keyring::Entry::new(SERVICE, k).and_then(|e| e.get_password()) {
            Ok(v) => {
                crate::redact::register_secret(&v);
                Ok(Some(v))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Store(e.to_string())),
        }
    }
    fn delete(&self, k: &str) -> Result<()> {
        match keyring::Entry::new(SERVICE, k).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Store(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_runs_roundtrip() {
        let s = Store::memory().unwrap();
        let p = Project {
            id: 0, name: "x".into(), path: "/tmp/x".into(), owner: "o".into(), repo: "r".into(), account_id: None,
            default_branch: "main".into(), release_branch: "main".into(), tag_format: "v{version}".into(),
            local_only: true, adapters: vec![AdapterConfig::GitTag], pipeline: vec![], options: Default::default(),
        };
        let id = s.add_project(&p).unwrap();
        assert!(s.add_project(&p).is_err(), "duplicate path rejected");
        let got = s.project(id).unwrap();
        assert!(got.local_only);
        assert_eq!(got.pipeline.len(), DEFAULT_PIPELINE.len());
        let run = s.start_run(id, "1.0.0", "1.1.0", true).unwrap();
        let tok = format!("ghp_{}", "z".repeat(36));
        s.record_step(run, "push", "ok", &format!("used {tok}"), &serde_json::json!({ "t": tok }), "t0").unwrap();
        s.set_run(run, None, "done").unwrap();
        let runs = s.runs(Some(id)).unwrap();
        assert_eq!(runs[0].steps.len(), 1);
        assert!(!runs[0].steps[0].log.contains(&tok));
        assert!(!runs[0].steps[0].meta_json.contains(&tok));
        s.remove_project(id).unwrap();
        assert!(s.runs(Some(id)).unwrap().is_empty());
    }
}
