//! Rendering: home list on the left, a header and tabbed content on the
//! right, a key bar at the bottom.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use fleetdeck_core::backlog::Section;
use fleetdeck_core::collect::{in_flight_without_task, now_epoch, today_utc};
use fleetdeck_core::model::{HomeSnapshot, LockState, Reach, Role};

use crate::app::{App, Focus, Tab, Target, age};
use crate::detail::or_dash;

const DIM: Style = Style::new().fg(Color::DarkGray);
const HEAD: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);

/// Pads or cuts `s` to exactly `w` display columns; a cut ends in `…`.
pub fn fit(s: &str, w: usize) -> String {
    let s = s.replace(['\n', '\t'], " ");
    let mut out = String::new();
    let mut used = 0;
    if s.width() <= w {
        out.push_str(&s);
        used = s.width();
    } else if w > 0 {
        for c in s.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > w - 1 {
                break;
            }
            out.push(c);
            used += cw;
        }
        out.push('…');
        used += 1;
    }
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

fn state_style(state: &str) -> Style {
    match state {
        "working" => Style::new().fg(Color::Green),
        "parked" | "needs-decision" => Style::new().fg(Color::Magenta),
        "blocked" | "failed" => Style::new().fg(Color::Red),
        "paused" | "captain-held" => Style::new().fg(Color::Yellow),
        "done" | "resolved" => Style::new().fg(Color::Blue),
        _ => DIM,
    }
}

/// The dot and color that summarize a home's health in the list.
fn home_health(h: &HomeSnapshot) -> (&'static str, Color) {
    match (&h.reach, h.session.lock) {
        (Reach::Unreachable { .. }, _) => ("✖", Color::Red),
        (Reach::Missing, _) => ("✖", Color::Red),
        _ if h.collected_at == 0 => ("…", Color::DarkGray),
        (_, LockState::Held) if h.watcher.beat_fresh() => ("●", Color::Green),
        (_, LockState::Held) => ("●", Color::Yellow),
        (_, LockState::Free) => ("○", Color::DarkGray),
        (_, LockState::Stale) => ("●", Color::Red),
        (_, LockState::Unknown) => ("●", Color::Yellow),
    }
}

/// The content rows of the current view and what each row opens.
pub struct Rows {
    pub lines: Vec<Line<'static>>,
    pub targets: Vec<Option<Target>>,
}

impl Rows {
    fn new() -> Rows {
        Rows {
            lines: Vec::new(),
            targets: Vec::new(),
        }
    }

    fn heading(&mut self, text: String) {
        if !self.lines.is_empty() {
            self.lines.push(Line::raw(""));
            self.targets.push(None);
        }
        self.lines.push(Line::styled(text, HEAD));
        self.targets.push(None);
    }

    fn note(&mut self, text: impl Into<String>) {
        self.lines
            .push(Line::styled(format!("  {}", text.into()), DIM));
        self.targets.push(None);
    }

    fn row(&mut self, spans: Vec<Span<'static>>, target: Target) {
        let mut v = vec![Span::raw("  ")];
        v.extend(spans);
        self.lines.push(Line::from(v));
        self.targets.push(Some(target));
    }
}

fn ago_from(h: &HomeSnapshot, ts: Option<i64>) -> String {
    match (h.clock, ts) {
        (Some(now), Some(t)) => age(now - t),
        _ => "-".into(),
    }
}

pub fn rows(app: &App, width: u16) -> Rows {
    let mut r = Rows::new();
    let Some(h) = app.home() else {
        r.note("waiting for the first read…");
        return r;
    };
    if h.collected_at == 0 && h.reach == Reach::Ok {
        r.note("reading…");
        return r;
    }
    if h.reach != Reach::Ok && h.tasks.is_empty() && h.backlog.sections.is_empty() {
        r.note("no data: this home could not be read");
        return r;
    }
    let w = width.saturating_sub(4) as usize;
    match app.tab {
        Tab::Work => work(&mut r, h, w),
        Tab::Prs => prs(&mut r, app, h, w),
        Tab::Context => context(&mut r, h, w),
        Tab::Activity => activity(&mut r, h, w),
    }
    r
}

