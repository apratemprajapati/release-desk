//! Thin wrapper over the system git CLI. Read commands always run; write
//! commands go through `Ctx::side_effect` so dry-run never touches the repo.
use crate::error::{Error, Result};
use crate::exec::Ctx;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// Commands that are allowed to run during dry-run. Everything else is a write.
const READ_ONLY: &[&str] = &[
    "status", "diff", "log", "rev-parse", "remote", "describe", "tag", "rev-list",
    "symbolic-ref", "show", "ls-remote", "for-each-ref", "config", "ls-files", "cat-file",
];

fn is_read(args: &[&str]) -> bool {
    match args.first() {
        Some(&"tag") => args.len() == 1 || args[1] == "-l" || args[1] == "--list",
        Some(&"remote") => args.get(1).map_or(true, |a| *a == "get-url" || *a == "-v"),
        Some(&"config") => args.iter().any(|a| *a == "--get"),
        Some(&"branch") => args.iter().any(|a| *a == "--contains" || *a == "--show-current" || *a == "--list"),
        Some(c) => READ_ONLY.contains(c),
        None => true,
    }
}

fn raw(ctx: &Ctx, repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(&ctx.git_path)
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| Error::Git(format!("cannot run git ({}): {e}", ctx.git_path)))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(Error::Git(format!("git {} failed: {}", args.join(" "), stderr.trim())));
    }
    if !is_read(args) && !stderr.trim().is_empty() {
        ctx.log(stderr.trim());
    }
    Ok(stdout)
}

/// Run a read-only git command (always executes).
pub fn read(ctx: &Ctx, repo: &Path, args: &[&str]) -> Result<String> {
    debug_assert!(is_read(args), "git {:?} is not read-only", args);
    raw(ctx, repo, args)
}

