//! Code index: a MiniUtils external app for a code-index server (database-index): status,
//! search (hybrid, symbol, grep) with a file preview, the repository list with its actions,
//! live sync jobs and the enrichment and usage panel. Hub protocol: `miniutils-app`; server
//! API: `api.rs`; what the view shows: `views.rs`.
//!
//! Threads: the protocol loop (this file's `main`) never touches the network. Every call
//! to the server runs on a short-lived thread that answers through the same channel as
//! the hub's messages (`Msg`), so one loop owns all the state. While a sync job runs, the
//! loop asks for the jobs every few seconds; otherwise the hub's tick (30 s) drives it.

mod api;
mod config;
mod i18n;
mod state;
mod views;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use chrono::Utc;
use miniutils_app::{self as hub, Fact, HostMsg, Mood, os};
use serde_json::{Value, json};

use api::{ApiError, Client, EnrichStatus, Job, Me, Repo, SearchFilters, StartedJob, SyncResult, Usage};
use config::Config;
use i18n::{fmt, tr};
use state::{Denied, Event, Health, Snapshot};
use views::{RepoView, Target};

/// Results asked of the server when the setting is missing.
const DEFAULT_RESULTS: u32 = 10;
/// How often the jobs are read while one runs.
const FAST_POLL: Duration = Duration::from_secs(3);
/// The usage panel is read again after this long (only while it is open).
const USAGE_REFRESH: Duration = Duration::from_secs(120);
/// A two-click action forgets its first click after this long.
const CONFIRM_TTL: Duration = Duration::from_secs(8);
/// Polls in a row that may fail (timeout, server restarting) before the view drops the
/// data it has and shows the server as down.
const TOLERATED_FAILURES: u32 = 2;
/// Jobs read on every poll (the history shows the first `views::MAX_HISTORY`).
const JOBS_LIMIT: u32 = 10;

/// The app's manifest settings that are not about the connection (`config.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    expect_server: bool,
    notify_sync: bool,
    results: u32,
}

impl Settings {
    fn from(v: &Value) -> Settings {
        Settings {
            expect_server: v["expect_server"].as_bool().unwrap_or(false),
            notify_sync: v["notify_sync"].as_bool().unwrap_or(true),
            results: v["results"].as_u64().map_or(DEFAULT_RESULTS, |n| n.clamp(5, 50) as u32),
        }
    }
}

/// Everything one poll learns.
struct Poll {
    health: Health,
    me: Option<Me>,
    repos: Vec<Repo>,
    /// `None` when the token cannot read jobs.
    jobs: Option<Vec<Job>>,
    enrich: Option<EnrichStatus>,
    /// The server follows GitHub orgs (refreshing them makes sense).
    has_orgs: bool,
}

/// What a search found, already as rows.
struct Found {
    rows: Vec<Value>,
    targets: Vec<Target>,
    truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoAct {
    Index,
    Remove,
    Unignore,
}

impl RepoAct {
    fn parse(s: &str) -> Option<RepoAct> {
        match s {
            "index" => Some(RepoAct::Index),
            "remove" => Some(RepoAct::Remove),
            "unignore" => Some(RepoAct::Unignore),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            RepoAct::Index => "index",
            RepoAct::Remove => "remove",
            RepoAct::Unignore => "unignore",
        }
    }
}

enum Msg {
    Host(HostMsg),
    Polled(Box<Poll>),
    Jobs(Result<Vec<Job>, ApiError>),
    Usage(u32, Result<Usage, ApiError>),
    Searched { seq: u64, result: Result<Found, ApiError> },
    Previewed { seq: u64, target: Target, result: Result<String, ApiError> },
    SyncPreviewed(Result<SyncResult, ApiError>),
    SyncStarted(Result<StartedJob, ApiError>),
    RepoDone { act: RepoAct, name: String, result: Result<(), ApiError> },
    OrgsRefreshed(Result<Value, ApiError>),
}

/// Where the sync button is: the strings are the view's `sync-state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncStep {
    Idle,
    Checking,
    Confirm,
    Starting,
}

impl SyncStep {
    fn as_str(self) -> &'static str {
        match self {
            SyncStep::Idle => "idle",
            SyncStep::Checking => "checking",
            SyncStep::Confirm => "confirm",
            SyncStep::Starting => "starting",
        }
    }
}

#[derive(Default)]
struct SearchState {
    /// `search` (hybrid), `symbol` or `grep`.
    mode: String,
    query: String,
    /// `all`, `code` or `doc` (hybrid search only).
    kind: String,
    /// Repos typed in the filter field, as typed.
    repos: String,
    regex: bool,
    case: bool,
    /// Bumped per search and per preview: a late answer to an older one is dropped.
    seq: u64,
    searching: bool,
    note: String,
    rows: Vec<Value>,
    targets: Vec<Target>,
    selected: Option<usize>,
    preview_seq: u64,
    preview_loading: bool,
    preview_title: String,
    preview_text: String,
    preview_note: String,
}

struct App {
    tx: Sender<Msg>,
    cfg: Config,
    settings: Settings,
    health: Option<Health>,
    me: Me,
    repos: Vec<Repo>,
    jobs: Vec<Job>,
    enrich: Option<EnrichStatus>,
    has_orgs: bool,
    usage: Option<Usage>,
    usage_days: u32,
    usage_at: Option<Instant>,
    usage_loading: bool,
    /// The last snapshot that went through `state::transitions`.
    prev: Option<Snapshot>,
    polling: bool,
    jobs_polling: bool,
    /// Polls in a row that did not reach the server.
    failures: u32,
    /// When the data on screen was last refreshed, while polls fail.
    stale_since: Option<Instant>,
    last_fast: Instant,
    tab: String,
    search: SearchState,
    repo_filter: String,
    repo_status: String,
    repo_expanded: Option<String>,
    /// A two-click action waiting for its second click: `index:<name>`, `remove:<name>`,
    /// `unignore:<name>`, `orgs`.
    confirm: Option<(String, Instant)>,
    repo_note: String,
    /// Batch size of the next sync; 0 is the server's default.
    batch: u32,
    sync_step: SyncStep,
    sync_preview: String,
    sync_note: String,
    job_expanded: Option<String>,
    org_note: String,
    /// What the hub already has in each view property (only changes are sent).
    props: HashMap<&'static str, Value>,
    error: String,
    sent_mood: Option<Mood>,
    sent_summary: String,
}