fn work(r: &mut Rows, h: &HomeSnapshot, w: usize) {
    r.heading(format!("TASKS ({})", h.tasks.len()));
    let id_w = h
        .tasks
        .iter()
        .map(|t| t.id.width() + if t.orphan { 10 } else { 0 })
        .max()
        .unwrap_or(2)
        .clamp(12, 30);
    if h.tasks.is_empty() {
        r.note("no task records in state/*.meta");
    } else {
        r.note(format!(
            "{} {} {} {} {} {}",
            fit("id", id_w),
            fit("state", 8),
            fit("kind/mode", 18),
            fit("harness/model", 18),
            fit("open", 4),
            "last event"
        ));
    }
    for t in &h.tasks {
        let kind = match t.meta.get("mode") {
            Some(m) if m != t.meta.kind() => format!("{}/{m}", t.meta.kind()),
            _ => t.meta.kind().to_string(),
        };
        let harness_name = t.meta.get("harness").unwrap_or("-");
        let model = t.meta.get("model").unwrap_or("-");
        // `claude/claude-opus-5-5` reads as `claude/opus-5-5`.
        let model = model
            .strip_prefix(&format!("{harness_name}-"))
            .unwrap_or(model);
        let harness = format!("{harness_name}/{model}");
        let last = t.last_event().map_or_else(String::new, |e| {
            format!("{} {}: {}", ago_from(h, e.at), e.verb, e.note)
        });
        let open = if t.open.is_empty() {
            "-".to_string()
        } else {
            t.open.len().to_string()
        };
        let rest = w.saturating_sub(id_w + 8 + 18 + 18 + 4 + 5);
        let mut id = t.id.clone();
        if t.orphan {
            id.push_str(" (no meta)");
        }
        r.row(
            vec![
                Span::raw(fit(&id, id_w)).bold(),
                Span::raw(" "),
                Span::styled(fit(&t.state, 8), state_style(&t.state)),
                Span::raw(" "),
                Span::raw(fit(&kind, 18)),
                Span::raw(" "),
                Span::styled(fit(&harness, 18), DIM),
                Span::raw(" "),
                Span::styled(
                    fit(&open, 4),
                    if t.open.is_empty() {
                        DIM
                    } else {
                        Style::new().fg(Color::Magenta)
                    },
                ),
                Span::raw(" "),
                Span::raw(fit(&last, rest)),
            ],
            Target::Task(t.id.clone()),
        );
    }

    let decisions: Vec<_> = h.open_decisions().collect();
    r.heading(format!("OPEN DECISIONS AND BLOCKERS ({})", decisions.len()));
    if decisions.is_empty() {
        r.note("none open in the status logs");
    }
    for (task, d) in decisions.iter().rev() {
        let rest = w.saturating_sub(16 + 15 + 34 + 5 + 4);
        r.row(
            vec![
                Span::raw(fit(task, 16)),
                Span::raw(" "),
                Span::styled(fit(&d.verb, 15), state_style(&d.verb)),
                Span::raw(" "),
                Span::styled(fit(&d.key, 34), DIM),
                Span::raw(" "),
                Span::styled(fit(&ago_from(h, d.at), 5), DIM),
                Span::raw(" "),
                Span::raw(fit(&d.note, rest)),
            ],
            Target::Decision {
                task: task.to_string(),
                key: d.key.clone(),
            },
        );
    }

    let holds: Vec<_> = h.captain_holds().collect();
    r.heading(format!("CAPTAIN HOLDS ({})", holds.len()));
    if holds.is_empty() {
        r.note("no open captain holds in data/backlog.md");
    }
    for (section, item) in holds {
        let until = item
            .hold_until
            .as_deref()
            .map_or_else(String::new, |u| format!("until {u}"));
        let rest = w.saturating_sub(34 + 17 + 3);
        let reason = item.hold.as_deref().unwrap_or("");
        r.row(
            vec![
                Span::raw(fit(&item.id, 34)).bold(),
                Span::raw(" "),
                Span::styled(fit(&until, 17), DIM),
                Span::raw(" "),
                Span::raw(fit(&format!("{} — {reason}", item.title), rest)),
            ],
            Target::Backlog {
                section,
                id: item.id.clone(),
            },
        );
    }

    let today = today_utc(h.clock.unwrap_or_else(now_epoch));
    for section in [Section::InFlight, Section::Queued, Section::Done] {
        let items: Vec<_> = h.backlog.items(section).collect();
        r.heading(format!("BACKLOG · {} ({})", section.label(), items.len()));
        if items.is_empty() {
            r.note("empty");
        }
        for item in items {
            let mut tags = Vec::new();
            if item.held(section, &today) {
                tags.push(format!(
                    "held:{}",
                    item.hold_kind.as_deref().unwrap_or("hold")
                ));
            }
            if !item.blocked_by.is_empty() {
                tags.push(format!("blocked-by:{}", item.blocked_by.join(",")));
            }
            let date = match section {
                Section::Done => item.closed.as_ref().map(|(v, d)| format!("{v} {d}")),
                _ => item.since.clone(),
            }
            .unwrap_or_default();
            let meta = format!(
                "{}{}",
                item.kind.as_deref().unwrap_or("-"),
                item.repo
                    .as_deref()
                    .map_or_else(String::new, |r| format!("@{r}"))
            );
            let rest = w.saturating_sub(34 + 22 + 17 + 4);
            let mut spans = vec![
                Span::raw(fit(&item.id, 34)),
                Span::raw(" "),
                Span::styled(fit(&meta, 22), DIM),
                Span::raw(" "),
                Span::styled(fit(&date, 17), DIM),
                Span::raw(" "),
            ];
            let tag_text = if tags.is_empty() {
                String::new()
            } else {
                format!("[{}] ", tags.join(" "))
            };
            let tag_w = tag_text.width().min(rest);
            spans.push(Span::styled(
                fit(&tag_text, tag_w),
                Style::new().fg(Color::Yellow),
            ));
            spans.push(Span::raw(fit(&item.title, rest - tag_w)));
            r.row(
                spans,
                Target::Backlog {
                    section,
                    id: item.id.clone(),
                },
            );
        }
    }
    let missing = in_flight_without_task(h);
    if !missing.is_empty() && !h.is_secondmate() {
        r.heading(format!(
            "IN FLIGHT WITHOUT A TASK RECORD ({})",
            missing.len()
        ));
        for item in missing {
            r.row(
                vec![
                    Span::raw(fit(&item.id, 34)),
                    Span::raw(" "),
                    Span::raw(item.title.clone()),
                ],
                Target::Backlog {
                    section: Section::InFlight,
                    id: item.id.clone(),
                },
            );
        }
    }
}

