//! Offline integration tests: a temp repo with a local bare remote, and a
//! wiremock GitHub. Run with `cargo test --no-default-features`.
use release_desk::exec::Ctx;
use release_desk::pipeline::{self, Desk};
use release_desk::store::{MemorySecrets, Project, Secrets, Store};
use release_desk::version::{AdapterConfig, Bump};
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "ghp_TESTTOKEN000000000000000000000000000";

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Fx {
    _tmp: tempfile::TempDir,
    work: std::path::PathBuf,
    bare: std::path::PathBuf,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    sh(tmp.path(), &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
    sh(&work, &["init", "-q", "-b", "main"]);
    sh(&work, &["config", "user.name", "Test"]);
    sh(&work, &["config", "user.email", "t@example.com"]);
    sh(&work, &["config", "commit.gpgsign", "false"]);
    sh(&work, &["config", "tag.gpgsign", "false"]);
    std::fs::write(work.join("package.json"), "{\n  \"name\": \"demo\",\n  \"version\": \"1.0.0\"\n}\n").unwrap();
    sh(&work, &["add", "."]);
    sh(&work, &["commit", "-q", "-m", "chore: init"]);
    sh(&work, &["tag", "-a", "v1.0.0", "-m", "v1.0.0"]);
    sh(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    sh(&work, &["push", "-q", "origin", "main", "--tags"]);
    // Unreleased work: one feature, one fix.
    std::fs::write(work.join("a.txt"), "a").unwrap();
    sh(&work, &["add", "."]);
    sh(&work, &["commit", "-q", "-m", "feat(core): add a"]);
    std::fs::write(work.join("b.txt"), "b").unwrap();
    Fx { _tmp: tmp, work, bare }
}

fn project(fx: &Fx, local_only: bool) -> Project {
    Project {
        id: 0, name: "demo".into(), path: fx.work.to_string_lossy().into(), owner: "o".into(), repo: "r".into(),
        account_id: None, default_branch: "main".into(), release_branch: "main".into(),
        tag_format: "v{version}".into(), local_only, adapters: vec![AdapterConfig::Npm { file: None }], pipeline: vec![],
        options: Default::default(),
    }
}

fn desk(api: Option<String>) -> Desk {
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set("acc", TOKEN).unwrap();
    let store = Store::memory().unwrap();
    store.add_account("personal", "tester", "acc").unwrap();
    Desk { store, secrets, api_base: api, ai_base: None, runners: Default::default() }
}

/// Snapshot of everything a release could change.
fn snapshot(fx: &Fx) -> (String, String, String, String, String) {
    (
        sh(&fx.work, &["rev-parse", "HEAD"]),
        sh(&fx.work, &["tag", "-l"]),
        sh(&fx.work, &["status", "--porcelain"]),
        sh(&fx.bare, &["show-ref"]),
        std::fs::read_to_string(fx.work.join("package.json")).unwrap(),
    )
}

async fn github_mock() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/o/r/releases/tags/v1.1.0"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message":"Not Found"}))).mount(&s).await;
    Mock::given(method("GET")).and(path("/repos/o/r/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&s).await;
    Mock::given(method("POST")).and(path("/repos/o/r/releases")).and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(|req: &Request| {
            let b: serde_json::Value = req.body_json().unwrap();
            assert_eq!(b["draft"], true);
            ResponseTemplate::new(201).set_body_json(json!({
                "id": 77, "tag_name": b["tag_name"], "html_url": "https://github.com/o/r/releases/tag/v1.1.0",
                "draft": true, "prerelease": false }))
        })
        .mount(&s).await;
    Mock::given(method("PATCH")).and(path("/repos/o/r/releases/77"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 77, "tag_name": "v1.1.0", "html_url": "https://github.com/o/r/releases/tag/v1.1.0",
            "draft": false, "prerelease": false })))
        .mount(&s).await;
    s
}

