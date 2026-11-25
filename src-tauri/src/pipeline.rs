//! Release steps. UI-agnostic: the Tauri layer and the integration tests both
//! call these. Every step records itself in the audit log.
use crate::error::{Error, Result};
use crate::exec::Ctx;
use crate::github::Client;
use crate::store::{Account, Project, Secrets, Store};
use crate::version::{self, AdapterConfig, Bump, FileEdit, ReadCtx};
use crate::{git, notes};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Desk {
    pub store: Store,
    pub secrets: Arc<dyn Secrets>,
    /// Overridable for tests (wiremock). None = api.github.com.
    pub api_base: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StepOutcome {
    pub ok: bool,
    pub log: String,
    pub meta: Value,
}

// ---------------------------------------------------------------- guards

/// Hard stop for anything that touches a remote on a local-only project.
pub fn guard_remote(p: &Project, action: &str) -> Result<()> {
    if p.local_only {
        return Err(Error::Blocked(format!("'{}' is local-only: {action} is disabled", p.name)));
    }
    Ok(())
}

impl Desk {
    pub fn ctx(&self, dry_run: bool) -> Ctx {
        Ctx::new(dry_run).with_git(self.store.setting("git_path").ok().flatten())
    }

    pub fn repo(p: &Project) -> PathBuf {
        PathBuf::from(&p.path)
    }

    pub fn account_for(&self, p: &Project) -> Result<Option<Account>> {
        if let Some(id) = p.account_id {
            return Ok(Some(self.store.account(id)?));
        }
        Ok(self.store.accounts()?.into_iter().next())
    }

    pub fn client_for(&self, p: &Project) -> Result<Option<(Account, Client)>> {
        let Some(acc) = self.account_for(p)? else { return Ok(None) };
        let tok = self.secrets.get(&acc.keyring_ref)?.ok_or_else(|| {
            Error::Store(format!("token for account '{}' is missing from the keychain", acc.label))
        })?;
        let c = Client::new(self.api_base.as_deref(), &tok)?;
        Ok(Some((acc, c)))
    }

    fn record(&self, run_id: Option<i64>, key: &str, started: &str, ctx: &Ctx, r: &Result<Value>) -> StepOutcome {
        let mut log = ctx.take_log();
        let (ok, meta) = match r {
            Ok(v) => (true, v.clone()),
            Err(e) => {
                log.push_str(&format!("\nERROR: {e}"));
                (false, json!({ "error": e.to_string() }))
            }
        };
        let log = crate::redact::redact(&log);
        if let Some(id) = run_id {
            let status = if !ok { "failed" } else if ctx.dry_run { "dry-run" } else { "ok" };
            let _ = self.store.record_step(id, key, status, &log, &meta, started);
            if !ok {
                let _ = self.store.set_run(id, None, "failed");
            }
        }
        StepOutcome { ok, log, meta }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------- detect

#[derive(Debug, Serialize)]
pub struct Detected {
    pub name: String,
    pub path: String,
    pub owner: String,
    pub repo: String,
    pub remote_url: Option<String>,
    pub default_branch: String,
    pub adapters: Vec<AdapterConfig>,
    pub has_claude_md_push_ban: bool,
    pub workflows: Vec<String>,
}

pub fn detect(ctx: &Ctx, path: &Path) -> Result<Detected> {
    git::read(ctx, path, &["rev-parse", "--git-dir"]).map_err(|_| Error::Git("not a git repository".into()))?;
    let remote = git::remote_url(ctx, path);
    let (owner, repo) = remote.as_deref().and_then(git::parse_github_remote).unwrap_or_default();
    let claude = std::fs::read_to_string(path.join("CLAUDE.md")).unwrap_or_default().to_lowercase();
    let ban = ["do not push", "don't push", "never push", "no push", "not push"].iter().any(|k| claude.contains(k));
    let workflows = std::fs::read_dir(path.join(".github/workflows"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
                .collect()
        })
        .unwrap_or_default();
    Ok(Detected {
        name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        path: path.to_string_lossy().to_string(),
        owner,
        repo,
        remote_url: remote,
        default_branch: git::default_branch(ctx, path),
        adapters: version::detect(path),
        has_claude_md_push_ban: ban,
        workflows,
    })
}

// ---------------------------------------------------------------- overview

#[derive(Debug, Serialize)]
pub struct Overview {
    pub project: Project,
    pub version: Option<String>,
    pub version_error: Option<String>,
    pub branch: String,
    pub dirty: usize,
    pub ahead: u32,
    pub behind: u32,
    pub last_tag: Option<String>,
    pub unreleased: usize,
    pub path_missing: bool,
}

pub fn current_version(ctx: &Ctx, p: &Project) -> Result<Version> {
    let repo = Desk::repo(p);
    let rc = ReadCtx { repo: &repo, ctx, tag_format: &p.tag_format };
    let first = p.adapters.first().cloned().unwrap_or(AdapterConfig::GitTag);
    version::read(&first, &rc)
}

pub fn overview(ctx: &Ctx, p: &Project) -> Overview {
    let repo = Desk::repo(p);
    let mut o = Overview {
        project: p.clone(), version: None, version_error: None, branch: String::new(), dirty: 0,
        ahead: 0, behind: 0, last_tag: None, unreleased: 0, path_missing: !repo.exists(),
    };
    if o.path_missing {
        return o;
    }
    match current_version(ctx, p) {
        Ok(v) => o.version = Some(v.to_string()),
        Err(e) => o.version_error = Some(e.to_string()),
    }
    if let Ok(s) = git::status(ctx, &repo) {
        o.branch = s.branch;
        o.dirty = s.files.len();
        o.ahead = s.ahead;
        o.behind = s.behind;
    }
    o.last_tag = git::last_tag(ctx, &repo, &version::tag_glob(&p.tag_format));
    o.unreleased = git::commits_since(ctx, &repo, o.last_tag.as_deref()).map(|c| notes::parse_all(&c).len()).unwrap_or(0);
    o
}

// ---------------------------------------------------------------- plan

#[derive(Debug, Serialize)]
pub struct Plan {
    pub current: String,
    pub next: String,
    pub suggested_level: version::Level,
    pub tag: String,
    pub prev_tag: Option<String>,
    pub tag_exists: bool,
    pub commits: Vec<notes::Commit>,
    pub notes: String,
    pub edits: Vec<FileEdit>,
}

pub fn plan(ctx: &Ctx, p: &Project, bump: &Bump) -> Result<Plan> {
    let repo = Desk::repo(p);
    let cur = current_version(ctx, p)?;
    let prev_tag = git::last_tag(ctx, &repo, &version::tag_glob(&p.tag_format));
    let commits = notes::parse_all(&git::commits_since(ctx, &repo, prev_tag.as_deref())?);
    let next = version::next_version(&cur, bump, &commits)?;
    let tag = version::render_tag(&p.tag_format, &next);
    let mut edits = vec![];
    for a in &p.adapters {
        edits.extend(version::plan_write(a, &repo, &next)?);
    }
    let md = notes::render(
        &commits,
        &notes::NotesInput { owner: &p.owner, repo: &p.repo, version: &next.to_string(), prev_tag: prev_tag.as_deref(), tag: &tag },
    );
    Ok(Plan {
        current: cur.to_string(),
        next: next.to_string(),
        suggested_level: version::infer_level(&commits),
        tag_exists: git::tag_exists(ctx, &repo, &tag),
        tag,
        prev_tag,
        commits,
        notes: md,
        edits,
    })
}

// ---------------------------------------------------------------- steps

impl Desk {
    pub fn step_commit(&self, run: Option<i64>, p: &Project, dry: bool, files: &[String], message: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            let repo = Desk::repo(p);
            git::stage(&ctx, &repo, files)?;
            git::commit(&ctx, &repo, message)?;
            Ok(json!({ "files": files, "message": message }))
        })();
        self.record(run, "commit", &started, &ctx, &r)
    }

    pub fn step_push(&self, run: Option<i64>, p: &Project, dry: bool) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            guard_remote(p, "push")?;
            let repo = Desk::repo(p);
            let branch = git::current_branch(&ctx, &repo)?;
            git::push(&ctx, &repo, &branch)?;
            Ok(json!({ "branch": branch }))
        })();
        self.record(run, "push", &started, &ctx, &r)
    }

    /// Write the version into every adapter file. Does not commit.
    pub fn step_version(&self, run: Option<i64>, p: &Project, dry: bool, next: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            let repo = Desk::repo(p);
            let v = Version::parse(next.trim_start_matches('v')).map_err(|e| Error::Version(e.to_string()))?;
            let cur = current_version(&ctx, p)?;
            if cur == v {
                ctx.log(&format!("version already {v}, nothing to write"));
                return Ok(json!({ "files": [], "version": next, "skipped": true }));
            }
            let mut edits = vec![];
            for a in &p.adapters {
                edits.extend(version::plan_write(a, &repo, &v)?);
            }
            let files = version::apply_edits(&ctx, &repo, &edits)?;
            if let Some(id) = run {
                let _ = self.store.set_run(id, Some(next), "running");
            }
            Ok(json!({ "files": files, "from": cur.to_string(), "version": next }))
        })();
        self.record(run, "version", &started, &ctx, &r)
    }

    /// Record the final notes text in the audit log (no side effects).
    pub fn step_notes(&self, run: Option<i64>, p: &Project, dry: bool, notes_md: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        ctx.log(&format!("notes for {} ({} chars)", p.name, notes_md.len()));
        let r = Ok(json!({ "notes": notes_md }));
        self.record(run, "notes", &started, &ctx, &r)
    }

    /// `chore(release): vX` commit (if files changed), annotated tag, then push
    /// both unless the project is local-only.
    pub fn step_tag(&self, run: Option<i64>, p: &Project, dry: bool, next: &str, notes_md: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            let repo = Desk::repo(p);
            let v = Version::parse(next.trim_start_matches('v')).map_err(|e| Error::Version(e.to_string()))?;
            let tag = version::render_tag(&p.tag_format, &v);
            let branch = git::current_branch(&ctx, &repo)?;
            if branch != p.release_branch {
                ctx.log(&format!("warning: on '{branch}', release branch is '{}'", p.release_branch));
            }
            let mut committed = false;
            if git::tag_exists(&ctx, &repo, &tag) {
                ctx.log(&format!("tag {tag} already exists, skipping commit and tag"));
            } else {
                let mut files: Vec<String> = vec![];
                for a in &p.adapters {
                    files.extend(version::plan_write(a, &repo, &v)?.into_iter().map(|e| e.path));
                }
                let changed: Vec<String> = {
                    let st = git::status(&ctx, &repo)?;
                    files.into_iter().filter(|f| st.files.iter().any(|c| &c.path == f)).collect()
                };
                if !changed.is_empty() || (dry && !p.adapters.iter().all(|a| *a == AdapterConfig::GitTag)) {
                    let to_stage = if changed.is_empty() {
                        p.adapters.iter().flat_map(|a| version::plan_write(a, &repo, &v).unwrap_or_default()).map(|e| e.path).collect()
                    } else {
                        changed
                    };
                    git::stage(&ctx, &repo, &to_stage)?;
                    git::commit(&ctx, &repo, &format!("chore(release): {tag}"))?;
                    committed = true;
                }
                git::annotated_tag(&ctx, &repo, &tag, &format!("{tag}\n\n{notes_md}"))?;
            }
            let pushed = if p.local_only {
                ctx.log("local-only project: commit and tag stay local");
                false
            } else {
                guard_remote(p, "push")?;
                git::push(&ctx, &repo, &branch)?;
                git::push(&ctx, &repo, &format!("refs/tags/{tag}"))?;
                true
            };
            Ok(json!({ "tag": tag, "committed": committed, "pushed": pushed, "branch": branch }))
        })();
        self.record(run, "tag", &started, &ctx, &r)
    }

    /// Create (or reuse) a draft release for the tag.
    pub async fn step_release(&self, run: Option<i64>, p: &Project, dry: bool, next: &str, notes_md: &str, prerelease: bool) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "GitHub release")?;
            let v = Version::parse(next.trim_start_matches('v')).map_err(|e| Error::Version(e.to_string()))?;
            let tag = version::render_tag(&p.tag_format, &v);
            let Some((acc, gh)) = self.client_for(p)? else {
                if dry {
                    ctx.log(&format!("[dry-run] would POST /repos/{}/{}/releases (no account configured)", p.owner, p.repo));
                    return Ok(json!({ "tag": tag, "dry_run": true }));
                }
                return Err(Error::GitHub("no GitHub account configured (Settings)".into()));
            };
            ctx.log(&format!("account: {} ({})", acc.label, acc.github_login));
            let prerelease = prerelease || !v.pre.is_empty();
            let rel = match gh.release_by_tag(&ctx, &p.owner, &p.repo, &tag).await? {
                Some(r) => {
                    ctx.log(&format!("release for {tag} already exists (draft={}), updating notes", r.draft));
                    gh.update_body(&ctx, &p.owner, &p.repo, r.id, notes_md).await?;
                    r
                }
                None => gh.create_draft(&ctx, &p.owner, &p.repo, &tag, &tag, notes_md, prerelease).await?,
            };
            Ok(json!({ "tag": tag, "release_id": rel.id, "url": rel.html_url, "draft": rel.draft, "prerelease": prerelease, "account": acc.github_login }))
        }
        .await;
        self.record(run, "release", &started, &ctx, &r)
    }

    pub async fn step_publish(&self, run: Option<i64>, p: &Project, dry: bool, release_id: u64) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "publish release")?;
            let Some((_, gh)) = self.client_for(p)? else {
                if dry {
                    ctx.log(&format!("[dry-run] would PATCH /repos/{}/{}/releases/{release_id} draft=false", p.owner, p.repo));
                    return Ok(json!({ "dry_run": true }));
                }
                return Err(Error::GitHub("no GitHub account configured".into()));
            };
            let rel = gh.publish(&ctx, &p.owner, &p.repo, release_id).await?;
            Ok(json!({ "url": rel.html_url, "release_id": release_id }))
        }
        .await;
        let out = self.record(run, "publish", &started, &ctx, &r);
        if out.ok {
            if let Some(id) = run {
                let _ = self.store.set_run(id, None, if dry { "dry-run" } else { "done" });
            }
        }
        out
    }

    /// Finish a run that stops before publishing (e.g. local-only).
    pub fn finish_run(&self, run: i64, status: &str) -> Result<()> {
        self.store.set_run(run, None, status)
    }
}
