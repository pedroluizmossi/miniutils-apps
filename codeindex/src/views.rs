//! From the server's data to what the view shows: rows, lines and meters, as JSON values
//! for `{"type":"set"}`. Pure functions (the clock comes in as `now`), so they are tested
//! without a hub or a server. Texts the user reads go through `tr`/`fmt`; the view
//! translates the fixed labels itself (`@tr`).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::api::{EnrichStatus, GrepMatch, Hit, Job, Progress, Repo, Symbol, Usage};
use crate::i18n::{fmt, tr};
use crate::state::age_secs;

/// Most rows the repo list sends at once (the view has no use for 300 rows).
pub const MAX_REPO_ROWS: usize = 150;
/// Jobs in the sync history.
pub const MAX_HISTORY: usize = 10;

// ------------------------------------------------------------------ small texts

/// "5 min ago", from seconds.
pub fn ago(secs: i64) -> String {
    match secs {
        s if s < 60 => tr("just now").to_string(),
        s if s < 3600 => fmt("{} min ago", &[&(s / 60)]),
        s if s < 86_400 => fmt("{} h ago", &[&(s / 3600)]),
        s => fmt("{} d ago", &[&(s / 86_400)]),
    }
}

/// "45 s", "3 min", "2 h".
pub fn duration(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => fmt("{} s", &[&s]),
        s if s < 3600 => fmt("{} min", &[&(s / 60)]),
        s => fmt("{} h", &[&(s / 3600)]),
    }
}

fn ago_of(iso: Option<&str>, now: DateTime<Utc>) -> Option<String> {
    iso.and_then(|t| age_secs(t, now)).map(ago)
}

/// The repo name without its org prefix: `acme__billing` -> `billing`.
pub fn short_name(name: &str) -> &str {
    name.split_once("__").map_or(name, |(_, rest)| rest)
}

/// "repo/path:line", with the short repo name.
pub fn location(repo: &str, path: &str, line: u64) -> String {
    format!("{}/{}:{}", short_name(repo), path, line.max(1))
}

/// `n` with thousands grouped by a thin space: 16326 -> "16 326".
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { (part as f64 / whole as f64).clamp(0.0, 1.0) }
}

/// First non-blank lines of a chunk, trimmed, at most `max` lines.
fn first_lines(code: &str, max: usize) -> String {
    let lines: Vec<&str> = code.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).take(max).collect();
    // keep the indentation relative to the least indented line
    let indent = lines.iter().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| l.get(indent..).unwrap_or(l.trim_start())).collect::<Vec<_>>().join("\n")
}

// ------------------------------------------------------------------ search

/// Where a search result points: enough to preview it and to open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub repo: String,
    pub path: String,
    pub line: u64,
    /// Last line of the matched chunk (= `line` for symbols and grep).
    pub end: u64,
}

impl Target {
    /// Lines to show in the preview: some context around the match, at most 80 lines.
    pub fn preview_range(&self) -> (u64, u64) {
        let start = self.line.saturating_sub(2).max(1);
        let end = self.end.max(self.line) + 6;
        (start, end.min(start + 80))
    }
}

/// One result row: `{idx, title, sub, snippet, badge}` (the view's `Row`).
pub fn hit_rows(hits: &[Hit]) -> (Vec<Value>, Vec<Target>) {
    let rows = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let doc = h.kind == "doc";
            let snippet = match (&h.title, doc) {
                (Some(t), true) if !t.trim().is_empty() => t.trim().to_string(),
                _ => first_lines(&h.code, 2),
            };
            let what = if doc { tr("documentation").to_string() } else { h.language.clone().unwrap_or_default() };
            let score = h.score.map(|s| format!("{:.0}%", s * 100.0));
            let badge = [Some(what).filter(|w| !w.is_empty()), score].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            json!({
                "idx": i,
                "title": location(&h.repo, &h.path, h.start_line),
                "sub": "",
                "snippet": snippet,
                "badge": badge,
            })
        })
        .collect();
    let targets = hits
        .iter()
        .map(|h| Target { repo: h.repo.clone(), path: h.path.clone(), line: h.start_line.max(1), end: h.end_line.max(h.start_line) })
        .collect();
    (rows, targets)
}