impl App {
    fn new(tx: Sender<Msg>) -> App {
        App {
            tx,
            cfg: Config { url: config::DEFAULT_URL.into(), token: String::new(), repos_dir: None },
            settings: Settings::from(&Value::Null),
            health: None,
            me: Me::default(),
            repos: Vec::new(),
            jobs: Vec::new(),
            enrich: None,
            has_orgs: false,
            usage: None,
            usage_days: 7,
            usage_at: None,
            usage_loading: false,
            prev: None,
            polling: false,
            jobs_polling: false,
            failures: 0,
            stale_since: None,
            last_fast: Instant::now(),
            tab: "search".into(),
            search: SearchState { mode: "search".into(), kind: "all".into(), ..Default::default() },
            repo_filter: String::new(),
            repo_status: "all".into(),
            repo_expanded: None,
            confirm: None,
            repo_note: String::new(),
            batch: 0,
            sync_step: SyncStep::Idle,
            sync_preview: String::new(),
            sync_note: String::new(),
            job_expanded: None,
            org_note: String::new(),
            props: HashMap::new(),
            error: String::new(),
            sent_mood: None,
            sent_summary: String::new(),
        }
    }

    fn set(&mut self, prop: &'static str, value: Value) {
        if self.props.get(prop) != Some(&value) {
            hub::set(prop, value.clone());
            self.props.insert(prop, value);
        }
    }