/// Run a mutating git command (skipped and logged in dry-run).
pub fn write(ctx: &Ctx, repo: &Path, args: &[&str]) -> Result<String> {
    if args.iter().any(|a| *a == "--force" || *a == "-f" || a.starts_with("--force") || a.starts_with("+")) {
        return Err(Error::Blocked("force operations are never allowed".into()));
    }
    if ctx.side_effect(&format!("git {}", args.join(" "))) {
        let o = raw(ctx, repo, args)?;
        if !o.trim().is_empty() {
            ctx.log(o.trim());
        }
        Ok(o)
    } else {
        Ok(String::new())
    }
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct FileChange {
    pub path: String,
    /// Two-letter XY code from porcelain v2, or "??" for untracked.
    pub code: String,
    pub staged: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Status {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
}

pub fn parse_status(text: &str) -> Status {
    let mut s = Status::default();
    for line in text.lines() {
        if let Some(b) = line.strip_prefix("# branch.head ") {
            s.branch = b.to_string();
        } else if let Some(u) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(u.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for p in ab.split_whitespace() {
                if let Some(n) = p.strip_prefix('+') {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = p.strip_prefix('-') {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("1 ") {
            let xy = &rest[..2];
            let path = rest.splitn(8, ' ').nth(7).unwrap_or("").to_string();
            s.files.push(FileChange { staged: xy.as_bytes()[0] != b'.', code: xy.into(), path });
        } else if let Some(rest) = line.strip_prefix("2 ") {
            let xy = &rest[..2];
            let paths = rest.splitn(9, ' ').nth(8).unwrap_or("");
            let path = paths.split('\t').next().unwrap_or("").to_string();
            s.files.push(FileChange { staged: xy.as_bytes()[0] != b'.', code: xy.into(), path });
        } else if let Some(p) = line.strip_prefix("? ") {
            s.files.push(FileChange { path: p.into(), code: "??".into(), staged: false });
        } else if let Some(rest) = line.strip_prefix("u ") {
            let path = rest.splitn(10, ' ').nth(9).unwrap_or("").to_string();
            s.files.push(FileChange { path, code: "UU".into(), staged: false });
        }
    }
    s
}

pub fn status(ctx: &Ctx, repo: &Path) -> Result<Status> {
    Ok(parse_status(&read(ctx, repo, &["status", "--porcelain=v2", "-b", "--untracked-files=all"])?))
}

const DIFF_PREVIEW_MAX: u64 = 512 * 1024;

pub fn diff_file(ctx: &Ctx, repo: &Path, path: &str, untracked: bool) -> Result<String> {
    if crate::safety::looks_secret(path) {
        return Ok("(contents hidden: this file name usually holds a secret, such as a private key or .env file. \
It should not be committed. Move it out of the repository or add it to .gitignore.)".into());
    }
    if untracked {
        let full = crate::safety::repo_path(repo, path)?;
        let meta = std::fs::metadata(&full)?;
        if meta.is_dir() {
            return Ok("(folder)".into());
        }
        if meta.len() > DIFF_PREVIEW_MAX {
            return Ok(format!("(new file, {} KB: too large to preview)", meta.len() / 1024));
        }
        let bytes = std::fs::read(&full)?;
        if bytes.contains(&0) {
            return Ok(format!("(new binary file, {} bytes)", bytes.len()));
        }
        let content = String::from_utf8_lossy(&bytes);
        return Ok(content.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"));
    }
    read(ctx, repo, &["diff", "HEAD", "--", path])
}

pub fn current_branch(ctx: &Ctx, repo: &Path) -> Result<String> {
    Ok(read(ctx, repo, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string())
}

pub fn head_sha(ctx: &Ctx, repo: &Path) -> Result<String> {
    Ok(read(ctx, repo, &["rev-parse", "HEAD"])?.trim().to_string())
}

pub fn remote_url(ctx: &Ctx, repo: &Path) -> Option<String> {
    read(ctx, repo, &["remote", "get-url", "origin"]).ok().map(|s| s.trim().to_string())
}

pub fn default_branch(ctx: &Ctx, repo: &Path) -> String {
    read(ctx, repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| s.trim().strip_prefix("origin/").map(String::from))
        .or_else(|| current_branch(ctx, repo).ok())
        .unwrap_or_else(|| "main".into())
}

/// Parse `owner/repo` from an https or ssh GitHub remote URL.
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"github\.com[:/]([^/\s]+)/([^/\s]+?)(?:\.git)?/?$").unwrap();
    let c = re.captures(url.trim())?;
    Some((c[1].to_string(), c[2].to_string()))
}

/// Tags matching the project's tag format, newest first by version order.
pub fn last_tag(ctx: &Ctx, repo: &Path, tag_glob: &str) -> Option<String> {
    read(ctx, repo, &["describe", "--tags", "--abbrev=0", "--match", tag_glob])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn tag_exists(ctx: &Ctx, repo: &Path, tag: &str) -> bool {
    read(ctx, repo, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_ok()
}

/// Raw commit records since `from` (exclusive) up to HEAD.
pub fn commits_since(ctx: &Ctx, repo: &Path, from: Option<&str>) -> Result<Vec<RawCommit>> {
    let range = match from {
        Some(t) => format!("{t}..HEAD"),
        None => "HEAD".into(),
    };
    // Fields separated by \x1f, records by \x1e.
    let out = match read(ctx, repo, &["log", &range, "--format=%H%x1f%an%x1f%s%x1f%b%x1e"]) {
        Ok(o) => o,
        Err(_) => return Ok(vec![]), // empty repo
    };
    Ok(parse_log(&out))
}

/// Parse `git log --format=%H%x1f%an%x1f%s%x1f%b%x1e` output.
pub fn parse_log(out: &str) -> Vec<RawCommit> {
    out
        .split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .map(|r| {
            let f: Vec<&str> = r.trim_start_matches('\n').splitn(4, '\x1f').collect();
            RawCommit {
                sha: f.first().unwrap_or(&"").to_string(),
                author: f.get(1).unwrap_or(&"").to_string(),
                subject: f.get(2).unwrap_or(&"").to_string(),
                body: f.get(3).unwrap_or(&"").trim().to_string(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct RawCommit {
    pub sha: String,
    pub author: String,
    pub subject: String,
    pub body: String,
}

pub fn stage(ctx: &Ctx, repo: &Path, files: &[String]) -> Result<()> {
    stage_checked(ctx, repo, files, false)
}

/// Stage files. Secret-looking files are refused unless `allow_secrets`.
pub fn stage_checked(ctx: &Ctx, repo: &Path, files: &[String], allow_secrets: bool) -> Result<()> {
    if files.is_empty() {
        return Err(Error::Git("no files selected".into()));
    }
    for f in files {
        crate::safety::repo_path(repo, f)?;
    }
    let secrets: Vec<&String> = files.iter().filter(|f| crate::safety::looks_secret(f)).collect();
    if !secrets.is_empty() && !allow_secrets {
        return Err(Error::Blocked(format!(
            "refusing to commit files that look like secrets: {}",
            secrets.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )));
    }
    let mut args = vec!["add", "-A", "--"];
    args.extend(files.iter().map(|s| s.as_str()));
    write(ctx, repo, &args)?;
    Ok(())
}

pub fn commit(ctx: &Ctx, repo: &Path, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        return Err(Error::Git("empty commit message".into()));
    }
    let mut tmp = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut tmp, message.as_bytes())?;
    let p = tmp.path().to_string_lossy().to_string();
    // whitespace cleanup keeps lines starting with "#" (markdown headings, "#12 fixed").
    write(ctx, repo, &["commit", "--cleanup=whitespace", "-F", &p])?;
    Ok(())
}

pub fn annotated_tag(ctx: &Ctx, repo: &Path, tag: &str, message: &str) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut tmp, message.as_bytes())?;
    let p = tmp.path().to_string_lossy().to_string();
    crate::safety::git_ref(tag)?;
    write(ctx, repo, &["tag", "-a", "--cleanup=whitespace", tag, "-F", &p])?;
    Ok(())
}

pub fn push(ctx: &Ctx, repo: &Path, refspec: &str) -> Result<()> {
    crate::safety::git_ref(refspec)?;
    write(ctx, repo, &["push", "origin", refspec])?;
    Ok(())
}

pub fn is_tracked(ctx: &Ctx, repo: &Path, path: &str) -> bool {
    read(ctx, repo, &["ls-files", "--error-unmatch", "--", path]).is_ok()
}

pub fn subject(ctx: &Ctx, repo: &Path, rev: &str) -> Result<String> {
    Ok(read(ctx, repo, &["log", "-1", "--format=%s", rev])?.trim().to_string())
}

pub fn commit_of(ctx: &Ctx, repo: &Path, rev: &str) -> Result<String> {
    Ok(read(ctx, repo, &["rev-parse", &format!("{rev}^{{commit}}")])?.trim().to_string())
}

/// True when any remote-tracking branch already contains `rev`.
pub fn is_on_remote(ctx: &Ctx, repo: &Path, rev: &str) -> Result<bool> {
    Ok(!read(ctx, repo, &["branch", "-r", "--contains", rev])?.trim().is_empty())
}

pub fn fetch(ctx: &Ctx, repo: &Path) -> Result<()> {
    write(ctx, repo, &["fetch", "origin", "--prune", "--tags"])?;
    Ok(())
}

pub fn checkout(ctx: &Ctx, repo: &Path, branch: &str) -> Result<()> {
    crate::safety::git_ref(branch)?;
    // Trailing "--" makes git read `branch` as a branch, never as a path.
    write(ctx, repo, &["checkout", branch, "--"])?;
    Ok(())
}

pub fn merge_ff_only(ctx: &Ctx, repo: &Path, rev: &str) -> Result<()> {
    crate::safety::git_ref(rev)?;
    write(ctx, repo, &["merge", "--ff-only", rev])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let t = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n\
1 .M N... 100644 100644 100644 a b src/a.ts\n1 A. N... 000000 100644 100644 0 b new file.txt\n\
2 R. N... 100644 100644 100644 a b R100 to.txt\tfrom.txt\n? untracked.md\n";
        let s = parse_status(t);
        assert_eq!(s.branch, "main");
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.files.len(), 4);
        assert_eq!(s.files[0].path, "src/a.ts");
        assert!(!s.files[0].staged);
        assert_eq!(s.files[1].path, "new file.txt");
        assert!(s.files[1].staged);
        assert_eq!(s.files[2].path, "to.txt");
        assert_eq!(s.files[3].code, "??");
    }

    #[test]
    fn parses_remotes() {
        for u in ["https://github.com/a/b.git", "git@github.com:a/b.git", "https://github.com/a/b"] {
            assert_eq!(parse_github_remote(u), Some(("a".into(), "b".into())));
        }
        assert_eq!(parse_github_remote("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn classifies_reads() {
        assert!(is_read(&["tag", "-l"]));
        assert!(!is_read(&["tag", "-a", "v1"]));
        assert!(!is_read(&["push", "origin", "main"]));
        assert!(is_read(&["remote", "get-url", "origin"]));
        assert!(is_read(&["branch", "-r", "--contains", "HEAD"]));
        assert!(!is_read(&["branch", "-D", "x"]));
        assert!(!is_read(&["fetch", "origin"]));
    }
}
