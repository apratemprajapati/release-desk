//! Validation for values that reach the filesystem, git arguments or GitHub
//! URLs. Repo files (`release-desk.json`) and commit data are untrusted input:
//! a cloned repo must not be able to write outside itself, smuggle git options,
//! or redirect an API call.
use crate::error::{Error, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Component, Path, PathBuf};

/// A path inside the repo: relative, no `..`, no drive or root.
pub fn repo_path(repo: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel.trim());
    if rel.trim().is_empty() || rel.contains('\0') {
        return Err(Error::Blocked("empty file path".into()));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(Error::Blocked(format!("'{rel}' must be a relative path inside the repository"))),
        }
    }
    if rel.starts_with('/') || rel.starts_with('\\') || rel.chars().nth(1) == Some(':') {
        return Err(Error::Blocked(format!("'{rel}' must be a relative path inside the repository")));
    }
    Ok(repo.join(p))
}

static REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/+-]*$").unwrap());

/// Branch or tag name safe to pass to git and to put in a URL path.
pub fn git_ref(name: &str) -> Result<&str> {
    let ok = name.len() <= 200
        && REF.is_match(name)
        && !name.contains("..")
        && !name.contains("//")
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.ends_with(".lock");
    if ok { Ok(name) } else { Err(Error::Blocked(format!("'{name}' is not a valid branch or tag name"))) }
}

static SLUG: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9._-]{1,100}$").unwrap());

pub fn slug(owner: &str, repo: &str) -> Result<()> {
    if SLUG.is_match(owner) && SLUG.is_match(repo) && owner != "." && owner != ".." && repo != "." && repo != ".." {
        Ok(())
    } else {
        Err(Error::Blocked(format!("'{owner}/{repo}' is not a valid GitHub repository")))
    }
}

static WORKFLOW: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._-]*\.ya?ml|[0-9]+)$").unwrap());

/// Workflow file name (`release.yml`) or numeric id, never a path.
pub fn workflow(name: &str) -> Result<&str> {
    if WORKFLOW.is_match(name) { Ok(name) } else { Err(Error::Blocked(format!("'{name}' is not a workflow file name"))) }
}

static SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)(^|/)(
            \.env(\..+)? | .*\.(key|pem|p12|pfx|jks|keystore|ppk|asc|gpg|kdbx) |
            id_(rsa|dsa|ecdsa|ed25519)(\.pub)? | .*\.key\.pub | credentials(\.json)? |
            \.npmrc | \.pypirc | \.netrc | .*secret.* | .*\.tfstate(\.backup)?
        )$",
    )
    .unwrap()
});

/// File names that usually hold secrets. Used to refuse committing them
/// without an explicit override, and to hide their contents in the diff view.
pub fn looks_secret(path: &str) -> bool {
    SECRET.is_match(&path.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_repo() {
        let r = Path::new("/repo");
        assert!(repo_path(r, "src-tauri/Cargo.toml").is_ok());
        assert!(repo_path(r, "./VERSION").is_ok());
        for bad in ["../x", "a/../../x", "/etc/passwd", "\\\\server\\x", "C:\\x", "C:x", ""] {
            assert!(repo_path(r, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refs_and_slugs() {
        for ok in ["main", "feat/phase-3", "v1.2.3", "v1.2.3-rc.1", "release-1.0+build"] {
            assert!(git_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["-f", "--upload-pack=x", "a..b", "a b", "x?y", "x#y", "x/", "x.lock", "", "a//b", "../x"] {
            assert!(git_ref(bad).is_err(), "{bad}");
        }
        assert!(slug("apratemprajapati", "release-desk").is_ok());
        assert!(slug("..", "x").is_err() && slug("a/b", "c").is_err() && slug("a", "c?x=1").is_err());
        assert!(workflow("release.yml").is_ok() && workflow("123").is_ok());
        assert!(workflow("../../pulls").is_err() && workflow("a/b.yml").is_err() && workflow("x.yml?y").is_err());
    }

    #[test]
    fn secret_names() {
        for s in ["%USERPROFILE%/.tauri/release-desk.key", "%USERPROFILE%/.tauri/release-desk.key.pub", ".env",
            ".env.local", "certs/server.pem", "id_ed25519", "app.keystore", "gcp/credentials.json", "client_secret.txt"] {
            assert!(looks_secret(s), "{s}");
        }
        for s in ["src/key.ts", "keyboard.md", "package.json", "CHANGELOG.md", "env.d.ts"] {
            assert!(!looks_secret(s), "{s}");
        }
    }
}
