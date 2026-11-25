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
    /// Overridable for tests. None = api.anthropic.com.
    pub ai_base: Option<String>,
    pub runners: crate::runner::Runners,
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

    /// Stored project with the repo's `release-desk.json` applied on top.
    pub fn project(&self, id: i64) -> Result<Project> {
        let mut p = self.store.project(id)?;
        if let Some(c) = crate::config::load(&Desk::repo(&p))? {
            crate::config::apply(&mut p, &c)?;
        }
        Ok(p)
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
                if p.options.changelog {
                    let f = p.options.changelog_file.clone();
                    let old = std::fs::read_to_string(repo.join(&f)).unwrap_or_default();
                    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    match crate::notes::changelog_insert(&old, &v.to_string(), &date, notes_md) {
                        Some(new) => {
                            version::apply_edits(&ctx, &repo, &[version::FileEdit { path: f.clone(), content: new }])?;
                            files.push(f);
                        }
                        None => ctx.log(&format!("{f} already has a section for {v}")),
                    }
                }
                let changed: Vec<String> = {
                    let st = git::status(&ctx, &repo)?;
                    files.into_iter().filter(|f| st.files.iter().any(|c| &c.path == f)).collect()
                };
                if !changed.is_empty() || (dry && (p.options.changelog || !p.adapters.iter().all(|a| *a == AdapterConfig::GitTag))) {
                    let to_stage = if changed.is_empty() {
                        let mut f: Vec<String> = p.adapters.iter().flat_map(|a| version::plan_write(a, &repo, &v).unwrap_or_default()).map(|e| e.path).collect();
                        if p.options.changelog { f.push(p.options.changelog_file.clone()); }
                        f
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
                // Idempotent: skip the tag push when origin already has this exact tag.
                let local = git::read(&ctx, &repo, &["rev-parse", &format!("refs/tags/{tag}")]).unwrap_or_default();
                let remote = git::read(&ctx, &repo, &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")]).unwrap_or_default();
                if !local.trim().is_empty() && remote.split_whitespace().next() == Some(local.trim()) {
                    ctx.log(&format!("origin already has {tag}, not pushing it again"));
                } else {
                    git::push(&ctx, &repo, &format!("refs/tags/{tag}"))?;
                }
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

// ================================================================ Phase 2

/// Files inside an artifact zip, flattened to their base names.
fn unzip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| Error::Other(format!("artifact zip: {e}")))?;
    let mut out = vec![];
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| Error::Other(e.to_string()))?;
        // Directories and symlinks are skipped: only regular files become assets.
        if f.is_dir() || f.is_symlink() {
            continue;
        }
        let full = f.name().map_err(|e| Error::Other(format!("artifact zip: {e}")))?.to_string();
        let name = full.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let mut buf = vec![];
        std::io::Read::read_to_end(&mut f, &mut buf)?;
        out.push((name, buf));
    }
    Ok(out)
}

fn render_input(tpl: &str, version: &str, tag: &str, notes_md: &str) -> String {
    tpl.replace("{version}", version).replace("{tag}", tag).replace("{notes}", notes_md)
}

fn parse_next(next: &str) -> Result<Version> {
    Version::parse(next.trim_start_matches('v')).map_err(|e| Error::Version(e.to_string()))
}

#[derive(Debug, Serialize)]
pub struct PrStatus {
    pub pr: crate::github::Pr,
    pub checks: crate::github::Checks,
}

#[derive(Debug, Serialize)]
pub struct ResumeInfo {
    pub run_id: i64,
    pub project_id: i64,
    pub dry_run: bool,
    pub status: String,
    pub from_version: String,
    pub to_version: String,
    pub notes: Option<String>,
    pub pr_number: Option<u64>,
    pub release_id: Option<u64>,
    pub actions_run_id: Option<u64>,
    /// Latest status per step key.
    pub steps: std::collections::BTreeMap<String, String>,
}

impl Desk {
    fn gh(&self, p: &Project) -> Result<(Account, Client)> {
        self.client_for(p)?.ok_or_else(|| Error::GitHub("no GitHub account configured (Settings)".into()))
    }

    // ------------------------------------------------------------ merge

