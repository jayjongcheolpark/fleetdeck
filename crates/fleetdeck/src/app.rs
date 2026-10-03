//! TUI state and key handling. Rendering lives in `ui.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use fleetdeck_core::backlog::Section;
use fleetdeck_core::collect::now_epoch;
use fleetdeck_core::config::Config;
use fleetdeck_core::model::{HomeSnapshot, Reach};
use fleetdeck_core::pr::PrStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Work,
    Prs,
    Context,
    Activity,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Work, Tab::Prs, Tab::Context, Tab::Activity];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Work => "Work",
            Tab::Prs => "PRs",
            Tab::Context => "Context",
            Tab::Activity => "Activity",
        }
    }

    pub fn from_name(s: &str) -> Option<Tab> {
        Tab::ALL
            .into_iter()
            .find(|t| t.title().eq_ignore_ascii_case(s))
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Homes,
    Content,
}

/// What a content row points at; `Enter` opens it in the detail view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Task(String),
    Decision { task: String, key: String },
    Backlog { section: Section, id: String },
    Pr(String),
    Doc(String),
    Ledger(usize),
    Event { task: String, line_no: usize },
}

/// The full-screen view of one file, task or event.
#[derive(Debug, Clone)]
pub struct Detail {
    pub title: String,
    pub body: String,
    pub scroll: u16,
    /// Set while the file is read in the background: (home key, path).
    pub loading: Option<(String, String)>,
}

/// Work that the main loop runs on a background thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Refresh,
    ReadFile { home: String, path: String },
    FetchPrs(Vec<String>),
}

pub struct App {
    pub config: Config,
    /// Homes in display order: configured order, second mates after parents.
    pub homes: Vec<HomeSnapshot>,
    pub home_sel: usize,
    pub tab: Tab,
    pub focus: Focus,
    pub content_sel: usize,
    pub detail: Option<Detail>,
    pub help: bool,
    pub refreshing: bool,
    /// The terminal reported that it lost focus, so the timed refresh waits.
    /// A terminal that sends no focus events never sets this.
    pub paused: bool,
    pub last_refresh: Option<i64>,
    pub prs: HashMap<String, PrStatus>,
    pub prs_loading: HashSet<String>,
    pub prs_requested: HashSet<String>,
    pub message: Option<String>,
    pub quit: bool,
    /// Requests the main loop must start.
    pub requests: Vec<Request>,
}

impl App {
    pub fn new(config: Config) -> App {
        App {
            config,
            homes: Vec::new(),
            home_sel: 0,
            tab: Tab::Work,
            focus: Focus::Homes,
            content_sel: 0,
            detail: None,
            help: false,
            refreshing: false,
            paused: false,
            last_refresh: None,
            prs: HashMap::new(),
            prs_loading: HashSet::new(),
            prs_requested: HashSet::new(),
            message: None,
            quit: false,
            requests: Vec::new(),
        }
    }

    pub fn home(&self) -> Option<&HomeSnapshot> {
        self.homes.get(self.home_sel)
    }

    /// Stores a new snapshot. An unreachable read keeps the last good data
    /// and records the error, so a flaky host does not blank its view.
    pub fn upsert(&mut self, mut snap: HomeSnapshot) {
        let selected_key = self.home().map(|h| h.key.clone());
        if let Some(i) = self.homes.iter().position(|h| h.key == snap.key) {
            let old = &self.homes[i];
            if matches!(snap.reach, Reach::Unreachable { .. }) && old.reach == Reach::Ok {
                let mut kept = old.clone();
                kept.reach = snap.reach;
                kept.read_ms = snap.read_ms;
                if !kept.warnings.iter().any(|w| w.starts_with("showing data")) {
                    kept.warnings.insert(
                        0,
                        format!("showing data read at {}", clock_hm(old.collected_at)),
                    );
                }
                snap = kept;
            }
            snap.discovered_from = snap
                .discovered_from
                .or_else(|| self.homes[i].discovered_from.clone());
            self.homes[i] = snap;
        } else {
            let pos = match &snap.discovered_from {
                Some(parent) => match self.homes.iter().position(|h| &h.key == parent) {
                    Some(p) => {
                        let mut end = p + 1;
                        while end < self.homes.len()
                            && self.homes[end].discovered_from.as_deref() == Some(parent)
                        {
                            end += 1;
                        }
                        end
                    }
                    None => self.homes.len(),
                },
                None => {
                    // Keep configured homes in config order.
                    let order = |k: &str| {
                        self.config
                            .homes
                            .iter()
                            .position(|h| fleetdeck_core::collect::home_key(&h.location) == k)
                            .unwrap_or(usize::MAX)
                    };
                    let mine = order(&snap.key);
                    self.homes
                        .iter()
                        .position(|h| h.discovered_from.is_none() && order(&h.key) > mine)
                        .unwrap_or(self.homes.len())
                }
            };
            self.homes.insert(pos, snap);
        }
        if let Some(k) = selected_key
            && let Some(i) = self.homes.iter().position(|h| h.key == k)
        {
            self.home_sel = i;
        }
    }

