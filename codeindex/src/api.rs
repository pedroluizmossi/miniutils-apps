//! The code-index server's HTTP API (database-index, FastAPI): models and a blocking client.
//!
//! Routes used and the scope each one needs on the server:
//! - `GET /healthz`: none
//! - `GET /api/v1/me`: any token (tells the scopes)
//! - `GET /api/v1/repos`, `POST /api/v1/search`, `GET /api/v1/symbols`, `POST /api/v1/grep`,
//!   `GET /api/v1/files/{repo}/{path}`, `GET /api/v1/me/usage`: `search:read`
//! - `GET /api/v1/sync/jobs`, `POST /api/v1/sync`: `index:write`
//! - `GET /api/v1/enrich/status`: `enrich:write`
//! - `DELETE /api/v1/repos/{name}`, `POST /api/v1/repos/{name}/unignore`, `POST /api/v1/orgs/refresh`: `admin`
//!
//! The models only name the fields the app reads; everything has a default so a server that
//! adds, drops or nulls a field does not break the app. Parsing is separate from the network
//! (`parse`) so it is tested against sample JSON.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::Config;

/// Timeout of the status calls (the repo list is the slowest: ~0.5 s for 300 repos).
pub const SHORT: Duration = Duration::from_secs(10);
/// Timeout of the searches (hybrid search plus rerank, a regex over every repo).
pub const SEARCH: Duration = Duration::from_secs(40);

pub const SCOPE_SEARCH: &str = "search:read";
pub const SCOPE_INDEX: &str = "index:write";
pub const SCOPE_ENRICH: &str = "enrich:write";
pub const SCOPE_ADMIN: &str = "admin";

/// Why a call failed, already sorted by what the app does about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Could not reach the server (refused, DNS, bad URL).
    Offline(String),
    /// The server took longer than the timeout (busy or slow).
    Timeout,
    /// 401: the token is missing, expired or revoked.
    Unauthorized,
    /// 403: the token lacks the scope for this route (the server's message names it).
    Forbidden(String),
    /// 404: no such repo, file or job.
    NotFound(String),
    /// 409: for example a sync is already running.
    Conflict(String),
    /// 429: rate limit.
    RateLimited,
    /// Any other status.
    Http(u16, String),
    /// The body is not what the model expects.
    Parse(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Offline(e) => write!(f, "cannot reach the server: {e}"),
            ApiError::Timeout => write!(f, "the server took too long to answer"),
            ApiError::Unauthorized => write!(f, "401: token rejected"),
            ApiError::Forbidden(d) => write!(f, "403: {d}"),
            ApiError::NotFound(d) => write!(f, "404: {d}"),
            ApiError::Conflict(d) => write!(f, "409: {d}"),
            ApiError::RateLimited => write!(f, "429: rate limit"),
            ApiError::Http(code, d) => write!(f, "HTTP {code}: {d}"),
            ApiError::Parse(e) => write!(f, "unexpected response: {e}"),
        }
    }
}

// ------------------------------------------------------------------ models

/// `GET /api/v1/me`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl Me {
    /// `admin` implies every other scope (as on the server).
    pub fn can(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope || s == SCOPE_ADMIN)
    }
}

/// One entry of `GET /api/v1/repos` (`RepoStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Repo {
    pub name: String,
    #[serde(default)]
    pub org: Option<String>,
    /// `indexed`, `pending`, `error` or `ignored`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub language: Option<String>,
    /// `code` or `docs`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub indexed_at: Option<String>,
    #[serde(default)]
    pub pushed_at: Option<String>,
    /// Pushed after the last indexing.
    #[serde(default)]
    pub outdated: bool,
    #[serde(default)]
    pub files: Option<u64>,
    #[serde(default)]
    pub chunks: Option<u64>,
    #[serde(default)]
    pub skipped: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    /// The repo card's summary (written by an agent).
    #[serde(default)]
    pub summary: Option<String>,
}

