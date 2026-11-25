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
    /// `Cargo.toml` `[package]` / `[workspace.package]` version, plus `Cargo.lock`.
    Cargo { file: Option<String> },
    /// `pyproject.toml` `[project]` or `[tool.poetry]` version.
    Python { file: Option<String> },
    /// `tauri.conf.json` top-level `version` (Tauri 2) or `package.version` (Tauri 1).
    Tauri { file: Option<String> },
    /// Android `build.gradle(.kts)`: `versionName`, and `versionCode` + 1 on every bump.
    Android { file: Option<String> },
}

impl AdapterConfig {
    pub fn id(&self) -> &'static str {
        match self {
            AdapterConfig::Npm { .. } => "npm",
            AdapterConfig::PlainFile { .. } => "plain-file",
            AdapterConfig::Regex { .. } => "regex",
            AdapterConfig::GitTag => "git-tag",
            AdapterConfig::Cargo { .. } => "cargo",
            AdapterConfig::Python { .. } => "python",
            AdapterConfig::Tauri { .. } => "tauri",
            AdapterConfig::Android { .. } => "android",
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
    for f in ["Cargo.toml", "src-tauri/Cargo.toml"] {
        if repo.join(f).exists() && cargo_span(&std::fs::read_to_string(repo.join(f)).unwrap_or_default()).is_some() {
            v.push(AdapterConfig::Cargo { file: (f != "Cargo.toml").then(|| f.to_string()) });
            break;
        }
    }
    if repo.join("pyproject.toml").exists()
        && python_span(&std::fs::read_to_string(repo.join("pyproject.toml")).unwrap_or_default()).is_some()
    {
        v.push(AdapterConfig::Python { file: None });
    }
    for f in ["src-tauri/tauri.conf.json", "tauri.conf.json"] {
        if let Ok(t) = std::fs::read_to_string(repo.join(f)) {
            if tauri_span(&t).is_some() {
                v.push(AdapterConfig::Tauri { file: Some(f.to_string()) });
            }
            break;
        }
    }
    for f in ["app/build.gradle.kts", "app/build.gradle"] {
        if repo.join(f).exists() {
            v.push(AdapterConfig::Android { file: Some(f.to_string()) });
            break;
        }
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
        AdapterConfig::Cargo { file } => span_read(rc.repo, file.as_deref().unwrap_or("Cargo.toml"), cargo_span),
        AdapterConfig::Python { file } => span_read(rc.repo, file.as_deref().unwrap_or("pyproject.toml"), python_span),
        AdapterConfig::Tauri { file } => span_read(rc.repo, file.as_deref().unwrap_or("src-tauri/tauri.conf.json"), tauri_span),
        AdapterConfig::Android { file } => {
            let f = file.as_deref().unwrap_or("app/build.gradle.kts");
            let t = read_file(rc.repo, f)?;
            let (a, b) = android_name(&t).ok_or_else(|| Error::Version(format!("no versionName in {f}")))?;
            // Gradle versionName is often "1.2" or "1.2.3-debug"; pad to semver.
            parse_loose(&t[a..b])
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
        AdapterConfig::Cargo { file } => {
            let f = file.as_deref().unwrap_or("Cargo.toml");
            let text = read_file(repo, f)?;
            let mut edits = vec![FileEdit { path: f.into(), content: span_replace(&text, cargo_span, &nv, f)? }];
            // Cargo.lock sits next to the manifest or at the workspace root.
            let name = cargo_name(&text);
            let dir = Path::new(f).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            for lock in [format!("{dir}/Cargo.lock").trim_start_matches('/').to_string(), "Cargo.lock".to_string()] {
                if let (Some(name), Ok(lt)) = (&name, std::fs::read_to_string(repo.join(&lock))) {
                    let re = Regex::new(&format!(r#"(\[\[package\]\]\r?\nname = "{}"\r?\nversion = ")([^"]+)(")"#, regex::escape(name))).unwrap();
                    if re.is_match(&lt) {
                        let out = re.replacen(&lt, 1, |c: &regex::Captures| format!("{}{}{}", &c[1], nv, &c[3]));
                        edits.push(FileEdit { path: lock, content: out.into_owned() });
                        break;
                    }
                }
            }
            Ok(edits)
        }
        AdapterConfig::Python { file } => {
            let f = file.as_deref().unwrap_or("pyproject.toml");
            Ok(vec![FileEdit { path: f.into(), content: span_replace(&read_file(repo, f)?, python_span, &nv, f)? }])
        }
        AdapterConfig::Tauri { file } => {
            let f = file.as_deref().unwrap_or("src-tauri/tauri.conf.json");
            Ok(vec![FileEdit { path: f.into(), content: span_replace(&read_file(repo, f)?, tauri_span, &nv, f)? }])
        }
        AdapterConfig::Android { file } => {
            let f = file.as_deref().unwrap_or("app/build.gradle.kts");
            let mut t = read_file(repo, f)?;
            let (a, b) = android_name(&t).ok_or_else(|| Error::Version(format!("no versionName in {f}")))?;
            t.replace_range(a..b, &nv);
            if let Some((a, b)) = android_code(&t) {
                let n: u64 = t[a..b].parse().unwrap_or(0);
                t.replace_range(a..b, &(n + 1).to_string());
            }
            Ok(vec![FileEdit { path: f.into(), content: t }])
        }
    }
}

// ---------------------------------------------------------------- file formats

type Span = fn(&str) -> Option<(usize, usize)>;

fn span_read(repo: &Path, f: &str, span: Span) -> Result<Version> {
    let t = read_file(repo, f)?;
    let (a, b) = span(&t).ok_or_else(|| Error::Version(format!("no version found in {f}")))?;
    parse_ver(&t[a..b])
}

fn span_replace(text: &str, span: Span, nv: &str, f: &str) -> Result<String> {
    let (a, b) = span(text).ok_or_else(|| Error::Version(format!("no version found in {f}")))?;
    Ok(format!("{}{}{}", &text[..a], nv, &text[b..]))
}

/// Byte range of `key = "value"` inside the first matching TOML table.
fn toml_value(text: &str, tables: &[&str], key: &str) -> Option<(usize, usize)> {
    let hdr = Regex::new(r"(?m)^\s*\[([^\]\[]+)\]\s*(#.*)?$").unwrap();
    let kv = Regex::new(&format!(r#"(?m)^\s*{}\s*=\s*"([^"]*)""#, regex::escape(key))).unwrap();
    // Table boundaries: every [x] or [[x]] header.
    let any_hdr = Regex::new(r"(?m)^\s*\[").unwrap();
    for want in tables {
        for c in hdr.captures_iter(text) {
            if c[1].trim() != *want {
                continue;
            }
            let start = c.get(0).unwrap().end();
            let end = any_hdr.find_at(text, start).map(|m| m.start()).unwrap_or(text.len());
            if let Some(m) = kv.captures(&text[start..end]) {
                let g = m.get(1).unwrap();
                return Some((start + g.start(), start + g.end()));
            }
        }
    }
    None
}

fn cargo_span(t: &str) -> Option<(usize, usize)> {
    toml_value(t, &["package", "workspace.package"], "version")
}
fn cargo_name(t: &str) -> Option<String> {
    toml_value(t, &["package"], "name").map(|(a, b)| t[a..b].to_string())
}
fn python_span(t: &str) -> Option<(usize, usize)> {
    toml_value(t, &["project", "tool.poetry"], "version")
}

/// Tauri 2 top-level "version" or Tauri 1 "package": { "version" }. A version
/// pointing at a file (e.g. "../package.json") is not a version: use npm.
fn tauri_span(t: &str) -> Option<(usize, usize)> {
    let j: serde_json::Value = serde_json::from_str(t).ok()?;
    let v = j["version"].as_str().or_else(|| j["package"]["version"].as_str())?;
    if v.ends_with(".json") {
        return None;
    }
    let re = Regex::new(&format!(r#""version"\s*:\s*"({})""#, regex::escape(v))).unwrap();
    let g = re.captures(t)?.get(1)?;
    Some((g.start(), g.end()))
}

fn android_name(t: &str) -> Option<(usize, usize)> {
    let re = Regex::new(r#"(?m)^\s*versionName\s*(?:=\s*)?["']([^"']+)["']"#).unwrap();
    let g = re.captures(t)?.get(1)?;
    Some((g.start(), g.end()))
}
fn android_code(t: &str) -> Option<(usize, usize)> {
    let re = Regex::new(r"(?m)^\s*versionCode\s*(?:=\s*)?(\d+)").unwrap();
    let g = re.captures(t)?.get(1)?;
    Some((g.start(), g.end()))
}

/// "1.2" -> 1.2.0, "3" -> 3.0.0; otherwise strict semver.
fn parse_loose(s: &str) -> Result<Version> {
    let t = s.trim().trim_start_matches('v');
    let (core, rest) = match t.find(['-', '+']) {
        Some(i) => (&t[..i], &t[i..]),
        None => (t, ""),
    };
    let parts = core.split('.').count();
    let padded = match parts {
        1 => format!("{core}.0.0{rest}"),
        2 => format!("{core}.0{rest}"),
        _ => t.to_string(),
    };
    parse_ver(&padded)
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
    fn cargo_adapter_updates_lock() {
        let toml = "[workspace]\nmembers = []\n\n[package]\nname = \"app\"\nversion = \"0.3.1\" # keep\n\n[dependencies]\nserde = { version = \"1\" }\n";
        let lock = "[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"app\"\nversion = \"0.3.1\"\n";
        let d = fixture(&[("Cargo.toml", toml), ("Cargo.lock", lock)]);
        let e = roundtrip(&AdapterConfig::Cargo { file: None }, &d, "0.3.1", "0.4.0");
        assert_eq!(e.len(), 2);
        let t = std::fs::read_to_string(d.path().join("Cargo.toml")).unwrap();
        assert!(t.contains("version = \"0.4.0\" # keep") && t.contains("serde = { version = \"1\" }"));
        let l = std::fs::read_to_string(d.path().join("Cargo.lock")).unwrap();
        assert!(l.contains("name = \"app\"\nversion = \"0.4.0\"") && l.contains("1.0.0"));
    }

    #[test]
    fn cargo_workspace_package() {
        let d = fixture(&[("Cargo.toml", "[workspace.package]\nversion = \"2.0.0\"\n")]);
        roundtrip(&AdapterConfig::Cargo { file: None }, &d, "2.0.0", "2.1.0");
    }

    #[test]
    fn python_adapter() {
        let d = fixture(&[("pyproject.toml", "[build-system]\nrequires = [\"x\"]\n\n[tool.poetry]\nname = \"p\"\nversion = \"1.4.0\"\n")]);
        roundtrip(&AdapterConfig::Python { file: None }, &d, "1.4.0", "1.5.0");
        let d2 = fixture(&[("pyproject.toml", "[project]\nname = \"p\"\nversion = \"0.1.0\"\n")]);
        roundtrip(&AdapterConfig::Python { file: None }, &d2, "0.1.0", "0.2.0");
    }

    #[test]
    fn tauri_adapter() {
        let d = fixture(&[("tauri.conf.json", "{\n  \"productName\": \"x\",\n  \"version\": \"0.9.0\",\n  \"plugins\": { \"a\": { \"version\": \"9\" } }\n}\n")]);
        roundtrip(&AdapterConfig::Tauri { file: Some("tauri.conf.json".into()) }, &d, "0.9.0", "1.0.0");
        assert!(tauri_span("{\"version\": \"../package.json\"}").is_none());
        assert!(tauri_span("{\"package\": {\"version\": \"1.0.0\"}}").is_some());
    }

    #[test]
    fn android_adapter_bumps_code() {
        for (f, src) in [
            ("build.gradle.kts", "android {\n    defaultConfig {\n        versionCode = 41\n        versionName = \"2.3\"\n    }\n}\n"),
            ("build.gradle", "android {\n  defaultConfig {\n    versionCode 41\n    versionName \"2.3.0\"\n  }\n}\n"),
        ] {
            let d = fixture(&[(f, src)]);
            roundtrip(&AdapterConfig::Android { file: Some(f.into()) }, &d, "2.3.0", "2.4.0");
            let t = std::fs::read_to_string(d.path().join(f)).unwrap();
            assert!(t.contains("42") && !t.contains("41"), "{t}");
        }
    }

    #[test]
    fn detects() {
        let d = fixture(&[("package.json", "{\"version\":\"1.0.0\"}")]);
        assert_eq!(detect(d.path()), vec![AdapterConfig::Npm { file: None }]);
        let e = fixture(&[]);
        assert_eq!(detect(e.path()), vec![AdapterConfig::GitTag]);
    }
}