    pub fn refresh_finished(&mut self) {
        self.refreshing = false;
        self.last_refresh = Some(now_epoch());
    }

    pub fn request_refresh(&mut self) {
        if !self.refreshing {
            self.refreshing = true;
            self.requests.push(Request::Refresh);
        }
    }

    /// True when the timed refresh must start: `since_start` has passed
    /// since the last refresh started, no refresh runs, and the terminal has focus.
    pub fn refresh_due(&self, since_start: Duration) -> bool {
        !self.paused
            && !self.refreshing
            && since_start >= Duration::from_secs(self.config.refresh_secs)
    }

    pub fn focus_lost(&mut self) {
        self.paused = true;
    }

    /// Reads every home now; the timer starts again from this refresh.
    pub fn focus_gained(&mut self) {
        if self.paused {
            self.paused = false;
            self.request_refresh();
        }
    }

    /// PR URLs of the selected home: task PRs, then in-flight backlog links.
    pub fn pr_urls(&self) -> Vec<String> {
        let Some(h) = self.home() else {
            return Vec::new();
        };
        let mut urls: Vec<String> = h
            .tasks
            .iter()
            .filter_map(|t| t.pr.as_ref().map(|(u, _)| u.clone()))
            .collect();
        for item in h.backlog.items(Section::InFlight) {
            urls.extend(item.pr_urls.iter().cloned());
        }
        let mut seen = HashSet::new();
        urls.retain(|u| seen.insert(u.clone()));
        urls
    }

    pub fn request_prs(&mut self, force: bool) {
        let urls: Vec<String> = self
            .pr_urls()
            .into_iter()
            .filter(|u| !self.prs_loading.contains(u))
            .filter(|u| force || !self.prs_requested.contains(u))
            .collect();
        if urls.is_empty() {
            return;
        }
        for u in &urls {
            self.prs_loading.insert(u.clone());
            self.prs_requested.insert(u.clone());
        }
        self.requests.push(Request::FetchPrs(urls));
    }

    pub fn pr_done(&mut self, status: PrStatus) {
        self.prs_loading.remove(&status.url);
        self.prs.insert(status.url.clone(), status);
    }

    pub fn file_done(&mut self, home: &str, path: &str, result: Result<String, String>) {
        if let Some(d) = self.detail.as_mut()
            && d.loading
                .as_ref()
                .is_some_and(|(h, p)| h == home && p == path)
        {
            d.loading = None;
            d.body = match result {
                Ok(text) => text,
                Err(e) => format!("cannot read {path}: {e}"),
            };
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.content_sel = 0;
        }
        if tab == Tab::Prs {
            self.request_prs(false);
        }
    }

    fn move_home(&mut self, delta: isize) {
        if self.homes.is_empty() {
            return;
        }
        let n = self.homes.len() as isize;
        let next = (self.home_sel as isize + delta).clamp(0, n - 1) as usize;
        if next != self.home_sel {
            self.home_sel = next;
            self.content_sel = 0;
            if self.tab == Tab::Prs {
                self.request_prs(false);
            }
        }
    }