#[tokio::test]
async fn full_pipeline_offline() {
    let fx = fixture();
    let gh = github_mock().await;
    let d = desk(Some(gh.uri()));
    let p = project(&fx, false);
    let pid = d.store.add_project(&p).unwrap();
    let p = d.store.project(pid).unwrap();
    let run = d.store.start_run(pid, "1.0.0", "1.0.0", false).unwrap();

    let st = release_desk::git::status(&Ctx::new(false), &fx.work).unwrap();
    assert_eq!(st.files.len(), 1);
    assert!(d.step_commit(Some(run), &p, false, &["b.txt".into()], "fix: add b").ok);
    assert!(d.step_push(Some(run), &p, false).ok);

    let plan = pipeline::plan(&Ctx::new(false), &p, &Bump::Auto).unwrap();
    assert_eq!(plan.current, "1.0.0");
    assert_eq!(plan.next, "1.1.0");
    assert_eq!(plan.tag, "v1.1.0");
    assert!(plan.notes.contains("### Features") && plan.notes.contains("### Fixes"));

    assert!(d.step_version(Some(run), &p, false, &plan.next).ok);
    assert!(d.step_notes(Some(run), &p, false, &plan.notes).ok);
    let t = d.step_tag(Some(run), &p, false, &plan.next, &plan.notes);
    assert!(t.ok, "{}", t.log);
    assert_eq!(t.meta["pushed"], true);
    assert_eq!(sh(&fx.work, &["log", "-1", "--format=%s"]), "chore(release): v1.1.0");
    assert!(sh(&fx.bare, &["tag", "-l"]).contains("v1.1.0"));
    assert!(sh(&fx.work, &["status", "--porcelain"]).is_empty());
    let tag_msg = sh(&fx.work, &["tag", "-l", "--format=%(contents)", "v1.1.0"]);
    assert!(tag_msg.contains("### Features") && tag_msg.contains("### Fixes"), "markdown headings kept in tag: {tag_msg}");

    let r = d.step_release(Some(run), &p, false, &plan.next, &plan.notes, false).await;
    assert!(r.ok, "{}", r.log);
    assert_eq!(r.meta["release_id"], 77);
    let pb = d.step_publish(Some(run), &p, false, 77).await;
    assert!(pb.ok, "{}", pb.log);

    // Idempotent re-run of tag step: no duplicate commit or tag error.
    let again = d.step_tag(Some(run), &p, false, &plan.next, &plan.notes);
    assert!(again.ok && again.meta["committed"] == false, "{}", again.log);
    assert!(again.log.contains("origin already has v1.1.0"), "{}", again.log);

    let runs = d.store.runs(Some(pid)).unwrap();
    assert_eq!(runs[0].status, "done");
    assert_eq!(runs[0].to_version, "1.1.0");
    let keys: Vec<_> = runs[0].steps.iter().map(|s| s.step_key.as_str()).collect();
    assert_eq!(keys, ["commit", "push", "version", "notes", "tag", "release", "publish", "tag"]);
    for s in &runs[0].steps {
        assert!(!s.log.contains(TOKEN) && !s.meta_json.contains(TOKEN), "token leaked in {}", s.step_key);
    }
}

