//! Live end-to-end release against a real GitHub repo, driving the same core
//! the UI uses. Every step is idempotent, so re-running resumes.
//!
//!   GH_TOKEN=... cargo run --no-default-features --example live_release -- <repo dir> <owner/repo> <feature branch>
//!
//! Without LIVE_CONFIRM=yes it runs as a dry run.
use release_desk::exec::Ctx;
use release_desk::pipeline::{self, Desk};
use release_desk::store::{MemorySecrets, Project, Secrets, Store};
use release_desk::version::Bump;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn say(s: &str) {
    println!("[{}] {}", chrono::Utc::now().format("%H:%M:%S"), release_desk::redact::redact(s));
}

fn check(step: &str, o: &pipeline::StepOutcome) {
    say(&format!("== {step}: {}\n{}", if o.ok { "OK" } else { "FAILED" }, o.log));
    if !o.ok {
        std::process::exit(1);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, slug, feature) = (&args[1], &args[2], &args[3]);
    let (owner, repo) = slug.split_once('/').expect("owner/repo");
    let live = std::env::var("LIVE_CONFIRM").as_deref() == Ok("yes");
    let dry = !live;
    let token = std::env::var("GH_TOKEN").expect("GH_TOKEN");

    let secrets = Arc::new(MemorySecrets::default());
    secrets.set("live", &token).unwrap();
    let store = Store::open(std::path::Path::new("/tmp/live/desk.sqlite3")).unwrap();
    let desk = Desk { store, secrets, api_base: None, ai_base: None, runners: Default::default() };
    if desk.store.accounts().unwrap().is_empty() {
        desk.store.add_account("live", owner, "live").unwrap();
    }
    let pid = match desk.store.projects().unwrap().into_iter().find(|p| p.path == *dir) {
        Some(p) => p.id,
        None => {
            let d = pipeline::detect(&Ctx::new(false), std::path::Path::new(dir)).unwrap();
            let mut p = Project {
                id: 0, name: d.name, path: d.path, owner: owner.into(), repo: repo.into(), account_id: None,
                default_branch: "main".into(), release_branch: "main".into(), tag_format: "v{version}".into(),
                local_only: false, adapters: d.adapters, pipeline: vec![], options: Default::default(),
            };
            p.options.merge_mode = "pr".into();
            p.options.merge_method = "squash".into();
            desk.store.add_project(&p).unwrap()
        }
    };
    // Effective project: the repo's release-desk.json applies on top.
    let p = desk.project(pid).unwrap();
    say(&format!("project {} -> {}/{} dry_run={dry} adapters={:?} workflow={:?} changelog={}",
        p.name, p.owner, p.repo, p.adapters.iter().map(|a| a.id()).collect::<Vec<_>>(), p.options.workflow, p.options.changelog));
    let run = match desk.store.runs(Some(pid)).unwrap().into_iter().find(|r| r.status == "running" || r.status == "failed") {
        Some(r) => { desk.resume_info(r.id).unwrap(); r.id }
        None => desk.store.start_run(pid, "0.1.0", "0.1.0", dry).unwrap(),
    };
    say(&format!("audit run #{run}"));
    let ctx = Ctx::new(false);
    let repo_path = std::path::Path::new(dir);

    // 1. PR from the feature branch, wait for real checks, squash-merge.
    let branch = release_desk::git::current_branch(&ctx, repo_path).unwrap();
    if branch == *feature {
        let o = desk.step_open_pr(Some(run), &p, dry, "feat: add Phase 3 (changelog, repo config, runners, AI notes, updater)",
            "Phase 3 of the spec: CHANGELOG.md, release-desk.json, pipeline templates, self-hosted runner panel, AI notes polishing, updater and Windows release workflow.").await;
        check("open PR", &o);
        let Some(n) = o.meta["pr"].as_u64() else { say("dry run: stopping before checks"); return };
        let start = Instant::now();
        loop {
            let s = desk.pr_status(&p, n).await.unwrap();
            say(&format!("PR #{n}: {} checks, {} passed, {} failed, {} pending", s.checks.total, s.checks.passed, s.checks.failed, s.checks.pending));
            if s.checks.failed > 0 {
                for c in &s.checks.runs { say(&format!("  {} {} {:?} {:?}", c.name, c.status, c.conclusion, c.html_url)); }
                say("checks failed: not merging");
                std::process::exit(2);
            }
            if s.checks.total >= 2 && s.checks.pending == 0 { break; }
            if start.elapsed() > Duration::from_secs(25 * 60) { say("timed out waiting for checks"); std::process::exit(3); }
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
        check("merge PR", &desk.step_merge_pr(Some(run), &p, dry, n, false).await);
    } else {
        say(&format!("on {branch}: PR already merged, continuing"));
    }

    // 2. Version, notes (+ changelog), release commit + tag + push.
    let tag_exists_already = release_desk::git::last_tag(&ctx, repo_path, "v*").is_some();
    let plan = if tag_exists_already {
        let v = desk.store.run(run).unwrap().to_version;
        pipeline::plan(&ctx, &p, &Bump::Custom { version: v }).unwrap()
    } else {
        pipeline::plan(&ctx, &p, &Bump::Auto).unwrap()
    };
    say(&format!("plan: {} -> {} ({:?}), {} commits, tag {}", plan.current, plan.next, plan.suggested_level, plan.commits.len(), plan.tag));
    check("version", &desk.step_version(Some(run), &p, dry, &plan.next));
    let notes = desk.store.run(run).unwrap().steps.iter().rev().find(|s| s.step_key == "notes")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s.meta_json).ok())
        .and_then(|m| m["notes"].as_str().map(String::from))
        .unwrap_or(plan.notes.clone());
    check("notes", &desk.step_notes(Some(run), &p, dry, &notes));
    check("tag + push", &desk.step_tag(Some(run), &p, dry, &plan.next, &notes));

    // 3. Real workflow_dispatch of the cheap CI workflow, to prove the dispatch call.
    let mut ci = p.clone();
    ci.options.workflow = Some("ci.yml".into());
    ci.options.dispatch_inputs = [("reason".to_string(), "release-desk live test {tag}".to_string())].into();
    check("dispatch ci.yml", &desk.step_dispatch(Some(run), &ci, dry, &plan.next, &notes).await);

    // 4. Watch the tag-triggered release build.
    let start = Instant::now();
    let release_run = loop {
        let runs = desk.actions_runs(&p, Some(&plan.tag)).await.unwrap();
        for r in &runs {
            say(&format!("  run {} #{} {} {:?}/{:?}", r.name.as_deref().unwrap_or("?"), r.run_number, r.event, r.status, r.conclusion));
        }
        if let Some(r) = runs.iter().find(|r| r.name.as_deref() == Some("release") && r.event == "push") {
            if let Ok(jobs) = desk.actions_jobs(&p, r.id).await {
                for j in jobs {
                    let cur = j.steps.iter().find(|s| s.status != "completed").map(|s| s.name.clone()).unwrap_or_default();
                    say(&format!("    job {} {} {:?} {cur}", j.name, j.status, j.conclusion));
                }
            }
            if r.status.as_deref() == Some("completed") {
                if r.conclusion.as_deref() != Some("success") {
                    say(&format!("release build failed: {}", r.html_url));
                    std::process::exit(4);
                }
                break r.clone();
            }
        }
        if start.elapsed() > Duration::from_secs(45 * 60) { say("timed out waiting for the build"); std::process::exit(5); }
        tokio::time::sleep(Duration::from_secs(30)).await;
    };
    say(&format!("build done: {}", release_run.html_url));

    // 5. Draft release, attach the installers, publish.
    let r = desk.step_release(Some(run), &p, dry, &plan.next, &notes, false).await;
    check("draft release", &r);
    let rid = r.meta["release_id"].as_u64().unwrap_or(0);
    check("attach artifacts", &desk.step_attach(Some(run), &p, dry, rid, release_run.id).await);
    check("publish", &desk.step_publish(Some(run), &p, dry, rid).await);
    say(&format!("RELEASED {} -> run #{run} status {}", plan.tag, desk.store.run(run).unwrap().status));
}
