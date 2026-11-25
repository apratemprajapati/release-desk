//! Version adapters and semver logic.
use crate::error::{Error, Result};
use crate::exec::Ctx;
use crate::notes::Commit;
use regex::Regex;
use semver::{Prerelease, Version};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Adapter configuration stored per project (`adapters_json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AdapterConfig {
    /// `package.json` (and `package-lock.json` top-level version if present).
    Npm { file: Option<String> },
    /// A file whose whole content is the version, default `VERSION`.
    PlainFile { file: Option<String> },
    /// Any file plus a regex with one capture group around the version.
    Regex { file: String, pattern: String },
    /// No file: the version is the last tag.
    GitTag,
}

impl AdapterConfig {
    pub fn id(&self) -> &'static str {
        match self {
            AdapterConfig::Npm { .. } => "npm",
            AdapterConfig::PlainFile { .. } => "plain-file",
            AdapterConfig::Regex { .. } => "regex",
            AdapterConfig::GitTag => "git-tag",
        }
    }
}

/// A planned file change. Nothing is written until `apply_edits`.
#[derive(Debug, Clone, Serialize)]
pub struct FileEdit {
    pub path: String,
    pub content: String,
}

pub fn detect(repo: &Path) -> Vec<AdapterConfig> {
    let mut v = vec![];
    if repo.join("package.json").exists() {
        v.push(AdapterConfig::Npm { file: None });
    }
    if repo.join("VERSION").exists() {
        v.push(AdapterConfig::PlainFile { file: None });
    }
    if v.is_empty() {
        v.push(AdapterConfig::GitTag);
    }
    v
}

fn parse_ver(s: &str) -> Result<Version> {
    let t = s.trim().trim_start_matches('v');
    Version::parse(t).map_err(|e| Error::Version(format!("'{t}' is not semver: {e}")))
}

fn rel(repo: &Path, f: &str) -> PathBuf {
    repo.join(f)
}

fn read_file(repo: &Path, f: &str) -> Result<String> {
    std::fs::read_to_string(rel(repo, f)).map_err(|e| Error::Version(format!("read {f}: {e}")))
}

/// `"version": "x"` as the first top-level occurrence; edits preserve formatting.
fn npm_re() -> Regex {
    Regex::new(r#"(?m)^(\s{0,4}"version"\s*:\s*")([^"]+)(")"#).unwrap()
}

pub struct ReadCtx<'a> {
    pub repo: &'a Path,
    pub ctx: &'a Ctx,
    pub tag_format: &'a str,
}

pub fn read(a: &AdapterConfig, rc: &ReadCtx) -> Result<Version> {
    match a {
        AdapterConfig::Npm { file } => {
            let f = file.as_deref().unwrap_or("package.json");
            let j: serde_json::Value = serde_json::from_str(&read_file(rc.repo, f)?)?;
            parse_ver(j["version"].as_str().ok_or_else(|| Error::Version(format!("{f} has no version")))?)
        }
        AdapterConfig::PlainFile { file } => parse_ver(&read_file(rc.repo, file.as_deref().unwrap_or("VERSION"))?),
        AdapterConfig::Regex { file, pattern } => {
            let re = Regex::new(pattern).map_err(|e| Error::Version(e.to_string()))?;
            let text = read_file(rc.repo, file)?;
            let c = re.captures(&text).ok_or_else(|| Error::Version(format!("pattern not found in {file}")))?;
            parse_ver(c.get(1).ok_or_else(|| Error::Version("pattern needs one capture group".into()))?.as_str())
        }
        AdapterConfig::GitTag => {
            let glob = tag_glob(rc.tag_format);
            match crate::git::last_tag(rc.ctx, rc.repo, &glob) {
                Some(t) => parse_ver(&version_from_tag(rc.tag_format, &t).unwrap_or(t)),
                None => Ok(Version::new(0, 0, 0)),
            }
        }
    }
}

pub fn plan_write(a: &AdapterConfig, repo: &Path, next: &Version) -> Result<Vec<FileEdit>> {
    let nv = next.to_string();
    match a {
        AdapterConfig::Npm { file } => {
            let f = file.as_deref().unwrap_or("package.json");
            let mut edits = vec![FileEdit { path: f.into(), content: replace_first(&npm_re(), &read_file(repo, f)?, &nv, f)? }];
            // Keep package-lock in sync: top-level "version" and packages[""].version.
            if file.is_none() && repo.join("package-lock.json").exists() {
                let lock = read_file(repo, "package-lock.json")?;
                let re = Regex::new(r#"(?m)^(\s{2,6}"version"\s*:\s*")([^"]+)(")"#).unwrap();
                let mut n = 0;
                let out = re.replace_all(&lock, |c: &regex::Captures| {
                    n += 1;
                    if n <= 2 { format!("{}{}{}", &c[1], nv, &c[3]) } else { c[0].to_string() }
                });
                edits.push(FileEdit { path: "package-lock.json".into(), content: out.into_owned() });
            }
            Ok(edits)
        }
        AdapterConfig::PlainFile { file } => {
            let f = file.as_deref().unwrap_or("VERSION");
            let old = read_file(repo, f).unwrap_or_default();
            let nl = if old.ends_with('\n') || old.is_empty() { "\n" } else { "" };
            Ok(vec![FileEdit { path: f.into(), content: format!("{nv}{nl}") }])
        }
        AdapterConfig::Regex { file, pattern } => {
            let re = Regex::new(pattern).map_err(|e| Error::Version(e.to_string()))?;
            let text = read_file(repo, file)?;
            let c = re.captures(&text).ok_or_else(|| Error::Version(format!("pattern not found in {file}")))?;
            let g = c.get(1).ok_or_else(|| Error::Version("pattern needs one capture group".into()))?;
            let old = g.as_str();
            let new_v = if old.starts_with('v') { format!("v{nv}") } else { nv.clone() };
            let content = format!("{}{}{}", &text[..g.start()], new_v, &text[g.end()..]);
            Ok(vec![FileEdit { path: file.clone(), content }])
        }
        AdapterConfig::GitTag => Ok(vec![]),
    }
}

fn replace_first(re: &Regex, text: &str, nv: &str, f: &str) -> Result<String> {
    if !re.is_match(text) {
        return Err(Error::Version(format!("no version field in {f}")));
    }
    Ok(re.replacen(text, 1, |c: &regex::Captures| format!("{}{}{}", &c[1], nv, &c[3])).into_owned())
}

/// Write planned edits (skipped in dry-run). Returns changed paths.
pub fn apply_edits(ctx: &Ctx, repo: &Path, edits: &[FileEdit]) -> Result<Vec<String>> {
    for e in edits {
        if ctx.side_effect(&format!("write {}", e.path)) {
            std::fs::write(rel(repo, &e.path), &e.content)?;
        }
    }
    Ok(edits.iter().map(|e| e.path.clone()).collect())
}

// ---------------------------------------------------------------- semver

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    None,
    Patch,
    Minor,
    Major,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Bump {
    Auto,
    Patch,
    Minor,
    Major,
    Prerelease { id: String },
    Custom { version: String },
}

/// feat -> minor, fix/perf -> patch, `!` or BREAKING CHANGE -> major.
pub fn infer_level(commits: &[Commit]) -> Level {
    commits.iter().map(|c| {
        if c.breaking {
            Level::Major
        } else {
            match c.kind.as_deref() {
                Some("feat") => Level::Minor,
                Some("fix") | Some("perf") => Level::Patch,
                _ => Level::None,
            }
        }
    }).max().unwrap_or(Level::None)
}

pub fn apply_level(cur: &Version, level: Level) -> Version {
    let mut v = cur.clone();
    let was_pre = !v.pre.is_empty();
    v.pre = Prerelease::EMPTY;
    v.build = semver::BuildMetadata::EMPTY;
    // 1.2.0-rc.1 + minor -> 1.2.0 (finish the prerelease), like semver tools do.
    if was_pre && level != Level::None {
        let finishes = match level {
            Level::Major => cur.minor == 0 && cur.patch == 0,
            Level::Minor => cur.patch == 0,
            _ => true,
        };
        if finishes {
            return v;
        }
    }
    // Pre-1.0: breaking changes bump minor, as release-please does by default.
    let level = if cur.major == 0 && level == Level::Major { Level::Minor } else { level };
    match level {
        Level::Major => { v.major += 1; v.minor = 0; v.patch = 0; }
        Level::Minor => { v.minor += 1; v.patch = 0; }
        Level::Patch | Level::None => { v.patch += 1; }
    }
    v
}

pub fn next_version(cur: &Version, bump: &Bump, commits: &[Commit]) -> Result<Version> {
    Ok(match bump {
        Bump::Auto => apply_level(cur, infer_level(commits)),
        Bump::Patch => apply_level(cur, Level::Patch),
        Bump::Minor => apply_level(cur, Level::Minor),
        Bump::Major => {
            let mut v = cur.clone();
            v.pre = Prerelease::EMPTY;
            v.major += 1; v.minor = 0; v.patch = 0;
            v
        }
        Bump::Prerelease { id } => {
            let id = if id.trim().is_empty() { "rc" } else { id.trim() };
            let pre = cur.pre.as_str();
            if let Some(n) = pre.strip_prefix(&format!("{id}.")).and_then(|n| n.parse::<u64>().ok()) {
                let mut v = cur.clone();
                v.pre = Prerelease::new(&format!("{id}.{}", n + 1)).unwrap();
                v
            } else {
                let mut v = if pre.is_empty() { apply_level(cur, infer_level(commits)) } else { cur.clone() };
                v.pre = Prerelease::new(&format!("{id}.1")).map_err(|e| Error::Version(e.to_string()))?;
                v
            }
        }
        Bump::Custom { version } => parse_ver(version)?,
    })
}

// ---------------------------------------------------------------- tags

/// Render a tag from a format such as `v{version}` or `release-{version}`.
pub fn render_tag(format: &str, v: &Version) -> String {
    let f = if format.contains("{version}") { format } else { "v{version}" };
    f.replace("{version}", &v.to_string())
}

pub fn tag_glob(format: &str) -> String {
    let f = if format.contains("{version}") { format } else { "v{version}" };
    f.replace("{version}", "*")
}

pub fn version_from_tag(format: &str, tag: &str) -> Option<String> {
    let f = if format.contains("{version}") { format } else { "v{version}" };
    let (pre, post) = f.split_once("{version}")?;
    tag.strip_prefix(pre)?.strip_suffix(post).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::parse_commit;

    fn v(s: &str) -> Version { Version::parse(s).unwrap() }
    fn cs(subjects: &[&str]) -> Vec<Commit> {
        subjects.iter().map(|s| parse_commit("x", "a", s, "")).collect()
    }

    #[test]
    fn infers_levels() {
        assert_eq!(infer_level(&cs(&["fix: a", "chore: b"])), Level::Patch);
        assert_eq!(infer_level(&cs(&["fix: a", "feat(ui): b"])), Level::Minor);
        assert_eq!(infer_level(&cs(&["feat!: drop node 16"])), Level::Major);
        assert_eq!(infer_level(&[parse_commit("x", "a", "refactor: y", "BREAKING CHANGE: z")]), Level::Major);
        assert_eq!(infer_level(&cs(&["docs: a"])), Level::None);
    }

    #[test]
    fn bumps() {
        let c = cs(&["feat: x"]);
        assert_eq!(next_version(&v("1.2.3"), &Bump::Auto, &c).unwrap(), v("1.3.0"));
        assert_eq!(next_version(&v("1.2.3"), &Bump::Patch, &[]).unwrap(), v("1.2.4"));
        assert_eq!(next_version(&v("1.2.3"), &Bump::Major, &[]).unwrap(), v("2.0.0"));
        assert_eq!(next_version(&v("0.4.1"), &Bump::Auto, &cs(&["feat!: x"])).unwrap(), v("0.5.0"));
        assert_eq!(next_version(&v("1.2.3"), &Bump::Prerelease { id: "beta".into() }, &c).unwrap(), v("1.3.0-beta.1"));
        assert_eq!(next_version(&v("1.3.0-beta.1"), &Bump::Prerelease { id: "beta".into() }, &c).unwrap(), v("1.3.0-beta.2"));
        assert_eq!(next_version(&v("1.3.0-beta.2"), &Bump::Auto, &c).unwrap(), v("1.3.0"));
        assert_eq!(next_version(&v("1.0.0"), &Bump::Custom { version: "v4.5.6".into() }, &[]).unwrap(), v("4.5.6"));
        assert_eq!(next_version(&v("1.0.0"), &Bump::Auto, &cs(&["docs: x"])).unwrap(), v("1.0.1"));
    }

    #[test]
    fn tag_formats() {
        assert_eq!(render_tag("v{version}", &v("1.2.3")), "v1.2.3");
        assert_eq!(render_tag("app-{version}-final", &v("1.2.3")), "app-1.2.3-final");
        assert_eq!(render_tag("", &v("1.2.3")), "v1.2.3");
        assert_eq!(tag_glob("app-{version}"), "app-*");
        assert_eq!(version_from_tag("app-{version}", "app-2.0.0").as_deref(), Some("2.0.0"));
        assert_eq!(version_from_tag("v{version}", "x2.0.0"), None);
    }

    fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, c) in files {
            std::fs::write(d.path().join(p), c).unwrap();
        }
        d
    }

    fn roundtrip(a: &AdapterConfig, d: &tempfile::TempDir, from: &str, to: &str) -> Vec<FileEdit> {
        let ctx = Ctx::new(false);
        let rc = ReadCtx { repo: d.path(), ctx: &ctx, tag_format: "v{version}" };
        assert_eq!(read(a, &rc).unwrap(), v(from));
        let edits = plan_write(a, d.path(), &v(to)).unwrap();
        apply_edits(&ctx, d.path(), &edits).unwrap();
        assert_eq!(read(a, &rc).unwrap(), v(to));
        edits
    }

    #[test]
    fn npm_adapter_preserves_formatting_and_lock() {
        let pkg = "{\n  \"name\": \"x\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"y\": { \"version\": \"9.9.9\" }\n  }\n}\n";
        let lock = "{\n  \"name\": \"x\",\n  \"version\": \"1.0.0\",\n  \"packages\": {\n    \"\": {\n      \"name\": \"x\",\n      \"version\": \"1.0.0\"\n    },\n    \"node_modules/y\": {\n      \"version\": \"9.9.9\"\n    }\n  }\n}\n";
        let d = fixture(&[("package.json", pkg), ("package-lock.json", lock)]);
        let e = roundtrip(&AdapterConfig::Npm { file: None }, &d, "1.0.0", "1.1.0");
        assert_eq!(e.len(), 2);
        let p = std::fs::read_to_string(d.path().join("package.json")).unwrap();
        assert_eq!(p, pkg.replace("\"1.0.0\"", "\"1.1.0\""));
        let l = std::fs::read_to_string(d.path().join("package-lock.json")).unwrap();
        assert_eq!(l.matches("1.1.0").count(), 2);
        assert!(l.contains("9.9.9"));
    }

    #[test]
    fn plain_file_adapter() {
        let d = fixture(&[("VERSION", "0.3.0\n")]);
        roundtrip(&AdapterConfig::PlainFile { file: None }, &d, "0.3.0", "0.4.0");
        assert_eq!(std::fs::read_to_string(d.path().join("VERSION")).unwrap(), "0.4.0\n");
    }

    #[test]
    fn regex_adapter() {
        let d = fixture(&[("app.py", "NAME = 'x'\n__version__ = \"2.1.0\"\n")]);
        let a = AdapterConfig::Regex { file: "app.py".into(), pattern: r#"__version__\s*=\s*"([^"]+)""#.into() };
        roundtrip(&a, &d, "2.1.0", "2.2.0");
        assert!(std::fs::read_to_string(d.path().join("app.py")).unwrap().contains("__version__ = \"2.2.0\""));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let d = fixture(&[("VERSION", "1.0.0\n")]);
        let ctx = Ctx::new(true);
        let edits = plan_write(&AdapterConfig::PlainFile { file: None }, d.path(), &v("2.0.0")).unwrap();
        apply_edits(&ctx, d.path(), &edits).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("VERSION")).unwrap(), "1.0.0\n");
        assert!(ctx.performed().is_empty());
    }

    #[test]
    fn detects() {
        let d = fixture(&[("package.json", "{\"version\":\"1.0.0\"}")]);
        assert_eq!(detect(d.path()), vec![AdapterConfig::Npm { file: None }]);
        let e = fixture(&[]);
        assert_eq!(detect(e.path()), vec![AdapterConfig::GitTag]);
    }
}
