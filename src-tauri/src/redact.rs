//! Redaction filter applied to every log line and error that leaves the core.
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::RwLock;

static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        // GitHub token formats: classic, fine-grained, OAuth, app, refresh.
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        // Authorization headers and credentials embedded in URLs.
        r"(?i)(authorization:\s*(bearer|token)\s+)[^\s]+",
        r"(https?://)[^/\s:@]+:[^/\s@]+@",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// Exact secrets registered at runtime (e.g. the token just read from the keychain).
static KNOWN: Lazy<RwLock<Vec<String>>> = Lazy::new(|| RwLock::new(Vec::new()));

pub fn register_secret(s: &str) {
    if s.len() >= 8 {
        let mut k = KNOWN.write().unwrap();
        if !k.iter().any(|x| x == s) {
            k.push(s.to_string());
        }
    }
}

pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for s in KNOWN.read().unwrap().iter() {
        out = out.replace(s.as_str(), "***");
    }
    out = PATTERNS[0].replace_all(&out, "***").into_owned();
    out = PATTERNS[1].replace_all(&out, "***").into_owned();
    out = PATTERNS[2].replace_all(&out, "${1}***").into_owned();
    out = PATTERNS[3].replace_all(&out, "${1}***@").into_owned();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_shapes() {
        let classic = format!("ghp_{}", "a".repeat(36));
        let fine = format!("github_pat_{}", "B1_".repeat(20));
        let log = format!(
            "push with {classic}\nAuthorization: Bearer {fine}\nurl https://user:{classic}@github.com/o/r.git"
        );
        let r = redact(&log);
        assert!(!r.contains(&classic));
        assert!(!r.contains(&fine));
        assert!(r.contains("Authorization: Bearer ***"));
        assert!(r.contains("https://***@github.com"));
    }

    #[test]
    fn redacts_registered_secret() {
        register_secret("plain-secret-value-123");
        assert_eq!(redact("x plain-secret-value-123 y"), "x *** y");
    }
}