#[tokio::test]
async fn dry_run_has_zero_side_effects() {
    let fx = fixture();
    let gh = MockServer::start().await;
    // Reads are allowed; any mutating request fails the test via expect(0).
    Mock::given(method("GET")).and(path("/repos/o/r/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&gh).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(404).set_body_json(json!({}))).mount(&gh).await;
    for m in ["POST", "PUT", "PATCH", "DELETE"] {
        Mock::given(method(m)).respond_with(ResponseTemplate::new(500)).expect(0).mount(&gh).await;
    }
    let d = desk(Some(gh.uri()));
    let pid = d.store.add_project(&project(&fx, false)).unwrap();
    let p = d.store.project(pid).unwrap();
    let before = snapshot(&fx);

    let mut logs = String::new();
    for o in [
        d.step_commit(None, &p, true, &["b.txt".into()], "fix: add b"),
        d.step_push(None, &p, true),
        d.step_version(None, &p, true, "1.1.0"),
        d.step_notes(None, &p, true, "notes"),
        d.step_tag(None, &p, true, "1.1.0", "notes"),
        d.step_release(None, &p, true, "1.1.0", "notes", false).await,
        d.step_publish(None, &p, true, 1).await,
    ] {
        assert!(o.ok, "{}", o.log);
        logs.push_str(&o.log);
        logs.push('\n');
    }
    assert_eq!(before, snapshot(&fx), "dry run changed the repo or remote");
    for planned in ["would git add", "would git commit", "would git push origin main", "would write package.json",
        "would git tag -a --cleanup=whitespace v1.1.0", "would POST /repos/o/r/releases", "would PATCH /repos/o/r/releases/1"] {
        assert!(logs.contains(planned), "missing plan line '{planned}' in:\n{logs}");
    }
    assert!(!logs.contains(TOKEN));
    gh.verify().await;
}

#[tokio::test]
async fn local_only_cannot_reach_remote() {
    let fx = fixture();
    let gh = MockServer::start().await;
    Mock::given(wiremock::matchers::any()).respond_with(ResponseTemplate::new(500)).expect(0).mount(&gh).await;
    let d = desk(Some(gh.uri()));
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.store.project(pid).unwrap();
    let remote_before = sh(&fx.bare, &["show-ref"]);

    for dry in [false, true] {
        let push = d.step_push(None, &p, dry);
        assert!(!push.ok && push.log.contains("local-only"));
        assert!(!d.step_release(None, &p, dry, "1.1.0", "n", false).await.ok);
        assert!(!d.step_publish(None, &p, dry, 1).await.ok);
    }
    // Local commit + tag still work, but nothing is pushed.
    assert!(d.step_commit(None, &p, false, &["b.txt".into()], "fix: b").ok);
    assert!(d.step_version(None, &p, false, "1.1.0").ok);
    let t = d.step_tag(None, &p, false, "1.1.0", "n");
    assert!(t.ok && t.meta["pushed"] == false, "{}", t.log);
    assert!(sh(&fx.work, &["tag", "-l"]).contains("v1.1.0"));
    assert_eq!(remote_before, sh(&fx.bare, &["show-ref"]));
    gh.verify().await;
}

#[test]
fn detect_reads_remote_adapters_and_push_ban() {
    let fx = fixture();
    sh(&fx.work, &["remote", "set-url", "origin", "git@github.com:apratemprajapati/ytdwld.git"]);
    std::fs::write(fx.work.join("CLAUDE.md"), "# Rules\nDo NOT push to any remote.\n").unwrap();
    let d = pipeline::detect(&Ctx::new(false), &fx.work).unwrap();
    assert_eq!((d.owner.as_str(), d.repo.as_str()), ("apratemprajapati", "ytdwld"));
    assert_eq!(d.default_branch, "main");
    assert_eq!(d.adapters, vec![AdapterConfig::Npm { file: None }]);
    assert!(d.has_claude_md_push_ban);
}

// ================================================================ Phase 2

fn pr_json(merged: bool) -> serde_json::Value {
    json!({ "number": 5, "html_url": "https://github.com/o/r/pull/5", "state": "open", "merged": merged,
        "mergeable": true, "head": { "ref": "feature", "sha": "abc123" }, "base": { "ref": "main", "sha": "000" } })
}

async fn mock_checks(gh: &MockServer, statuses: &[(&str, Option<&str>)]) {
    let runs: Vec<_> = statuses.iter().map(|(s, c)| json!({ "name": "ci", "status": s, "conclusion": c })).collect();
    Mock::given(method("GET")).and(path("/repos/o/r/commits/abc123/check-runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "total_count": runs.len(), "check_runs": runs })))
        .mount(gh).await;
}

#[tokio::test]
async fn pr_open_wait_and_merge() {
    let fx = fixture();
    sh(&fx.work, &["checkout", "-q", "-b", "feature"]);
    sh(&fx.work, &["add", "."]);
    sh(&fx.work, &["commit", "-q", "-m", "fix: b"]);
    sh(&fx.work, &["push", "-q", "origin", "feature"]);
    let gh = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/o/r/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&gh).await;
    Mock::given(method("POST")).and(path("/repos/o/r/pulls"))
        .respond_with(|req: &Request| {
            let b: serde_json::Value = req.body_json().unwrap();
            assert_eq!((b["head"].as_str(), b["base"].as_str()), (Some("feature"), Some("main")));
            ResponseTemplate::new(201).set_body_json(pr_json(false))
        }).expect(1).mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/pulls/5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_json(false))).mount(&gh).await;
    mock_checks(&gh, &[("completed", Some("success")), ("in_progress", None)]).await;
    Mock::given(method("PUT")).and(path("/repos/o/r/pulls/5/merge"))
        .respond_with(|req: &Request| {
            let b: serde_json::Value = req.body_json().unwrap();
            assert_eq!(b["sha"], "abc123", "merge must pin the checked sha");
            assert_eq!(b["merge_method"], "squash");
            ResponseTemplate::new(200).set_body_json(json!({ "merged": true }))
        }).expect(1).mount(&gh).await;

    let d = desk(Some(gh.uri()));
    let mut p = project(&fx, false);
    p.options.merge_mode = "pr".into();
    let pid = d.store.add_project(&p).unwrap();
    let p = d.store.project(pid).unwrap();
    assert_eq!(p.options.merge_mode, "pr", "options persist");

    let o = d.step_open_pr(None, &p, false, "Release", "").await;
    assert!(o.ok, "{}", o.log);
    assert_eq!(o.meta["pr"], 5);

    let st = d.pr_status(&p, 5).await.unwrap();
    assert_eq!((st.checks.passed, st.checks.pending), (1, 1));
    let blocked = d.step_merge_pr(None, &p, false, 5, false).await;
    assert!(!blocked.ok && blocked.log.contains("not green"), "{}", blocked.log);

    let m = d.step_merge_pr(None, &p, false, 5, true).await;
    assert!(m.ok, "{}", m.log);
    assert_eq!(m.meta["synced"], true);
    assert_eq!(sh(&fx.work, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    gh.verify().await;
}

#[test]
fn local_fast_forward_merge() {
    let fx = fixture();
    sh(&fx.work, &["checkout", "-q", "-b", "feature"]);
    sh(&fx.work, &["add", "."]);
    sh(&fx.work, &["commit", "-q", "-m", "fix: b"]);
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.store.project(pid).unwrap();
    let feature = sh(&fx.work, &["rev-parse", "HEAD"]);
    let o = d.step_merge_ff(None, &p, false);
    assert!(o.ok, "{}", o.log);
    assert_eq!(sh(&fx.work, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(sh(&fx.work, &["rev-parse", "HEAD"]), feature);
}

#[tokio::test]
async fn dispatch_and_attach_artifacts() {
    let fx = fixture();
    let gh = MockServer::start().await;
    Mock::given(method("POST")).and(path("/repos/o/r/actions/workflows/release.yml/dispatches"))
        .respond_with(|req: &Request| {
            let b: serde_json::Value = req.body_json().unwrap();
            assert_eq!(b, json!({ "ref": "v1.1.0", "inputs": { "version": "1.1.0", "title": "Release v1.1.0" } }));
            ResponseTemplate::new(204)
        }).expect(1).mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/actions/runs/900/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "artifacts": [
            { "id": 1, "name": "dist", "size_in_bytes": 4, "expired": false },
            { "id": 2, "name": "old", "size_in_bytes": 4, "expired": true },
            { "id": 3, "name": "docs", "size_in_bytes": 4, "expired": false } ] })))
        .mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/actions/artifacts/1/zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK\x03\x04".to_vec())).expect(1).mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/releases/77"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 77, "tag_name": "v1.1.0", "html_url": "",
            "draft": true, "prerelease": false, "upload_url": format!("{}/upload/77/assets{{?name,label}}", gh.uri()),
            "assets": [ { "id": 9, "name": "docs.zip" } ] })))
        .mount(&gh).await;
    Mock::given(method("POST")).and(path("/upload/77/assets")).and(wiremock::matchers::query_param("name", "dist.zip"))
        .respond_with(|req: &Request| {
            assert_eq!(req.body, b"PK\x03\x04".to_vec());
            ResponseTemplate::new(201).set_body_json(json!({ "id": 10 }))
        }).expect(1).mount(&gh).await;

    let d = desk(Some(gh.uri()));
    let mut p = project(&fx, false);
    p.options.workflow = Some("release.yml".into());
    p.options.dispatch = true;
    p.options.dispatch_inputs.insert("title".into(), "Release {tag}".into());
    p.options.attach_mode = "zip".into();
    let pid = d.store.add_project(&p).unwrap();
    let p = d.store.project(pid).unwrap();

    let o = d.step_dispatch(None, &p, false, "1.1.0", "notes").await;
    assert!(o.ok, "{}", o.log);
    let a = d.step_attach(None, &p, false, 77, 900).await;
    assert!(a.ok, "{}", a.log);
    assert_eq!(a.meta["uploaded"], json!(["dist.zip"]));
    assert_eq!(a.meta["skipped"], json!(["docs.zip"]));
    gh.verify().await;
}