    /// Runs `work` on its own thread and delivers its answer to the loop.
    fn spawn(&self, work: impl FnOnce(&Client) -> Msg + Send + 'static) {
        let (cfg, tx) = (self.cfg.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work(&Client::new(&cfg)));
        });
    }

    fn online(&self) -> bool {
        matches!(self.health, Some(Health::Online(_)))
    }

    fn job_active(&self) -> bool {
        self.jobs.first().is_some_and(Job::active)
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(HostMsg::Start(s)) => {
                i18n::init(&s.lang);
                self.settings = Settings::from(&s.settings);
                self.cfg = Config::load(&s.settings, &|k| std::env::var(k).ok());
                eprintln!("code-index app started: {:?}", self.cfg);
                self.set("subtitle", json!(tr("starting…")));
                self.render();
                self.poll();
            }
            Msg::Host(HostMsg::Tick) => self.poll(),
            Msg::Host(HostMsg::Shown) => {
                self.poll();
                if self.tab == "health" {
                    self.load_usage(false);
                }
            }
            Msg::Host(HostMsg::Settings { settings }) => {
                self.settings = Settings::from(&settings);
                let cfg = Config::load(&settings, &|k| std::env::var(k).ok());
                if cfg != self.cfg {
                    // another server or token: what is on screen belongs to the old one
                    eprintln!("connection settings changed: {cfg:?}");
                    self.cfg = cfg;
                    self.reset_server_data();
                    self.poll();
                }
                self.render();
            }
            Msg::Host(HostMsg::Action { name, arg }) => self.action(&name, &arg),
            Msg::Host(HostMsg::Event { .. } | HostMsg::Stop) => {}
            Msg::Polled(poll) => self.polled(*poll),
            Msg::Jobs(result) => self.jobs_polled(result),
            Msg::Usage(days, result) => self.usage_loaded(days, result),
            Msg::Searched { seq, result } => self.searched(seq, result),
            Msg::Previewed { seq, target, result } => self.previewed(seq, &target, result),
            Msg::SyncPreviewed(result) => self.sync_previewed(result),
            Msg::SyncStarted(result) => self.sync_started(result),
            Msg::RepoDone { act, name, result } => self.repo_done(act, &name, result),
            Msg::OrgsRefreshed(result) => self.orgs_refreshed(result),
        }
    }

    fn reset_server_data(&mut self) {
        self.health = None;
        self.me = Me::default();
        self.repos.clear();
        self.jobs.clear();
        self.enrich = None;
        self.usage = None;
        self.usage_at = None;
        self.prev = None;
        self.search.rows.clear();
        self.search.targets.clear();
        self.search.selected = None;
        self.search.note.clear();
        self.search.preview_text.clear();
        self.search.preview_title.clear();
        self.sync_step = SyncStep::Idle;
        self.sync_preview.clear();
    }

    // ---------------------------------------------------------------- polling

    fn poll(&mut self) {
        if !self.cfg.configured() {
            self.polled(Poll { health: Health::NotConfigured, me: None, repos: Vec::new(), jobs: None, enrich: None, has_orgs: false });
            return;
        }
        if self.polling {
            return;
        }
        self.polling = true;
        self.spawn(|c| Msg::Polled(Box::new(poll_server(c))));
    }

    fn polled(&mut self, poll: Poll) {
        self.polling = false;
        if let Some(me) = poll.me {
            self.me = me;
        }
        // a slow or restarting server: keep what is on screen for a couple of polls
        let transient = matches!(poll.health, Health::Offline(_) | Health::Failed(_));
        if transient && self.online() && self.failures < TOLERATED_FAILURES {
            self.failures += 1;
            self.stale_since.get_or_insert_with(Instant::now);
            eprintln!("poll failed ({}), keeping the last data: {:?}", self.failures, poll.health);
            self.render();
            return;
        }
        if let Health::Online(_) = &poll.health {
            self.failures = 0;
            self.stale_since = None;
            self.repos = poll.repos;
            if let Some(jobs) = poll.jobs {
                self.jobs = jobs;
            }
            self.enrich = poll.enrich;
            self.has_orgs = poll.has_orgs;
            self.health = Some(poll.health);
            self.check_transitions();
        } else {
            self.failures += 1;
            self.stale_since = None;
            self.health = Some(poll.health);
        }
        self.render();
    }

    /// Fast path while a job runs: only the jobs.
    fn fast_tick(&mut self) {
        if self.last_fast.elapsed() < FAST_POLL {
            return;
        }
        self.last_fast = Instant::now();
        if let Some((_, at)) = &self.confirm
            && at.elapsed() > CONFIRM_TTL
        {
            self.confirm = None;
            self.render();
        }
        let watching = self.job_active() || self.sync_step == SyncStep::Starting;
        if !watching || self.jobs_polling || !self.online() || !self.me.can(api::SCOPE_INDEX) {
            return;
        }
        self.jobs_polling = true;
        self.spawn(|c| Msg::Jobs(c.jobs(JOBS_LIMIT)));
    }

    fn jobs_polled(&mut self, result: Result<Vec<Job>, ApiError>) {
        self.jobs_polling = false;
        let Ok(jobs) = result else { return };
        let was_active = self.job_active();
        self.jobs = jobs;
        self.check_transitions();
        if was_active && !self.job_active() {
            // the job ended: the repo list changed too
            self.poll();
        }
        self.render();
    }

    /// Facts and notifications from what changed since the last look.
    fn check_transitions(&mut self) {
        let now = Snapshot::build(&self.repos, &self.jobs);
        for event in state::transitions(self.prev.as_ref(), &now, self.settings.notify_sync) {
            emit(&event);
        }
        self.health = Some(Health::Online(now.clone()));
        self.prev = Some(now);
    }

    fn load_usage(&mut self, force: bool) {
        if self.usage_loading || !self.online() {
            return;
        }
        let fresh = self.usage_at.is_some_and(|t| t.elapsed() < USAGE_REFRESH);
        if fresh && !force {
            return;
        }
        self.usage_loading = true;
        let days = self.usage_days;
        self.spawn(move |c| Msg::Usage(days, c.usage(days)));
        self.render();
    }

    fn usage_loaded(&mut self, days: u32, result: Result<Usage, ApiError>) {
        self.usage_loading = false;
        if days != self.usage_days {
            // the user switched the period meanwhile
            self.load_usage(true);
            return;
        }
        match result {
            Ok(u) => {
                self.usage = Some(u);
                self.usage_at = Some(Instant::now());
            }
            Err(e) => eprintln!("usage unavailable: {e}"),
        }
        self.render();
    }

    // ---------------------------------------------------------------- actions

    fn action(&mut self, name: &str, arg: &str) {
        match name {
            "tab" => {
                self.tab = arg.to_string();
                match arg {
                    "health" => self.load_usage(false),
                    "sync" | "repos" => self.poll(),
                    _ => {}
                }
            }
            "refresh" => {
                self.poll();
                if self.tab == "health" {
                    self.load_usage(true);
                }
            }
            "web" => self.open_web(arg),
            // search
            "search-mode" => {
                if self.search.mode != arg {
                    self.search.mode = arg.to_string();
                    self.clear_results();
                }
            }
            "search-kind" => self.search.kind = arg.to_string(),
            "search-repos" => self.search.repos = arg.to_string(),
            "grep-regex" => self.search.regex = !self.search.regex,
            "grep-case" => self.search.case = !self.search.case,
            "search" => self.run_search(arg),
            "select" => self.select(arg.parse().ok()),
            "open-editor" => self.open_selected(),
            // repos
            "repo-filter" => self.repo_filter = arg.to_string(),
            "repo-status" => {
                self.repo_status = arg.to_string();
                self.repo_expanded = None;
            }
            "repo-toggle" => {
                self.repo_expanded = if self.repo_expanded.as_deref() == Some(arg) { None } else { Some(arg.to_string()) };
                self.confirm = None;
            }
            "repo-act" => self.repo_act(arg),
            "repo-search" => {
                // search only this repo
                self.search.repos = views::short_name(arg).to_string();
                self.tab = "search".into();
                self.clear_results();
            }
            // sync
            "batch" => {
                let delta: i64 = arg.parse().unwrap_or(0);
                self.batch = (self.batch as i64 + delta).clamp(0, 50) as u32;
                self.cancel_sync_preview();
            }
            "sync-preview" => self.sync_preview_start(),
            "sync-confirm" => self.sync_confirm(),
            "sync-cancel" => self.cancel_sync_preview(),
            "job-toggle" => {
                self.job_expanded = if self.job_expanded.as_deref() == Some(arg) { None } else { Some(arg.to_string()) };
            }
            "orgs-refresh" => self.orgs_refresh(),
            // health
            "usage-days" => {
                let days = arg.parse().unwrap_or(7);
                if days != self.usage_days {
                    self.usage_days = days;
                    self.load_usage(true);
                }
            }
            other => eprintln!("unknown action {other}"),
        }
        self.render();
    }

    fn open_web(&self, hash: &str) {
        let url = if hash.is_empty() { format!("{}/ui/", self.cfg.url) } else { format!("{}/ui/#{hash}", self.cfg.url) };
        if let Err(e) = os::open_path(Path::new(&url)) {
            eprintln!("could not open the web UI: {e}");
        }
    }

    // ---------------------------------------------------------------- search

    fn clear_results(&mut self) {
        let s = &mut self.search;
        s.seq += 1;
        s.searching = false;
        s.rows.clear();
        s.targets.clear();
        s.selected = None;
        s.note.clear();
        s.preview_text.clear();
        s.preview_title.clear();
        s.preview_note.clear();
        s.preview_loading = false;
    }

    /// Full repo names for what the user typed in the filter: exact names or short names
    /// first, otherwise every repo whose name contains the word.
    fn resolve_repos(&self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for word in self.search.repos.split([',', ' ']).map(str::trim).filter(|w| !w.is_empty()) {
            let w = word.to_lowercase();
            let exact: Vec<&Repo> = self.repos.iter().filter(|r| r.name.to_lowercase() == w || views::short_name(&r.name).to_lowercase() == w).collect();
            let found: Vec<&Repo> = if exact.is_empty() { self.repos.iter().filter(|r| r.name.to_lowercase().contains(&w)).collect() } else { exact };
            if found.is_empty() {
                return Err(fmt("No repository named “{}”", &[&word]));
            }
            out.extend(found.into_iter().take(50).map(|r| r.name.clone()));
        }
        out.dedup();
        Ok(out)
    }

    fn run_search(&mut self, query: &str) {
        let query = query.trim().to_string();
        self.search.query = query.clone();
        if query.is_empty() || !self.online() {
            return;
        }
        let repos = match self.resolve_repos() {
            Ok(r) => r,
            Err(note) => {
                self.clear_results();
                self.search.note = note;
                return;
            }
        };
        self.clear_results();
        self.search.searching = true;
        self.search.note = tr("Searching…").to_string();
        let seq = self.search.seq;
        let limit = self.settings.results;
        let (mode, kind, regex, case) = (self.search.mode.clone(), self.search.kind.clone(), self.search.regex, self.search.case);
        self.spawn(move |c| {
            let result = match mode.as_str() {
                "symbol" => c.symbols(&query, &repos, limit).map(|s| {
                    let (rows, targets) = views::symbol_rows(&s);
                    Found { rows, targets, truncated: false }
                }),
                "grep" => c.grep(&query, regex, case, &repos, limit.max(20)).map(|g| {
                    let (rows, targets) = views::grep_rows(&g.matches);
                    Found { rows, targets, truncated: g.truncated }
                }),
                _ => {
                    let kind = match kind.as_str() {
                        "code" => Some("code"),
                        "doc" => Some("doc"),
                        _ => None,
                    };
                    c.search(&query, limit, &SearchFilters { kind, repos }).map(|h| {
                        let (rows, targets) = views::hit_rows(&h);
                        Found { rows, targets, truncated: false }
                    })
                }
            };
            Msg::Searched { seq, result }
        });
    }

    fn searched(&mut self, seq: u64, result: Result<Found, ApiError>) {
        if seq != self.search.seq {
            return;
        }
        self.search.searching = false;
        match result {
            Ok(found) => {
                self.search.note = match (found.rows.len(), found.truncated) {
                    (0, _) => fmt("No results for “{}”", &[&self.search.query]),
                    (n, true) => fmt("{} results (more exist: narrow the search)", &[&n]),
                    (n, false) => fmt("{} results · click one to see the file", &[&n]),
                };
                self.search.rows = found.rows;
                self.search.targets = found.targets;
                if !self.search.targets.is_empty() {
                    self.select(Some(0));
                }
            }
            Err(e) => self.search.note = action_error(&e),
        }
        self.render();
    }

    fn select(&mut self, idx: Option<usize>) {
        let Some(target) = idx.and_then(|i| self.search.targets.get(i)).cloned() else { return };
        self.search.selected = idx;
        self.search.preview_seq += 1;
        self.search.preview_loading = true;
        self.search.preview_title = views::location(&target.repo, &target.path, target.line);
        self.search.preview_note.clear();
        let seq = self.search.preview_seq;
        let (first, last) = target.preview_range();
        self.spawn(move |c| {
            let result = c.file(&target.repo, &target.path, first, last);
            Msg::Previewed { seq, target, result }
        });
    }

    fn previewed(&mut self, seq: u64, target: &Target, result: Result<String, ApiError>) {
        if seq != self.search.preview_seq {
            return;
        }
        self.search.preview_loading = false;
        match result {
            Ok(text) => {
                self.search.preview_text = views::mark_hit(&text, (target.line, target.end));
                self.search.preview_note.clear();
            }
            Err(e) => {
                self.search.preview_text.clear();
                self.search.preview_note = action_error(&e);
            }
        }
        self.render();
    }

    fn selected_file(&self) -> Option<(PathBuf, u64)> {
        let t = self.search.selected.and_then(|i| self.search.targets.get(i))?;
        clone_file(self.cfg.repos_dir.as_deref(), &t.repo, &t.path).map(|f| (f, t.line))
    }

    fn open_selected(&mut self) {
        match self.selected_file() {
            Some((file, line)) => open_in_editor(&file, line),
            None => self.search.preview_note = tr("The file is not in the clones folder (Settings › Apps)").to_string(),
        }
    }

    // ---------------------------------------------------------------- repos

    /// Two clicks: the first arms `key`, the second (within `CONFIRM_TTL`) returns true.
    fn confirmed(&mut self, key: &str) -> bool {
        let armed = self.confirm.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < CONFIRM_TTL);
        self.confirm = if armed { None } else { Some((key.to_string(), Instant::now())) };
        armed
    }

    fn repo_act(&mut self, arg: &str) {
        let Some((act, name)) = arg.split_once(':').and_then(|(a, n)| RepoAct::parse(a).map(|a| (a, n.to_string()))) else { return };
        if !self.confirmed(&format!("{}:{name}", act.key())) {
            self.repo_note.clear();
            return;
        }
        self.repo_note = fmt("Working on {}…", &[&views::short_name(&name)]);
        let n = name.clone();
        self.spawn(move |c| {
            let result = match act {
                RepoAct::Index => c.sync_start(std::slice::from_ref(&n), None).map(|_| ()),
                RepoAct::Remove => c.remove_repo(&n),
                RepoAct::Unignore => c.unignore_repo(&n),
            };
            Msg::RepoDone { act, name: n, result }
        });
    }

    fn repo_done(&mut self, act: RepoAct, name: &str, result: Result<(), ApiError>) {
        let short = views::short_name(name);
        self.repo_note = match (&result, act) {
            (Ok(()), RepoAct::Index) => fmt("Indexing {} started (see Sync)", &[&short]),
            (Ok(()), RepoAct::Remove) => fmt("{} left the index", &[&short]),
            (Ok(()), RepoAct::Unignore) => fmt("{} is back in the queue", &[&short]),
            (Err(e), _) => action_error(e),
        };
        if result.is_ok() {
            if act == RepoAct::Index {
                self.sync_step = SyncStep::Starting;
            }
            self.poll();
        }
        self.render();
    }

    // ---------------------------------------------------------------- sync

    fn batch(&self) -> Option<u32> {
        (self.batch > 0).then_some(self.batch)
    }

    fn cancel_sync_preview(&mut self) {
        if self.sync_step != SyncStep::Starting {
            self.sync_step = SyncStep::Idle;
        }
        self.sync_preview.clear();
    }

    /// First click on "Index next batch": asks the server what it would do (dry run).
    fn sync_preview_start(&mut self) {
        if self.sync_step != SyncStep::Idle || !self.online() {
            return;
        }
        self.sync_step = SyncStep::Checking;
        self.sync_note.clear();
        let batch = self.batch();
        self.spawn(move |c| Msg::SyncPreviewed(c.sync_preview(&[], batch)));
    }

    fn sync_previewed(&mut self, result: Result<SyncResult, ApiError>) {
        if self.sync_step != SyncStep::Checking {
            return;
        }
        match result {
            Ok(dry) if dry.selected.is_empty() => {
                self.sync_step = SyncStep::Idle;
                self.sync_note = views::preview_text(&[], 0);
            }
            Ok(dry) => {
                self.sync_step = SyncStep::Confirm;
                self.sync_preview = views::preview_text(&dry.selected, dry.remaining);
            }
            Err(e) => {
                self.sync_step = SyncStep::Idle;
                self.sync_note = action_error(&e);
            }
        }
        self.render();
    }

    fn sync_confirm(&mut self) {
        if self.sync_step != SyncStep::Confirm {
            return;
        }
        self.sync_step = SyncStep::Starting;
        self.sync_preview.clear();
        let batch = self.batch();
        self.spawn(move |c| Msg::SyncStarted(c.sync_start(&[], batch)));
    }

    fn sync_started(&mut self, result: Result<StartedJob, ApiError>) {
        match result {
            Ok(job) => {
                eprintln!("sync job {} started", job.job_id);
                self.sync_note = tr("Sync started").to_string();
                self.last_fast = Instant::now() - FAST_POLL;
            }
            Err(e) => self.sync_note = action_error(&e),
        }
        self.sync_step = SyncStep::Idle;
        self.poll();
        self.render();
    }

    fn orgs_refresh(&mut self) {
        if !self.confirmed("orgs") {
            self.org_note.clear();
            return;
        }
        self.org_note = tr("Looking for new repositories…").to_string();
        self.spawn(|c| Msg::OrgsRefreshed(c.refresh_orgs()));
    }

    fn orgs_refreshed(&mut self, result: Result<Value, ApiError>) {
        self.org_note = match result {
            Ok(v) => org_refresh_note(&v),
            Err(e) => action_error(&e),
        };
        self.poll();
        self.render();
    }

    // ---------------------------------------------------------------- rendering

    /// Sends everything the view shows (only what changed reaches the hub).
    fn render(&mut self) {
        let Some(health) = self.health.clone() else { return };
        let mood = state::mood(&health, self.settings.expect_server);
        if self.sent_mood != Some(mood) {
            hub::mood(mood);
            self.sent_mood = Some(mood);
        }
        let (server, subtitle, notice, error) = describe(&health, &self.cfg.url, self.settings.expect_server);
        if self.sent_summary != subtitle {
            hub::summary(&subtitle);
            self.sent_summary.clone_from(&subtitle);
        }
        self.send_error(&error);
        let now = Utc::now();
        let online = matches!(health, Health::Online(_));

        // header and counters
        self.set("server-state", json!(server));
        self.set("subtitle", json!(subtitle));
        self.set("notice", json!(notice));
        self.set("tab", json!(self.tab));
        let warning = match self.stale_since {
            Some(t) => fmt("The server is not answering (busy or restarting): data from {}", &[&views::ago(t.elapsed().as_secs() as i64)]),
            None => String::new(),
        };
        self.set("warning", json!(warning));
        let by = |s: &str| self.repos.iter().filter(|r| r.status == s).count();
        let (indexed, pending, errors, ignored) = (by("indexed"), by("pending"), by("error"), by("ignored"));
        self.set("indexed", json!(indexed));
        self.set("pending", json!(pending));
        self.set("errors", json!(errors));
        self.set("ignored", json!(ignored));
        self.set("outdated", json!(views::outdated_count(&self.repos)));
        self.set("can-sync", json!(online && self.me.can(api::SCOPE_INDEX)));
        self.set("can-admin", json!(online && self.me.can(api::SCOPE_ADMIN)));
        self.set("can-enrich", json!(online && self.me.can(api::SCOPE_ENRICH)));
        self.set("last-sync", json!(views::last_sync_line(&self.jobs, now)));

        // the live job
        let active = self.jobs.first().filter(|j| j.active()).cloned();
        match &active {
            Some(j) => {
                let (title, progress, line) = views::live_summary(j);
                self.set("job-active", json!(true));
                self.set("job-title", json!(title));
                self.set("job-pct", json!(progress));
                self.set("job-line", json!(line));
                self.set("live-repos", json!(views::live_repos(j)));
            }
            None => {
                self.set("job-active", json!(self.sync_step == SyncStep::Starting));
                self.set("job-title", json!(if self.sync_step == SyncStep::Starting { tr("Starting") } else { "" }));
                self.set("job-pct", json!(0.0));
                self.set("job-line", json!(""));
                self.set("live-repos", json!([]));
            }
        }

        // search
        let can_editor = self.selected_file().is_some();
        let s = &self.search;
        let search_props = [
            ("search-mode", json!(s.mode)),
            ("query", json!(s.query)),
            ("search-kind", json!(s.kind)),
            ("search-repos", json!(s.repos)),
            ("grep-regex", json!(s.regex)),
            ("grep-case", json!(s.case)),
            ("searching", json!(s.searching)),
            ("search-note", json!(s.note)),
            ("results", json!(s.rows)),
            ("selected", json!(s.selected.map_or(-1, |i| i as i64))),
            ("preview-title", json!(s.preview_title)),
            ("preview-text", json!(s.preview_text)),
            ("preview-note", json!(s.preview_note)),
            ("preview-loading", json!(s.preview_loading)),
            ("can-editor", json!(can_editor)),
        ];
        for (k, v) in search_props {
            self.set(k, v);
        }

        // repos
        let confirm = self.confirm.as_ref().filter(|(_, at)| at.elapsed() < CONFIRM_TTL).map(|(k, _)| k.clone());
        let view = RepoView {
            filter: &self.repo_filter,
            status: &self.repo_status,
            expanded: self.repo_expanded.as_deref(),
            confirm: confirm.as_deref(),
            can_sync: online && self.me.can(api::SCOPE_INDEX),
            can_admin: online && self.me.can(api::SCOPE_ADMIN),
        };
        let (rows, note) = views::repo_rows(&self.repos, &view, now);
        self.set("repo-filter", json!(self.repo_filter));
        self.set("repo-status", json!(self.repo_status));
        self.set("repo-rows", json!(rows));
        self.set("repo-count", json!(note));
        self.set("repo-note", json!(self.repo_note));

        // sync
        self.set("batch", json!(self.batch));
        self.set("sync-state", json!(self.sync_step.as_str()));
        self.set("sync-preview", json!(self.sync_preview));
        self.set("sync-note", json!(self.sync_note));
        self.set("history", json!(views::history_rows(&self.jobs, self.job_expanded.as_deref(), now)));
        self.set("can-orgs", json!(online && self.has_orgs && self.me.can(api::SCOPE_ADMIN)));
        self.set("orgs-confirm", json!(confirm.as_deref() == Some("orgs")));
        self.set("org-note", json!(self.org_note));

        // health
        let (meters, stats) = match &self.enrich {
            Some(e) => (views::enrich_meters(e), views::enrich_stats(e)),
            None => (Vec::new(), Vec::new()),
        };
        self.set("meters", json!(meters));
        self.set("enrich-stats", json!(stats));
        self.set("usage-days", json!(self.usage_days));
        self.set("usage-loading", json!(self.usage_loading));
        let (u_stats, bars, queries, clients) = match &self.usage {
            Some(u) => (views::usage_stats(u), views::usage_bars(u, self.usage_days as usize), views::top_queries(u), views::clients_line(u)),
            None => (Vec::new(), Vec::new(), Vec::new(), String::new()),
        };
        self.set("usage-stats", json!(u_stats));
        self.set("usage-bars", json!(bars));
        self.set("top-queries", json!(queries));
        self.set("clients", json!(clients));
    }

    /// The hub shows `error` on the cell and keeps the mascot in alarm while it is set.
    fn send_error(&mut self, error: &str) {
        if self.error == error {
            return;
        }
        if error.is_empty() {
            hub::clear_error();
        } else {
            hub::error(error);
        }
        self.error = error.to_string();
    }
}