    /// Moves the content selection over selectable rows only.
    pub fn move_content(&mut self, targets: &[Option<Target>], delta: isize) {
        let selectable: Vec<usize> = targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_some())
            .map(|(i, _)| i)
            .collect();
        if selectable.is_empty() {
            self.content_sel = 0;
            return;
        }
        let cur = selectable
            .iter()
            .position(|&i| i >= self.content_sel)
            .unwrap_or(selectable.len() - 1) as isize;
        let next = (cur + delta).clamp(0, selectable.len() as isize - 1) as usize;
        self.content_sel = selectable[next];
    }

    pub fn open(&mut self, target: &Target) {
        let Some(home) = self.home() else { return };
        let (title, body, loading) = crate::detail::build(home, target, &self.prs);
        let key = home.key.clone();
        if let Some(path) = &loading {
            self.requests.push(Request::ReadFile {
                home: key.clone(),
                path: path.clone(),
            });
        }
        self.detail = Some(Detail {
            title,
            body,
            scroll: 0,
            loading: loading.map(|p| (key, p)),
        });
    }

    /// Handles one key. `targets` are the content rows of the current view.
    pub fn on_key(&mut self, key: Key, targets: &[Option<Target>], page: u16) {
        if key == Key::CtrlC {
            self.quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if let Some(d) = self.detail.as_mut() {
            match key {
                Key::Esc | Key::Char('q') | Key::Enter | Key::Left | Key::Char('h') => {
                    self.detail = None
                }
                Key::Down | Key::Char('j') => d.scroll = d.scroll.saturating_add(1),
                Key::Up | Key::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                Key::PageDown | Key::Char(' ') => d.scroll = d.scroll.saturating_add(page),
                Key::PageUp => d.scroll = d.scroll.saturating_sub(page),
                Key::Home | Key::Char('g') => d.scroll = 0,
                Key::End | Key::Char('G') => {
                    d.scroll = d.body.lines().count().saturating_sub(page as usize) as u16
                }
                _ => {}
            }
            return;
        }
        match key {
            Key::Char('q') | Key::CtrlC => self.quit = true,
            Key::Char('?') => self.help = true,
            Key::Char('r') => self.request_refresh(),
            Key::Char('p') => {
                self.set_tab(Tab::Prs);
                self.request_prs(true);
            }
            Key::Char('1') => self.set_tab(Tab::Work),
            Key::Char('2') => self.set_tab(Tab::Prs),
            Key::Char('3') => self.set_tab(Tab::Context),
            Key::Char('4') => self.set_tab(Tab::Activity),
            Key::Tab => {
                self.focus = match self.focus {
                    Focus::Homes => Focus::Content,
                    Focus::Content => Focus::Homes,
                };
                if self.focus == Focus::Content {
                    self.move_content(targets, 0);
                }
            }
            Key::BackTab | Key::Char('[') => {
                let i = (self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len();
                self.set_tab(Tab::ALL[i]);
            }
            Key::Char(']') => {
                let i = (self.tab.index() + 1) % Tab::ALL.len();
                self.set_tab(Tab::ALL[i]);
            }
            Key::Right | Key::Char('l') => {
                if self.focus == Focus::Homes {
                    self.focus = Focus::Content;
                    self.move_content(targets, 0);
                }
            }
            Key::Left | Key::Char('h') | Key::Esc => self.focus = Focus::Homes,
            Key::Down | Key::Char('j') => match self.focus {
                Focus::Homes => self.move_home(1),
                Focus::Content => self.move_content(targets, 1),
            },
            Key::Up | Key::Char('k') => match self.focus {
                Focus::Homes => self.move_home(-1),
                Focus::Content => self.move_content(targets, -1),
            },
            Key::PageDown => match self.focus {
                Focus::Homes => self.move_home(page as isize),
                Focus::Content => self.move_content(targets, page as isize),
            },
            Key::PageUp => match self.focus {
                Focus::Homes => self.move_home(-(page as isize)),
                Focus::Content => self.move_content(targets, -(page as isize)),
            },
            Key::Home | Key::Char('g') => match self.focus {
                Focus::Homes => self.move_home(-(self.homes.len() as isize)),
                Focus::Content => self.move_content(targets, -(targets.len() as isize)),
            },
            Key::End | Key::Char('G') => match self.focus {
                Focus::Homes => self.move_home(self.homes.len() as isize),
                Focus::Content => self.move_content(targets, targets.len() as isize),
            },
            Key::Enter => match self.focus {
                Focus::Homes => {
                    self.focus = Focus::Content;
                    self.move_content(targets, 0);
                }
                Focus::Content => {
                    if let Some(Some(t)) = targets.get(self.content_sel) {
                        let t = t.clone();
                        self.open(&t);
                    }
                }
            },
            _ => {}
        }
    }
}

/// Front-end independent keys, so the state machine is testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    CtrlC,
}

/// `HH:MM` local-agnostic (UTC) clock for an epoch.
pub fn clock_hm(epoch: i64) -> String {
    let s = epoch.rem_euclid(86_400);
    format!("{:02}:{:02}Z", s / 3600, (s % 3600) / 60)
}