/// The server's `SyncResult`: the job's `result`, and the answer of a dry run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SyncResult {
    /// Repos picked for this batch.
    #[serde(default)]
    pub selected: Vec<String>,
    /// Repo name to the commit indexed.
    #[serde(default)]
    pub indexed: HashMap<String, Option<String>>,
    /// Repo name to the error message.
    #[serde(default)]
    pub errors: HashMap<String, String>,
    /// Candidates left for the next batches.
    #[serde(default)]
    pub remaining: u64,
}

/// One repo in a running job (`RepoProgress`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RepoProgress {
    pub name: String,
    /// `queued`, `fetching`, `indexing`, `done`, `error` or `skipped`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub files_total: Option<u64>,
    #[serde(default)]
    pub files_done: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// The job's `progress` (`SyncProgress.snapshot`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Progress {
    /// `discover`, `select`, `fetch`, `index` or `finish`.
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub repos_total: u64,
    #[serde(default)]
    pub repos_done: u64,
    #[serde(default)]
    pub files_total: u64,
    #[serde(default)]
    pub files_done: u64,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub repos: Vec<RepoProgress>,
}

/// One entry of `GET /api/v1/sync/jobs` (`JobRecord`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Job {
    pub id: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    /// `queued`, `running`, `done` or `failed`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub result: Option<SyncResult>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub progress: Option<Progress>,
}

impl Job {
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "running")
    }
}

/// `POST /api/v1/sync` without dry run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StartedJob {
    pub job_id: String,
}

/// One result of `POST /api/v1/search` (`Hit`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Hit {
    pub repo: String,
    pub path: String,
    #[serde(default)]
    pub start_line: u64,
    #[serde(default)]
    pub end_line: u64,
    /// The matching chunk of code (or documentation text).
    #[serde(default)]
    pub code: String,
    /// `code` or `doc`.
    #[serde(default)]
    pub kind: String,
    /// Page title of a documentation hit.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
}

/// One result of `GET /api/v1/symbols`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Symbol {
    pub repo: String,
    pub path: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

/// One match of `POST /api/v1/grep`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GrepMatch {
    pub repo: String,
    pub path: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GrepResult {
    #[serde(default)]
    pub matches: Vec<GrepMatch>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct SummaryCounts {
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub fresh: u64,
    #[serde(default)]
    pub stale: u64,
    #[serde(default)]
    pub missing: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct ImplCounts {
    #[serde(default)]
    pub operations: u64,
    #[serde(default)]
    pub matched: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct DocCounts {
    #[serde(default)]
    pub pending_pages: u64,
}

/// `GET /api/v1/enrich/status`: the numbers the app shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct EnrichStatus {
    /// Indexed code repos.
    #[serde(default)]
    pub repos: u64,
    /// Repos whose card is up to date.
    #[serde(default)]
    pub cards_ok: u64,
    #[serde(default)]
    pub cards_stale: u64,
    #[serde(default)]
    pub cards_missing: u64,
    /// Open enrichment tasks.
    #[serde(default)]
    pub pending: u64,
    /// Tasks an agent is working on right now.
    #[serde(default)]
    pub leased: Vec<Value>,
    #[serde(default)]
    pub summaries: SummaryCounts,
    #[serde(default)]
    pub impls: ImplCounts,
    #[serde(default)]
    pub docs: DocCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct UsageTotals {
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub avg_ms: Option<f64>,
    #[serde(default)]
    pub p95_ms: Option<f64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UsageDay {
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub mcp: u64,
    #[serde(default)]
    pub api: u64,
    #[serde(default)]
    pub ui: u64,
    #[serde(default)]
    pub errors: u64,
}

impl UsageDay {
    pub fn calls(&self) -> u64 {
        self.mcp + self.api + self.ui
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TopQuery {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub avg_results: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ClientUsage {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub calls: u64,
}

/// `GET /api/v1/me/usage?days=N`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub days: u64,
    #[serde(default)]
    pub totals: UsageTotals,
    #[serde(default)]
    pub daily: Vec<UsageDay>,
    #[serde(default)]
    pub top_queries: Vec<TopQuery>,
    #[serde(default)]
    pub by_client: Vec<ClientUsage>,
}

/// What `POST /api/v1/search` gets besides the query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    /// `None` for code and docs, `Some("code")` or `Some("doc")`.
    pub kind: Option<&'static str>,
    /// Only these repos (full names); empty for all.
    pub repos: Vec<String>,
}

// ------------------------------------------------------------------ parsing

/// Any model from a JSON body.
pub fn parse<T: DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|e| ApiError::Parse(e.to_string()))
}

/// The server's error message: FastAPI's `{"detail": ...}`, or the start of the body.
pub fn error_detail(body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|v| match v.get("detail") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) if !other.is_null() => Some(other.to_string()),
        _ => None,
    });
    detail.unwrap_or_else(|| body.trim().chars().take(200).collect())
}

