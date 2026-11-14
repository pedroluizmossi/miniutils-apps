//! From each repository's status to what the view and the mascot show. All pure (no disk
//! or process), so it can be tested.

use std::path::PathBuf;

use miniutils_app::Mood;
use serde_json::{Value, json};

use crate::agents::Seen;
use crate::gh::{Ci, GhInfo};
use crate::git::Status;
use crate::i18n::{fmt, tr};

/// What the app knows about a repository as of the last read.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub path: PathBuf,
    pub status: Result<Status, String>,
    /// Half-done operation (merge, rebase…), as shown in the row.
    pub op: Option<&'static str>,
    /// The repository has some remote configured.
    pub has_remote: bool,
    /// The remote is on github.com (`gh` can read its CI and PRs).
    pub github: bool,
    /// The main branch ("origin/main"), if it can be told.
    pub base: Option<String>,
    /// Time (seconds since 1970) of the oldest uncommitted change.
    pub oldest_change: Option<u64>,
    /// Time of the oldest unpushed commit.
    pub oldest_unpushed: Option<u64>,
}

impl Default for RepoInfo {
    fn default() -> Self {
        RepoInfo {
            path: PathBuf::new(),
            status: Ok(Status::default()),
            op: None,
            has_remote: false,
            github: false,
            base: None,
            oldest_change: None,
            oldest_unpushed: None,
        }
    }
}

impl RepoInfo {
    /// Has uncommitted changes?
    pub fn dirty(&self) -> bool {
        self.status.as_ref().is_ok_and(Status::dirty)
    }

    /// Commits not pushed to the upstream.
    pub fn unpushed(&self) -> u32 {
        self.status.as_ref().map_or(0, |s| if s.upstream.is_some() { s.ahead } else { 0 })
    }

    /// Can be pushed in one click: has an upstream, commits ahead and nothing half-done.
    pub fn can_push(&self) -> bool {
        self.status.as_ref().is_ok_and(|s| s.upstream.is_some() && s.ahead > 0 && s.conflicts == 0) && self.op.is_none()
    }

    /// Can be pulled in one click: behind the upstream and with nothing local the pull
    /// could mix in (no changes, no commits ahead: `--ff-only` only moves forward).
    pub fn can_pull(&self) -> bool {
        self.status.as_ref().is_ok_and(|s| s.upstream.is_some() && s.behind > 0 && s.ahead == 0 && !s.dirty()) && self.op.is_none()
    }
}

/// The state of a repository, from most urgent to calmest (the order of the list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Conflict, or half-done merge/rebase.
    Conflict,
    /// The main branch's CI failed.
    Broken,
    /// Pending work (uncommitted or unpushed) older than the limit.
    Stale,
    /// Recent uncommitted changes.
    Work,
    /// Recent unpushed commits.
    Push,
    /// git could not read it.
    Error,
    Clean,
}

impl State {
    pub fn key(self) -> &'static str {
        match self {
            State::Conflict => "conflict",
            State::Broken => "broken",
            State::Stale => "stale",
            State::Work => "work",
            State::Push => "push",
            State::Error => "error",
            State::Clean => "clean",
        }
    }

    pub fn pending(self) -> bool {
        matches!(self, State::Conflict | State::Broken | State::Stale | State::Work | State::Push)
    }
}

/// What, besides git, changes a repository's state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    /// The main branch's CI failed.
    pub ci_broken: bool,
    /// An agent is working in it (or waiting for the user): the pending work is the
    /// agent's, and does not count as old yet.
    pub agent_active: bool,
}

pub fn classify(info: &RepoInfo, now: u64, stale_secs: u64, flags: Flags) -> State {
    let st = match &info.status {
        Ok(st) => st,
        Err(_) => return State::Error,
    };
    if st.conflicts > 0 || info.op.is_some() {
        return State::Conflict;
    }
    if flags.ci_broken {
        return State::Broken;
    }
    let old = |since: Option<u64>| !flags.agent_active && since.is_some_and(|t| now.saturating_sub(t) >= stale_secs);
    let dirty = st.dirty();
    let unpushed = st.ahead > 0;
    if (dirty && old(info.oldest_change)) || (unpushed && old(info.oldest_unpushed)) {
        State::Stale
    } else if dirty {
        State::Work
    } else if unpushed {
        State::Push
    } else {
        State::Clean
    }
}

/// How long ago, ready for the view: "now", "12 min ago", "5 h ago", "3 d ago".
pub fn ago(secs: u64) -> String {
    let span = match secs {
        0..60 => return tr("now").to_string(),
        60..3600 => format!("{} min", secs / 60),
        3600..86_400 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    };
    fmt("{} ago", &[&span])
}