#[tokio::test]
async fn phase2_dry_run_and_local_only_send_nothing() {
    let fx = fixture();
    let gh = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/o/r/actions/runs/900/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "artifacts": [ { "id": 1, "name": "dist", "size_in_bytes": 4 } ] })))
        .mount(&gh).await;
    for m in ["POST", "PUT", "PATCH", "DELETE"] {
        Mock::given(method(m)).respond_with(ResponseTemplate::new(500)).expect(0).mount(&gh).await;
    }
    let d = desk(Some(gh.uri()));
    let mut p = project(&fx, false);
    p.options.workflow = Some("release.yml".into());
    let pid = d.store.add_project(&p).unwrap();
    let p = d.store.project(pid).unwrap();
    let before = snapshot(&fx);
    let disp = d.step_dispatch(None, &p, true, "1.1.0", "n").await;
    assert!(disp.ok && disp.log.contains("[dry-run] would POST"), "{}", disp.log);
    let att = d.step_attach(None, &p, true, 0, 900).await;
    assert!(att.ok && att.log.contains("[dry-run] would upload dist.zip"), "{}", att.log);
    assert!(d.actions_control(None, &p, true, 900, "cancel").await.ok);
    assert_eq!(before, snapshot(&fx));

    let mut lp = p.clone();
    lp.local_only = true;
    for o in [
        d.step_open_pr(None, &lp, false, "t", "").await,
        d.step_merge_pr(None, &lp, false, 5, true).await,
        d.step_dispatch(None, &lp, false, "1.1.0", "n").await,
        d.step_attach(None, &lp, false, 77, 900).await,
        d.actions_control(None, &lp, false, 900, "rerun").await,
    ] {
        assert!(!o.ok && o.log.contains("local-only"), "{}", o.log);
    }
    assert!(d.actions_runs(&lp, None).await.is_err());
    gh.verify().await;
}