    /// Find or open a PR from the current branch into the default branch.
    pub async fn step_open_pr(&self, run: Option<i64>, p: &Project, dry: bool, title: &str, body: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "pull request")?;
            let repo = Desk::repo(p);
            let head = git::current_branch(&ctx, &repo)?;
            if head == p.default_branch {
                return Err(Error::Blocked(format!(
                    "already on '{head}'. Work on a feature branch, or set merge mode to 'none' for this project"
                )));
            }
            let (acc, gh) = self.gh(p)?;
            ctx.log(&format!("account: {} ({})", acc.label, acc.github_login));
            let pr = match gh.find_open_pr(&ctx, &p.owner, &p.repo, &head, &p.default_branch).await? {
                Some(pr) => {
                    ctx.log(&format!("PR #{} already open for {head}", pr.number));
                    Some(pr)
                }
                None => gh.create_pr(&ctx, &p.owner, &p.repo, &head, &p.default_branch, title, body).await?,
            };
            Ok(match pr {
                Some(pr) => json!({ "pr": pr.number, "url": pr.html_url, "head": head, "base": p.default_branch }),
                None => json!({ "pr": null, "head": head, "base": p.default_branch, "dry_run": true }),
            })
        }
        .await;
        self.record(run, "pr", &started, &ctx, &r)
    }

    /// PR state plus check-run summary for its head commit (read-only, polled by the UI).
    pub async fn pr_status(&self, p: &Project, n: u64) -> Result<PrStatus> {
        guard_remote(p, "pull request")?;
        let ctx = self.ctx(false);
        let (_, gh) = self.gh(p)?;
        let pr = gh.get_pr(&ctx, &p.owner, &p.repo, n).await?;
        let checks = gh.checks(&ctx, &p.owner, &p.repo, &pr.head.sha).await?;
        Ok(PrStatus { pr, checks })
    }

    /// Merge the PR (pinned to the head sha that was checked), then bring the
    /// local default branch up to date with a fast-forward.
    pub async fn step_merge_pr(&self, run: Option<i64>, p: &Project, dry: bool, n: u64, allow_failing: bool) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "merge")?;
            let repo = Desk::repo(p);
            if dry && n == 0 {
                // Dry run never created the PR, so there is nothing to look up.
                ctx.side_effect(&format!("PUT /repos/{}/{}/pulls/<new>/merge ({})", p.owner, p.repo, p.options.merge_method));
                git::fetch(&ctx, &repo)?;
                git::checkout(&ctx, &repo, &p.default_branch)?;
                git::merge_ff_only(&ctx, &repo, &format!("origin/{}", p.default_branch))?;
                return Ok(json!({ "pr": null, "dry_run": true }));
            }
            let (_, gh) = self.gh(p)?;
            let pr = gh.get_pr(&ctx, &p.owner, &p.repo, n).await?;
            if pr.merged {
                ctx.log(&format!("PR #{n} is already merged"));
            } else {
                let c = gh.checks(&ctx, &p.owner, &p.repo, &pr.head.sha).await?;
                ctx.log(&format!("checks: {} passed, {} failed, {} pending", c.passed, c.failed, c.pending));
                if (c.failed > 0 || c.pending > 0) && !allow_failing {
                    return Err(Error::Blocked("checks are not green yet".into()));
                }
                if pr.mergeable == Some(false) {
                    return Err(Error::Blocked(format!("PR #{n} has conflicts")));
                }
                gh.merge_pr(&ctx, &p.owner, &p.repo, n, &p.options.merge_method, &pr.head.sha).await?;
            }
            let st = git::status(&ctx, &repo)?;
            if !st.files.is_empty() {
                ctx.log("working tree not clean: skipped switching to the default branch");
                return Ok(json!({ "pr": n, "merged": true, "synced": false }));
            }
            git::fetch(&ctx, &repo)?;
            git::checkout(&ctx, &repo, &p.default_branch)?;
            git::merge_ff_only(&ctx, &repo, &format!("origin/{}", p.default_branch))?;
            Ok(json!({ "pr": n, "merged": true, "synced": true, "method": p.options.merge_method }))
        }
        .await;
        self.record(run, "merge", &started, &ctx, &r)
    }

    /// Local fast-forward of the default branch to the current branch. Allowed
    /// for local-only projects: it never touches a remote.
    pub fn step_merge_ff(&self, run: Option<i64>, p: &Project, dry: bool) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            let repo = Desk::repo(p);
            let head = git::current_branch(&ctx, &repo)?;
            if head == p.default_branch {
                ctx.log(&format!("already on {head}, nothing to merge"));
                return Ok(json!({ "skipped": true }));
            }
            if !git::status(&ctx, &repo)?.files.is_empty() {
                return Err(Error::Blocked("commit or discard changes before merging".into()));
            }
            git::checkout(&ctx, &repo, &p.default_branch)?;
            git::merge_ff_only(&ctx, &repo, &head)?;
            Ok(json!({ "from": head, "into": p.default_branch }))
        })();
        self.record(run, "merge", &started, &ctx, &r)
    }

    // ------------------------------------------------------------ actions

    pub async fn step_dispatch(&self, run: Option<i64>, p: &Project, dry: bool, next: &str, notes_md: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "workflow dispatch")?;
            let wf = p.options.workflow.clone().filter(|w| !w.trim().is_empty())
                .ok_or_else(|| Error::Other("no workflow set for this project (Edit project)".into()))?;
            let v = parse_next(next)?;
            let tag = version::render_tag(&p.tag_format, &v);
            let inputs: serde_json::Map<String, Value> = p.options.dispatch_inputs.iter()
                .map(|(k, t)| (k.clone(), Value::String(render_input(t, &v.to_string(), &tag, notes_md))))
                .collect();
            let (_, gh) = self.gh(p)?;
            gh.dispatch(&ctx, &p.owner, &p.repo, &wf, &tag, &Value::Object(inputs.clone())).await?;
            Ok(json!({ "workflow": wf, "ref": tag, "inputs": inputs }))
        }
        .await;
        self.record(run, "dispatch", &started, &ctx, &r)
    }

    /// Runs for the release tag's commit (falls back to HEAD).
    pub async fn actions_runs(&self, p: &Project, git_ref: Option<&str>) -> Result<Vec<crate::github::WorkflowRun>> {
        guard_remote(p, "Actions")?;
        let ctx = self.ctx(false);
        let repo = Desk::repo(p);
        let sha = match git_ref {
            Some(r) if !r.is_empty() => git::commit_of(&ctx, &repo, r).or_else(|_| git::head_sha(&ctx, &repo))?,
            _ => git::head_sha(&ctx, &repo)?,
        };
        let (_, gh) = self.gh(p)?;
        gh.runs_for_sha(&ctx, &p.owner, &p.repo, &sha).await
    }

    pub async fn actions_jobs(&self, p: &Project, run_id: u64) -> Result<Vec<crate::github::Job>> {
        guard_remote(p, "Actions")?;
        let (_, gh) = self.gh(p)?;
        gh.jobs(&self.ctx(false), &p.owner, &p.repo, run_id).await
    }

    /// Cancel or re-run failed jobs. `action` is "cancel" or "rerun".
    pub async fn actions_control(&self, run: Option<i64>, p: &Project, dry: bool, run_id: u64, action: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "Actions")?;
            let (_, gh) = self.gh(p)?;
            match action {
                "cancel" => gh.cancel_run(&ctx, &p.owner, &p.repo, run_id).await?,
                "rerun" => gh.rerun_failed(&ctx, &p.owner, &p.repo, run_id).await?,
                _ => return Err(Error::Other(format!("unknown action {action}"))),
            }
            Ok(json!({ "actions_run": run_id, "action": action }))
        }
        .await;
        self.record(run, &format!("actions-{action}"), &started, &ctx, &r)
    }

    /// Download each artifact of a workflow run and upload it to the release as
    /// `<name>.zip`. Assets that already exist are skipped (safe to repeat).
    pub async fn step_attach(&self, run: Option<i64>, p: &Project, dry: bool, release_id: u64, actions_run: u64) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = async {
            guard_remote(p, "release assets")?;
            let (_, gh) = self.gh(p)?;
            let arts = gh.run_artifacts(&ctx, &p.owner, &p.repo, actions_run).await?;
            let rel: Option<crate::github::Release> = if release_id == 0 {
                None
            } else {
                Some(gh.get_json(&ctx, &format!("/repos/{}/{}/releases/{release_id}", p.owner, p.repo), "get release").await?)
            };
            let mut uploaded = vec![];
            let mut skipped = vec![];
            for a in arts.iter().filter(|a| !a.expired) {
                let name = format!("{}.zip", a.name);
                if rel.as_ref().map_or(false, |r| r.assets.iter().any(|x| x.name == name)) {
                    ctx.log(&format!("{name} already attached"));
                    skipped.push(name);
                    continue;
                }
                match &rel {
                    Some(rel) if !dry && p.options.attach_mode == "files" => {
                        let bytes = gh.download_artifact(&ctx, &p.owner, &p.repo, a.id).await?;
                        for (file, data) in unzip(&bytes)? {
                            if rel.assets.iter().any(|x| x.name == file) {
                                ctx.log(&format!("{file} already attached"));
                                skipped.push(file);
                                continue;
                            }
                            gh.upload_asset(&ctx, rel, &file, data).await?;
                            uploaded.push(file);
                        }
                        continue;
                    }
                    Some(rel) if !dry => {
                        let bytes = gh.download_artifact(&ctx, &p.owner, &p.repo, a.id).await?;
                        gh.upload_asset(&ctx, rel, &name, bytes).await?;
                    }
                    _ => {
                        ctx.side_effect(&format!("upload {name} ({} bytes) to release {release_id}", a.size_in_bytes));
                    }
                }
                uploaded.push(name);
            }
            if arts.is_empty() {
                ctx.log("this run has no artifacts");
            }
            Ok(json!({ "actions_run": actions_run, "uploaded": uploaded, "skipped": skipped }))
        }
        .await;
        self.record(run, "attach", &started, &ctx, &r)
    }

    // ------------------------------------------------------------ rollback

    /// Undo a local release that has not reached the remote: delete the local
    /// tag, drop the `chore(release)` commit (soft reset) and restore the version
    /// files. Refuses if the commit or tag is already on the remote.
    pub fn rollback(&self, run: Option<i64>, p: &Project, dry: bool, next: &str) -> StepOutcome {
        let ctx = self.ctx(dry);
        let started = now();
        let r = (|| {
            let repo = Desk::repo(p);
            let v = parse_next(next)?;
            let tag = version::render_tag(&p.tag_format, &v);
            let subject = git::subject(&ctx, &repo, "HEAD")?;
            let release_commit = subject == format!("chore(release): {tag}");
            if release_commit && !p.local_only && git::is_on_remote(&ctx, &repo, "HEAD")? {
                return Err(Error::Blocked(format!("{tag} commit is already pushed. Rollback never rewrites remote history")));
            }
            if git::tag_exists(&ctx, &repo, &tag) {
                if !p.local_only && git::read(&ctx, &repo, &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")])
                    .map(|o| !o.trim().is_empty()).unwrap_or(false)
                {
                    return Err(Error::Blocked(format!("tag {tag} is already on origin")));
                }
                git::write(&ctx, &repo, &["tag", "-d", &tag])?;
            }
            if release_commit {
                git::write(&ctx, &repo, &["reset", "--soft", "HEAD~1"])?;
            }
            // Restore version files to HEAD (drops the bump whether committed or not).
            let mut files = vec![];
            for a in &p.adapters {
                files.extend(version::plan_write(a, &repo, &v).unwrap_or_default().into_iter().map(|e| e.path));
            }
            if p.options.changelog {
                let f = p.options.changelog_file.clone();
                if git::is_tracked(&ctx, &repo, &f) || release_commit {
                    files.push(f);
                }
            }
            // Files new in the release commit are not in HEAD after the reset: unstage them instead.
            let (tracked, new): (Vec<String>, Vec<String>) = files.into_iter().partition(|f| {
                git::read(&ctx, &repo, &["cat-file", "-e", &format!("HEAD:{f}")]).is_ok()
            });
            if !new.is_empty() {
                let mut args = vec!["reset", "-q", "--"];
                args.extend(new.iter().map(|s| s.as_str()));
                git::write(&ctx, &repo, &args)?;
                ctx.log(&format!("left as untracked (new in this release): {}", new.join(", ")));
            }
            let files = tracked;
            if !files.is_empty() {
                let mut args = vec!["checkout", "HEAD", "--"];
                args.extend(files.iter().map(|s| s.as_str()));
                git::write(&ctx, &repo, &args)?;
            }
            if let Some(id) = run {
                let _ = self.store.set_run(id, None, if dry { "dry-run" } else { "rolled-back" });
            }
            Ok(json!({ "tag": tag, "removed_commit": release_commit, "restored": files }))
        })();
        self.record(run, "rollback", &started, &ctx, &r)
    }

    // ------------------------------------------------------------ resume

    pub fn resume_info(&self, run_id: i64) -> Result<ResumeInfo> {
        let r = self.store.run(run_id)?;
        let mut info = ResumeInfo {
            run_id, project_id: r.project_id, dry_run: r.dry_run, status: r.status.clone(),
            from_version: r.from_version, to_version: r.to_version, notes: None, pr_number: None,
            release_id: None, actions_run_id: None, steps: Default::default(),
        };
        for s in &r.steps {
            info.steps.insert(s.step_key.clone(), s.status.clone());
            let m: Value = serde_json::from_str(&s.meta_json).unwrap_or(Value::Null);
            if s.status == "failed" {
                continue;
            }
            match s.step_key.as_str() {
                "notes" => info.notes = m["notes"].as_str().map(String::from),
                "pr" | "merge" => info.pr_number = m["pr"].as_u64().or(info.pr_number),
                "release" => info.release_id = m["release_id"].as_u64(),
                "attach" | "actions-rerun" => info.actions_run_id = m["actions_run"].as_u64(),
                _ => {}
            }
        }
        if r.status == "failed" || r.status == "running" {
            self.store.set_run(run_id, None, "running")?;
        }
        Ok(info)
    }
}
