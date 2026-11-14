//! AI agents working in the repositories, through the hub's `agents` provider (the same as
//! the Agents app: Claude Code, Codex and OpenCode). The hub sends the list of sessions
//! with each one's folder; here it becomes "who is in which repository".
//!
//! All pure (no disk or process), so it can be tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::i18n::fmt;

/// What the agent is doing, from calmest to most in need of attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Finished its turn and waits for the next request.
    Idle,
    Working,
    /// Stopped waiting for the user (permission, question).
    Waiting,
}

impl Activity {
    /// In the middle of something (working or waiting for an answer).
    pub fn active(self) -> bool {
        self != Activity::Idle
    }
}

/// The most relevant agent of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// "Claude", "Codex", "OpenCode".
    pub tool: String,
    pub activity: Activity,
}

impl Seen {
    /// The row's text: "Claude working", "Codex is waiting for you"; idle shows nothing.
    pub fn label(&self) -> String {
        match self.activity {
            Activity::Working => fmt("{} working", &[&self.tool]),
            Activity::Waiting => fmt("{} is waiting for you", &[&self.tool]),
            Activity::Idle => String::new(),
        }
    }

    /// "working", "waiting" or "" (for the color in the view).
    pub fn key(&self) -> &'static str {
        match self.activity {
            Activity::Working => "working",
            Activity::Waiting => "waiting",
            Activity::Idle => "",
        }
    }
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    agents: Vec<Session>,
}

#[derive(Debug, Clone, Deserialize)]
struct Session {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    state: String,
    /// Subagent: follows the agent that started it, which already counts.
    #[serde(default)]
    parent: Option<String>,
}

fn tool_name(key: &str) -> String {
    match key {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "opencode" => "OpenCode".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// Path for comparing: forward slashes, no trailing slash and, on Windows, case-insensitive
/// (there `C:\Projects` and `c:\projects` are the same folder).
fn norm(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// The agents of each repository, by each session's folder (a session in a subfolder
/// counts for the repository; with nested repositories, the innermost one). With several
/// agents in the same repository, the one most in need of attention wins.
pub fn by_repo(data: &Value, repos: &[PathBuf]) -> HashMap<PathBuf, Seen> {
    let Ok(snap) = Snapshot::deserialize(data) else { return HashMap::new() };
    let keys: Vec<(String, &PathBuf)> = repos.iter().map(|r| (norm(&r.to_string_lossy()), r)).collect();
    let mut out: HashMap<PathBuf, Seen> = HashMap::new();
    for s in snap.agents.iter().filter(|s| s.parent.is_none() && !s.cwd.is_empty()) {
        let activity = match s.state.as_str() {
            "working" => Activity::Working,
            "waiting" => Activity::Waiting,
            _ => Activity::Idle,
        };
        let cwd = norm(&s.cwd);
        let inside = |k: &str| cwd == k || cwd.strip_prefix(k).is_some_and(|rest| rest.starts_with('/'));
        let Some((_, repo)) = keys.iter().filter(|(k, _)| inside(k)).max_by_key(|(k, _)| k.len()) else { continue };
        let seen = Seen { tool: tool_name(&s.tool), activity };
        match out.get(*repo) {
            Some(old) if old.activity >= activity => {}
            _ => {
                out.insert((*repo).clone(), seen);
            }
        }
    }
    out
}

/// Repositories where an agent was working and stopped (finished its turn or closed).
pub fn finished<'a>(before: &'a HashMap<PathBuf, Seen>, now: &HashMap<PathBuf, Seen>) -> Vec<(&'a Path, &'a str)> {
    before
        .iter()
        .filter(|(repo, s)| s.activity == Activity::Working && !now.get(*repo).is_some_and(|n| n.activity.active()))
        .map(|(repo, s)| (repo.as_path(), s.tool.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn each_session_goes_to_its_repository() {
        let repos = [path("/p/api"), path("/p/api/vendor/lib"), path("/p/site"), path("/p/apix")];
        let data = json!({"agents": [
            {"tool": "claude", "cwd": "/p/api/src", "state": "working", "parent": null},
            {"tool": "codex", "cwd": "/p/api/vendor/lib", "state": "idle"},
            {"tool": "opencode", "cwd": "/p/site/", "state": "waiting"},
            // subagent: the parent already counts
            {"tool": "claude", "cwd": "/p/apix", "state": "working", "parent": "abc"},
            {"tool": "claude", "cwd": "/elsewhere", "state": "working"},
        ], "found": ["claude"], "errors": []});
        let m = by_repo(&data, &repos);
        assert_eq!(m.len(), 3);
        assert_eq!(m[&path("/p/api")].label(), "Claude working");
        assert_eq!(m[&path("/p/api/vendor/lib")].activity, Activity::Idle);
        assert_eq!(m[&path("/p/site")].label(), "OpenCode is waiting for you");
        assert!(!m.contains_key(&path("/p/apix")), "/p/apix is not /p/api and the subagent does not count");
    }

    #[test]
    fn in_the_same_repository_the_one_needing_attention_wins() {
        let repos = [path("/p/api")];
        let data = json!({"agents": [
            {"tool": "codex", "cwd": "/p/api", "state": "working"},
            {"tool": "claude", "cwd": "/p/api", "state": "waiting"},
            {"tool": "opencode", "cwd": "/p/api", "state": "idle"},
        ]});
        assert_eq!(by_repo(&data, &repos)[&path("/p/api")].tool, "Claude");
        assert!(by_repo(&json!({"something_new": 1}), &repos).is_empty());
        assert!(by_repo(&Value::Null, &repos).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_compares_ignoring_case_and_slashes() {
        let repos = [path(r"C:\Projects\api")];
        let data = json!({"agents": [{"tool": "claude", "cwd": "c:/projects/API/src", "state": "working"}]});
        assert_eq!(by_repo(&data, &repos).len(), 1);
    }

    #[test]
    fn agent_that_stopped_working() {
        let w = |t: &str, a| Seen { tool: t.into(), activity: a };
        let before = HashMap::from([
            (path("/a"), w("Claude", Activity::Working)),
            (path("/b"), w("Codex", Activity::Working)),
            (path("/c"), w("Claude", Activity::Working)),
            (path("/d"), w("Claude", Activity::Waiting)),
        ]);
        let now = HashMap::from([
            (path("/a"), w("Claude", Activity::Idle)),
            (path("/c"), w("Claude", Activity::Waiting)),
        ]);
        let mut done: Vec<_> = finished(&before, &now).into_iter().map(|(p, t)| (p.to_path_buf(), t)).collect();
        done.sort();
        // /c started waiting for the user (did not finish); /d was not working anymore
        assert_eq!(done, [(path("/a"), "Claude"), (path("/b"), "Codex")]);
    }
}
