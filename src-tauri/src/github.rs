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