/// "1 staged" or "3 staged": `one` when `n` is 1, `many` otherwise; nothing for 0.
fn counted(n: u32, one: &str, many: String) -> Option<String> {
    match n {
        0 => None,
        1 => Some(one.to_string()),
        _ => Some(many),
    }
}

/// What the row shows besides git.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extra<'a> {
    /// The agent working (or waiting) in the repository.
    pub agent: Option<&'a Seen>,
    /// The agent that recently finished there ("Claude"), if it left uncommitted changes.
    pub finished: Option<&'a str>,
    pub gh: Option<&'a GhInfo>,
    /// Running action: "pushing…", "pulling…", "deleting branches…".
    pub busy: Option<&'a str>,
    pub pinned: bool,
    pub hidden: bool,
}

/// The CI the row shows: the current branch's (which may be the main one).
fn shown_ci(st: &Status, gh: &GhInfo) -> Option<Ci> {
    if !gh.base.is_empty() && st.branch == gh.base { gh.base_ci } else { gh.branch_ci }
}

/// One row of the view's list (the fields of `struct Repo` in `view.slint`).
pub fn row(info: &RepoInfo, name: &str, state: State, now: u64, extra: Extra) -> Value {
    let path = info.path.display().to_string();
    let mut base = json!({
        "name": name, "path": path, "branch": "", "changes": tr("error"), "ahead": 0, "behind": 0,
        "age": "", "state": state.key(), "note": "",
        "agent": "", "agent-state": "", "can-push": false, "can-pull": false,
        "busy": extra.busy.unwrap_or_default(), "ci": "", "ci-state": "",
        "pinned": extra.pinned, "hidden": extra.hidden,
    });
    let st = match &info.status {
        Ok(st) => st,
        Err(e) => {
            base["note"] = json!(e);
            return base;
        }
    };
    let changes: Vec<String> = [
        counted(st.conflicts, tr("1 conflicted"), fmt("{} conflicted", &[&st.conflicts])),
        counted(st.staged, tr("1 staged"), fmt("{} staged", &[&st.staged])),
        counted(st.unstaged, tr("1 modified"), fmt("{} modified", &[&st.unstaged])),
        counted(st.untracked, tr("1 new"), fmt("{} new", &[&st.untracked])),
    ]
    .into_iter()
    .flatten()
    .collect();
    let since = if st.dirty() { info.oldest_change } else if st.ahead > 0 { info.oldest_unpushed } else { None };
    let mut notes: Vec<String> = Vec::new();
    if let Some(op) = info.op {
        notes.push(op.to_string());
    }
    if st.branch.is_empty() {
        notes.push(tr("detached HEAD").into());
    } else if st.upstream.is_none() {
        notes.push(if info.has_remote { tr("branch without upstream") } else { tr("local only") }.into());
    }
    if let Some(s) = counted(st.stash, tr("1 stashed"), fmt("{} stashed", &[&st.stash])) {
        notes.push(s);
    }
    let (mut ci, mut ci_state) = ("", "");
    if let Some(gh) = extra.gh {
        if gh.base_ci == Some(Ci::Fail) && st.branch != gh.base {
            notes.push(fmt("CI on {} failed", &[&gh.base]));
        }
        if let Some(c) = shown_ci(st, gh) {
            (ci, ci_state) = (c.label(), c.key());
        }
        if let Some(s) = counted(gh.reviews, tr("1 PR awaits your review"), fmt("{} PRs await your review", &[&gh.reviews])) {
            notes.push(s);
        }
    }
    let (agent, agent_state) = match (extra.agent.filter(|a| a.activity.active()), extra.finished) {
        (Some(a), _) => (a.label(), a.key()),
        (None, Some(tool)) if st.dirty() => (fmt("{} finished · not committed", &[&tool]), "finished"),
        _ => (String::new(), ""),
    };
    let busy = extra.busy.is_some();
    base["branch"] = json!(if st.branch.is_empty() { tr("(detached)") } else { st.branch.as_str() });
    base["changes"] = json!(if changes.is_empty() { tr("clean").to_string() } else { changes.join(" · ") });
    base["ahead"] = json!(st.ahead);
    base["behind"] = json!(st.behind);
    // "5 h ago" (or "now"): ready for the view
    base["age"] = json!(since.map(|t| ago(now.saturating_sub(t))).unwrap_or_default());
    base["note"] = json!(notes.join(" · "));
    base["agent"] = json!(agent);
    base["agent-state"] = json!(agent_state);
    base["can-push"] = json!(!busy && info.can_push());
    base["can-pull"] = json!(!busy && info.can_pull());
    base["ci"] = json!(ci);
    base["ci-state"] = json!(ci_state);
    base
}

/// The summary of all of them: mood, the view's subtitle and the tray line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub mood: Mood,
    pub subtitle: String,
    pub summary: String,
}