pub fn symbol_rows(symbols: &[Symbol]) -> (Vec<Value>, Vec<Target>) {
    let rows = symbols
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let snippet = s.signature.clone().filter(|x| !x.trim().is_empty()).unwrap_or_else(|| s.name.clone());
            let sub = [Some(s.kind.as_str()), s.container.as_deref(), s.language.as_deref()]
                .into_iter()
                .flatten()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            json!({ "idx": i, "title": location(&s.repo, &s.path, s.line), "sub": sub, "snippet": snippet.trim(), "badge": "" })
        })
        .collect();
    let targets = symbols.iter().map(|s| Target { repo: s.repo.clone(), path: s.path.clone(), line: s.line.max(1), end: s.line.max(1) }).collect();
    (rows, targets)
}

pub fn grep_rows(matches: &[GrepMatch]) -> (Vec<Value>, Vec<Target>) {
    let rows = matches
        .iter()
        .enumerate()
        .map(|(i, m)| json!({ "idx": i, "title": location(&m.repo, &m.path, m.line), "sub": "", "snippet": m.text.trim(), "badge": "" }))
        .collect();
    let targets = matches.iter().map(|m| Target { repo: m.repo.clone(), path: m.path.clone(), line: m.line.max(1), end: m.line.max(1) }).collect();
    (rows, targets)
}

