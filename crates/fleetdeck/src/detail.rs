//! Text for the detail view of a task, decision, backlog row, PR, file or
//! ledger event.

use std::collections::HashMap;
use std::fmt::Write;

use fleetdeck_core::model::{HomeSnapshot, PrSource};
use fleetdeck_core::pr::PrStatus;

use crate::app::{Target, age};

/// Largest file the detail view reads on demand.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

fn ago(home: &HomeSnapshot, ts: Option<i64>) -> String {
    match (home.clock, ts) {
        (Some(now), Some(t)) => format!("{} ago", age(now - t)),
        _ => "time unknown".into(),
    }
}

/// Returns (title, body, path to read in the background).
pub fn build(
    home: &HomeSnapshot,
    target: &Target,
    prs: &HashMap<String, PrStatus>,
) -> (String, String, Option<String>) {
    let mut b = String::new();
    match target {
        Target::Task(id) | Target::Event { task: id, .. } | Target::Decision { task: id, .. } => {
            let Some(t) = home.tasks.iter().find(|t| &t.id == id) else {
                return (id.clone(), "task no longer present".into(), None);
            };
            if let Target::Event { line_no, .. } = target
                && let Some(e) = t.events.iter().find(|e| e.line_no == *line_no)
            {
                let _ = writeln!(b, "{} · {}", e.verb, ago(home, e.at));
                if let Some(k) = &e.key {
                    let _ = writeln!(b, "key: {k}");
                }
                if let Some(c) = &e.corr {
                    let _ = writeln!(b, "corr: {c}");
                }
                let _ = writeln!(b, "\n{}", e.raw);
                return (format!("{id} · status event"), b, None);
            }
            if let Target::Decision { key, .. } = target
                && let Some(d) = t.open.iter().find(|d| &d.key == key)
            {
                let _ = writeln!(b, "{} [key={}] · {}", d.verb, d.key, ago(home, d.at));
                let _ = writeln!(b, "\n{}", d.note);
                let _ = writeln!(
                    b,
                    "\nOpen until a `resolved [key={}]` line lands in state/{}.status.",
                    d.key, t.id
                );
                return (format!("{id} · open {}", d.verb), b, None);
            }
            let _ = writeln!(b, "state: {}   kind: {}", t.state, t.meta.kind());
            if let Some((section, title)) = &t.backlog {
                let _ = writeln!(b, "backlog: {} · {title}", section.label());
            }
            if let Some((url, src)) = &t.pr {
                let src = match src {
                    PrSource::Meta => "meta pr=",
                    PrSource::StatusEvent => "first PR link in the status log",
                };
                let _ = writeln!(b, "PR: {url}  (from {src})");
            }
            if let Some(busy) = &t.busy {
                let _ = writeln!(
                    b,
                    "busy: {} ({}{}, {})",
                    busy.state,
                    busy.event,
                    if busy.trusted {
                        ""
                    } else {
                        ", stale generation"
                    },
                    ago(home, busy.ts)
                );
            }
            let _ = writeln!(
                b,
                "steering inbox: {} pending, {} handled",
                t.inbox_pending, t.inbox_handled
            );
            let _ = writeln!(b, "\nMETA  state/{}.meta", t.id);
            if t.meta.pairs.is_empty() {
                let _ = writeln!(b, "  (no meta file)");
            }
            for (k, v) in &t.meta.pairs {
                let _ = writeln!(b, "  {k} = {v}");
            }
            let _ = writeln!(b, "\nOPEN DECISIONS AND BLOCKERS ({})", t.open.len());
            for d in t.open.iter().rev() {
                let _ = writeln!(
                    b,
                    "  {} [{}] {}: {}",
                    d.verb,
                    d.key,
                    ago(home, d.at),
                    d.note
                );
            }
            let _ = writeln!(
                b,
                "\nSTATUS EVENTS  state/{}.status  (newest first{})",
                t.id,
                if t.log_partial {
                    "; only the last 1 MiB was read"
                } else {
                    ""
                }
            );
            for e in t.events.iter().rev() {
                let when = match e.at {
                    Some(_) => ago(home, e.at),
                    None => "-".into(),
                };
                let _ = writeln!(b, "  [{when}] {}", e.raw);
            }
            (format!("task {id}"), b, None)
        }
        Target::Backlog { section, id } => {
            let Some((_, item)) = home.backlog.find(id) else {
                return (id.clone(), "backlog row no longer present".into(), None);
            };
            let _ = writeln!(b, "{} · {}", section.label(), item.title);
            let field = |b: &mut String, name: &str, v: &Option<String>| {
                if let Some(v) = v {
                    let _ = writeln!(b, "{name}: {v}");
                }
            };
            field(&mut b, "repo", &item.repo);
            field(&mut b, "kind", &item.kind);
            field(&mut b, "since", &item.since);
            if let Some((verb, date)) = &item.closed {
                let _ = writeln!(b, "{verb}: {date}");
            }
            field(&mut b, "hold", &item.hold);
            field(&mut b, "hold kind", &item.hold_kind);
            field(&mut b, "hold until", &item.hold_until);
            field(&mut b, "hold set", &item.hold_set);
            if !item.blocked_by.is_empty() {
                let _ = writeln!(b, "blocked by: {}", item.blocked_by.join(", "));
            }
            for u in &item.pr_urls {
                let _ = writeln!(b, "PR: {u}");
            }
            let _ = writeln!(b, "\n{}", item.body);
            let _ = writeln!(b, "\n(data/backlog.md line {})", item.line_no + 1);
            (format!("backlog {id}"), b, None)
        }
        Target::Pr(url) => {
            let _ = writeln!(b, "{url}\n");
            match prs.get(url) {
                None => {
                    let _ = writeln!(b, "Not fetched yet. Press p to read it with gh.");
                }
                Some(p) if p.error.is_some() => {
                    let _ = writeln!(b, "gh error: {}", p.error.as_deref().unwrap_or(""));
                }
                Some(p) => {
                    let _ = writeln!(b, "{}", p.title);
                    let _ = writeln!(
                        b,
                        "state: {}{}",
                        p.state,
                        if p.draft { " (draft)" } else { "" }
                    );
                    let _ = writeln!(b, "mergeable: {} / {}", p.mergeable, p.merge_state);
                    let _ = writeln!(b, "review: {}", or_dash(&p.review_decision));
                    let _ = writeln!(
                        b,
                        "checks: {} passing, {} failing, {} pending",
                        p.checks.pass, p.checks.fail, p.checks.pending
                    );
                    let threads = p
                        .unresolved_threads
                        .map_or_else(|| "unknown".to_string(), |n| n.to_string());
                    let _ = writeln!(b, "unresolved review threads: {threads}");
                }
            }
            let users: Vec<&str> = home
                .tasks
                .iter()
                .filter(|t| t.pr.as_ref().is_some_and(|(u, _)| u == url))
                .map(|t| t.id.as_str())
                .collect();
            if !users.is_empty() {
                let _ = writeln!(b, "\ntasks: {}", users.join(", "));
            }
            ("pull request".into(), b, None)
        }
        Target::Doc(path) => {
            let doc = home
                .context
                .iter()
                .chain(home.config.iter())
                .chain(home.data_docs.iter())
                .find(|d| &d.path == path);
            match doc.and_then(|d| d.content.as_ref().filter(|_| !d.partial).map(|c| (d, c))) {
                Some((_, content)) => (path.clone(), content.clone(), None),
                None => (path.clone(), "reading…".into(), Some(path.clone())),
            }
        }
        Target::Ledger(i) => {
            let Some(e) = home.ledger.get(*i) else {
                return ("ledger".into(), "event no longer present".into(), None);
            };
            let pretty = serde_json::from_str::<serde_json::Value>(&e.raw)
                .and_then(|v| serde_json::to_string_pretty(&v))
                .unwrap_or_else(|_| e.raw.clone());
            let _ = writeln!(b, "{} · {} · {}\n", e.event, e.task, ago(home, Some(e.ts)));
            let _ = writeln!(b, "{pretty}");
            ("fleet ledger event".into(), b, None)
        }
    }
}

pub fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}