/// Maps an HTTP status (and body) to the error the app reacts to; `None` for success.
pub fn status_error(status: u16, body: &str) -> Option<ApiError> {
    match status {
        200..=299 => None,
        401 => Some(ApiError::Unauthorized),
        403 => Some(ApiError::Forbidden(error_detail(body))),
        404 => Some(ApiError::NotFound(error_detail(body))),
        409 => Some(ApiError::Conflict(error_detail(body))),
        429 => Some(ApiError::RateLimited),
        code => Some(ApiError::Http(code, error_detail(body))),
    }
}

/// Percent-encodes a query value or a path segment (RFC 3986 unreserved characters stay).
/// With `keep_slash`, `/` stays too (a file path after the repo).
pub fn encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let plain = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || (keep_slash && b == b'/');
        if plain {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Body of `POST /api/v1/search`.
pub fn search_body(query: &str, top_k: u32, filters: &SearchFilters) -> Value {
    let mut body = json!({ "query": query, "top_k": top_k.clamp(1, 50) });
    if let Some(kind) = filters.kind {
        body["kind"] = json!(kind);
    }
    if !filters.repos.is_empty() {
        body["repos"] = json!(filters.repos);
    }
    body
}

/// Body of `POST /api/v1/sync`: `repos` empty for the automatic batch, `batch` `None` for
/// the server's default size.
pub fn sync_body(repos: &[String], batch: Option<u32>, dry_run: bool) -> Value {
    let mut body = json!({ "dry_run": dry_run });
    if !repos.is_empty() {
        body["repos"] = json!(repos);
    }
    if let Some(b) = batch {
        body["batch"] = json!(b.min(50));
    }
    body
}

// ------------------------------------------------------------------ client

/// A failure before an HTTP status: a timeout is told apart (the server is up but busy).
fn connection_error(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::Timeout(_) => ApiError::Timeout,
        other => ApiError::Offline(other.to_string()),
    }
}

/// Blocking client. Cheap to build: one per background job.
pub struct Client {
    base: String,
    token: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Client {
        Client { base: cfg.url.clone(), token: cfg.token.clone() }
    }

    fn agent(timeout: Duration) -> ureq::Agent {
        let config = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build();
        config.into()
    }

