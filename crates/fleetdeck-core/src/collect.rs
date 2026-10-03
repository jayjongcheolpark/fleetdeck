//! Reads homes with the gather script and turns each bundle into a
//! [`HomeSnapshot`].

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::backlog::{self, Section};
use crate::bundle::{Bundle, RawFile, ReadMode};
use crate::config::{Config, HomeSpec, Location};
use crate::ledger;
use crate::meta::Meta;
use crate::model::*;
use crate::projects;
use crate::routes::{self, Route};
use crate::status;
use crate::transport::{self, TransportError};

/// The read-only script that runs on the machine holding a home.
pub const GATHER_SCRIPT: &str = include_str!("gather.sh");

/// Context documents loaded into a firstmate session, in digest order.
pub const CONTEXT_DOCS: &[&str] = &[
    "data/projects.md",
    "data/secondmates.md",
    "data/captain.md",
    "data/captain-shared.md",
    "data/learnings.md",
    "data/charter.md",
];

/// Process names that count as a harness (`fm_harness_process_matches`).
const HARNESSES: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "grok",
    "kimi",
    "pi",
    "pi-signed",
    "omp",
];

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn boundary() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("@@FLEETDECK-{:x}{:x}@@", nanos, std::process::id())
}

/// The key that identifies a home across refreshes.
pub fn home_key(location: &Location) -> String {
    location.label()
}

/// Reads several homes on one machine with a single gather run.
///
/// All `specs` must share a machine (the same host, or all local).
pub fn read_homes(specs: &[HomeSpec], timeout: Duration) -> Vec<HomeSnapshot> {
    let Some(first) = specs.first() else {
        return Vec::new();
    };
    let b = boundary();
    let paths: Vec<String> = specs.iter().map(|s| s.location.path_str()).collect();
    let mut args: Vec<&str> = vec![&b, "gather"];
    args.extend(paths.iter().map(String::as_str));
    let started = Instant::now();
    let result = transport::run_script(&first.location, GATHER_SCRIPT, &args, timeout);
    let read_ms = started.elapsed().as_millis() as u64;
    let collected_at = now_epoch();
    let fail = |reach: Reach| -> Vec<HomeSnapshot> {
        specs
            .iter()
            .map(|s| {
                let mut snap = blank(s, reach.clone());
                snap.collected_at = collected_at;
                snap.read_ms = read_ms;
                snap
            })
            .collect()
    };
    let out = match result {
        Ok(out) => out,
        Err(e) => {
            return fail(Reach::Unreachable {
                error: describe(&first.location, &e),
            });
        }
    };
    let bundles = match Bundle::parse_many(&b, &out) {
        Ok(v) => v,
        Err(e) => {
            return fail(Reach::Unreachable {
                error: format!("unreadable gather output: {e}"),
            });
        }
    };
    specs
        .iter()
        .zip(paths.iter())
        .map(|(spec, path)| {
            let found = bundles.iter().find(|(p, _)| p == path).map(|(_, b)| b);
            let mut snap = match found {
                Some(bundle) => from_bundle(spec, bundle),
                None => blank(
                    spec,
                    Reach::Unreachable {
                        error: "no output for this home".into(),
                    },
                ),
            };
            snap.collected_at = collected_at;
            snap.read_ms = read_ms;
            snap
        })
        .collect()
}

fn describe(location: &Location, e: &TransportError) -> String {
    match location.host() {
        Some(h) => format!("ssh {h}: {e}"),
        None => e.to_string(),
    }
}

fn blank(spec: &HomeSpec, reach: Reach) -> HomeSnapshot {
    HomeSnapshot::failed(
        home_key(&spec.location),
        spec.name.clone(),
        spec.location.host().map(str::to_string),
        spec.location.path_str(),
        reach,
    )
}

