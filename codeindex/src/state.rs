//! What the app knows about the server after one poll, and the decisions taken from it:
//! the mascot's mood and the transitions that become facts and notifications. Everything
//! here is pure (no network, no hub), so the decision tables are unit-tested.

use chrono::{DateTime, NaiveDateTime, Utc};
use miniutils_app::Mood;

use crate::api::{Job, Repo};

/// Repos by status (ignored ones are not counted).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub indexed: usize,
    pub pending: usize,
    pub error: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
}

/// The latest sync job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    pub id: String,
    pub state: JobState,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Repos indexed by the job.
    pub indexed: usize,
    /// Repos of the job that failed.
    pub failed: usize,
    /// Repos the job picked.
    pub selected: usize,
    /// Why the whole job failed.
    pub error: Option<String>,
}

impl JobInfo {
    pub fn from_api(job: &Job) -> JobInfo {
        let state = match job.status.as_str() {
            "queued" | "running" => JobState::Running,
            "failed" => JobState::Failed,
            _ => JobState::Done,
        };
        let result = job.result.clone().unwrap_or_default();
        JobInfo {
            id: job.id.clone(),
            state,
            started_at: job.started_at.clone(),
            finished_at: job.finished_at.clone(),
            indexed: result.indexed.len(),
            failed: result.errors.len(),
            selected: result.selected.len(),
            error: job.error.clone().filter(|e| !e.trim().is_empty()),
        }
    }
}

/// One successful poll of a configured server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub counts: Counts,
    /// Names of the repos in error, sorted.
    pub error_repos: Vec<String>,
    /// The latest sync job; `None` without one, or when the token cannot read jobs.
    pub job: Option<JobInfo>,
}

impl Snapshot {
    pub fn build(repos: &[Repo], jobs: &[Job]) -> Snapshot {
        let count = |status: &str| repos.iter().filter(|r| r.status == status).count();
        let mut error_repos: Vec<String> = repos.iter().filter(|r| r.status == "error").map(|r| r.name.clone()).collect();
        error_repos.sort();
        Snapshot {
            counts: Counts { indexed: count("indexed"), pending: count("pending"), error: error_repos.len() },
            error_repos,
            job: jobs.first().map(JobInfo::from_api),
        }
    }

    fn job_state(&self) -> Option<JobState> {
        self.job.as_ref().map(|j| j.state)
    }

    /// Something is wrong that the user should look at: the last sync failed or repos are in error.
    pub fn in_trouble(&self) -> bool {
        self.job_state() == Some(JobState::Failed) || !self.error_repos.is_empty()
    }
}

/// Why the server refused the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// 401.
    Unauthorized,
    /// 403, with the server's message ("scope necessário: search:read").
    Forbidden(String),
}

/// The outcome of a poll.
// One value per poll (every 30 s), moved a couple of times: boxing the snapshot would only
// add noise to every match.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// No token anywhere.
    NotConfigured,
    /// The server did not answer.
    Offline(String),
    Denied(Denied),
    /// The server answered, but not what was expected (5xx, bad JSON).
    Failed(String),
    Online(Snapshot),
}

/// The mascot's mood for a poll.
pub fn mood(health: &Health, expect_server: bool) -> Mood {
    match health {
        Health::NotConfigured | Health::Denied(_) | Health::Failed(_) => Mood::Alarm,
        Health::Offline(_) if expect_server => Mood::Alarm,
        Health::Offline(_) => Mood::Sleep,
        Health::Online(s) if s.job_state() == Some(JobState::Running) => Mood::Busy,
        Health::Online(s) if s.in_trouble() => Mood::Alarm,
        Health::Online(s) if s.counts.pending == 0 => Mood::Happy,
        Health::Online(_) => Mood::Idle,
    }
}

/// What changed between two polls, for the chatter between mascots and for notifications.
/// The texts are built where the language is known (`main.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A repo entered the error state (fact `trouble`).
    RepoFailed(String),
    /// The sync job failed (fact `trouble`).
    SyncFailedFact,
    /// Everything that was wrong is fine again (fact `recovered`).
    Recovered,
    /// This many repos wait in the queue (fact `forecast`).
    Queue(usize),
    /// Notification: a sync job finished.
    SyncFinished { indexed: usize, failed: usize },
    /// Notification: a sync job failed.
    SyncFailed { reason: Option<String> },
}

/// At most this many repos become a fact per poll (a bad batch must not flood the chat).
const MAX_REPO_FACTS: usize = 3;