/// One full look at the server: who the token is, the repos, the jobs and the enrichment.
fn poll_server(client: &Client) -> Poll {
    let fail = |health: Health| Poll { health, me: None, repos: Vec::new(), jobs: None, enrich: None, has_orgs: false };
    if let Err(e) = client.health() {
        return fail(classify(e));
    }
    let me = match client.me() {
        Ok(me) => me,
        Err(e) => return fail(classify(e)),
    };
    if !me.can(api::SCOPE_SEARCH) {
        return Poll { me: Some(me), ..fail(Health::Denied(Denied::Forbidden(api::SCOPE_SEARCH.into()))) };
    }
    let repos = match client.repos() {
        Ok(r) => r,
        Err(e) => return Poll { me: Some(me), ..fail(classify(e)) },
    };
    let jobs = if me.can(api::SCOPE_INDEX) {
        match client.jobs(JOBS_LIMIT) {
            Ok(j) => Some(j),
            Err(e) => {
                eprintln!("sync jobs unavailable: {e}");
                None
            }
        }
    } else {
        Some(Vec::new())
    };
    let enrich = if me.can(api::SCOPE_ENRICH) {
        client.enrich().map_err(|e| eprintln!("enrich status unavailable: {e}")).ok()
    } else {
        None
    };
    let has_orgs = me.can(api::SCOPE_ADMIN) && client.orgs().is_ok_and(|o| !o.is_empty());
    let snap = Snapshot::build(&repos, jobs.as_deref().unwrap_or_default());
    Poll { health: Health::Online(snap), me: Some(me), repos, jobs, enrich, has_orgs }
}