#[test]
fn rollback_local_release_and_refuse_pushed() {
    let fx = fixture();
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.store.project(pid).unwrap();
    let head = sh(&fx.work, &["rev-parse", "HEAD"]);
    assert!(d.step_version(None, &p, false, "1.1.0").ok);
    assert!(d.step_tag(None, &p, false, "1.1.0", "n").ok);
    assert_ne!(sh(&fx.work, &["rev-parse", "HEAD"]), head);

    let r = d.rollback(None, &p, false, "1.1.0");
    assert!(r.ok, "{}", r.log);
    assert_eq!(sh(&fx.work, &["rev-parse", "HEAD"]), head);
    assert!(!sh(&fx.work, &["tag", "-l"]).contains("v1.1.0"));
    assert!(std::fs::read_to_string(fx.work.join("package.json")).unwrap().contains("\"1.0.0\""));
    assert_eq!(sh(&fx.work, &["status", "--porcelain"]), "?? b.txt", "only the pre-existing untracked file remains");

    // Pushed release: refused, nothing changes.
    let pid2 = d.store.add_project(&Project { path: format!("{}/", fx.work.display()), ..project(&fx, false) }).unwrap();
    let p2 = d.store.project(pid2).unwrap();
    assert!(d.step_version(None, &p2, false, "1.1.0").ok);
    assert!(d.step_tag(None, &p2, false, "1.1.0", "n").ok);
    let pushed = sh(&fx.work, &["rev-parse", "HEAD"]);
    let r2 = d.rollback(None, &p2, false, "1.1.0");
    assert!(!r2.ok && r2.log.contains("already pushed"), "{}", r2.log);
    assert_eq!(sh(&fx.work, &["rev-parse", "HEAD"]), pushed);
}