    fn finish(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, ApiError> {
        let mut resp = resp.map_err(connection_error)?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(32 << 20)
            .read_to_string()
            .map_err(connection_error)?;
        match status_error(status, &body) {
            Some(e) => Err(e),
            None => Ok(body),
        }
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    fn get(&self, path: &str, timeout: Duration) -> Result<String, ApiError> {
        Self::finish(Self::agent(timeout).get(format!("{}{path}", self.base)).header("Authorization", self.auth()).call())
    }

    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<String, ApiError> {
        let req = Self::agent(timeout)
            .post(format!("{}{path}", self.base))
            .header("Authorization", self.auth())
            .header("Content-Type", "application/json");
        Self::finish(req.send(body.to_string()))
    }

    fn delete(&self, path: &str) -> Result<String, ApiError> {
        Self::finish(Self::agent(SHORT).delete(format!("{}{path}", self.base)).header("Authorization", self.auth()).call())
    }

    /// `GET /healthz` (no token needed).
    pub fn health(&self) -> Result<(), ApiError> {
        Self::finish(Self::agent(SHORT).get(format!("{}/healthz", self.base)).call()).map(|_| ())
    }

    pub fn me(&self) -> Result<Me, ApiError> {
        parse(&self.get("/api/v1/me", SHORT)?)
    }

    pub fn repos(&self) -> Result<Vec<Repo>, ApiError> {
        parse(&self.get("/api/v1/repos", SHORT)?)
    }

    /// The most recent sync jobs, newest first.
    pub fn jobs(&self, limit: u32) -> Result<Vec<Job>, ApiError> {
        parse(&self.get(&format!("/api/v1/sync/jobs?limit={limit}"), SHORT)?)
    }

    pub fn enrich(&self) -> Result<EnrichStatus, ApiError> {
        parse(&self.get("/api/v1/enrich/status", SHORT)?)
    }

    pub fn usage(&self, days: u32) -> Result<Usage, ApiError> {
        parse(&self.get(&format!("/api/v1/me/usage?days={days}"), SHORT)?)
    }

    pub fn search(&self, query: &str, top_k: u32, filters: &SearchFilters) -> Result<Vec<Hit>, ApiError> {
        parse(&self.post("/api/v1/search", &search_body(query, top_k, filters), SEARCH)?)
    }

    pub fn symbols(&self, name: &str, repos: &[String], limit: u32) -> Result<Vec<Symbol>, ApiError> {
        let mut path = format!("/api/v1/symbols?name={}&limit={}", encode(name, false), limit.clamp(1, 200));
        for r in repos {
            path.push_str(&format!("&repo={}", encode(r, false)));
        }
        parse(&self.get(&path, SEARCH)?)
    }

    pub fn grep(&self, pattern: &str, regex: bool, case_sensitive: bool, repos: &[String], max: u32) -> Result<GrepResult, ApiError> {
        let mut body = json!({ "pattern": pattern, "regex": regex, "case_sensitive": case_sensitive, "max_results": max.clamp(1, 500) });
        if !repos.is_empty() {
            body["repos"] = json!(repos);
        }
        parse(&self.post("/api/v1/grep", &body, SEARCH)?)
    }

    /// Lines `start..=end` of an indexed file (secrets masked by the server).
    pub fn file(&self, repo: &str, path: &str, start: u64, end: u64) -> Result<String, ApiError> {
        let url = format!("/api/v1/files/{}/{}?start_line={}&end_line={}", encode(repo, false), encode(path, true), start.max(1), end.max(start.max(1)));
        self.get(&url, SHORT)
    }

    /// A dry run says what would be indexed; otherwise a job starts on the server.
    pub fn sync_preview(&self, repos: &[String], batch: Option<u32>) -> Result<SyncResult, ApiError> {
        parse(&self.post("/api/v1/sync", &sync_body(repos, batch, true), SEARCH)?)
    }

    pub fn sync_start(&self, repos: &[String], batch: Option<u32>) -> Result<StartedJob, ApiError> {
        parse(&self.post("/api/v1/sync", &sync_body(repos, batch, false), SHORT)?)
    }

    /// Takes the repo out of the index (a repo of an org becomes ignored). Keeps the clone.
    pub fn remove_repo(&self, name: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/repos/{}?keep_clone=true", encode(name, false))).map(|_| ())
    }

    /// An ignored repo goes back to the queue.
    pub fn unignore_repo(&self, name: &str) -> Result<(), ApiError> {
        self.post(&format!("/api/v1/repos/{}/unignore", encode(name, false)), &json!({}), SHORT).map(|_| ())
    }

    /// The GitHub orgs the server follows (only their count matters here).
    pub fn orgs(&self) -> Result<Vec<Value>, ApiError> {
        parse(&self.get("/api/v1/orgs", SHORT)?)
    }

    /// Discovers new, removed and pushed repos of every org (no embedding cost).
    pub fn refresh_orgs(&self) -> Result<Value, ApiError> {
        parse(&self.post("/api/v1/orgs/refresh", &json!({}), SEARCH)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes taken from database-index (RepoStatus, JobRecord, SyncProgress, Hit, Symbol,
    // grep, enrich status and usage), with made-up names.
    const REPOS: &str = r#"[
      {"name":"acme__billing","org":"acme","status":"indexed","language":"python","source":"github","commit":"abc1234def","indexed_at":"2026-11-13T10:00:00-03:00","pushed_at":null,"outdated":false,"files":120,"chunks":900,"error":null,"size_kb":10,"kind":"code","skipped":2,"summary":"Bills customers.","tags":[],"card_stale":false,"owners":[],"owner_origin":null,"owner_confidence":null},
      {"name":"acme__web","org":"acme","status":"pending","error":null},
      {"name":"acme__old","status":"error","error":"clone failed"},
      {"name":"docs__wiki","org":null,"status":"ignored","kind":"docs"}
    ]"#;

    const JOBS: &str = r#"[
      {"id":"j2","requested_by":"me@x","params":{},"status":"running","started_at":"2026-11-14T09:00:00+00:00","finished_at":null,"result":null,"error":null,
       "progress":{"phase":"index","phase_label":"indexando","repos_total":2,"repos_done":1,"files_total":50,"files_done":20,"seconds":12.5,
         "repos":[{"name":"acme__web","status":"done","files_total":10,"files_done":10,"files_unchanged":0,"errors":0,"error":null,"seconds":3.1},
                  {"name":"acme__api","status":"indexing","files_total":40,"files_done":10,"files_unchanged":2,"errors":1,"error":null,"seconds":9.0}]}},
      {"id":"j1","status":"failed","started_at":"2026-11-13T09:00:00+00:00","finished_at":"2026-11-13T09:05:00+00:00",
       "result":{"selected":["acme__old"],"indexed":{},"errors":{"acme__old":"boom"},"discovered":{},"remaining":12},"error":"all 1 repo(s) failed","progress":null}
    ]"#;

    #[test]
    fn parses_repos_jobs_and_progress() {
        let repos: Vec<Repo> = parse(REPOS).unwrap();
        assert_eq!(repos.len(), 4);
        assert_eq!(repos[0].files, Some(120));
        assert_eq!(repos[0].summary.as_deref(), Some("Bills customers."));
        assert_eq!(repos[2].error.as_deref(), Some("clone failed"));
        assert_eq!(repos[3].kind.as_deref(), Some("docs"));
        let jobs: Vec<Job> = parse(JOBS).unwrap();
        assert!(jobs[0].active() && !jobs[1].active());
        let p = jobs[0].progress.as_ref().unwrap();
        assert_eq!((p.repos_done, p.repos_total, p.files_done, p.files_total), (1, 2, 20, 50));
        assert_eq!(p.repos[1].files_total, Some(40));
        assert_eq!(jobs[1].result.as_ref().unwrap().remaining, 12);
    }

    #[test]
    fn parses_search_symbols_grep() {
        let hits: Vec<Hit> = parse(r#"[{"repo":"acme__api","path":"src/a.py","language":"python","start_line":93,"end_line":110,"score":0.81,"code":"x = 1\n","kind":"code"}]"#).unwrap();
        assert_eq!((hits[0].start_line, hits[0].end_line), (93, 110));
        let syms: Vec<Symbol> = parse(r#"[{"repo":"acme__web","path":"a.ts","line":199,"name":"TokenService","kind":"container","container":"","signature":"export interface TokenService {","language":"typescript"}]"#).unwrap();
        assert_eq!(syms[0].signature.as_deref(), Some("export interface TokenService {"));
        let grep: GrepResult = parse(r#"{"matches":[{"repo":"acme__api","path":"b.java","line":26,"text":"verify_token(x)"}],"truncated":true}"#).unwrap();
        assert!(grep.truncated);
        assert_eq!(grep.matches[0].line, 26);
    }

    #[test]
    fn parses_enrich_and_usage() {
        let e: EnrichStatus = parse(r#"{"repos":263,"with_facts":263,"cards_ok":5,"cards_stale":1,"cards_missing":257,"with_sdl":56,"tasks":{"done":12},"rejections":2,"leased":[],"pending":258,"impls":{"operations":1083,"matched":889},"docs":{"pending_pages":1539,"pending_unlinked":878},"summaries":{"files":16326,"fresh":46,"stale":0,"missing":16280}}"#).unwrap();
        assert_eq!((e.cards_ok, e.pending, e.summaries.fresh, e.impls.matched, e.docs.pending_pages), (5, 258, 46, 889, 1539));
        let u: Usage = parse(r#"{"days":7,"totals":{"calls":553,"errors":2,"avg_ms":197.6,"p95_ms":592.8,"cost_usd":0.0029},"daily":[{"day":"2026-11-14","mcp":1,"api":55,"ui":48,"errors":1}],"by_action":[],"by_client":[{"client":"ureq","channel":"api","calls":48}],"top_queries":[{"action":"POST /api/v1/search","query":"token","calls":2,"avg_results":4.0}],"recent":[]}"#).unwrap();
        assert_eq!(u.totals.calls, 553);
        assert_eq!(u.daily[0].calls(), 104);
        assert_eq!(u.top_queries[0].query, "token");
        assert_eq!(u.by_client[0].calls, 48);
        // a field the server nulls or drops must not break the app
        let u: Usage = parse(r#"{"totals":{"calls":1,"avg_ms":null}}"#).unwrap();
        assert_eq!(u.totals.avg_ms, None);
    }

    #[test]
    fn me_scopes() {
        let me: Me = parse(r#"{"subject":"me@x","kind":"agent","scopes":["search:read","index:write"]}"#).unwrap();
        assert!(me.can(SCOPE_SEARCH) && me.can(SCOPE_INDEX) && !me.can(SCOPE_ADMIN) && !me.can(SCOPE_ENRICH));
        let admin = Me { subject: String::new(), scopes: vec![SCOPE_ADMIN.into()] };
        assert!(admin.can(SCOPE_ENRICH) && admin.can(SCOPE_INDEX));
    }

    #[test]
    fn status_errors_and_details() {
        assert_eq!(status_error(200, "[]"), None);
        assert_eq!(status_error(401, ""), Some(ApiError::Unauthorized));
        assert_eq!(status_error(403, r#"{"detail":"scope necessário: admin"}"#), Some(ApiError::Forbidden("scope necessário: admin".into())));
        assert_eq!(status_error(404, r#"{"detail":"no file"}"#), Some(ApiError::NotFound("no file".into())));
        assert_eq!(status_error(409, r#"{"detail":"running"}"#), Some(ApiError::Conflict("running".into())));
        assert_eq!(status_error(429, ""), Some(ApiError::RateLimited));
        assert_eq!(status_error(500, "  boom  "), Some(ApiError::Http(500, "boom".into())));
        assert!(matches!(parse::<Vec<Repo>>("not json"), Err(ApiError::Parse(_))));
    }

    #[test]
    fn encodes_queries_and_paths() {
        assert_eq!(encode("a b/c?d&é", false), "a%20b%2Fc%3Fd%26%C3%A9");
        assert_eq!(encode("src/my file.py", true), "src/my%20file.py");
        assert_eq!(encode("Class.method_1~-", false), "Class.method_1~-");
    }

    #[test]
    fn request_bodies() {
        let f = SearchFilters { kind: Some("code"), repos: vec!["acme__api".into()] };
        assert_eq!(search_body("q", 99, &f), json!({"query":"q","top_k":50,"kind":"code","repos":["acme__api"]}));
        assert_eq!(search_body("q", 10, &SearchFilters::default()), json!({"query":"q","top_k":10}));
        assert_eq!(sync_body(&[], None, true), json!({"dry_run": true}));
        assert_eq!(sync_body(&["a".into()], Some(80), false), json!({"dry_run": false, "repos": ["a"], "batch": 50}));
    }
}
