//! Conventional Commit parser and release-notes / changelog rendering.
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub kind: Option<String>,
    pub scope: Option<String>,
    pub breaking: bool,
    pub description: String,
    /// PR number from a trailing "(#123)" in the subject.
    pub pr: Option<u64>,
}

pub fn parse_commit(sha: &str, author: &str, subject: &str, body: &str) -> Commit {
    let re = Regex::new(r"^(?P<type>[a-zA-Z]+)(?:\((?P<scope>[^)]*)\))?(?P<bang>!)?:\s+(?P<desc>.+)$").unwrap();
    let pr_re = Regex::new(r"\s*\(#(\d+)\)\s*$").unwrap();
    let subject = subject.trim();
    let pr = pr_re.captures(subject).and_then(|c| c[1].parse().ok());
    let body_breaking = body.lines().any(|l| l.starts_with("BREAKING CHANGE:") || l.starts_with("BREAKING-CHANGE:"));
    match re.captures(subject) {
        Some(c) => Commit {
            sha: sha.into(),
            author: author.into(),
            kind: Some(c["type"].to_lowercase()),
            scope: c.name("scope").map(|s| s.as_str().to_string()).filter(|s| !s.is_empty()),
            breaking: c.name("bang").is_some() || body_breaking,
            description: pr_re.replace(&c["desc"], "").trim().to_string(),
            pr,
        },
        None => Commit {
            sha: sha.into(),
            author: author.into(),
            kind: None,
            scope: None,
            breaking: body_breaking,
            description: pr_re.replace(subject, "").trim().to_string(),
            pr,
        },
    }
}

pub fn parse_all(raw: &[crate::git::RawCommit]) -> Vec<Commit> {
    raw.iter()
        .map(|r| parse_commit(&r.sha, &r.author, &r.subject, &r.body))
        // The app's own release commits never appear in notes.
        .filter(|c| !(c.kind.as_deref() == Some("chore") && c.scope.as_deref() == Some("release")))
        .collect()
}

pub struct NotesInput<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
    pub version: &'a str,
    pub prev_tag: Option<&'a str>,
    pub tag: &'a str,
}

fn line(c: &Commit, n: &NotesInput) -> String {
    let scope = c.scope.as_ref().map(|s| format!("**{s}:** ")).unwrap_or_default();
    let short = &c.sha[..c.sha.len().min(7)];
    let link = if n.owner.is_empty() {
        format!(" ({short})")
    } else if let Some(pr) = c.pr {
        format!(" ([#{pr}](https://github.com/{}/{}/pull/{pr}))", n.owner, n.repo)
    } else {
        format!(" ([{short}](https://github.com/{}/{}/commit/{}))", n.owner, n.repo, c.sha)
    };
    format!("- {scope}{}{link}", c.description)
}

/// Markdown grouped by Breaking / Features / Fixes / Other, with contributors.
pub fn render(commits: &[Commit], n: &NotesInput) -> String {
    let mut out = String::new();
    let groups: [(&str, Box<dyn Fn(&Commit) -> bool>); 4] = [
        ("Breaking changes", Box::new(|c: &Commit| c.breaking)),
        ("Features", Box::new(|c: &Commit| !c.breaking && c.kind.as_deref() == Some("feat"))),
        ("Fixes", Box::new(|c: &Commit| !c.breaking && matches!(c.kind.as_deref(), Some("fix") | Some("perf")))),
        ("Other", Box::new(|c: &Commit| {
            !c.breaking && !matches!(c.kind.as_deref(), Some("feat") | Some("fix") | Some("perf"))
        })),
    ];
    for (title, f) in groups.iter() {
        let items: Vec<String> = commits.iter().filter(|c| f(c)).map(|c| line(c, n)).collect();
        if !items.is_empty() {
            out.push_str(&format!("### {title}\n\n{}\n\n", items.join("\n")));
        }
    }
    if commits.is_empty() {
        out.push_str("No notable changes.\n\n");
    }
    let mut authors: Vec<&str> = commits.iter().map(|c| c.author.as_str()).filter(|a| !a.is_empty()).collect();
    authors.sort();
    authors.dedup();
    if !authors.is_empty() {
        out.push_str(&format!("### Contributors\n\n{}\n\n", authors.join(", ")));
    }
    if let (Some(prev), false) = (n.prev_tag, n.owner.is_empty()) {
        out.push_str(&format!(
            "**Full changelog:** https://github.com/{}/{}/compare/{prev}...{}\n",
            n.owner, n.repo, n.tag
        ));
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_conventional() {
        let c = parse_commit("abcdef123", "Ann", "feat(ui): add dark mode (#42)", "");
        assert_eq!(c.kind.as_deref(), Some("feat"));
        assert_eq!(c.scope.as_deref(), Some("ui"));
        assert_eq!(c.description, "add dark mode");
        assert_eq!(c.pr, Some(42));
        assert!(!c.breaking);

        assert!(parse_commit("a", "b", "fix!: x", "").breaking);
        assert!(parse_commit("a", "b", "refactor: x", "text\n\nBREAKING CHANGE: api").breaking);
        let plain = parse_commit("a", "b", "Update readme", "");
        assert_eq!(plain.kind, None);
        assert_eq!(plain.description, "Update readme");
        assert_eq!(parse_commit("a", "b", "FEAT: shout", "").kind.as_deref(), Some("feat"));
        assert_eq!(parse_commit("a", "b", "feat:missing space", "").kind, None);
    }

    #[test]
    fn renders_groups() {
        let cs = vec![
            parse_commit("1111111aaa", "Ann", "feat: one", ""),
            parse_commit("2222222bbb", "Bob", "fix(core): two (#7)", ""),
            parse_commit("3333333ccc", "Ann", "docs: three", ""),
            parse_commit("4444444ddd", "Cy", "feat!: four", ""),
        ];
        let md = render(&cs, &NotesInput { owner: "o", repo: "r", version: "1.0.0", prev_tag: Some("v0.9.0"), tag: "v1.0.0" });
        let b = md.find("### Breaking").unwrap();
        let f = md.find("### Features").unwrap();
        let x = md.find("### Fixes").unwrap();
        let o = md.find("### Other").unwrap();
        assert!(b < f && f < x && x < o);
        assert!(md.contains("- **core:** two ([#7](https://github.com/o/r/pull/7))"));
        assert!(md.contains("Ann, Bob, Cy"));
        assert!(md.contains("compare/v0.9.0...v1.0.0"));
    }

    #[test]
    fn skips_release_commits() {
        let raw = vec![
            crate::git::RawCommit { sha: "a".into(), author: "x".into(), subject: "chore(release): v1.0.0".into(), body: "".into() },
            crate::git::RawCommit { sha: "b".into(), author: "x".into(), subject: "fix: y".into(), body: "".into() },
        ];
        assert_eq!(parse_all(&raw).len(), 1);
    }
}