fn classify(e: ApiError) -> Health {
    match e {
        ApiError::Offline(m) => Health::Offline(m),
        ApiError::Timeout => Health::Failed(tr("the server took too long to answer").to_string()),
        ApiError::Unauthorized => Health::Denied(Denied::Unauthorized),
        ApiError::Forbidden(d) => Health::Denied(Denied::Forbidden(d)),
        other => Health::Failed(other.to_string()),
    }
}

// ---------------------------------------------------------------- texts

/// `(server-state, summary line, notice for the view, error for the hub)`.
fn describe(health: &Health, url: &str, expect_server: bool) -> (&'static str, String, String, String) {
    match health {
        Health::NotConfigured => {
            let msg = tr("Not configured: type the server's token in Settings › Apps").to_string();
            ("unconfigured", tr("Not configured").to_string(), msg.clone(), msg)
        }
        Health::Offline(_) => {
            let error = if expect_server { tr("Server offline").to_string() } else { String::new() };
            ("offline", tr("Server offline").to_string(), fmt("Cannot reach {}: trying again every 30 s", &[&url]), error)
        }
        Health::Denied(Denied::Unauthorized) => {
            let msg = tr("The server rejected the token (401): update it in Settings › Apps").to_string();
            ("denied", tr("Token rejected").to_string(), msg.clone(), msg)
        }
        Health::Denied(Denied::Forbidden(detail)) => {
            let msg = fmt("The token lacks a permission: {}", &[detail]);
            ("denied", tr("Token lacks permission").to_string(), msg.clone(), msg)
        }
        Health::Failed(detail) => {
            let msg = fmt("The server answered with an error: {}", &[detail]);
            ("error", tr("Server error").to_string(), msg.clone(), msg)
        }
        Health::Online(s) => ("online", summary_line(s), String::new(), String::new()),
    }
}

