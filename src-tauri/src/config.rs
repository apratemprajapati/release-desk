//! Optional per-repo `release-desk.json`. Values in the repo override the app's
//! project settings, so a team can share one release setup. A repo file can
//! switch local-only ON but never OFF: it cannot widen what the app may do.
use crate::error::{Error, Result};
use crate::store::Project;
use crate::version::AdapterConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const FILE: &str = "release-desk.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepoConfig {
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub tag_format: Option<String>,
    pub default_branch: Option<String>,
    pub release_branch: Option<String>,
    pub local_only: Option<bool>,
    pub version_files: Option<Vec<AdapterConfig>>,
    pub pipeline: Option<Vec<String>>,
    /// Partial `ProjectOptions`: only the keys present are applied.
    pub options: Option<serde_json::Map<String, Value>>,
}

pub fn load(repo: &Path) -> Result<Option<RepoConfig>> {
    let path = repo.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
    serde_json::from_str(&text).map(Some).map_err(|e| Error::Other(format!("{FILE}: {e}")))
}

/// Apply the repo file on top of the stored project. Returns the keys it set.
pub fn apply(p: &mut Project, c: &RepoConfig) -> Result<Vec<String>> {
    let mut set = vec![];
    if let Some(v) = &c.tag_format {
        if !v.contains("{version}") {
            return Err(Error::Other(format!("{FILE}: tag_format must contain {{version}}")));
        }
        p.tag_format = v.clone();
        set.push("tag_format".into());
    }
    if let Some(v) = &c.default_branch { p.default_branch = v.clone(); set.push("default_branch".into()); }
    if let Some(v) = &c.release_branch { p.release_branch = v.clone(); set.push("release_branch".into()); }
    if c.local_only == Some(true) { p.local_only = true; set.push("local_only".into()); }
    if let Some(v) = &c.version_files { p.adapters = v.clone(); set.push("version_files".into()); }
    if let Some(v) = &c.pipeline { p.pipeline = v.clone(); set.push("pipeline".into()); }
    if let Some(o) = &c.options {
        let mut cur = serde_json::to_value(&p.options)?;
        for (k, v) in o {
            if cur.get(k).is_none() {
                return Err(Error::Other(format!("{FILE}: unknown option '{k}'")));
            }
            cur[k] = v.clone();
            set.push(format!("options.{k}"));
        }
        p.options = serde_json::from_value(cur).map_err(|e| Error::Other(format!("{FILE}: {e}")))?;
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> Project {
        Project {
            id: 1, name: "x".into(), path: "".into(), owner: "".into(), repo: "".into(), account_id: None,
            default_branch: "main".into(), release_branch: "main".into(), tag_format: "v{version}".into(),
            local_only: false, adapters: vec![AdapterConfig::GitTag], pipeline: vec![], options: Default::default(),
        }
    }

    #[test]
    fn this_repos_release_desk_json_is_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let c = load(root).unwrap().expect("release-desk.json at repo root");
        let mut p = proj();
        apply(&mut p, &c).unwrap();
        assert_eq!(p.adapters.len(), 3);
        assert_eq!(p.options.workflow.as_deref(), Some("release.yml"));
    }

    #[test]
    fn applies_and_validates() {
        let c: RepoConfig = serde_json::from_str(r#"{
            "$schema": "./release-desk.schema.json", "tag_format": "app-v{version}",
            "version_files": [{"kind":"cargo","file":null}], "options": {"changelog": true, "merge_mode": "pr"}
        }"#).unwrap();
        let mut p = proj();
        let set = apply(&mut p, &c).unwrap();
        assert_eq!(p.tag_format, "app-v{version}");
        assert!(p.options.changelog && p.options.merge_mode == "pr");
        assert_eq!(p.options.merge_method, "squash", "untouched keys keep their value");
        assert!(set.contains(&"options.changelog".to_string()));

        let mut lp = proj();
        lp.local_only = true;
        apply(&mut lp, &serde_json::from_str(r#"{"local_only": false}"#).unwrap()).unwrap();
        assert!(lp.local_only, "repo file cannot turn local-only off");

        assert!(serde_json::from_str::<RepoConfig>(r#"{"bogus": 1}"#).is_err());
        assert!(apply(&mut proj(), &serde_json::from_str(r#"{"options": {"nope": 1}}"#).unwrap()).is_err());
        assert!(apply(&mut proj(), &serde_json::from_str(r#"{"tag_format": "v"}"#).unwrap()).is_err());
    }
}
