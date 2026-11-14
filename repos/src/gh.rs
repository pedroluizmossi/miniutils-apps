//! GitHub CI and PRs through `gh` (the official CLI), when it is installed and logged in.
//! Only for repositories with a remote on github.com. Read along with the fetch, every
//! 30 min: `gh` spends the GitHub API quota, and CI does not change every second.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::git;

/// Deadline of a `gh` query (network).
const TIMEOUT: Duration = Duration::from_secs(20);

/// State of the last CI run on a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ci {
    Ok,
    Fail,
    Running,
}

impl Ci {
    /// The row's short text: "CI ✓", "CI ✗", "CI …".
    pub fn label(self) -> &'static str {
        match self {
            Ci::Ok => "CI ✓",
            Ci::Fail => "CI ✗",
            Ci::Running => "CI …",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Ci::Ok => "ok",
            Ci::Fail => "fail",
            Ci::Running => "running",
        }
    }
}

/// What GitHub says about a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GhInfo {
    /// Local name of the main branch ("main"), if known.
    pub base: String,
    /// CI of the main branch.
    pub base_ci: Option<Ci>,
    /// CI of the current branch, when it is not the main one.
    pub branch_ci: Option<Ci>,
    /// Open PRs that request your review.
    pub reviews: u32,
}

/// Is `gh` installed and logged in? (without a login, every query would just fail)
pub fn ready(dir: &Path) -> bool {
    miniutils_app::os::which("gh").is_some() && git::run("gh", dir, &["auth", "status"], TIMEOUT).is_ok()
}

/// Is the repository's remote on github.com? (from the `config`, without running anything)
pub fn is_github(config: &str) -> bool {
    config.lines().any(|l| {
        let l = l.trim();
        l.starts_with("url") && l.contains("github.com")
    })
}

#[derive(Deserialize)]
struct Run {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
}

/// The output of `gh run list --limit 1 --json status,conclusion`. Cancelled or skipped
/// says nothing about the code: no state.
pub fn parse_run(out: &[u8]) -> Option<Ci> {
    let runs: Vec<Run> = serde_json::from_slice(out).ok()?;
    let run = runs.first()?;
    if run.status != "completed" {
        return Some(Ci::Running);
    }
    match run.conclusion.as_str() {
        "success" => Some(Ci::Ok),
        "failure" | "timed_out" | "startup_failure" => Some(Ci::Fail),
        _ => None,
    }
}

/// How many items the JSON list has (`gh pr list --json number`).
pub fn parse_count(out: &[u8]) -> u32 {
    serde_json::from_slice::<Vec<serde_json::Value>>(out).map(|v| v.len() as u32).unwrap_or(0)
}

fn ci_of(dir: &Path, branch: &str) -> Option<Ci> {
    match git::run("gh", dir, &["run", "list", "--branch", branch, "--limit", "1", "--json", "status,conclusion"], TIMEOUT) {
        Ok(out) => parse_run(&out),
        Err(e) => {
            eprintln!("gh run list in {}: {e}", dir.display());
            None
        }
    }
}

/// Reads the main branch's CI (and the current branch's, if it is another one) and the
/// review requests.
pub fn read(dir: &Path, branch: &str, base: Option<&str>) -> GhInfo {
    let base = base.map(git::short_branch).unwrap_or_default().to_string();
    let base_ci = (!base.is_empty()).then(|| ci_of(dir, &base)).flatten();
    let branch_ci = (!branch.is_empty() && branch != base).then(|| ci_of(dir, branch)).flatten();
    let reviews = git::run(
        "gh",
        dir,
        &["pr", "list", "--state", "open", "--search", "review-requested:@me", "--json", "number", "--limit", "100"],
        TIMEOUT,
    )
    .map(|o| parse_count(&o))
    .unwrap_or(0);
    GhInfo { base, base_ci, branch_ci, reviews }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_last_ci_run() {
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"success"}]"#), Some(Ci::Ok));
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"failure"}]"#), Some(Ci::Fail));
        assert_eq!(parse_run(br#"[{"status":"in_progress","conclusion":""}]"#), Some(Ci::Running));
        assert_eq!(parse_run(br#"[{"status":"queued"}]"#), Some(Ci::Running));
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"cancelled"}]"#), None);
        assert_eq!(parse_run(b"[]"), None);
        assert_eq!(parse_run(b"error"), None);
        assert_eq!(parse_count(br#"[{"number":1},{"number":7}]"#), 2);
        assert_eq!(parse_count(b""), 0);
    }

    #[test]
    fn only_github_remotes() {
        let gh = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:me/x.git\n";
        let gl = "[remote \"origin\"]\n\turl = https://gitlab.com/me/x.git\n";
        assert!(is_github(gh));
        assert!(!is_github(gl));
        assert!(!is_github(""));
    }
}