/// "120 indexed · 4 pending · 1 error".
fn summary_line(s: &Snapshot) -> String {
    let c = s.counts;
    match c.error {
        0 => fmt("{} indexed · {} pending", &[&c.indexed, &c.pending]),
        1 => fmt("{} indexed · {} pending · 1 error", &[&c.indexed, &c.pending]),
        n => fmt("{} indexed · {} pending · {} errors", &[&c.indexed, &c.pending, &n]),
    }
}

/// What a failed click tells the user.
fn action_error(e: &ApiError) -> String {
    match e {
        ApiError::Offline(_) => tr("Server offline").to_string(),
        ApiError::Timeout => tr("The server took too long to answer: try again").to_string(),
        ApiError::Unauthorized => tr("Token rejected").to_string(),
        ApiError::Forbidden(d) => fmt("The token lacks a permission: {}", &[d]),
        ApiError::NotFound(d) => fmt("Not found: {}", &[d]),
        ApiError::Conflict(d) => fmt("Not now: {}", &[d]),
        ApiError::RateLimited => tr("Too many requests: try again in a moment").to_string(),
        ApiError::Http(code, d) => fmt("The server answered with an error: {}", &[&format!("{code} {d}")]),
        ApiError::Parse(d) => fmt("The server answered with an error: {}", &[d]),
    }
}