/// Compares two polls. Without a previous poll nothing is announced: what already
/// was at the start is not news. `notify_sync` gates only the notifications.
pub fn transitions(prev: Option<&Snapshot>, now: &Snapshot, notify_sync: bool) -> Vec<Event> {
    let Some(prev) = prev else { return Vec::new() };
    let mut events = Vec::new();

    // a repo entering the error state
    events.extend(now.error_repos.iter().filter(|r| !prev.error_repos.contains(r)).take(MAX_REPO_FACTS).cloned().map(Event::RepoFailed));

    // the sync job: a new one, or the same one that changed state
    let finished_now = matches!(now.job_state(), Some(JobState::Done | JobState::Failed));
    let changed = match (&prev.job, &now.job) {
        (Some(p), Some(n)) => p.id != n.id || p.state != n.state,
        (None, Some(_)) => true,
        _ => false,
    };
    if changed && finished_now {
        let job = now.job.as_ref().expect("a finished job exists");
        if job.state == JobState::Failed {
            events.push(Event::SyncFailedFact);
        }
        if notify_sync {
            events.push(match job.state {
                JobState::Failed => Event::SyncFailed { reason: job.error.clone() },
                _ => Event::SyncFinished { indexed: job.indexed, failed: job.failed },
            });
        }
    }

    if prev.in_trouble() && !now.in_trouble() {
        events.push(Event::Recovered);
    }
    if now.counts.pending != prev.counts.pending && now.counts.pending > 0 {
        events.push(Event::Queue(now.counts.pending));
    }
    events
}