fn prs(r: &mut Rows, app: &App, h: &HomeSnapshot, w: usize) {
    let urls = app.pr_urls();
    let loading = urls.iter().filter(|u| app.prs_loading.contains(*u)).count();
    let state = if loading > 0 {
        format!("gh: reading {loading}…")
    } else {
        "gh: press p to refresh".into()
    };
    r.heading(format!("PULL REQUESTS ({})   {state}", urls.len()));
    if urls.is_empty() {
        r.note("no PR in task meta, status logs or in-flight backlog rows");
        return;
    }
    r.note(format!(
        "{} {} {} {} {} {} {} {}",
        fit("task", 26),
        fit("PR", 34),
        fit("state", 8),
        fit("checks", 12),
        fit("mergeable", 12),
        fit("review", 18),
        fit("threads", 7),
        "read"
    ));
    for url in &urls {
        let task = h
            .tasks
            .iter()
            .find(|t| t.pr.as_ref().is_some_and(|(u, _)| u == url))
            .map(|t| t.id.clone())
            .or_else(|| {
                h.backlog
                    .items(Section::InFlight)
                    .find(|i| i.pr_urls.contains(url))
                    .map(|i| format!("{} (backlog)", i.id))
            })
            .unwrap_or_default();
        let short = url
            .trim_start_matches("https://")
            .trim_start_matches("github.com/")
            .replace("/pull/", "#");
        let mut spans = vec![
            Span::raw(fit(&task, 26)),
            Span::raw(" "),
            Span::raw(fit(&short, 34)),
            Span::raw(" "),
        ];
        match app.prs.get(url) {
            _ if app.prs_loading.contains(url) && !app.prs.contains_key(url) => {
                spans.push(Span::styled("reading…", DIM));
            }
            None => spans.push(Span::styled("not read yet", DIM)),
            Some(p) if p.error.is_some() => spans.push(Span::styled(
                fit(p.error.as_deref().unwrap_or(""), w.saturating_sub(62)),
                Style::new().fg(Color::Red),
            )),
            Some(p) => {
                let st = if p.draft {
                    "DRAFT".to_string()
                } else {
                    p.state.clone()
                };
                let st_style = match st.as_str() {
                    "MERGED" => Style::new().fg(Color::Magenta),
                    "OPEN" => Style::new().fg(Color::Green),
                    _ => DIM,
                };
                let checks_style = if p.checks.fail > 0 {
                    Style::new().fg(Color::Red)
                } else if p.checks.pending > 0 {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().fg(Color::Green)
                };
                let merge_style = match p.mergeable.as_str() {
                    "CONFLICTING" => Style::new().fg(Color::Red),
                    "MERGEABLE" => Style::new().fg(Color::Green),
                    _ => DIM,
                };
                let threads = p
                    .unresolved_threads
                    .map_or_else(|| "?".to_string(), |n| n.to_string());
                let read_age = age(now_epoch() - p.fetched_at);
                spans.extend([
                    Span::styled(fit(&st, 8), st_style),
                    Span::raw(" "),
                    Span::styled(fit(&p.checks.label(), 12), checks_style),
                    Span::raw(" "),
                    Span::styled(fit(or_dash(&p.mergeable), 12), merge_style),
                    Span::raw(" "),
                    Span::raw(fit(or_dash(&p.review_decision), 18)),
                    Span::raw(" "),
                    Span::raw(fit(&threads, 7)),
                    Span::raw(" "),
                    Span::styled(read_age, DIM),
                ]);
            }
        }
        r.row(spans, Target::Pr(url.clone()));
    }
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / 1024.0 / 1024.0)
    }
}