/// "3 new repositories found", from the server's answer to an org refresh (one object per
/// org with lists or counts of what changed).
fn org_refresh_note(v: &Value) -> String {
    fn added(v: &Value) -> u64 {
        match v {
            Value::Object(m) => m
                .iter()
                .map(|(k, x)| match (k.as_str(), x) {
                    ("added" | "new", Value::Array(a)) => a.len() as u64,
                    ("added" | "new", Value::Number(n)) => n.as_u64().unwrap_or(0),
                    _ => added(x),
                })
                .sum(),
            Value::Array(a) => a.iter().map(added).sum(),
            _ => 0,
        }
    }
    match added(v) {
        0 => tr("No new repositories").to_string(),
        n => fmt("{} new repositories in the queue", &[&n]),
    }
}

fn emit(event: &Event) {
    match event {
        Event::RepoFailed(name) => hub::fact(Fact::Trouble, views::short_name(name), None),
        Event::SyncFailedFact => hub::fact(Fact::Trouble, tr("sync"), None),
        Event::Recovered => hub::fact(Fact::Recovered, tr("code index"), None),
        Event::Queue(n) => hub::fact(Fact::Forecast, tr("repos in the queue"), Some(*n as f64)),
        Event::SyncFinished { indexed, failed: 0 } => hub::notify(tr("Code index"), &fmt("Sync finished: {} indexed", &[indexed])),
        Event::SyncFinished { indexed, failed } => {
            hub::notify(tr("Code index"), &fmt("Sync finished: {} indexed, {} failed", &[indexed, failed]));
        }
        Event::SyncFailed { reason: None } => hub::notify(tr("Code index"), tr("Sync failed")),
        Event::SyncFailed { reason: Some(r) } => hub::notify(tr("Code index"), &fmt("Sync failed: {}", &[r])),
    }
}

// ---------------------------------------------------------------- opening files

/// The file inside the local clones, if `repos_dir` is set and the file exists. The path
/// comes from the server, so anything that could leave the clone (`..`, absolute) is refused.
fn clone_file(repos_dir: Option<&Path>, repo: &str, path: &str) -> Option<PathBuf> {
    let root = repos_dir?;
    let safe = |p: &Path| p.components().all(|c| matches!(c, Component::Normal(_)));
    if !safe(Path::new(repo)) || !safe(Path::new(path)) {
        return None;
    }
    let file = root.join(repo).join(path);
    file.is_file().then_some(file)
}

/// `code -g file:line`; without VS Code, the default program for the file.
fn open_in_editor(file: &Path, line: u64) {
    let result = match os::which("code") {
        Some(code) => os::spawn_detached(os::command(code).arg("-g").arg(format!("{}:{line}", file.display()))),
        None => os::open_path(file),
    };
    if let Err(e) = result {
        eprintln!("could not open {}: {e}", file.display());
    }
}

