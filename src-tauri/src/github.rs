//! Minimal GitHub REST client. GET always runs; POST/PUT/PATCH/DELETE go
//! through `Ctx::side_effect` so dry-run sends no mutating request.
use crate::error::{Error, Result};
use crate::exec::Ctx;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const API: &str = "https://api.github.com";

pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub html_url: String,
    pub draft: bool,
    pub prerelease: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub upload_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Asset {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Pr {
    pub number: u64,
    pub html_url: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub mergeable_state: Option<String>,
    #[serde(default)]
    pub head: PrRef,
    #[serde(default)]
    pub base: PrRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrRef {
    #[serde(rename = "ref", default)]
    pub git_ref: String,
    #[serde(default)]
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Checks {
    pub total: usize,
    pub pending: usize,
    pub passed: usize,
    pub failed: usize,
    pub runs: Vec<CheckRun>,
}

impl Checks {
    pub fn from_runs(runs: Vec<CheckRun>) -> Self {
        let mut c = Checks { total: runs.len(), ..Default::default() };
        for r in &runs {
            match (r.status.as_str(), r.conclusion.as_deref()) {
                ("completed", Some("success" | "neutral" | "skipped")) => c.passed += 1,
                ("completed", _) => c.failed += 1,
                _ => c.pending += 1,
            }
        }
        c.runs = runs;
        c
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Workflow {
    pub id: u64,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowRun {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub run_number: u64,
    #[serde(default)]
    pub event: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobStep {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub number: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub steps: Vec<JobStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artifact {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub size_in_bytes: u64,
    #[serde(default)]
    pub expired: bool,
}

impl Client {
    pub fn new(base: Option<&str>, token: &str) -> Result<Self> {
        crate::redact::register_secret(token);
        Ok(Client {
            base: base.unwrap_or(API).trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::Client::builder()
                .user_agent(concat!("release-desk/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Result<(StatusCode, Value)> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok((status, v))
    }

    pub async fn get(&self, ctx: &Ctx, path: &str) -> Result<(StatusCode, Value)> {
        ctx.log(&format!("GET {path}"));
        self.send(Method::GET, path, None).await
    }

    /// Mutating call. Returns `None` in dry-run.
    pub async fn mutate(&self, ctx: &Ctx, method: Method, path: &str, body: Value) -> Result<Option<Value>> {
        if !ctx.side_effect(&format!("{method} {path} {}", body)) {
            return Ok(None);
        }
        let (s, v) = self.send(method.clone(), path, Some(body)).await?;
        if !s.is_success() {
            return Err(Error::GitHub(format!("{method} {path} -> {s}: {}", v["message"].as_str().unwrap_or(""))));
        }
        ctx.log(&format!("-> {s}"));
        Ok(Some(v))
    }

    pub async fn whoami(&self, ctx: &Ctx) -> Result<String> {
        let (s, v) = self.get(ctx, "/user").await?;
        if !s.is_success() {
            return Err(Error::GitHub(format!("token rejected ({s})")));
        }
        Ok(v["login"].as_str().unwrap_or_default().to_string())
    }

    pub async fn release_by_tag(&self, ctx: &Ctx, owner: &str, repo: &str, tag: &str) -> Result<Option<Release>> {
        // Draft releases are not returned by /releases/tags/{tag}, so scan the list too.
        let (s, v) = self.get(ctx, &format!("/repos/{owner}/{repo}/releases/tags/{tag}")).await?;
        if s.is_success() {
            return Ok(Some(serde_json::from_value(v)?));
        }
        let (s, v) = self.get(ctx, &format!("/repos/{owner}/{repo}/releases?per_page=30")).await?;
        if !s.is_success() {
            return Err(Error::GitHub(format!("list releases -> {s}: {}", v["message"].as_str().unwrap_or(""))));
        }
        let list: Vec<Release> = serde_json::from_value(v)?;
        Ok(list.into_iter().find(|r| r.tag_name == tag))
    }

    pub async fn latest_release(&self, ctx: &Ctx, owner: &str, repo: &str) -> Result<Option<Release>> {
        let (s, v) = self.get(ctx, &format!("/repos/{owner}/{repo}/releases/latest")).await?;
        if s == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !s.is_success() {
            return Err(Error::GitHub(format!("latest release -> {s}")));
        }
        Ok(Some(serde_json::from_value(v)?))
    }

    pub async fn create_draft(
        &self, ctx: &Ctx, owner: &str, repo: &str, tag: &str, name: &str, body: &str, prerelease: bool,
    ) -> Result<Release> {
        let r = self
            .mutate(
                ctx,
                Method::POST,
                &format!("/repos/{owner}/{repo}/releases"),
                json!({ "tag_name": tag, "name": name, "body": body, "draft": true, "prerelease": prerelease }),
            )
            .await?;
        match r {
            Some(v) => Ok(serde_json::from_value(v)?),
            None => Ok(Release { tag_name: tag.into(), draft: true, prerelease, ..Default::default() }),
        }
    }

    pub async fn update_body(&self, ctx: &Ctx, owner: &str, repo: &str, id: u64, body: &str) -> Result<()> {
        self.mutate(ctx, Method::PATCH, &format!("/repos/{owner}/{repo}/releases/{id}"), json!({ "body": body }))
            .await?;
        Ok(())
    }

    fn check(s: StatusCode, v: &Value, what: &str) -> Result<()> {
        if s.is_success() {
            Ok(())
        } else {
            Err(Error::GitHub(format!("{what} -> {s}: {}", v["message"].as_str().unwrap_or(""))))
        }
    }

    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, ctx: &Ctx, path: &str, what: &str) -> Result<T> {
        let (s, v) = self.get(ctx, path).await?;
        Self::check(s, &v, what)?;
        Ok(serde_json::from_value(v)?)
    }

    // ------------------------------------------------------------ pull requests

    pub async fn find_open_pr(&self, ctx: &Ctx, o: &str, r: &str, head: &str, base: &str) -> Result<Option<Pr>> {
        let list: Vec<Pr> = self
            .get_json(ctx, &format!("/repos/{o}/{r}/pulls?state=open&head={o}:{head}&base={base}"), "list PRs")
            .await?;
        Ok(list.into_iter().next())
    }

    pub async fn get_pr(&self, ctx: &Ctx, o: &str, r: &str, n: u64) -> Result<Pr> {
        self.get_json(ctx, &format!("/repos/{o}/{r}/pulls/{n}"), "get PR").await
    }

    pub async fn create_pr(&self, ctx: &Ctx, o: &str, r: &str, head: &str, base: &str, title: &str, body: &str) -> Result<Option<Pr>> {
        let v = self
            .mutate(ctx, Method::POST, &format!("/repos/{o}/{r}/pulls"),
                json!({ "head": head, "base": base, "title": title, "body": body }))
            .await?;
        Ok(match v { Some(v) => Some(serde_json::from_value(v)?), None => None })
    }

    pub async fn checks(&self, ctx: &Ctx, o: &str, r: &str, sha: &str) -> Result<Checks> {
        let v: Value = self.get_json(ctx, &format!("/repos/{o}/{r}/commits/{sha}/check-runs?per_page=100"), "check runs").await?;
        let runs: Vec<CheckRun> = serde_json::from_value(v["check_runs"].clone()).unwrap_or_default();
        Ok(Checks::from_runs(runs))
    }

    /// Merge with `sha` pinned so a push after the checks passed cannot slip in.
    pub async fn merge_pr(&self, ctx: &Ctx, o: &str, r: &str, n: u64, method: &str, sha: &str) -> Result<()> {
        self.mutate(ctx, Method::PUT, &format!("/repos/{o}/{r}/pulls/{n}/merge"),
            json!({ "merge_method": method, "sha": sha }))
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------ actions

    pub async fn workflows(&self, ctx: &Ctx, o: &str, r: &str) -> Result<Vec<Workflow>> {
        let v: Value = self.get_json(ctx, &format!("/repos/{o}/{r}/actions/workflows?per_page=100"), "workflows").await?;
        Ok(serde_json::from_value(v["workflows"].clone()).unwrap_or_default())
    }

    pub async fn dispatch(&self, ctx: &Ctx, o: &str, r: &str, workflow: &str, git_ref: &str, inputs: &Value) -> Result<()> {
        self.mutate(ctx, Method::POST, &format!("/repos/{o}/{r}/actions/workflows/{workflow}/dispatches"),
            json!({ "ref": git_ref, "inputs": inputs }))
            .await?;
        Ok(())
    }

    pub async fn runs_for_sha(&self, ctx: &Ctx, o: &str, r: &str, sha: &str) -> Result<Vec<WorkflowRun>> {
        let v: Value = self.get_json(ctx, &format!("/repos/{o}/{r}/actions/runs?head_sha={sha}&per_page=30"), "runs").await?;
        Ok(serde_json::from_value(v["workflow_runs"].clone()).unwrap_or_default())
    }

    pub async fn jobs(&self, ctx: &Ctx, o: &str, r: &str, run: u64) -> Result<Vec<Job>> {
        let v: Value = self.get_json(ctx, &format!("/repos/{o}/{r}/actions/runs/{run}/jobs?per_page=100"), "jobs").await?;
        Ok(serde_json::from_value(v["jobs"].clone()).unwrap_or_default())
    }

    pub async fn cancel_run(&self, ctx: &Ctx, o: &str, r: &str, run: u64) -> Result<()> {
        self.mutate(ctx, Method::POST, &format!("/repos/{o}/{r}/actions/runs/{run}/cancel"), json!({})).await?;
        Ok(())
    }

    pub async fn rerun_failed(&self, ctx: &Ctx, o: &str, r: &str, run: u64) -> Result<()> {
        self.mutate(ctx, Method::POST, &format!("/repos/{o}/{r}/actions/runs/{run}/rerun-failed-jobs"), json!({})).await?;
        Ok(())
    }

    pub async fn run_artifacts(&self, ctx: &Ctx, o: &str, r: &str, run: u64) -> Result<Vec<Artifact>> {
        let v: Value = self.get_json(ctx, &format!("/repos/{o}/{r}/actions/runs/{run}/artifacts?per_page=100"), "artifacts").await?;
        Ok(serde_json::from_value(v["artifacts"].clone()).unwrap_or_default())
    }

    /// Artifact zip. GitHub redirects to blob storage; reqwest drops the
    /// Authorization header on the cross-host redirect.
    pub async fn download_artifact(&self, ctx: &Ctx, o: &str, r: &str, id: u64) -> Result<Vec<u8>> {
        let path = format!("/repos/{o}/{r}/actions/artifacts/{id}/zip");
        ctx.log(&format!("GET {path}"));
        let resp = self.http.get(format!("{}{}", self.base, path)).bearer_auth(&self.token)
            .header("X-GitHub-Api-Version", "2022-11-28").send().await?;
        if !resp.status().is_success() {
            return Err(Error::GitHub(format!("download artifact {id} -> {}", resp.status())));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    pub async fn upload_asset(&self, ctx: &Ctx, release: &Release, name: &str, bytes: Vec<u8>) -> Result<()> {
        let base = release.upload_url.split('{').next().unwrap_or("").to_string();
        if !ctx.side_effect(&format!("POST {base}?name={name} ({} bytes)", bytes.len())) {
            return Ok(());
        }
        if base.is_empty() {
            return Err(Error::GitHub("release has no upload_url".into()));
        }
        let resp = self.http.post(&base).query(&[("name", name)]).bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json").header("Content-Type", "application/zip")
            .body(bytes).send().await?;
        if !resp.status().is_success() {
            return Err(Error::GitHub(format!("upload {name} -> {}", resp.status())));
        }
        ctx.log(&format!("-> {}", resp.status()));
        Ok(())
    }

    /// Self-hosted runners registered on the repo (needs Administration: read).
    pub async fn runners(&self, ctx: &Ctx, o: &str, r: &str) -> Result<Value> {
        let (s, v) = self.get(ctx, &format!("/repos/{o}/{r}/actions/runners?per_page=100")).await?;
        if s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND {
            return Err(Error::GitHub("the token cannot list runners: it needs Administration read access on this repo".into()));
        }
        Self::check(s, &v, "runners")?;
        Ok(v["runners"].clone())
    }

    pub async fn publish(&self, ctx: &Ctx, owner: &str, repo: &str, id: u64) -> Result<Release> {
        let r = self
            .mutate(ctx, Method::PATCH, &format!("/repos/{owner}/{repo}/releases/{id}"), json!({ "draft": false }))
            .await?;
        match r {
            Some(v) => Ok(serde_json::from_value(v)?),
            None => Ok(Release { id, draft: false, ..Default::default() }),
        }
    }
}

// ---------------------------------------------------------------- device flow

pub const WEB: &str = "https://github.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DevicePoll {
    Pending { interval: u64 },
    Done { token: String },
}

fn web_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("release-desk/", env!("CARGO_PKG_VERSION"))).build()?)
}

/// Step 1 of the OAuth device flow. `scope` "repo workflow" covers releases and dispatch.
pub async fn device_start(web: Option<&str>, client_id: &str, scope: &str) -> Result<DeviceCode> {
    let v: Value = web_client()?
        .post(format!("{}/login/device/code", web.unwrap_or(WEB)))
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", scope)])
        .send().await?.json().await?;
    if let Some(e) = v["error"].as_str() {
        return Err(Error::GitHub(format!("device flow: {e} {}", v["error_description"].as_str().unwrap_or(""))));
    }
    Ok(serde_json::from_value(v)?)
}

/// Step 2: poll until the user approves. Errors on denial or expiry.
pub async fn device_poll(web: Option<&str>, client_id: &str, device_code: &str, interval: u64) -> Result<DevicePoll> {
    let v: Value = web_client()?
        .post(format!("{}/login/oauth/access_token", web.unwrap_or(WEB)))
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")])
        .send().await?.json().await?;
    if let Some(t) = v["access_token"].as_str() {
        crate::redact::register_secret(t);
        return Ok(DevicePoll::Done { token: t.to_string() });
    }
    match v["error"].as_str() {
        Some("authorization_pending") => Ok(DevicePoll::Pending { interval }),
        Some("slow_down") => Ok(DevicePoll::Pending { interval: v["interval"].as_u64().unwrap_or(interval + 5) }),
        Some(e) => Err(Error::GitHub(format!("sign-in failed: {e}"))),
        None => Err(Error::GitHub("unexpected device flow response".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_checks() {
        let r = |s: &str, c: Option<&str>| CheckRun { name: "x".into(), status: s.into(), conclusion: c.map(String::from), html_url: None };
        let c = Checks::from_runs(vec![r("completed", Some("success")), r("completed", Some("failure")),
            r("in_progress", None), r("completed", Some("skipped"))]);
        assert_eq!((c.total, c.passed, c.failed, c.pending), (4, 2, 1, 1));
    }
}
