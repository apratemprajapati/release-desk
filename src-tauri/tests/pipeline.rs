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
    }
}

fn desk(api: Option<String>) -> Desk {
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set("acc", TOKEN).unwrap();
    let store = Store::memory().unwrap();
    store.add_account("personal", "tester", "acc").unwrap();
    Desk { store, secrets, api_base: api }
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

    let r = d.step_release(Some(run), &p, false, &plan.next, &plan.notes, false).await;
    assert!(r.ok, "{}", r.log);
    assert_eq!(r.meta["release_id"], 77);
    let pb = d.step_publish(Some(run), &p, false, 77).await;
    assert!(pb.ok, "{}", pb.log);

    // Idempotent re-run of tag step: no duplicate commit or tag error.
    let again = d.step_tag(Some(run), &p, false, &plan.next, &plan.notes);
    assert!(again.ok && again.meta["committed"] == false, "{}", again.log);

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
        "would git tag -a v1.1.0", "would POST /repos/o/r/releases", "would PATCH /repos/o/r/releases/1"] {
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