/// Seconds between a server timestamp and `now`; `None` if it cannot be read. The server
/// writes ISO 8601, with an offset (`+00:00`) or without one (then it is UTC).
pub fn age_secs(iso: &str, now: DateTime<Utc>) -> Option<i64> {
    let when = DateTime::parse_from_rfc3339(iso)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f").map(|n| n.and_utc()))
        .ok()?;
    Some((now - when).num_seconds().max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, state: JobState) -> JobInfo {
        JobInfo { id: id.into(), state, started_at: None, finished_at: None, indexed: 3, failed: 1, selected: 4, error: Some("boom".into()) }
    }

    fn snap(indexed: usize, pending: usize, errors: &[&str], job: Option<JobInfo>) -> Snapshot {
        Snapshot {
            counts: Counts { indexed, pending, error: errors.len() },
            error_repos: errors.iter().map(|s| s.to_string()).collect(),
            job,
        }
    }

    #[test]
    fn mood_decision_table() {
        let online = |s: Snapshot| Health::Online(s);
        let cases: Vec<(Health, bool, Mood)> = vec![
            (Health::NotConfigured, false, Mood::Alarm),
            (Health::Denied(Denied::Unauthorized), false, Mood::Alarm),
            (Health::Denied(Denied::Forbidden("scope".into())), false, Mood::Alarm),
            (Health::Failed("500".into()), false, Mood::Alarm),
            (Health::Offline("refused".into()), false, Mood::Sleep),
            (Health::Offline("refused".into()), true, Mood::Alarm),
            // a running sync wins over everything that is wrong
            (online(snap(10, 2, &["a"], Some(job("1", JobState::Running)))), false, Mood::Busy),
            (online(snap(10, 0, &[], Some(job("1", JobState::Failed)))), false, Mood::Alarm),
            (online(snap(10, 0, &["a"], Some(job("1", JobState::Done)))), false, Mood::Alarm),
            (online(snap(10, 0, &[], Some(job("1", JobState::Done)))), false, Mood::Happy),
            (online(snap(10, 0, &[], None)), false, Mood::Happy),
            (online(snap(10, 4, &[], Some(job("1", JobState::Done)))), false, Mood::Idle),
            // expect_server only matters when the server is down
            (online(snap(10, 4, &[], None)), true, Mood::Idle),
        ];
        for (health, expect, want) in cases {
            assert_eq!(mood(&health, expect), want, "{health:?} expect_server={expect}");
        }
    }

    #[test]
    fn first_poll_announces_nothing() {
        let now = snap(1, 5, &["a"], Some(job("1", JobState::Failed)));
        assert!(transitions(None, &now, true).is_empty());
    }

    #[test]
    fn repo_entering_error_is_trouble_and_clearing_is_recovered() {
        let ok = snap(10, 0, &[], Some(job("1", JobState::Done)));
        let bad = snap(9, 0, &["acme__x"], Some(job("1", JobState::Done)));
        assert_eq!(transitions(Some(&ok), &bad, true), vec![Event::RepoFailed("acme__x".into())]);
        // still in error: nothing new
        assert!(transitions(Some(&bad), &bad, true).is_empty());
        assert_eq!(transitions(Some(&bad), &ok, true), vec![Event::Recovered]);
    }

    #[test]
    fn job_finishing_notifies_once() {
        let running = snap(10, 5, &[], Some(job("2", JobState::Running)));
        let done = snap(13, 2, &[], Some(job("2", JobState::Done)));
        assert_eq!(
            transitions(Some(&running), &done, true),
            vec![Event::SyncFinished { indexed: 3, failed: 1 }, Event::Queue(2)]
        );
        // the same finished job on the next poll: no second notification
        assert!(transitions(Some(&done), &done, true).is_empty());
        // gated by the setting, the queue forecast stays
        assert_eq!(transitions(Some(&running), &done, false), vec![Event::Queue(2)]);
    }

    #[test]
    fn job_that_started_and_finished_between_polls_is_noticed() {
        let before = snap(10, 5, &[], Some(job("1", JobState::Done)));
        let after = snap(15, 5, &[], Some(job("2", JobState::Done)));
        assert_eq!(transitions(Some(&before), &after, true), vec![Event::SyncFinished { indexed: 3, failed: 1 }]);
        // first job ever, seen finished
        let none = snap(10, 5, &[], None);
        assert_eq!(transitions(Some(&none), &after, true), vec![Event::SyncFinished { indexed: 3, failed: 1 }]);
    }

    #[test]
    fn failed_job_is_trouble_and_notifies_then_recovers_on_the_next_good_job() {
        let running = snap(10, 5, &[], Some(job("2", JobState::Running)));
        let failed = snap(10, 5, &[], Some(job("2", JobState::Failed)));
        assert_eq!(
            transitions(Some(&running), &failed, true),
            vec![Event::SyncFailedFact, Event::SyncFailed { reason: Some("boom".into()) }]
        );
        let again = snap(10, 5, &[], Some(job("3", JobState::Done)));
        assert_eq!(
            transitions(Some(&failed), &again, true),
            vec![Event::SyncFinished { indexed: 3, failed: 1 }, Event::Recovered]
        );
    }

    #[test]
    fn queue_forecast_only_when_the_count_changes_and_is_not_empty() {
        let a = snap(10, 12, &[], None);
        let b = snap(10, 11, &[], None);
        let empty = snap(10, 0, &[], None);
        assert_eq!(transitions(Some(&a), &b, true), vec![Event::Queue(11)]);
        assert!(transitions(Some(&b), &b, true).is_empty());
        assert!(transitions(Some(&b), &empty, true).is_empty());
    }

    #[test]
    fn many_new_errors_are_capped() {
        let ok = snap(10, 0, &[], None);
        let bad = snap(5, 0, &["a", "b", "c", "d", "e"], None);
        let facts = transitions(Some(&ok), &bad, true).len();
        assert_eq!(facts, MAX_REPO_FACTS);
    }

    #[test]
    fn snapshot_counts_statuses_and_reads_the_first_job() {
        let repo = |n: &str, s: &str| Repo { name: n.into(), status: s.into(), ..Default::default() };
        let repos = [repo("b", "error"), repo("a", "error"), repo("c", "indexed"), repo("d", "pending"), repo("e", "ignored")];
        let s = Snapshot::build(&repos, &[]);
        assert_eq!(s.counts, Counts { indexed: 1, pending: 1, error: 2 });
        assert_eq!(s.error_repos, ["a", "b"]);
        assert!(s.job.is_none());
        for status in ["queued", "running"] {
            let jobs = [Job { id: "j".into(), status: status.into(), ..Default::default() }];
            assert_eq!(Snapshot::build(&repos, &jobs).job.unwrap().state, JobState::Running);
        }
    }

    #[test]
    fn ages_with_and_without_offset() {
        let now = DateTime::parse_from_rfc3339("2026-11-14T10:00:00+00:00").unwrap().with_timezone(&Utc);
        assert_eq!(age_secs("2026-11-14T09:55:00+00:00", now), Some(300));
        assert_eq!(age_secs("2026-11-14T07:00:00-03:00", now), Some(0));
        assert_eq!(age_secs("2026-11-14T09:00:00", now), Some(3600));
        assert_eq!(age_secs("2026-11-14T09:00:00.000000", now), Some(3600));
        assert_eq!(age_secs("yesterday", now), None);
        // a clock a little behind the server's never gives a negative age
        assert_eq!(age_secs("2026-11-14T10:00:30+00:00", now), Some(0));
    }
}