fn context(r: &mut Rows, h: &HomeSnapshot, w: usize) {
    let budget = h
        .config
        .iter()
        .find(|d| d.path == "config/startup-memory-budget")
        .and_then(|d| d.content.as_deref())
        .and_then(|c| c.trim().parse::<u64>().ok());
    let memory: u64 = h
        .context
        .iter()
        .filter(|d| {
            matches!(
                d.path.as_str(),
                "data/captain.md" | "data/captain-shared.md" | "data/learnings.md"
            )
        })
        .map(|d| d.est_tokens())
        .sum();
    let budget_text = match budget {
        Some(b) if memory > b => format!("≈{memory} tokens, OVER the {b} budget"),
        Some(b) => format!("≈{memory} of {b} budget tokens"),
        None => format!("≈{memory} tokens"),
    };
    r.heading(format!("SESSION CONTEXT   startup memory {budget_text}"));
    r.note("loaded into every firstmate session at start, in digest order");
    for d in &h.context {
        let rest = w.saturating_sub(28 + 11 + 13 + 6 + 3);
        let first = d
            .content
            .as_deref()
            .and_then(|c| c.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or("");
        r.row(
            vec![
                Span::raw(fit(&d.path, 28)).bold(),
                Span::raw(" "),
                Span::raw(fit(&size(d.size), 11)),
                Span::raw(" "),
                Span::styled(fit(&format!("≈{} tok", d.est_tokens()), 13), DIM),
                Span::raw(" "),
                Span::styled(fit(&ago_from(h, Some(d.mtime)), 6), DIM),
                Span::styled(fit(first, rest), DIM),
            ],
            Target::Doc(d.path.clone()),
        );
    }

    r.heading(format!("CONFIG ({})", h.config.len()));
    if h.config.is_empty() {
        r.note("no files in config/");
    }
    for d in &h.config {
        let value = match (&d.content, d.size) {
            (Some(c), _) if d.size == 0 => {
                let _ = c;
                "(present)".to_string()
            }
            (Some(c), _) if c.lines().count() <= 1 => c.trim().to_string(),
            (Some(c), _) => format!("{} lines", c.lines().count()),
            (None, _) if d.path.ends_with('/') => "directory".into(),
            (None, _) => "not read (backup or secret-looking name)".into(),
        };
        let rest = w.saturating_sub(40 + 11 + 2);
        r.row(
            vec![
                Span::raw(fit(&d.path, 40)),
                Span::raw(" "),
                Span::styled(fit(&size(d.size), 11), DIM),
                Span::raw(" "),
                Span::raw(fit(&value, rest)),
            ],
            Target::Doc(d.path.clone()),
        );
    }

    r.heading(format!("PROJECTS ({})  data/projects.md", h.projects.len()));
    for p in &h.projects {
        let mode = format!(
            "{}{}{}",
            p.mode,
            if p.yolo { " +yolo" } else { "" },
            p.forge
                .as_deref()
                .map_or_else(String::new, |f| format!(" forge={f}"))
        );
        let rest = w.saturating_sub(24 + 28 + 2);
        r.row(
            vec![
                Span::raw(fit(&p.name, 24)).bold(),
                Span::raw(" "),
                Span::styled(fit(&mode, 28), Style::new().fg(Color::Yellow)),
                Span::raw(" "),
                Span::styled(fit(&p.description, rest), DIM),
            ],
            Target::Doc("data/projects.md".into()),
        );
    }

    if !h.routes.is_empty() {
        r.heading(format!(
            "SECOND MATES ({})  data/secondmates.md",
            h.routes.len()
        ));
        for route in &h.routes {
            let place = match &route.host {
                Some(host) => format!("{host}:{}", route.home),
                None => route.home.clone(),
            };
            let rest = w.saturating_sub(18 + 50 + 2);
            r.row(
                vec![
                    Span::raw(fit(&route.id, 18)).bold(),
                    Span::raw(" "),
                    Span::styled(fit(&place, 50), DIM),
                    Span::raw(" "),
                    Span::raw(fit(&route.scope, rest)),
                ],
                Target::Doc("data/secondmates.md".into()),
            );
        }
    }

    r.heading(format!("OTHER DATA DOCUMENTS ({})", h.data_docs.len()));
    for d in &h.data_docs {
        r.row(
            vec![
                Span::raw(fit(&d.path, 52)),
                Span::raw(" "),
                Span::styled(fit(&size(d.size), 11), DIM),
                Span::raw(" "),
                Span::styled(ago_from(h, Some(d.mtime)), DIM),
            ],
            Target::Doc(d.path.clone()),
        );
    }
}

fn activity(r: &mut Rows, h: &HomeSnapshot, w: usize) {
    let q = &h.queues;
    r.heading("QUEUES (counted, never drained)".into());
    r.note(format!(
        "wake queue {} · operational inbox {} · task inboxes {} pending · captain notes {}",
        q.wake.map_or_else(|| "-".to_string(), |n| n.to_string()),
        q.operational_inbox,
        q.task_inbox_pending,
        q.captain_notes,
    ));
    r.note(format!(
        "open pending replies {} · undelivered terminal outcomes {} · when-watches {}",
        q.pending_replies_open, q.terminal_outcomes_pending, q.when_watches
    ));

    let mut events: Vec<_> = h
        .tasks
        .iter()
        .flat_map(|t| t.events.iter().map(move |e| (t, e)))
        .filter(|(_, e)| e.at.is_some())
        .collect();
    events.sort_by_key(|(_, e)| std::cmp::Reverse(e.at));
    events.truncate(60);
    r.heading(format!("RECENT STATUS EVENTS ({})", events.len()));
    if events.is_empty() {
        r.note("no stamped status events");
    }
    for (t, e) in events {
        let rest = w.saturating_sub(6 + 26 + 15 + 3);
        r.row(
            vec![
                Span::styled(fit(&ago_from(h, e.at), 6), DIM),
                Span::raw(fit(&t.id, 26)),
                Span::raw(" "),
                Span::styled(fit(&e.verb, 15), state_style(&e.verb)),
                Span::raw(" "),
                Span::raw(fit(&e.note, rest)),
            ],
            Target::Event {
                task: t.id.clone(),
                line_no: e.line_no,
            },
        );
    }

    r.heading(format!(
        "FLEET LEDGER  state/fleet-ledger.jsonl  (last {})",
        h.ledger.len()
    ));
    if h.ledger.is_empty() {
        r.note("no ledger (config/fleet-ledger is off, or nothing recorded yet)");
    }
    for (i, e) in h.ledger.iter().enumerate().rev() {
        let rest = w.saturating_sub(6 + 26 + 18 + 3);
        let ev = e.event.trim_start_matches("task.");
        r.row(
            vec![
                Span::styled(fit(&ago_from(h, Some(e.ts)), 6), DIM),
                Span::raw(fit(&e.task, 26)),
                Span::raw(" "),
                Span::styled(fit(ev, 18), Style::new().fg(Color::Cyan)),
                Span::raw(" "),
                Span::raw(fit(&e.summary, rest)),
            ],
            Target::Ledger(i),
        );
    }
}

fn header(h: &HomeSnapshot) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let role = match &h.role {
        Role::Main => "main home".to_string(),
        Role::Secondmate { id, .. } => format!("second mate {id}"),
    };
    let place = match &h.host {
        Some(host) => format!("{host}:{}", h.path),
        None => h.path.clone(),
    };
    let machine = h
        .machine
        .as_deref()
        .map_or_else(String::new, |m| format!(" · on {m}"));
    lines.push(Line::from(vec![
        Span::raw(h.name.clone()).bold(),
        Span::styled(format!("  {role} · {place}{machine}"), DIM),
    ]));
    match &h.reach {
        Reach::Unreachable { error } => lines.push(Line::styled(
            format!("unreachable: {error}"),
            Style::new().fg(Color::Red),
        )),
        Reach::Missing => lines.push(Line::styled(
            "the home directory does not exist",
            Style::new().fg(Color::Red),
        )),
        Reach::Ok => {}
    }
    if h.collected_at == 0 {
        return lines;
    }

    let s = &h.session;
    let (lock_text, lock_color) = match s.lock {
        LockState::Held => (
            format!(
                "session held by pid {} ({})",
                s.pid.unwrap_or(0),
                s.process
                    .as_deref()
                    .and_then(|p| p.split('\t').next())
                    .unwrap_or("?")
            ),
            Color::Green,
        ),
        LockState::Free => ("no session (lock free)".into(), Color::DarkGray),
        LockState::Stale => (
            format!("stale lock: pid {} is gone", s.pid.unwrap_or(0)),
            Color::Red,
        ),
        LockState::Unknown => ("session lock unclear".into(), Color::Yellow),
    };
    let since = s
        .since
        .map_or_else(String::new, |t| format!(" for {}", ago_from(h, Some(t))));
    let startup = if s.lock == LockState::Held && !s.startup_complete {
        " · startup not finished"
    } else {
        ""
    };
    let wt = &h.watcher;
    let (watch_text, watch_color) = match (wt.alive, wt.beat_age) {
        (true, Some(a)) if wt.beat_fresh() => {
            (format!("watcher up, beat {}", age(a)), Color::Green)
        }
        (_, Some(a)) => (format!("watcher beat {} ago", age(a)), Color::Yellow),
        (true, None) => ("watcher up, no beat".to_string(), Color::Yellow),
        (false, None) => ("no watcher".to_string(), Color::DarkGray),
    };
    let mut l2 = vec![
        Span::styled(lock_text, Style::new().fg(lock_color)),
        Span::styled(format!("{since}{startup}"), DIM),
        Span::raw(" · "),
        Span::styled(watch_text, Style::new().fg(watch_color)),
    ];
    if let Some(a) = &wt.away {
        l2.push(Span::raw(" · "));
        l2.push(Span::styled(
            format!("away mode: {a}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines.push(Line::from(l2));

    let mut l3 = Vec::new();
    if let Some(sum) = &h.summary {
        let st = sum.state.clone().unwrap_or_else(|| "?".into());
        let valid = match sum.valid {
            Some(false) => " (invalid)",
            _ => "",
        };
        l3.push(Span::styled("home summary: ", DIM));
        l3.push(Span::styled(
            format!("{st}{valid}"),
            match st.as_str() {
                "captain_decision" => Style::new().fg(Color::Magenta),
                "active_child_work" => Style::new().fg(Color::Green),
                "no_active_work" => DIM,
                _ => Style::new().fg(Color::Yellow),
            },
        ));
        if let Some(t) = sum.decisions_total {
            l3.push(Span::styled(format!(", {t} decisions open"), DIM));
        }
        l3.push(Span::styled(
            format!(
                ", written {} ago",
                ago_from(h, sum.generated_epoch).replace('-', "?")
            ),
            DIM,
        ));
        l3.push(Span::raw(" · "));
    }
    let open: usize = h.tasks.iter().map(|t| t.open.len()).sum();
    l3.push(Span::raw(format!(
        "{} tasks · {} open in logs · {} in flight · {} queued",
        h.tasks.len(),
        open,
        h.backlog.count(Section::InFlight),
        h.backlog.count(Section::Queued)
    )));
    lines.push(Line::from(l3));
    for warn in h.warnings.iter().take(2) {
        lines.push(Line::styled(
            format!("! {warn}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines
}

fn home_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .homes
        .iter()
        .map(|h| {
            let (dot, color) = home_health(h);
            let indent = if h.discovered_from.is_some() {
                "  └ "
            } else {
                ""
            };
            let open: usize = h.tasks.iter().map(|t| t.open.len()).sum();
            let mut spans = vec![
                Span::raw(indent),
                Span::styled(format!("{dot} "), Style::new().fg(color)),
                Span::raw(h.name.clone()),
            ];
            if let Some(host) = &h.host
                && h.discovered_from.is_none()
                && !h.name.starts_with(host.as_str())
            {
                spans.push(Span::styled(format!(" @{host}"), DIM));
            }
            let mut info = Vec::new();
            if !h.tasks.is_empty() {
                info.push(format!("{}t", h.tasks.len()));
            }
            if open > 0 {
                info.push(format!("{open}?"));
            }
            if !info.is_empty() {
                spans.push(Span::styled(format!(" {}", info.join(" ")), DIM));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let focused = app.focus == Focus::Homes && app.detail.is_none();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Homes ")
        .border_style(if focused {
            Style::new().fg(Color::Cyan)
        } else {
            DIM
        });
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(if app.homes.is_empty() {
        None
    } else {
        Some(app.home_sel)
    });
    f.render_stateful_widget(list, area, &mut state);
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let [main, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let list_w = app
        .homes
        .iter()
        .map(|h| h.name.width() + if h.discovered_from.is_some() { 4 } else { 0 } + 12)
        .max()
        .unwrap_or(20)
        .clamp(22, 40) as u16;
    let [left, right] =
        Layout::horizontal([Constraint::Length(list_w), Constraint::Min(20)]).areas(main);
    home_list(f, app, left);

    let head_lines = app.home().map(header).unwrap_or_default();
    let [head_area, tabs_area, body_area] = Layout::vertical([
        Constraint::Length(head_lines.len() as u16 + 1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(right);
    f.render_widget(
        Paragraph::new(head_lines).block(Block::default().borders(Borders::TOP).border_style(DIM)),
        head_area,
    );
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| Line::raw(format!("{} {}", i + 1, t.title())))
        .collect();
    let selected = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    f.render_widget(
        Tabs::new(titles)
            .select(selected)
            .highlight_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .style(DIM),
        tabs_area,
    );

    let content = rows(app, body_area.width);
    let focused = app.focus == Focus::Content && app.detail.is_none();
    let items: Vec<ListItem> = content.lines.into_iter().map(ListItem::new).collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if focused {
                    Style::new().fg(Color::Cyan)
                } else {
                    DIM
                }),
        )
        .highlight_style(if focused {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        });
    let mut state = ListState::default().with_selected(Some(app.content_sel));
    f.render_stateful_widget(list, body_area, &mut state);

    footer_bar(f, app, footer);
    if let Some(d) = &app.detail {
        detail(f, d, area);
    }
    if app.help {
        help(f, area);
    }
}

fn footer_bar(f: &mut Frame, app: &App, area: Rect) {
    let refresh = if app.refreshing {
        "reading…".to_string()
    } else {
        match app.last_refresh {
            Some(t) => format!("read {} ago", age(now_epoch() - t)),
            None => "not read yet".into(),
        }
    };
    let keys = if app.detail.is_some() {
        "↑↓/PgUp/PgDn scroll · g/G top/bottom · Esc close"
    } else {
        "↑↓ move · Tab pane · 1-4/[ ] tabs · Enter open · r refresh · p PRs · ? help · q quit"
    };
    let mut spans = vec![
        Span::styled(" fleetdeck ", Style::new().fg(Color::Black).bg(Color::Cyan)),
        Span::styled(
            " read-only ",
            Style::new().fg(Color::Black).bg(Color::Green),
        ),
    ];
    if app.paused {
        spans.push(Span::styled(
            " paused ",
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    spans.extend([
        Span::raw(format!(
            " {refresh} · every {}s · ",
            app.config.refresh_secs
        )),
        Span::styled(keys, DIM),
        Span::raw(
            app.message
                .clone()
                .map_or_else(String::new, |m| format!(" · {m}")),
        ),
    ]);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn centered(area: Rect, pw: u16, ph: u16) -> Rect {
    let w = area.width * pw / 100;
    let h = area.height * ph / 100;
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn detail(f: &mut Frame, d: &crate::app::Detail, area: Rect) {
    let rect = centered(area, 92, 88);
    f.render_widget(Clear, rect);
    let body = if d.loading.is_some() {
        "reading…"
    } else {
        d.body.as_str()
    };
    let p = Paragraph::new(body.to_string())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", d.title))
                .title_bottom(Line::styled(" Esc close ", DIM))
                .border_style(Style::new().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false })
        .scroll((d.scroll, 0));
    f.render_widget(p, rect);
}

fn help(f: &mut Frame, area: Rect) {
    let rect = centered(area, 60, 70);
    f.render_widget(Clear, rect);
    let text = "\
fleetdeck is read-only: it never writes, moves or deletes anything in a home.

Keys
  ↑/↓ or j/k     move in the focused pane
  Tab, ←/→       switch between the home list and the content
  1 2 3 4        Work, PRs, Context, Activity tabs ([ and ] cycle)
  Enter          open the selected task, decision, row, file or event
  r              read every home again now (also runs on a timer)
  p              read PR status with gh (read-only calls)
  PgUp/PgDn g/G  page, top, bottom
  q              quit

Home list
  ● green   a session holds the home and the watcher beat is fresh
  ● yellow  a session holds the home, watcher beat is late or unclear
  ● red     stale lock (the session pid is gone)
  ○ grey    no session holds the home
  ✖ red     unreachable or missing
  Nt / N?   task records / open decisions and blockers in status logs

Press any key to close.";
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help ")
                    .border_style(Style::new().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_cuts() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("a\nb", 3), "a b");
        assert_eq!(fit("한글", 3), "한…");
        assert_eq!(fit("x", 0), "");
    }
}