/// Reads one file of a home, for the detail view.
pub fn read_file(
    location: &Location,
    rel: &str,
    max_bytes: u64,
    timeout: Duration,
) -> Result<RawFile, String> {
    let b = boundary();
    let path = location.path_str();
    let max = max_bytes.to_string();
    let out = transport::run_script(
        location,
        GATHER_SCRIPT,
        &[&b, "file", &path, rel, &max],
        timeout,
    )
    .map_err(|e| describe(location, &e))?;
    let bundle = Bundle::parse(&b, &out)?;
    if bundle.no_home {
        return Err("home directory is missing".into());
    }
    bundle
        .files
        .get(rel)
        .cloned()
        .ok_or_else(|| format!("{rel}: not a readable file"))
}

/// Text of a file whose first line may be cut by a byte tail.
fn whole_lines(f: &RawFile) -> &str {
    if f.mode == ReadMode::Tail && f.partial() {
        f.content.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &f.content
    }
}

fn harness_like(proc: &str) -> bool {
    proc.split(['\t', '/'])
        .map(str::trim)
        .any(|part| HARNESSES.contains(&part))
}

fn first_line(b: &Bundle, path: &str) -> Option<String> {
    b.text(path)
        .and_then(|t| t.lines().next())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

fn session(b: &Bundle) -> Session {
    let since = b.stat("state/.lock").map(|(_, m)| m);
    let session_id = first_line(b, "state/.lock-session");
    let Some(lock) = b.file("state/.lock") else {
        return Session {
            lock: LockState::Free,
            pid: None,
            process: None,
            session_id,
            since,
            startup_complete: false,
        };
    };
    let line = lock.content.lines().next().unwrap_or("").trim();
    let pid: Option<u32> = if !line.is_empty() && line.bytes().all(|c| c.is_ascii_digit()) {
        line.parse().ok()
    } else {
        None
    };
    let process = pid.and_then(|p| b.procs.get(&p)).cloned();
    let state = match (pid, process.as_deref()) {
        (None, _) => LockState::Unknown,
        (Some(_), None) => LockState::Unknown,
        (Some(_), Some("")) => LockState::Stale,
        (Some(_), Some(p)) if harness_like(p) => LockState::Held,
        (Some(_), Some(_)) => LockState::Unknown,
    };
    let startup_complete = pid.is_some()
        && first_line(b, "state/.session-start-complete") == pid.map(|p| p.to_string());
    Session {
        lock: state,
        pid,
        process: process.filter(|p| !p.is_empty()),
        session_id,
        since,
        startup_complete,
    }
}

fn watcher(b: &Bundle, clock: Option<i64>) -> Watcher {
    let pid: Option<u32> = first_line(b, "state/.watch.lock/pid").and_then(|p| p.parse().ok());
    let alive = pid
        .and_then(|p| b.procs.get(&p))
        .is_some_and(|p| !p.is_empty());
    let beat_age = match (clock, b.stat("state/.last-watcher-beat")) {
        (Some(now), Some((_, m))) => Some((now - m).max(0)),
        _ => None,
    };
    let away = match first_line(b, "state/.afk") {
        Some(l) if l == "quiet" => Some("quiet".to_string()),
        Some(_) => Some("away".to_string()),
        None if b.stat("state/.afk-contract").is_some() => Some("away".to_string()),
        None => None,
    };
    Watcher {
        pid,
        alive,
        beat_age,
        down_marker: first_line(b, "state/.watcher-down"),
        away,
    }
}

fn busy(b: &Bundle, id: &str) -> Option<Busy> {
    let line = first_line(b, &format!("state/{id}.busy-state"))?;
    let mut words = line.split_whitespace();
    if words.next() != Some("v1") {
        return None;
    }
    let fields: BTreeMap<&str, &str> = words.filter_map(|w| w.split_once('=')).collect();
    let gen_file = first_line(b, &format!("state/{id}.busy-gen"));
    let trusted =
        gen_file.as_deref().is_some() && fields.get("gen").copied() == gen_file.as_deref();
    let state = fields.get("state").copied().unwrap_or("unknown");
    Some(Busy {
        state: if trusted { state } else { "unknown" }.to_string(),
        event: fields.get("event").copied().unwrap_or("").to_string(),
        ts: fields.get("ts").and_then(|t| t.parse().ok()),
        trusted,
    })
}

fn pr_in_events(events: &[status::StatusEvent]) -> Option<String> {
    static PR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"https?://[^\s)\]>]+/(?:pull|pulls|merge_requests)/[0-9]+").unwrap()
    });
    events
        .iter()
        .find_map(|e| PR.find(&e.note).map(|m| m.as_str().to_string()))
}