#[tokio::test]
async fn resume_collects_state() {
    let fx = fixture();
    let gh = github_mock().await;
    let d = desk(Some(gh.uri()));
    let pid = d.store.add_project(&project(&fx, false)).unwrap();
    let p = d.store.project(pid).unwrap();
    let run = d.store.start_run(pid, "1.0.0", "1.0.0", false).unwrap();
    assert!(d.step_version(Some(run), &p, false, "1.1.0").ok);
    assert!(d.step_notes(Some(run), &p, false, "my notes").ok);
    assert!(d.step_tag(Some(run), &p, false, "1.1.0", "my notes").ok);
    assert!(d.step_release(Some(run), &p, false, "1.1.0", "my notes", false).await.ok);
    // Simulated failure on publish (bad id), then resume.
    let f = d.step_publish(Some(run), &p, false, 999).await;
    assert!(!f.ok);
    assert_eq!(d.store.run(run).unwrap().status, "failed");
    let info = d.resume_info(run).unwrap();
    assert_eq!(info.to_version, "1.1.0");
    assert_eq!(info.notes.as_deref(), Some("my notes"));
    assert_eq!(info.release_id, Some(77));
    assert_eq!(info.steps.get("publish").map(String::as_str), Some("failed"));
    assert_eq!(d.store.run(run).unwrap().status, "running");
    assert!(d.step_publish(Some(run), &p, false, 77).await.ok);
    assert_eq!(d.store.run(run).unwrap().status, "done");
}

fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (n, d) in files {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, d).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[tokio::test]
async fn attach_uploads_files_inside_artifacts() {
    let fx = fixture();
    let gh = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/o/r/actions/runs/900/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "artifacts": [ { "id": 1, "name": "windows-installer", "size_in_bytes": 9 } ] })))
        .mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/actions/artifacts/1/zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(make_zip(&[
            ("bundle/msi/App_1.1.0_x64_en-US.msi", b"MSI"), ("bundle/nsis/App_1.1.0_x64-setup.exe", b"EXE"), ("latest.json", b"{}")])))
        .mount(&gh).await;
    Mock::given(method("GET")).and(path("/repos/o/r/releases/77"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 77, "tag_name": "v1.1.0", "html_url": "",
            "draft": true, "prerelease": false, "upload_url": format!("{}/upload/77/assets{{?name,label}}", gh.uri()),
            "assets": [ { "id": 9, "name": "latest.json" } ] })))
        .mount(&gh).await;
    Mock::given(method("POST")).and(path("/upload/77/assets"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 10 }))).expect(2).mount(&gh).await;
    let d = desk(Some(gh.uri()));
    let pid = d.store.add_project(&project(&fx, false)).unwrap();
    let p = d.store.project(pid).unwrap();
    assert_eq!(p.options.attach_mode, "files");
    let a = d.step_attach(None, &p, false, 77, 900).await;
    assert!(a.ok, "{}", a.log);
    assert_eq!(a.meta["uploaded"], json!(["App_1.1.0_x64_en-US.msi", "App_1.1.0_x64-setup.exe"]));
    assert_eq!(a.meta["skipped"], json!(["latest.json"]));
    gh.verify().await;
}