fn main() {
    let (tx, rx) = channel::<Msg>();
    hub::listen(tx.clone(), Msg::Host);
    let mut app = App::new(tx);
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Msg::Host(HostMsg::Stop)) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(msg) => app.handle(msg),
            Err(RecvTimeoutError::Timeout) => {}
        }
        app.fast_tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use state::Counts;

    fn snapshot(indexed: usize, pending: usize, error: usize) -> Snapshot {
        Snapshot { counts: Counts { indexed, pending, error }, error_repos: Vec::new(), job: None }
    }

    #[test]
    fn settings_have_defaults_and_limits() {
        assert_eq!(Settings::from(&Value::Null), Settings { expect_server: false, notify_sync: true, results: 10 });
        let s = Settings::from(&json!({"expect_server": true, "notify_sync": false, "results": 99}));
        assert_eq!(s, Settings { expect_server: true, notify_sync: false, results: 50 });
        assert_eq!(Settings::from(&json!({"results": 1})).results, 5);
    }

    #[test]
    fn summary_line_counts() {
        assert_eq!(summary_line(&snapshot(120, 4, 1)), "120 indexed · 4 pending · 1 error");
        assert_eq!(summary_line(&snapshot(120, 4, 3)), "120 indexed · 4 pending · 3 errors");
        assert_eq!(summary_line(&snapshot(120, 0, 0)), "120 indexed · 0 pending");
    }

    #[test]
    fn describe_each_state() {
        let d = |h: Health, expect| describe(&h, "http://x", expect);
        let (state, line, notice, error) = d(Health::NotConfigured, false);
        assert_eq!((state, line.as_str()), ("unconfigured", "Not configured"));
        assert!(notice.contains("Settings") && error == notice);
        let (state, _, notice, error) = d(Health::Offline("x".into()), false);
        assert_eq!(state, "offline");
        assert!(notice.contains("http://x"));
        assert_eq!(error, "");
        assert_eq!(d(Health::Offline("x".into()), true).3, "Server offline");
        assert_eq!(d(Health::Denied(Denied::Unauthorized), false).0, "denied");
        assert_eq!(d(Health::Failed("500".into()), false).0, "error");
        assert_eq!(d(Health::Online(snapshot(1, 0, 0)), false).3, "");
    }

    #[test]
    fn classifies_errors() {
        assert!(matches!(classify(ApiError::Offline("x".into())), Health::Offline(_)));
        assert!(matches!(classify(ApiError::Unauthorized), Health::Denied(Denied::Unauthorized)));
        assert!(matches!(classify(ApiError::Forbidden("s".into())), Health::Denied(Denied::Forbidden(_))));
        assert!(matches!(classify(ApiError::Http(500, "x".into())), Health::Failed(_)));
    }

    #[test]
    fn action_errors_read_well() {
        assert_eq!(action_error(&ApiError::Conflict("a sync is running".into())), "Not now: a sync is running");
        assert_eq!(action_error(&ApiError::RateLimited), "Too many requests: try again in a moment");
        assert_eq!(action_error(&ApiError::NotFound("file".into())), "Not found: file");
    }

    #[test]
    fn org_refresh_counts_new_repos() {
        assert_eq!(org_refresh_note(&json!({"acme": {"added": ["a", "b"], "removed": []}, "beta": {"new": 3}})), "5 new repositories in the queue");
        assert_eq!(org_refresh_note(&json!({})), "No new repositories");
    }

    #[test]
    fn clone_file_stays_inside_the_clone() {
        let root = std::env::temp_dir().join(format!("codeindex-clone-{}", std::process::id()));
        std::fs::create_dir_all(root.join("acme__api/src")).unwrap();
        std::fs::write(root.join("acme__api/src/a.py"), "x").unwrap();
        assert_eq!(clone_file(Some(&root), "acme__api", "src/a.py"), Some(root.join("acme__api/src/a.py")));
        assert_eq!(clone_file(Some(&root), "acme__api", "src/missing.py"), None);
        assert_eq!(clone_file(Some(&root), "acme__api", "../../etc/passwd"), None);
        assert_eq!(clone_file(Some(&root), "..", "acme__api/src/a.py"), None);
        assert_eq!(clone_file(None, "acme__api", "src/a.py"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_clicks_confirm_and_expire() {
        let (tx, _rx) = channel();
        let mut app = App::new(tx);
        assert!(!app.confirmed("remove:a"));
        assert!(app.confirmed("remove:a"));
        // after confirming, it starts over
        assert!(!app.confirmed("remove:a"));
        // another target re-arms instead of confirming
        assert!(!app.confirmed("remove:b"));
        assert!(!app.confirmed("remove:a"));
        app.confirm = Some(("remove:a".into(), Instant::now() - CONFIRM_TTL - Duration::from_secs(1)));
        assert!(!app.confirmed("remove:a"), "an old first click does not count");
    }

    #[test]
    fn a_failing_poll_keeps_the_data_for_a_while() {
        let (tx, _rx) = channel();
        let mut app = App::new(tx);
        app.cfg.token = "cix_x".into();
        let online = || Poll {
            health: Health::Online(snapshot(1, 0, 0)),
            me: None,
            repos: vec![Repo { name: "acme__a".into(), status: "indexed".into(), ..Default::default() }],
            jobs: Some(Vec::new()),
            enrich: None,
            has_orgs: false,
        };
        let down = || Poll { health: Health::Failed("timeout".into()), me: None, repos: Vec::new(), jobs: None, enrich: None, has_orgs: false };
        app.polled(online());
        for _ in 0..TOLERATED_FAILURES {
            app.polled(down());
            assert!(app.online(), "a short failure keeps the screen");
            assert_eq!(app.repos.len(), 1);
            assert!(app.stale_since.is_some());
        }
        app.polled(down());
        assert!(!app.online(), "then the failure shows");
        app.polled(online());
        assert!(app.online() && app.stale_since.is_none() && app.failures == 0);
        // a rejected token is not transient
        app.polled(Poll { health: Health::Denied(Denied::Unauthorized), ..down() });
        assert!(matches!(app.health, Some(Health::Denied(_))));
    }

    #[test]
    fn repo_filter_resolves_names() {
        let (tx, _rx) = channel();
        let mut app = App::new(tx);
        app.repos = ["acme__api", "acme__api-gateway", "acme__web"].iter().map(|n| Repo { name: n.to_string(), ..Default::default() }).collect();
        app.search.repos = "api".into();
        assert_eq!(app.resolve_repos().unwrap(), ["acme__api"], "a short name that exists wins over substrings");
        app.search.repos = "gate, web".into();
        assert_eq!(app.resolve_repos().unwrap(), ["acme__api-gateway", "acme__web"]);
        app.search.repos = "zzz".into();
        assert_eq!(app.resolve_repos().unwrap_err(), "No repository named “zzz”");
        app.search.repos = String::new();
        assert!(app.resolve_repos().unwrap().is_empty());
    }
}