fn task(b: &Bundle, id: &str, backlog: &backlog::Backlog, orphan: bool) -> Task {
    let meta = b
        .text(&format!("state/{id}.meta"))
        .map(Meta::parse)
        .unwrap_or_default();
    let status_path = format!("state/{id}.status");
    let (events, log_partial, status_mtime) = match b.file(&status_path) {
        Some(f) => (status::parse(whole_lines(f)), f.partial(), Some(f.mtime)),
        None => (Vec::new(), false, None),
    };
    let kind = meta.get("kind").map(str::to_string);
    let kind_for_fold = if orphan {
        Some("unknown")
    } else {
        kind.as_deref().or(Some("ship"))
    };
    let open = status::open_decisions(&events, kind_for_fold);
    let state = status::log_state(&events, &open).to_string();
    // A second mate's log relays many tasks' PRs, so only its meta counts.
    let pr = meta
        .pr()
        .map(|u| (u.to_string(), PrSource::Meta))
        .or_else(|| {
            (meta.kind() != "secondmate")
                .then(|| pr_in_events(&events))
                .flatten()
                .map(|u| (u, PrSource::StatusEvent))
        });
    let count = |g: String| b.count(&g).unwrap_or(0);
    Task {
        id: id.to_string(),
        state,
        log_partial,
        status_mtime,
        open,
        pr,
        busy: busy(b, id),
        turn_ended: b.stat(&format!("state/{id}.turn-ended")).map(|(_, m)| m),
        inbox_pending: count(format!("state/{id}.inbox/*.msg")),
        inbox_handled: count(format!("state/{id}.inbox/handled/*.msg")),
        backlog: backlog.find(id).map(|(s, i)| (s, i.title.clone())),
        orphan,
        meta,
        events,
    }
}