#[test]
fn changelog_in_release_commit_and_repo_config() {
    let fx = fixture();
    // The repo file switches the changelog on and changes the tag format.
    std::fs::write(fx.work.join("release-desk.json"), r#"{"tag_format": "rel-{version}", "options": {"changelog": true}}"#).unwrap();
    sh(&fx.work, &["add", "release-desk.json"]);
    sh(&fx.work, &["commit", "-q", "-m", "chore: add release-desk.json"]);
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.project(pid).unwrap();
    assert_eq!(p.tag_format, "rel-{version}");
    assert!(p.options.changelog);
    assert!(d.step_version(None, &p, false, "1.1.0").ok);
    let t = d.step_tag(None, &p, false, "1.1.0", "### Features\n\n- a\n");
    assert!(t.ok, "{}", t.log);
    let files = sh(&fx.work, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("CHANGELOG.md") && files.contains("package.json"), "{files}");
    assert!(sh(&fx.work, &["tag", "-l"]).contains("rel-1.1.0"));
    let cl = std::fs::read_to_string(fx.work.join("CHANGELOG.md")).unwrap();
    assert!(cl.contains("## [1.1.0] - ") && cl.contains("\n### Features\n"));
    // Rollback removes the commit; the new changelog is left untracked, not deleted.
    let r = d.rollback(None, &p, false, "1.1.0");
    assert!(r.ok, "{}", r.log);
    assert_eq!(sh(&fx.work, &["log", "-1", "--format=%s"]), "chore: add release-desk.json");
    assert!(sh(&fx.work, &["status", "--porcelain"]).contains("?? CHANGELOG.md"));
}

// ================================================================ audit fixes

#[test]
fn secret_files_are_refused_and_hidden() {
    let fx = fixture();
    let keydir = fx.work.join("%USERPROFILE%/.tauri");
    std::fs::create_dir_all(&keydir).unwrap();
    std::fs::write(keydir.join("release-desk.key"), "dW50cnVzdGVkIGNvbW1lbnQ6IHJzaWduIGVuY3J5cHRlZCBzZWNyZXQga2V5").unwrap();
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.store.project(pid).unwrap();
    let key = "%USERPROFILE%/.tauri/release-desk.key".to_string();
    let head = sh(&fx.work, &["rev-parse", "HEAD"]);

    let o = d.step_commit_checked(None, &p, false, &[key.clone(), "b.txt".into()], "chore: x", false);
    assert!(!o.ok && o.log.contains("look like secrets"), "{}", o.log);
    assert_eq!(sh(&fx.work, &["rev-parse", "HEAD"]), head, "nothing committed");
    assert!(sh(&fx.work, &["diff", "--cached", "--name-only"]).is_empty(), "nothing staged");

    let preview = release_desk::git::diff_file(&Ctx::new(false), &fx.work, &key, true).unwrap();
    assert!(preview.contains("contents hidden") && !preview.contains("dW50cnVz"), "{preview}");

    // Explicit override still works (the UI asks first).
    let ok = d.step_commit_checked(None, &p, false, &[key], "chore: commit on purpose", true);
    assert!(ok.ok, "{}", ok.log);
}

#[test]
fn hostile_paths_and_refs_are_refused() {
    let fx = fixture();
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let mut p = d.store.project(pid).unwrap();
    let ctx = Ctx::new(false);
    // Path traversal in a staged file and in a version file.
    assert!(!d.step_commit(None, &p, false, &["../outside.txt".into()], "fix: x").ok);
    p.adapters = vec![AdapterConfig::PlainFile { file: Some("../../evil".into()) }];
    assert!(!d.step_version(None, &p, false, "9.9.9").ok);
    assert!(!fx.work.parent().unwrap().parent().unwrap().join("evil").exists());
    // Option injection through a branch name.
    assert!(release_desk::git::checkout(&ctx, &fx.work, "--orphan=x").is_err());
    assert!(release_desk::git::push(&ctx, &fx.work, "+main").is_err());
    // Force is an error, not a panic.
    assert!(release_desk::git::write(&ctx, &fx.work, &["push", "--force", "origin", "main"]).is_err());
    // A repo slug that would change the API URL.
    let mut r = project(&fx, false);
    r.owner = "o/../../user".into();
    assert!(release_desk::pipeline::guard_remote(&r, "x").is_err());
}

#[test]
fn notes_for_an_existing_tag_cover_previous_tag_to_it() {
    let fx = fixture();
    sh(&fx.work, &["add", "."]);
    sh(&fx.work, &["commit", "-q", "-m", "fix: b"]);
    sh(&fx.work, &["tag", "-a", "v1.1.0", "-m", "v1.1.0"]);
    let d = desk(None);
    let pid = d.store.add_project(&project(&fx, true)).unwrap();
    let p = d.store.project(pid).unwrap();
    let plan = pipeline::plan(&Ctx::new(false), &p, &Bump::Custom { version: "1.1.0".into() }).unwrap();
    assert!(plan.tag_exists);
    assert_eq!(plan.prev_tag.as_deref(), Some("v1.0.0"));
    assert_eq!(plan.commits.len(), 2, "feat(core) and fix since v1.0.0");
    assert!(plan.notes.contains("### Features") && plan.notes.contains("### Fixes"));
}