pub fn overview(states: &[(String, State)]) -> Overview {
    if states.is_empty() {
        let text = tr("no repositories in the watched folders").to_string();
        return Overview { mood: Mood::Sleep, subtitle: text.clone(), summary: text };
    }
    let n = states.len();
    let with = |s: State| states.iter().filter(|(_, x)| *x == s).map(|(name, _)| name.as_str()).collect::<Vec<_>>();
    let pending: Vec<&str> = states.iter().filter(|(_, s)| s.pending()).map(|(name, _)| name.as_str()).collect();
    let (conflict, broken, stale) = (with(State::Conflict), with(State::Broken), with(State::Stale));
    let mood = if !conflict.is_empty() || !broken.is_empty() {
        Mood::Alarm
    } else if !stale.is_empty() {
        Mood::Busy
    } else if !pending.is_empty() || !with(State::Error).is_empty() {
        Mood::Idle
    } else {
        Mood::Happy
    };
    let total = if n == 1 { tr("1 repository").to_string() } else { fmt("{} repositories", &[&n]) };
    let subtitle = match pending.len() {
        0 => format!("{total} · {}", tr("all committed and pushed")),
        p => format!("{total} · {}", fmt("{} with pending work", &[&p])),
    };
    let list = |names: &[&str]| {
        let shown: Vec<&str> = names.iter().take(3).copied().collect();
        let more = names.len().saturating_sub(3);
        if more > 0 { fmt("{} and {} more", &[&shown.join(", "), &more]) } else { shown.join(", ") }
    };
    let summary = if !conflict.is_empty() {
        fmt("conflicted: {}", &[&list(&conflict)])
    } else if !broken.is_empty() {
        fmt("broken CI: {}", &[&list(&broken)])
    } else if !pending.is_empty() {
        fmt("{} with pending work: {}", &[&pending.len(), &list(&pending)])
    } else {
        tr("all committed and pushed").into()
    };
    Overview { mood, subtitle, summary }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::Activity;

    const H: u64 = 3600;
    const NONE: Flags = Flags { ci_broken: false, agent_active: false };

    fn info(st: Status) -> RepoInfo {
        RepoInfo { path: PathBuf::from("/p/api"), status: Ok(st), ..Default::default() }
    }

    #[test]
    fn classifies_by_the_oldest_pending_work() {
        let now = 100 * H;
        let clean = Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() };
        assert_eq!(classify(&info(clean.clone()), now, 24 * H, NONE), State::Clean);
        // being behind the remote is not the user's pending work
        assert_eq!(classify(&info(Status { behind: 3, ..clean.clone() }), now, 24 * H, NONE), State::Clean);
        let dirty = Status { unstaged: 2, ..clean.clone() };
        let mut i = info(dirty);
        i.oldest_change = Some(now - H);
        assert_eq!(classify(&i, now, 24 * H, NONE), State::Work);
        i.oldest_change = Some(now - 30 * H);
        assert_eq!(classify(&i, now, 24 * H, NONE), State::Stale);
        // with an agent working in it, the pending work is not old yet
        assert_eq!(classify(&i, now, 24 * H, Flags { agent_active: true, ..NONE }), State::Work);
        let mut i = info(Status { ahead: 1, ..clean.clone() });
        i.oldest_unpushed = Some(now - 2 * H);
        assert_eq!(classify(&i, now, 24 * H, NONE), State::Push);
        assert_eq!(classify(&i, now, H, NONE), State::Stale);
        // broken CI goes ahead of pending work, but not of a conflict
        assert_eq!(classify(&i, now, H, Flags { ci_broken: true, ..NONE }), State::Broken);
        let mut i = info(clean.clone());
        i.op = Some("rebase in progress");
        assert_eq!(classify(&i, now, 24 * H, Flags { ci_broken: true, ..NONE }), State::Conflict);
        assert_eq!(classify(&info(Status { conflicts: 1, ..clean }), now, 24 * H, NONE), State::Conflict);
        let error = RepoInfo { status: Err("dubious ownership".into()), ..Default::default() };
        assert_eq!(classify(&error, now, 24 * H, NONE), State::Error);
    }

    #[test]
    fn view_row() {
        let now = 100 * H;
        let st = Status { branch: "main".into(), staged: 1, unstaged: 2, untracked: 1, ahead: 2, stash: 1, ..Default::default() };
        let mut i = info(st);
        i.has_remote = true;
        i.oldest_change = Some(now - 5 * H);
        let r = row(&i, "api", State::Work, now, Extra::default());
        assert_eq!(r["changes"], "1 staged · 2 modified · 1 new");
        assert_eq!(r["age"], "5 h ago");
        assert_eq!(r["note"], "branch without upstream · 1 stashed");
        assert_eq!((r["ahead"].as_u64(), r["state"].as_str()), (Some(2), Some("work")));
        // without an upstream it cannot be pushed in one click
        assert_eq!(r["can-push"], false);
        let r = row(&info(Status::default()), "x", State::Clean, now, Extra::default());
        assert_eq!((r["changes"].as_str(), r["branch"].as_str(), r["note"].as_str()), (Some("clean"), Some("(detached)"), Some("detached HEAD")));
        let r = row(&RepoInfo { status: Err("bad".into()), ..Default::default() }, "e", State::Error, now, Extra::default());
        assert_eq!((r["note"].as_str(), r["can-pull"].as_bool(), r["changes"].as_str()), (Some("bad"), Some(false), Some("error")));
        assert_eq!(ago(30), "now");
        assert_eq!(ago(12 * 60), "12 min ago");
        assert_eq!(ago(3 * 86_400 + 5), "3 d ago");
    }

    #[test]
    fn push_and_pull_in_one_click() {
        let up = Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() };
        assert!(info(Status { ahead: 1, ..up.clone() }).can_push());
        assert!(!info(Status { ahead: 1, conflicts: 1, ..up.clone() }).can_push());
        assert!(info(Status { behind: 2, ..up.clone() }).can_pull());
        // a local change or a commit ahead: the pull would not just move forward
        assert!(!info(Status { behind: 2, unstaged: 1, ..up.clone() }).can_pull());
        assert!(!info(Status { behind: 2, ahead: 1, ..up.clone() }).can_pull());
        let i = info(Status { ahead: 1, ..up });
        let busy = row(&i, "a", State::Push, 0, Extra { busy: Some("pushing…"), ..Default::default() });
        assert_eq!((busy["can-push"].as_bool(), busy["busy"].as_str()), (Some(false), Some("pushing…")));
        assert_eq!(i.unpushed(), 1);
    }

    #[test]
    fn agent_ci_and_reviews_in_the_row() {
        let st = Status { branch: "feat".into(), upstream: Some("origin/feat".into()), unstaged: 1, ..Default::default() };
        let i = info(st);
        let working = Seen { tool: "Claude".into(), activity: Activity::Working };
        let gh = GhInfo { base: "main".into(), base_ci: Some(Ci::Fail), branch_ci: Some(Ci::Ok), reviews: 2 };
        let r = row(&i, "api", State::Broken, 0, Extra { agent: Some(&working), gh: Some(&gh), ..Default::default() });
        assert_eq!((r["agent"].as_str(), r["agent-state"].as_str()), (Some("Claude working"), Some("working")));
        assert_eq!((r["ci"].as_str(), r["ci-state"].as_str()), (Some("CI ✓"), Some("ok")));
        assert_eq!(r["note"], "CI on main failed · 2 PRs await your review");
        // finished and left changes: the row nags
        let r = row(&i, "api", State::Work, 0, Extra { finished: Some("Codex"), ..Default::default() });
        assert_eq!((r["agent"].as_str(), r["agent-state"].as_str()), (Some("Codex finished · not committed"), Some("finished")));
        // on the main branch, the CI shown is its own
        let main = info(Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() });
        let r = row(&main, "api", State::Broken, 0, Extra { gh: Some(&gh), ..Default::default() });
        assert_eq!((r["ci"].as_str(), r["note"].as_str()), (Some("CI ✗"), Some("2 PRs await your review")));
    }

    #[test]
    fn mood_and_summary() {
        let s = |v: &[(&str, State)]| v.iter().map(|(n, s)| (n.to_string(), *s)).collect::<Vec<_>>();
        assert_eq!(overview(&[]).mood, Mood::Sleep);
        let all = overview(&s(&[("a", State::Clean), ("b", State::Clean)]));
        assert_eq!((all.mood, all.subtitle.as_str()), (Mood::Happy, "2 repositories · all committed and pushed"));
        let o = overview(&s(&[("a", State::Work), ("b", State::Stale), ("c", State::Clean)]));
        assert_eq!(o.mood, Mood::Busy);
        assert_eq!(o.subtitle, "3 repositories · 2 with pending work");
        assert_eq!(o.summary, "2 with pending work: a, b");
        let o = overview(&s(&[("a", State::Conflict), ("b", State::Stale)]));
        assert_eq!((o.mood, o.summary.as_str()), (Mood::Alarm, "conflicted: a"));
        let o = overview(&s(&[("a", State::Broken), ("b", State::Clean)]));
        assert_eq!((o.mood, o.summary.as_str()), (Mood::Alarm, "broken CI: a"));
        let many = s(&[("a", State::Push), ("b", State::Push), ("c", State::Push), ("d", State::Push), ("e", State::Push)]);
        assert_eq!(overview(&many).summary, "5 with pending work: a, b, c and 2 more");
    }
}