/// A short age such as `45s`, `12m`, `3h` or `9d`.
pub fn age(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetdeck_core::config::Config;

    fn snap(key: &str, parent: Option<&str>) -> HomeSnapshot {
        let mut s = HomeSnapshot::failed(key.into(), key.into(), None, key.into(), Reach::Ok);
        s.discovered_from = parent.map(str::to_string);
        s
    }

    fn app() -> App {
        let cfg = Config::parse(
            "[[home]]\nname='a'\npath='/a'\n[[home]]\nname='b'\nhost='h'\npath='/b'\n",
        )
        .unwrap();
        App::new(cfg)
    }

    #[test]
    fn orders_homes_by_config_with_children_after_parents() {
        let mut a = app();
        a.upsert(snap("h:/b", None));
        a.upsert(snap("/a", None));
        a.upsert(snap("h:/b/child", Some("h:/b")));
        a.upsert(snap("/a/child", Some("/a")));
        let keys: Vec<_> = a.homes.iter().map(|h| h.key.as_str()).collect();
        assert_eq!(keys, ["/a", "/a/child", "h:/b", "h:/b/child"]);
    }

    #[test]
    fn unreachable_read_keeps_last_good_data() {
        let mut a = app();
        let mut good = snap("/a", None);
        good.tasks.push(fleetdeck_core::model::Task {
            id: "t".into(),
            meta: Default::default(),
            state: "working".into(),
            events: vec![],
            log_partial: false,
            status_mtime: None,
            open: vec![],
            pr: None,
            busy: None,
            turn_ended: None,
            inbox_pending: 0,
            inbox_handled: 0,
            backlog: None,
            orphan: false,
        });
        a.upsert(good);
        let bad = HomeSnapshot::failed(
            "/a".into(),
            "a".into(),
            None,
            "/a".into(),
            Reach::Unreachable {
                error: "timed out".into(),
            },
        );
        a.upsert(bad);
        assert_eq!(a.homes[0].tasks.len(), 1);
        assert!(matches!(a.homes[0].reach, Reach::Unreachable { .. }));
        assert!(a.homes[0].warnings[0].starts_with("showing data"));
    }

    #[test]
    fn content_moves_skip_headings() {
        let mut a = app();
        let t = |s: &str| Some(Target::Task(s.into()));
        let rows = vec![None, t("x"), None, t("y")];
        a.focus = Focus::Content;
        a.move_content(&rows, 0);
        assert_eq!(a.content_sel, 1);
        a.on_key(Key::Down, &rows, 10);
        assert_eq!(a.content_sel, 3);
        a.on_key(Key::Down, &rows, 10);
        assert_eq!(a.content_sel, 3);
        a.on_key(Key::Up, &rows, 10);
        assert_eq!(a.content_sel, 1);
    }

    #[test]
    fn focus_loss_pauses_the_timer_and_focus_gain_refreshes() {
        let mut a = app();
        let late = Duration::from_secs(a.config.refresh_secs);
        assert!(a.refresh_due(late));
        assert!(!a.refresh_due(late - Duration::from_secs(1)));

        a.focus_lost();
        assert!(a.paused);
        assert!(!a.refresh_due(late * 10));

        // The manual refresh key still works while paused.
        a.on_key(Key::Char('r'), &[], 10);
        assert_eq!(a.requests, [Request::Refresh]);
        a.requests.clear();
        a.refresh_finished();

        a.focus_gained();
        assert!(!a.paused);
        assert_eq!(a.requests, [Request::Refresh]);
        assert!(!a.refresh_due(late), "the gain refresh is still running");
        a.refresh_finished();
        assert!(a.refresh_due(late));

        // A second gain, or a gain without a loss, does not refresh again.
        a.requests.clear();
        a.focus_gained();
        assert!(a.requests.is_empty());
    }

    #[test]
    fn keys_switch_tabs_and_request_refresh_once() {
        let mut a = app();
        a.on_key(Key::Char('3'), &[], 10);
        assert_eq!(a.tab, Tab::Context);
        a.on_key(Key::Char(']'), &[], 10);
        assert_eq!(a.tab, Tab::Activity);
        a.on_key(Key::Char('r'), &[], 10);
        a.on_key(Key::Char('r'), &[], 10);
        assert_eq!(a.requests, vec![Request::Refresh]);
        a.on_key(Key::Char('q'), &[], 10);
        assert!(a.quit);
    }

    #[test]
    fn ages_are_short() {
        assert_eq!(age(5), "5s");
        assert_eq!(age(125), "2m");
        assert_eq!(age(7300), "2h");
        assert_eq!(age(200_000), "2d");
        assert_eq!(clock_hm(3_600 * 25 + 60 * 7), "01:07Z");
    }
}
