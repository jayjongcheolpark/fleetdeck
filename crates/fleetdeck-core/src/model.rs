//! The collected, front-end independent view of one firstmate home.

use serde::Serialize;

use crate::backlog::{Backlog, Section};
use crate::ledger::LedgerEvent;
use crate::meta::Meta;
use crate::projects::Project;
use crate::routes::Route;
use crate::status::{OpenDecision, StatusEvent};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reach {
    Ok,
    /// The machine answered, but the home directory is missing.
    Missing,
    /// The machine did not answer in time, or the read failed.
    Unreachable {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Role {
    Main,
    /// A second mate home, from its `.fm-secondmate-home` marker.
    Secondmate {
        id: String,
        /// `local` or `remote`, from `.fm-secondmate-parent`.
        route: Option<String>,
        parent_home: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    /// No `state/.lock`: no session holds the home.
    Free,
    /// The lock pid is a live harness process.
    Held,
    /// The lock pid is gone.
    Stale,
    /// The lock is unreadable, malformed, or its pid is not a harness.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub lock: LockState,
    pub pid: Option<u32>,
    /// The lock holder's command name and argv[0].
    pub process: Option<String>,
    pub session_id: Option<String>,
    /// mtime of `state/.lock`: about when the session took the home.
    pub since: Option<i64>,
    /// True when `state/.session-start-complete` names the lock pid.
    pub startup_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Watcher {
    pub pid: Option<u32>,
    pub alive: bool,
    /// Seconds since `state/.last-watcher-beat` changed.
    pub beat_age: Option<i64>,
    /// `state/.watcher-down`: `<pending|announced|acked>:<handling|downtime>:<gen>`.
    pub down_marker: Option<String>,
    /// Away mode: `away` or `quiet` from `state/.afk`, or `away` from a
    /// leftover `state/.afk-contract`.
    pub away: Option<String>,
}

/// The beat grace in `bin/fm-wake-lib.sh`: `max(300, poll + 60)`.
pub const WATCHER_BEAT_GRACE_SECS: i64 = 300;

impl Watcher {
    pub fn beat_fresh(&self) -> bool {
        self.beat_age.is_some_and(|a| a <= WATCHER_BEAT_GRACE_SECS)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Queues {
    /// Rows in `state/.wake-queue` (read without draining).
    pub wake: Option<u64>,
    /// `state/operational-inbox/*.msg` records present (never acknowledged).
    pub operational_inbox: u64,
    pub captain_notes: u64,
    pub terminal_outcomes_pending: u64,
    pub when_watches: u64,
    pub pending_replies_open: u64,
    /// Unhandled `state/<id>.inbox/*.msg` across all tasks.
    pub task_inbox_pending: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrSource {
    Meta,
    StatusEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Busy {
    /// `busy`, `idle` or `unknown`.
    pub state: String,
    pub event: String,
    pub ts: Option<i64>,
    /// False when `gen` does not match `state/<id>.busy-gen`.
    pub trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: String,
    pub meta: Meta,
    /// The status-log state: working, parked, blocked, paused, done,
    /// failed or unknown.
    pub state: String,
    pub events: Vec<StatusEvent>,
    /// True when the status log was larger than what was read.
    pub log_partial: bool,
    pub status_mtime: Option<i64>,
    pub open: Vec<OpenDecision>,
    pub pr: Option<(String, PrSource)>,
    pub busy: Option<Busy>,
    pub turn_ended: Option<i64>,
    pub inbox_pending: u64,
    pub inbox_handled: u64,
    /// The matching backlog row: (section, title).
    pub backlog: Option<(Section, String)>,
    /// True when no `.meta` exists and only the status log was found.
    pub orphan: bool,
}

impl Task {
    pub fn last_event(&self) -> Option<&StatusEvent> {
        self.events
            .iter()
            .rev()
            .find(|e| e.recognized && e.verb != "note")
    }

    pub fn last_at(&self) -> Option<i64> {
        self.events.iter().rev().find_map(|e| e.at)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Doc {
    /// Path relative to the home, for example `data/captain.md`.
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub content: Option<String>,
    /// True when `content` is only part of the file.
    pub partial: bool,
}

impl Doc {
    /// The startup-memory estimate firstmate uses: `ceil(bytes / 3)`.
    pub fn est_tokens(&self) -> u64 {
        self.size.div_ceil(3)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SummaryDecision {
    pub id: String,
    pub key: String,
    pub verb: String,
    pub summary: String,
    pub source: String,
}

/// The parts of `state/home-summary.json` that fleetdeck shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeSummary {
    pub schema: String,
    pub generated_epoch: Option<i64>,
    pub valid: Option<bool>,
    pub state: Option<String>,
    pub reason: Option<String>,
    pub decisions_open: Vec<SummaryDecision>,
    pub decisions_total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeSnapshot {
    /// A stable identity: `<path>` or `<host>:<path>`.
    pub key: String,
    pub name: String,
    pub host: Option<String>,
    pub path: String,
    /// The key of the home whose `data/secondmates.md` listed this home.
    pub discovered_from: Option<String>,
    pub reach: Reach,
    /// Local Unix time when the read finished.
    pub collected_at: i64,
    /// The home machine's clock during the read.
    pub clock: Option<i64>,
    /// How long the read took, in milliseconds.
    pub read_ms: u64,
    pub machine: Option<String>,
    pub resolved_path: Option<String>,
    pub role: Role,
    pub session: Session,
    pub watcher: Watcher,
    pub queues: Queues,
    pub tasks: Vec<Task>,
    pub backlog: Backlog,
    pub routes: Vec<Route>,
    pub projects: Vec<Project>,
    pub summary: Option<HomeSummary>,
    /// Startup context: captain, shared captain, learnings, projects,
    /// second mates and charter.
    pub context: Vec<Doc>,
    pub config: Vec<Doc>,
    /// Other `data/*.md` files, size and mtime only.
    pub data_docs: Vec<Doc>,
    pub ledger: Vec<LedgerEvent>,
    pub warnings: Vec<String>,
}

impl HomeSnapshot {
    /// A snapshot that only records why a home could not be read.
    pub fn failed(
        key: String,
        name: String,
        host: Option<String>,
        path: String,
        reach: Reach,
    ) -> Self {
        HomeSnapshot {
            key,
            name,
            host,
            path,
            discovered_from: None,
            reach,
            collected_at: 0,
            clock: None,
            read_ms: 0,
            machine: None,
            resolved_path: None,
            role: Role::Main,
            session: Session {
                lock: LockState::Unknown,
                pid: None,
                process: None,
                session_id: None,
                since: None,
                startup_complete: false,
            },
            watcher: Watcher {
                pid: None,
                alive: false,
                beat_age: None,
                down_marker: None,
                away: None,
            },
            queues: Queues::default(),
            tasks: Vec::new(),
            backlog: Backlog::default(),
            routes: Vec::new(),
            projects: Vec::new(),
            summary: None,
            context: Vec::new(),
            config: Vec::new(),
            data_docs: Vec::new(),
            ledger: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn is_secondmate(&self) -> bool {
        matches!(self.role, Role::Secondmate { .. })
    }

    /// Open decisions and blockers across tasks, as (task id, decision).
    pub fn open_decisions(&self) -> impl Iterator<Item = (&str, &OpenDecision)> {
        self.tasks
            .iter()
            .flat_map(|t| t.open.iter().map(move |d| (t.id.as_str(), d)))
    }

    /// Open captain calls: backlog rows with a captain hold that are not done.
    pub fn captain_holds(&self) -> impl Iterator<Item = (Section, &crate::backlog::Item)> {
        self.backlog
            .sections
            .iter()
            .flat_map(|s| s.items.iter().map(move |i| (s.section, i)))
            .filter(|(s, i)| i.captain_hold(*s))
    }
}