/// The server's file text (already numbered: `   93  code`) with the matched lines marked
/// by `▸`. A line that does not start with a number (the server's "showing lines" note)
/// stays as it is.
pub fn mark_hit(text: &str, hit: (u64, u64)) -> String {
    text.lines()
        .map(|l| {
            let n: Option<u64> = l.trim_start().split(' ').next().and_then(|w| w.parse().ok());
            let mark = if n.is_some_and(|n| n >= hit.0 && n <= hit.1) { '▸' } else { ' ' };
            format!("{mark}{}", l.replace('\t', "    "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------------ repos

/// Order of the statuses in the list: what needs attention first.
fn status_rank(r: &Repo) -> u8 {
    match r.status.as_str() {
        "error" => 0,
        "pending" => 1,
        "indexed" if r.outdated => 2,
        "indexed" => 3,
        _ => 4,
    }
}

/// Does `r` pass the status chip `filter` (`all`, `indexed`, `pending`, `error`, `ignored`,
/// `outdated`)? `all` hides the ignored ones (they are not part of the index).
pub fn status_matches(r: &Repo, filter: &str) -> bool {
    match filter {
        "all" => r.status != "ignored",
        "outdated" => r.outdated && r.status == "indexed",
        f => r.status == f,
    }
}

/// What the repo list needs to know about the user's choices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoView<'a> {
    pub filter: &'a str,
    pub status: &'a str,
    pub expanded: Option<&'a str>,
    /// The action waiting for its second click: `index:<name>`, `remove:<name>`...
    pub confirm: Option<&'a str>,
    pub can_sync: bool,
    pub can_admin: bool,
}

/// Rows of the repo list (`RepoRow`) and the line under it ("Showing 150 of 287").
pub fn repo_rows(repos: &[Repo], v: &RepoView, now: DateTime<Utc>) -> (Vec<Value>, String) {
    let needle = v.filter.trim().to_lowercase();
    let mut list: Vec<&Repo> = repos
        .iter()
        .filter(|r| status_matches(r, v.status))
        .filter(|r| needle.is_empty() || r.name.to_lowercase().contains(&needle))
        .collect();
    list.sort_by(|a, b| status_rank(a).cmp(&status_rank(b)).then_with(|| a.name.cmp(&b.name)));
    let total = list.len();
    let rows = list.iter().take(MAX_REPO_ROWS).map(|r| repo_row(r, v, now)).collect();
    let note = if total == 0 {
        tr("No repository matches").to_string()
    } else if total > MAX_REPO_ROWS {
        fmt("Showing {} of {}: narrow the filter", &[&MAX_REPO_ROWS, &total])
    } else {
        fmt("{} repositories", &[&total])
    };
    (rows, note)
}

fn repo_row(r: &Repo, v: &RepoView, now: DateTime<Utc>) -> Value {
    let mut line: Vec<String> = Vec::new();
    if r.kind.as_deref() == Some("docs") {
        line.push(tr("documentation").to_string());
    } else if let Some(lang) = r.language.as_deref().filter(|l| !l.is_empty()) {
        line.push(lang.to_string());
    }
    if let Some(f) = r.files.filter(|f| *f > 0) {
        line.push(fmt("{} files", &[&count(f)]));
    }
    if let Some(when) = ago_of(r.indexed_at.as_deref(), now) {
        line.push(fmt("indexed {}", &[&when]));
    }
    if r.outdated && r.status == "indexed" {
        line.push(tr("new push").to_string());
    }
    let expanded = v.expanded == Some(r.name.as_str());
    let mut detail: Vec<String> = Vec::new();
    if expanded {
        if let Some(org) = r.org.as_deref().filter(|o| !o.is_empty()) {
            detail.push(fmt("org {}", &[&org]));
        }
        if let Some(c) = r.commit.as_deref().filter(|c| !c.is_empty()) {
            detail.push(fmt("commit {}", &[&c.chars().take(8).collect::<String>()]));
        }
        if let Some(when) = ago_of(r.pushed_at.as_deref(), now) {
            detail.push(fmt("pushed {}", &[&when]));
        }
        if let Some(ch) = r.chunks {
            detail.push(fmt("{} chunks", &[&count(ch)]));
        }
        if let Some(s) = r.skipped.filter(|s| *s > 0) {
            detail.push(fmt("{} files skipped", &[&s]));
        }
    }
    let confirm = v
        .confirm
        .and_then(|c| c.split_once(':'))
        .filter(|(_, name)| *name == r.name)
        .map(|(kind, _)| kind.to_string())
        .unwrap_or_default();
    json!({
        "name": r.name,
        "short": short_name(&r.name),
        "status": if r.outdated && r.status == "indexed" { "outdated" } else { r.status.as_str() },
        "line": line.join(" · "),
        "error": r.error.clone().unwrap_or_default(),
        "expanded": expanded,
        "detail": detail.join(" · "),
        "summary": if expanded { r.summary.clone().unwrap_or_default() } else { String::new() },
        "can-index": v.can_sync && r.status != "ignored",
        "can-remove": v.can_admin && r.status != "ignored",
        "can-unignore": v.can_admin && r.status == "ignored",
        "confirm": confirm,
    })
}

/// Repos pushed after their last indexing.
pub fn outdated_count(repos: &[Repo]) -> usize {
    repos.iter().filter(|r| status_matches(r, "outdated")).count()
}

// ------------------------------------------------------------------ sync

/// "Indexing" for the phase key of a running job.
pub fn phase_label(job: &Job) -> String {
    if job.status == "queued" {
        return tr("Waiting for a worker").to_string();
    }
    let p = job.progress.as_ref();
    match p.map(|p| p.phase.as_str()).unwrap_or_default() {
        "discover" => tr("Discovering repositories").to_string(),
        "select" => tr("Choosing the batch").to_string(),
        "fetch" => tr("Downloading").to_string(),
        "index" if p.is_some_and(|p| p.repos_total > 0 && p.repos_done >= p.repos_total) => tr("Checking the other repositories").to_string(),
        "index" => tr("Indexing").to_string(),
        "finish" => tr("Finishing").to_string(),
        _ => tr("Starting").to_string(),
    }
}

/// The live job: `(title, progress 0..1, line)`.
pub fn live_summary(job: &Job) -> (String, f64, String) {
    let empty = Progress::default();
    let p = job.progress.as_ref().unwrap_or(&empty);
    let title = if p.repos_total > 0 {
        fmt("{} · {} of {} repositories", &[&phase_label(job), &p.repos_done, &p.repos_total])
    } else {
        phase_label(job)
    };
    // files are the finer measure; before the count exists, repos
    let progress = if p.files_total > 0 { pct(p.files_done, p.files_total) } else { pct(p.repos_done, p.repos_total) };
    let mut line = Vec::new();
    if p.files_total > 0 {
        line.push(fmt("{} of {} files", &[&count(p.files_done), &count(p.files_total)]));
    }
    if p.seconds > 0.0 {
        line.push(duration(p.seconds as i64));
    }
    (title, progress, line.join(" · "))
}

/// The repos of the live job (`LiveRepo`).
pub fn live_repos(job: &Job) -> Vec<Value> {
    let Some(p) = &job.progress else { return Vec::new() };
    p.repos
        .iter()
        .map(|r| {
            let mut line = match r.files_total {
                Some(t) if t > 0 => fmt("{} of {} files", &[&count(r.files_done), &count(t)]),
                _ => String::new(),
            };
            if r.errors > 0 {
                line = [line, fmt("{} errors", &[&r.errors])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            }
            if let Some(e) = r.error.as_deref().filter(|e| !e.is_empty()) {
                line = e.to_string();
            }
            let done = matches!(r.status.as_str(), "done" | "skipped" | "error");
            let progress = if done { 1.0 } else { pct(r.files_done, r.files_total.unwrap_or(0)) };
            json!({ "name": short_name(&r.name), "status": r.status, "line": line, "pct": progress })
        })
        .collect()
}

/// The sync history (`JobRow`), newest first.
pub fn history_rows(jobs: &[Job], expanded: Option<&str>, now: DateTime<Utc>) -> Vec<Value> {
    jobs.iter()
        .take(MAX_HISTORY)
        .map(|j| {
            let r = j.result.clone().unwrap_or_default();
            let mut title = Vec::new();
            if !r.indexed.is_empty() || j.status == "done" {
                title.push(fmt("{} indexed", &[&r.indexed.len()]));
            }
            if !r.errors.is_empty() {
                title.push(fmt("{} failed", &[&r.errors.len()]));
            }
            if j.active() {
                title.push(phase_label(j));
            }
            let mut line = Vec::new();
            if let Some(when) = ago_of(j.started_at.as_deref(), now) {
                line.push(fmt("started {}", &[&when]));
            }
            if let (Some(a), Some(b)) = (j.started_at.as_deref(), j.finished_at.as_deref())
                && let (Some(sa), Some(sb)) = (age_secs(a, now), age_secs(b, now))
            {
                line.push(fmt("took {}", &[&duration(sa - sb)]));
            }
            if let Some(by) = j.requested_by.as_deref().filter(|b| !b.is_empty()) {
                line.push(fmt("by {}", &[&by]));
            }
            let open = expanded == Some(j.id.as_str());
            let mut errors: Vec<String> = Vec::new();
            if open {
                if let Some(e) = j.error.as_deref().filter(|e| !e.is_empty()) {
                    errors.push(e.to_string());
                }
                let mut repo_errors: Vec<(&String, &String)> = r.errors.iter().collect();
                repo_errors.sort();
                errors.extend(repo_errors.into_iter().map(|(name, e)| format!("{}: {}", short_name(name), e)));
                if errors.is_empty() && !r.indexed.is_empty() {
                    let mut names: Vec<&str> = r.indexed.keys().map(|n| short_name(n)).collect();
                    names.sort();
                    errors.push(fmt("Indexed: {}", &[&names.join(", ")]));
                }
            }
            json!({
                "id": j.id,
                "status": j.status,
                "title": title.join(" · "),
                "line": line.join(" · "),
                "errors": errors.join("\n"),
                "expanded": open,
            })
        })
        .collect()
}

/// "Last sync: 10 indexed · 5 d ago", from the latest finished job.
pub fn last_sync_line(jobs: &[Job], now: DateTime<Utc>) -> String {
    let Some(j) = jobs.iter().find(|j| !j.active()) else { return String::new() };
    let r = j.result.clone().unwrap_or_default();
    let when = ago_of(j.finished_at.as_deref().or(j.started_at.as_deref()), now).unwrap_or_default();
    match j.status.as_str() {
        "failed" => fmt("Last sync failed · {}", &[&when]),
        _ => fmt("Last sync: {} indexed · {}", &[&r.indexed.len(), &when]),
    }
}

/// The dry run: "Next batch: a, b, c +2 · 7 left after it".
pub fn preview_text(selected: &[String], remaining: u64) -> String {
    if selected.is_empty() {
        return tr("Nothing to index: every repository is up to date").to_string();
    }
    let names: Vec<&str> = selected.iter().take(4).map(|n| short_name(n)).collect();
    let more = if selected.len() > names.len() { format!(" +{}", selected.len() - names.len()) } else { String::new() };
    fmt("Next batch ({}): {}{} · {} left after it", &[&selected.len(), &names.join(", "), &more, &remaining])
}

// ------------------------------------------------------------------ health

fn meter(label: String, value: String, progress: f64, tone: &str) -> Value {
    json!({ "label": label, "value": value, "pct": progress, "tone": tone })
}

fn tone(progress: f64) -> &'static str {
    match progress {
        p if p >= 0.9 => "ok",
        p if p >= 0.4 => "warn",
        _ => "alarm",
    }
}

/// The enrichment meters (`Meter`).
pub fn enrich_meters(e: &EnrichStatus) -> Vec<Value> {
    let cards = pct(e.cards_ok, e.repos);
    let sums = pct(e.summaries.fresh, e.summaries.files);
    let impls = pct(e.impls.matched, e.impls.operations);
    let mut out = vec![
        meter(
            tr("Repository cards").to_string(),
            fmt("{} of {} up to date · {} stale", &[&count(e.cards_ok), &count(e.repos), &count(e.cards_stale)]),
            cards,
            tone(cards),
        ),
        meter(
            tr("File summaries").to_string(),
            fmt("{} of {} · {} stale", &[&count(e.summaries.fresh), &count(e.summaries.files), &count(e.summaries.stale)]),
            sums,
            tone(sums),
        ),
    ];
    if e.impls.operations > 0 {
        out.push(meter(
            tr("API operations linked to code").to_string(),
            fmt("{} of {}", &[&count(e.impls.matched), &count(e.impls.operations)]),
            impls,
            tone(impls),
        ));
    }
    out
}

/// The enrichment counters (`Stat`).
pub fn enrich_stats(e: &EnrichStatus) -> Vec<Value> {
    let mut out = vec![
        json!({ "label": tr("Open tasks"), "value": count(e.pending), "tone": if e.pending > 0 { "warn" } else { "ok" } }),
        json!({ "label": tr("Agents working"), "value": e.leased.len().to_string(), "tone": if e.leased.is_empty() { "" } else { "ok" } }),
    ];
    if e.docs.pending_pages > 0 {
        out.push(json!({ "label": tr("Doc pages to review"), "value": count(e.docs.pending_pages), "tone": "" }));
    }
    out
}

/// The usage counters (`Stat`).
pub fn usage_stats(u: &Usage) -> Vec<Value> {
    let t = &u.totals;
    let ms = |v: Option<f64>| v.map(|m| format!("{m:.0} ms")).unwrap_or_else(|| "–".into());
    vec![
        json!({ "label": tr("Calls"), "value": count(t.calls), "tone": "" }),
        json!({ "label": tr("Errors"), "value": count(t.errors), "tone": if t.errors > 0 { "warn" } else { "ok" } }),
        json!({ "label": tr("Average"), "value": ms(t.avg_ms), "tone": "" }),
        json!({ "label": "p95", "value": ms(t.p95_ms), "tone": "" }),
        json!({ "label": tr("Cost"), "value": t.cost_usd.map(|c| format!("US$ {c:.4}")).unwrap_or_else(|| "–".into()), "tone": "" }),
    ]
}

/// One bar per day (`Bar`): height 0..1 against the busiest day, the newest last.
pub fn usage_bars(u: &Usage, days: usize) -> Vec<Value> {
    let daily: Vec<_> = u.daily.iter().rev().take(days).rev().collect();
    let top = daily.iter().map(|d| d.calls()).max().unwrap_or(0);
    daily
        .iter()
        .map(|d| {
            let label = d.day.get(8..10).unwrap_or(&d.day).to_string();
            json!({
                "h": pct(d.calls(), top),
                "err": pct(d.errors, top),
                "label": label,
                "tip": fmt("{}: {} calls, {} errors", &[&d.day, &d.calls(), &d.errors]),
            })
        })
        .collect()
}

/// "token · 2× · 4 results", the most repeated searches.
pub fn top_queries(u: &Usage) -> Vec<Value> {
    u.top_queries
        .iter()
        .filter(|q| !q.query.trim().is_empty())
        .take(6)
        .map(|q| {
            let results = q.avg_results.map(|r| fmt(" · {} results", &[&format!("{r:.0}")])).unwrap_or_default();
            json!(format!("{} · {}×{}", q.query.trim(), q.calls, results))
        })
        .collect()
}

/// "ureq 48 · browser 498".
pub fn clients_line(u: &Usage) -> String {
    let mut c: Vec<_> = u.by_client.iter().filter(|c| c.calls > 0).collect();
    c.sort_by_key(|a| std::cmp::Reverse(a.calls));
    c.iter().take(5).map(|c| format!("{} {}", c.client, count(c.calls))).collect::<Vec<_>>().join(" · ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::api::{RepoProgress, SyncResult};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00+00:00").unwrap().with_timezone(&Utc)
    }

    fn repo(name: &str, status: &str) -> Repo {
        Repo { name: name.into(), status: status.into(), ..Default::default() }
    }

    #[test]
    fn small_texts() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(3 * 86_400), "3 d ago");
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(300), "5 min");
        assert_eq!(short_name("acme__billing-api"), "billing-api");
        assert_eq!(short_name("solo"), "solo");
        assert_eq!(location("acme__api", "src/a.py", 0), "api/src/a.py:1");
        assert_eq!(count(16326), "16\u{202f}326");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000_000), "1\u{202f}000\u{202f}000");
    }

    #[test]
    fn search_rows_and_targets() {
        let hits = [
            Hit { repo: "acme__api".into(), path: "a.py".into(), start_line: 10, end_line: 20, code: "\n    def f():\n        return 1\n\n    x\n    y\n".into(), kind: "code".into(), language: Some("python".into()), score: Some(0.812), ..Default::default() },
            Hit { repo: "docs__wiki".into(), path: "p/x".into(), start_line: 1, kind: "doc".into(), title: Some(" Setup ".into()), ..Default::default() },
        ];
        let (rows, targets) = hit_rows(&hits);
        assert_eq!(rows[0]["title"], "api/a.py:10");
        assert_eq!(rows[0]["snippet"], "def f():\n    return 1");
        assert_eq!(rows[0]["badge"], "python · 81%");
        assert_eq!(rows[1]["snippet"], "Setup");
        assert_eq!(rows[1]["badge"], "documentation");
        assert_eq!(targets[0], Target { repo: "acme__api".into(), path: "a.py".into(), line: 10, end: 20 });
        assert_eq!(targets[0].preview_range(), (8, 26));
        // a huge chunk is capped
        let t = Target { repo: "r".into(), path: "p".into(), line: 2, end: 500 };
        assert_eq!(t.preview_range(), (1, 81));

        let (rows, _) = symbol_rows(&[Symbol { repo: "acme__web".into(), path: "a.ts".into(), line: 199, name: "TokenService".into(), kind: "container".into(), container: Some(String::new()), signature: Some("export interface TokenService {".into()), language: Some("typescript".into()) }]);
        assert_eq!(rows[0]["sub"], "container · typescript");
        assert_eq!(rows[0]["snippet"], "export interface TokenService {");
        let (rows, targets) = grep_rows(&[GrepMatch { repo: "acme__a".into(), path: "b".into(), line: 26, text: "  x()  ".into() }]);
        assert_eq!(rows[0]["snippet"], "x()");
        assert_eq!(targets[0].end, 26);
    }

    #[test]
    fn preview_marks_the_hit() {
        let text = "    9  a\n   10  \tb\n   11  c\n… mostrando linhas 9-11 de 99";
        assert_eq!(mark_hit(text, (10, 10)), "     9  a\n▸   10      b\n    11  c\n … mostrando linhas 9-11 de 99");
        assert_eq!(mark_hit("", (1, 1)), "");
    }

    #[test]
    fn repo_list_filters_sorts_and_caps() {
        let mut a = repo("acme__alpha", "indexed");
        a.outdated = true;
        a.files = Some(1200);
        a.language = Some("python".into());
        a.indexed_at = Some("2026-11-14T10:00:00+00:00".into());
        let mut b = repo("acme__beta", "error");
        b.error = Some("clone failed".into());
        let repos = vec![a, b, repo("acme__gamma", "pending"), repo("acme__delta", "indexed"), repo("acme__omega", "ignored")];
        let v = RepoView { status: "all", can_sync: true, can_admin: true, ..Default::default() };
        let (rows, note) = repo_rows(&repos, &v, now());
        let names: Vec<&str> = rows.iter().map(|r| r["short"].as_str().unwrap()).collect();
        // errors, pending, pushed after indexing, indexed; ignored hidden under "all"
        assert_eq!(names, ["beta", "gamma", "alpha", "delta"]);
        assert_eq!(note, "4 repositories");
        assert_eq!(rows[0]["error"], "clone failed");
        assert_eq!(rows[2]["status"], "outdated");
        assert_eq!(rows[2]["line"], "python · 1\u{202f}200 files · indexed 2 h ago · new push");
        // filter by text and by status
        let (rows, _) = repo_rows(&repos, &RepoView { filter: "OMEGA", status: "ignored", can_admin: true, ..Default::default() }, now());
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0]["can-unignore"].as_bool(), rows[0]["can-remove"].as_bool(), rows[0]["can-index"].as_bool()), (Some(true), Some(false), Some(false)));
        let (rows, note) = repo_rows(&repos, &RepoView { filter: "zzz", status: "all", ..Default::default() }, now());
        assert!(rows.is_empty());
        assert_eq!(note, "No repository matches");
        assert_eq!(outdated_count(&repos), 1);
        // more than the cap
        let many: Vec<Repo> = (0..200).map(|i| repo(&format!("r{i:03}"), "indexed")).collect();
        let (rows, note) = repo_rows(&many, &RepoView { status: "all", ..Default::default() }, now());
        assert_eq!(rows.len(), MAX_REPO_ROWS);
        assert_eq!(note, "Showing 150 of 200: narrow the filter");
    }

    #[test]
    fn expanded_repo_has_details_and_its_confirmation() {
        let mut r = repo("acme__api", "indexed");
        r.org = Some("acme".into());
        r.commit = Some("abcdef1234567".into());
        r.chunks = Some(900);
        r.skipped = Some(2);
        r.summary = Some("Bills customers.".into());
        let v = RepoView { status: "all", expanded: Some("acme__api"), confirm: Some("remove:acme__api"), can_admin: true, ..Default::default() };
        let (rows, _) = repo_rows(&[r.clone()], &v, now());
        assert_eq!(rows[0]["detail"], "org acme · commit abcdef12 · 900 chunks · 2 files skipped");
        assert_eq!(rows[0]["summary"], "Bills customers.");
        assert_eq!(rows[0]["confirm"], "remove");
        // collapsed: no details, and a confirmation for another repo is not this one's
        let v = RepoView { status: "all", confirm: Some("remove:acme__other"), ..Default::default() };
        let (rows, _) = repo_rows(&[r], &v, now());
        assert_eq!((rows[0]["detail"].as_str(), rows[0]["summary"].as_str(), rows[0]["confirm"].as_str()), (Some(""), Some(""), Some("")));
    }

    fn running_job() -> Job {
        Job {
            id: "j2".into(),
            status: "running".into(),
            progress: Some(Progress {
                phase: "index".into(),
                repos_total: 2,
                repos_done: 1,
                files_total: 50,
                files_done: 20,
                seconds: 75.0,
                repos: vec![
                    RepoProgress { name: "acme__web".into(), status: "done".into(), files_total: Some(10), files_done: 10, ..Default::default() },
                    RepoProgress { name: "acme__api".into(), status: "indexing".into(), files_total: Some(40), files_done: 10, errors: 1, ..Default::default() },
                    RepoProgress { name: "acme__bad".into(), status: "error".into(), error: Some("clone failed".into()), ..Default::default() },
                ],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn live_job() {
        let j = running_job();
        let (title, progress, line) = live_summary(&j);
        assert_eq!(title, "Indexing · 1 of 2 repositories");
        assert!((progress - 0.4).abs() < 1e-9);
        assert_eq!(line, "20 of 50 files · 1 min");
        let repos = live_repos(&j);
        assert_eq!(repos[0]["pct"], 1.0);
        assert_eq!(repos[1]["line"], "10 of 40 files · 1 errors");
        assert_eq!(repos[1]["pct"], 0.25);
        assert_eq!(repos[2]["line"], "clone failed");
        // every repo done but still in the index phase: the server checks the rest
        let mut j2 = j.clone();
        j2.progress.as_mut().unwrap().repos_done = 2;
        assert_eq!(phase_label(&j2), "Checking the other repositories");
        assert_eq!(phase_label(&Job { status: "queued".into(), ..Default::default() }), "Waiting for a worker");
        assert_eq!(live_summary(&Job { status: "running".into(), ..Default::default() }).0, "Starting");
    }

    #[test]
    fn history_and_last_sync() {
        let done = Job {
            id: "j1".into(),
            status: "done".into(),
            requested_by: Some("me@x".into()),
            started_at: Some("2026-11-14T10:00:00+00:00".into()),
            finished_at: Some("2026-11-14T10:05:00+00:00".into()),
            result: Some(SyncResult {
                indexed: HashMap::from([("acme__b".to_string(), None), ("acme__a".to_string(), Some("x".to_string()))]),
                errors: HashMap::from([("acme__c".to_string(), "boom".to_string())]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let failed = Job { id: "j0".into(), status: "failed".into(), error: Some("all failed".into()), finished_at: Some("2026-11-13T12:00:00+00:00".into()), ..Default::default() };
        let jobs = [running_job(), done.clone(), failed];
        let rows = history_rows(&jobs, Some("j1"), now());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["title"], "Indexing");
        assert_eq!(rows[1]["title"], "2 indexed · 1 failed");
        assert_eq!(rows[1]["line"], "started 2 h ago · took 5 min · by me@x");
        assert_eq!(rows[1]["errors"], "c: boom");
        assert_eq!(rows[2]["errors"], "", "collapsed rows carry no errors");
        let rows = history_rows(&jobs, Some("j0"), now());
        assert_eq!(rows[2]["errors"], "all failed");
        assert_eq!(last_sync_line(&jobs, now()), "Last sync: 2 indexed · 1 h ago");
        assert_eq!(last_sync_line(&jobs[2..], now()), "Last sync failed · 1 d ago");
        assert_eq!(last_sync_line(&[], now()), "");
        // a successful job without errors lists what it indexed when opened
        let mut ok = done;
        ok.result.as_mut().unwrap().errors.clear();
        assert_eq!(history_rows(&[ok], Some("j1"), now())[0]["errors"], "Indexed: a, b");
    }

    #[test]
    fn dry_run_preview() {
        let names: Vec<String> = ["acme__a", "acme__b", "acme__c", "acme__d", "acme__e", "acme__f"].map(String::from).to_vec();
        assert_eq!(preview_text(&names, 7), "Next batch (6): a, b, c, d +2 · 7 left after it");
        assert_eq!(preview_text(&names[..1], 0), "Next batch (1): a · 0 left after it");
        assert_eq!(preview_text(&[], 0), "Nothing to index: every repository is up to date");
    }

    #[test]
    fn health_meters_and_usage() {
        let e: EnrichStatus = crate::api::parse(r#"{"repos":263,"cards_ok":5,"cards_stale":1,"pending":258,"leased":[{}],"impls":{"operations":100,"matched":95},"docs":{"pending_pages":3},"summaries":{"files":1000,"fresh":500,"stale":2}}"#).unwrap();
        let m = enrich_meters(&e);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["value"], "5 of 263 up to date · 1 stale");
        assert_eq!(m[0]["tone"], "alarm");
        assert_eq!(m[1]["tone"], "warn");
        assert_eq!(m[2]["tone"], "ok");
        let s = enrich_stats(&e);
        assert_eq!((s[0]["value"].as_str(), s[1]["value"].as_str(), s.len()), (Some("258"), Some("1"), 3));

        let u: Usage = crate::api::parse(r#"{"days":7,"totals":{"calls":10,"errors":1,"avg_ms":197.6,"p95_ms":null,"cost_usd":0.0029},
          "daily":[{"day":"2026-11-12","api":2},{"day":"2026-11-13","api":0},{"day":"2026-11-14","api":3,"ui":1,"errors":1}],
          "top_queries":[{"query":"token","calls":2,"avg_results":4.0},{"query":"  ","calls":9}],
          "by_client":[{"client":"ureq","calls":48},{"client":"browser","calls":498},{"client":"x","calls":0}]}"#).unwrap();
        let stats = usage_stats(&u);
        assert_eq!(stats[2]["value"], "198 ms");
        assert_eq!(stats[3]["value"], "–");
        assert_eq!(stats[4]["value"], "US$ 0.0029");
        let bars = usage_bars(&u, 2);
        assert_eq!(bars.len(), 2);
        assert_eq!((bars[0]["h"].as_f64(), bars[1]["h"].as_f64()), (Some(0.0), Some(1.0)));
        assert_eq!(bars[1]["label"], "14");
        assert_eq!(bars[1]["err"], 0.25);
        assert_eq!(top_queries(&u), vec![json!("token · 2× · 4 results")]);
        assert_eq!(clients_line(&u), "browser 498 · ureq 48");
    }
}