fn summary(b: &Bundle, warnings: &mut Vec<String>) -> Option<HomeSummary> {
    let text = b.text("state/home-summary.json")?;
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("state/home-summary.json: {e}"));
            return None;
        }
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let decisions_open = v
        .get("decisions_open")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|d| {
                    let f = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    SummaryDecision {
                        id: f("id"),
                        key: f("key"),
                        verb: f("verb"),
                        summary: f("summary"),
                        source: f("source"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(HomeSummary {
        schema: s("schema").unwrap_or_default(),
        generated_epoch: v.get("generated_epoch").and_then(Value::as_i64),
        valid: v.get("valid").and_then(Value::as_bool),
        state: s("state"),
        reason: s("reason"),
        decisions_open,
        decisions_total: v
            .get("counts")
            .and_then(|c| c.get("decisions_open"))
            .and_then(Value::as_u64),
    })
}

fn doc(b: &Bundle, path: &str) -> Option<Doc> {
    if let Some(f) = b.file(path) {
        return Some(Doc {
            path: path.to_string(),
            size: f.size,
            mtime: f.mtime,
            content: Some(whole_lines(f).to_string()),
            partial: f.partial(),
        });
    }
    b.stats.get(path).map(|&(size, mtime)| Doc {
        path: path.to_string(),
        size,
        mtime,
        content: None,
        partial: true,
    })
}

fn role(b: &Bundle) -> Role {
    let Some(id) = first_line(b, ".fm-secondmate-home") else {
        return Role::Main;
    };
    let parent = b
        .text(".fm-secondmate-parent")
        .map(Meta::parse)
        .unwrap_or_default();
    Role::Secondmate {
        id,
        route: parent.get("route").map(str::to_string),
        parent_home: parent.get("parent_home").map(str::to_string),
    }
}

/// Turns one home's gather output into a snapshot.
pub fn from_bundle(spec: &HomeSpec, b: &Bundle) -> HomeSnapshot {
    let mut snap = blank(spec, Reach::Ok);
    if b.no_home {
        snap.reach = Reach::Missing;
        return snap;
    }
    let clock = b.now();
    snap.clock = clock;
    snap.machine = b.meta.get("host").cloned();
    snap.resolved_path = b.meta.get("pwd").cloned();
    snap.role = role(b);
    snap.session = session(b);
    snap.watcher = watcher(b, clock);

    let mut warnings = Vec::new();
    snap.backlog = b
        .file("data/backlog.md")
        .map(|f| backlog::parse(whole_lines(f)))
        .unwrap_or_default();
    snap.routes = b
        .text("data/secondmates.md")
        .map(routes::parse)
        .unwrap_or_default();
    snap.projects = b
        .text("data/projects.md")
        .map(projects::parse)
        .unwrap_or_default();
    snap.summary = summary(b, &mut warnings);
    snap.ledger = b
        .file("state/fleet-ledger.jsonl")
        .map(|f| ledger::parse(whole_lines(f)))
        .unwrap_or_default();

    // Tasks: every .meta, then status logs that have no meta.
    let state_names = b.list("state");
    let mut ids: Vec<(String, bool)> = state_names
        .iter()
        .filter_map(|n| n.strip_suffix(".meta"))
        .map(|id| (id.to_string(), false))
        .collect();
    for n in state_names {
        if let Some(id) = n.strip_suffix(".status")
            && !ids.iter().any(|(i, _)| i == id)
            && b.file(&format!("state/{id}.status")).is_some()
        {
            ids.push((id.to_string(), true));
        }
    }
    snap.tasks = ids
        .iter()
        .map(|(id, orphan)| task(b, id, &snap.backlog, *orphan))
        .collect();

    let count = |g: &str| b.count(g).unwrap_or(0);
    snap.queues = Queues {
        wake: b.count("state/.wake-queue"),
        operational_inbox: count("state/operational-inbox/*.msg"),
        captain_notes: count("state/inbox/*.note"),
        terminal_outcomes_pending: count("state/terminal-outcomes/*.pending"),
        when_watches: count("state/when/*.spec"),
        pending_replies_open: count("state/pending-replies/open"),
        task_inbox_pending: snap.tasks.iter().map(|t| t.inbox_pending).sum(),
    };

    snap.context = CONTEXT_DOCS.iter().filter_map(|p| doc(b, p)).collect();
    let mut config: Vec<Doc> = b
        .files
        .keys()
        .filter(|k| k.starts_with("config/"))
        .filter_map(|k| doc(b, k))
        .collect();
    for name in b.list("config") {
        let path = format!("config/{name}");
        if !config.iter().any(|d| d.path == path) {
            let (size, mtime) = b.stat(&path).unwrap_or((0, 0));
            config.push(Doc {
                path,
                size,
                mtime,
                content: None,
                partial: true,
            });
        }
    }
    config.sort_by(|a, b| a.path.cmp(&b.path));
    snap.config = config;
    snap.data_docs = b
        .stats
        .keys()
        .filter(|k| k.starts_with("data/") && k.ends_with(".md"))
        .filter(|k| !CONTEXT_DOCS.contains(&k.as_str()))
        .filter_map(|k| doc(b, k))
        .collect();

    for t in &snap.tasks {
        if t.log_partial {
            warnings.push(format!(
                "state/{}.status is large; open decisions use its last 1 MiB only",
                t.id
            ));
        }
    }
    if b.file("data/backlog.md").is_some_and(RawFile::partial) {
        warnings.push("data/backlog.md is over 2 MiB; only its end was read".into());
    }
    snap.warnings = warnings;
    snap
}

/// The homes a snapshot's `data/secondmates.md` lists, as specs.
///
/// A local route lives on the same machine as its parent. A remote route
/// names an ssh alias, which only resolves from this machine when the parent
/// is local; a remote route of a remote home is skipped.
pub fn discovered(parent: &HomeSpec, snap: &HomeSnapshot) -> Vec<HomeSpec> {
    snap.routes
        .iter()
        .filter_map(|r: &Route| {
            let location = match (&r.host, &parent.location) {
                (None, loc) => loc.sibling(&r.home),
                (Some(h), Location::Local(_)) => Location::Ssh {
                    host: h.clone(),
                    path: r.home.clone(),
                },
                (Some(_), Location::Ssh { .. }) => return None,
            };
            Some(HomeSpec {
                name: r.id.clone(),
                location,
                discover: false,
            })
        })
        .collect()
}

/// Groups specs by the machine that holds them, keeping first-seen order.
fn by_machine(specs: Vec<HomeSpec>) -> Vec<Vec<HomeSpec>> {
    let mut groups: Vec<(Option<String>, Vec<HomeSpec>)> = Vec::new();
    for s in specs {
        let host = s.location.host().map(str::to_string);
        match groups.iter_mut().find(|(h, _)| *h == host) {
            Some((_, g)) => g.push(s),
            None => groups.push((host, vec![s])),
        }
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

/// Reads every configured home and the second mates they list.
///
/// Each machine is read on its own thread with one gather run, so a slow or
/// unreachable host delays only its own homes. `on_home` receives every
/// snapshot as soon as it is ready.
pub fn collect_fleet<F>(config: &Config, on_home: F)
where
    F: Fn(HomeSnapshot) + Send + Sync + 'static,
{
    let timeout = Duration::from_secs(config.timeout_secs);
    let seen: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(
        config.homes.iter().map(|h| home_key(&h.location)).collect(),
    ));
    let on_home = Arc::new(on_home);
    let handles: Vec<_> = by_machine(config.homes.clone())
        .into_iter()
        .map(|group| {
            let seen = Arc::clone(&seen);
            let on_home = Arc::clone(&on_home);
            thread::spawn(move || read_tree(group, None, timeout, &seen, &on_home))
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}

fn read_tree<F>(
    group: Vec<HomeSpec>,
    parent: Option<String>,
    timeout: Duration,
    seen: &Arc<Mutex<HashSet<String>>>,
    on_home: &Arc<F>,
) where
    F: Fn(HomeSnapshot) + Send + Sync + 'static,
{
    let snaps = read_homes(&group, timeout);
    let mut children = Vec::new();
    for (spec, mut snap) in group.iter().zip(snaps) {
        snap.discovered_from.clone_from(&parent);
        if spec.discover && snap.reach == Reach::Ok {
            let key = snap.key.clone();
            let mut seen = seen.lock().unwrap();
            for c in discovered(spec, &snap) {
                if seen.insert(home_key(&c.location)) {
                    children.push((key.clone(), c));
                }
            }
        }
        on_home(snap);
    }
    // Children read per (parent, machine): one more run for each.
    let mut groups: Vec<(String, Option<String>, Vec<HomeSpec>)> = Vec::new();
    for (p, c) in children {
        let host = c.location.host().map(str::to_string);
        match groups
            .iter_mut()
            .find(|(gp, gh, _)| *gp == p && *gh == host)
        {
            Some((_, _, g)) => g.push(c),
            None => groups.push((p, host, vec![c])),
        }
    }
    let handles: Vec<_> = groups
        .into_iter()
        .map(|(p, _, g)| {
            let seen = Arc::clone(seen);
            let on_home = Arc::clone(on_home);
            thread::spawn(move || read_tree(g, Some(p), timeout, &seen, &on_home))
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}

/// The UTC date as `YYYY-MM-DD`, for comparing `hold-until` dates.
pub fn today_utc(now: i64) -> String {
    let days = now.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Rows that are in flight in the backlog but have no task record.
pub fn in_flight_without_task(snap: &HomeSnapshot) -> Vec<&backlog::Item> {
    snap.backlog
        .items(Section::InFlight)
        .filter(|i| !snap.tasks.iter().any(|t| t.id == i.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_detection_uses_name_and_path_parts() {
        assert!(harness_like("claude\tclaude"));
        assert!(harness_like(
            "2.1.287\t/home/u/.local/share/claude/versions/2.1.287"
        ));
        assert!(!harness_like("bash\tbash"));
        assert!(!harness_like("claudette\t/usr/bin/claudette"));
    }

    #[test]
    fn utc_date_from_epoch() {
        assert_eq!(today_utc(0), "1970-01-01");
        assert_eq!(today_utc(1790996745), "2026-10-03");
    }
}
