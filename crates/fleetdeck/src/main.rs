mod app;
mod cli;
mod detail;
mod ui;
mod update;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use fleetdeck_core::collect;
use fleetdeck_core::config::Config;
use fleetdeck_core::model::HomeSnapshot;
use fleetdeck_core::pr::{self, PrStatus};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;

use app::{App, Key, Request, Tab};

/// Results that background threads send to the UI loop.
enum Update {
    Home(Box<HomeSnapshot>),
    RefreshDone,
    File {
        home: String,
        path: String,
        result: Result<String, String>,
    },
    Pr(Box<PrStatus>),
}

const PR_TIMEOUT: Duration = Duration::from_secs(20);
const PR_THREADS: usize = 4;

fn main() {
    let parsed = match cli::parse(std::env::args().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fleetdeck: {e}");
            std::process::exit(2);
        }
    };
    let args = match parsed {
        cli::Parsed::Help => {
            print!("{}", cli::USAGE);
            return;
        }
        cli::Parsed::Version => {
            println!("fleetdeck {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        cli::Parsed::Update { check } => {
            if let Err(e) = update::run(check) {
                eprintln!("fleetdeck: {e:#}");
                std::process::exit(1);
            }
            return;
        }
        cli::Parsed::Run(a) => a,
    };
    let config = match cli::effective_config(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fleetdeck: {e}");
            std::process::exit(2);
        }
    };
    let result = if args.json {
        print_json(&config)
    } else if let Some((w, h)) = args.frame {
        print_frame(config, &args, w, h)
    } else {
        run_tui(config)
    };
    if let Err(e) = result {
        eprintln!("fleetdeck: {e:#}");
        std::process::exit(1);
    }
}

fn collect_all(config: &Config) -> Vec<HomeSnapshot> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&out);
    collect::collect_fleet(config, move |s| sink.lock().unwrap().push(s));
    Arc::try_unwrap(out)
        .map(|m| m.into_inner().unwrap())
        .unwrap_or_default()
}

fn print_json(config: &Config) -> Result<()> {
    let snaps = collect_all(config);
    println!("{}", serde_json::to_string_pretty(&snaps)?);
    Ok(())
}

/// Renders one frame to text, for screenshots in docs and PRs.
fn print_frame(config: Config, args: &cli::Args, w: u16, h: u16) -> Result<()> {
    let snaps = collect_all(&config);
    let mut app = App::new(config);
    for s in snaps {
        app.upsert(s);
    }
    app.refresh_finished();
    app.home_sel = args.select.min(app.homes.len().saturating_sub(1));
    if let Some(t) = args.tab.as_deref() {
        app.tab = Tab::from_name(t).ok_or_else(|| anyhow::anyhow!("unknown tab {t}"))?;
    }
    if app.tab == Tab::Prs {
        app.request_prs(false);
        for r in std::mem::take(&mut app.requests) {
            if let Request::FetchPrs(urls) = r {
                for u in urls {
                    app.pr_done(pr::fetch(&u, PR_TIMEOUT));
                }
            }
        }
    }
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| ui::draw(f, &app))?;
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
            line.push_str(sym);
        }
        println!("{}", line.trim_end());
    }
    Ok(())
}

fn spawn_request(req: Request, config: &Config, homes: &[HomeSnapshot], tx: &Sender<Update>) {
    let tx = tx.clone();
    match req {
        Request::Refresh => {
            let config = config.clone();
            thread::spawn(move || {
                let sink = tx.clone();
                collect::collect_fleet(&config, move |s| {
                    let _ = sink.send(Update::Home(Box::new(s)));
                });
                let _ = tx.send(Update::RefreshDone);
            });
        }
        Request::ReadFile { home, path } => {
            let Some(location) = location_of(config, homes, &home) else {
                let _ = tx.send(Update::File {
                    home,
                    path,
                    result: Err("home is no longer listed".into()),
                });
                return;
            };
            let timeout = Duration::from_secs(config.timeout_secs);
            thread::spawn(move || {
                let result = collect::read_file(&location, &path, detail::MAX_FILE_BYTES, timeout)
                    .map(|f| {
                        let mut text = f.content;
                        if f.size > detail::MAX_FILE_BYTES {
                            text.push_str("\n\n[fleetdeck: only the first 4 MiB are shown]");
                        }
                        text
                    });
                let _ = tx.send(Update::File { home, path, result });
            });
        }
        Request::FetchPrs(urls) => {
            // Up to PR_THREADS readers share the list.
            let queue = Arc::new(Mutex::new(urls));
            for _ in 0..PR_THREADS {
                let queue = Arc::clone(&queue);
                let tx = tx.clone();
                thread::spawn(move || {
                    while let Some(u) = queue.lock().unwrap().pop() {
                        let _ = tx.send(Update::Pr(Box::new(pr::fetch(&u, PR_TIMEOUT))));
                    }
                });
            }
        }
    }
}

/// The location of a home key, from the config or a discovered snapshot.
fn location_of(
    config: &Config,
    homes: &[HomeSnapshot],
    key: &str,
) -> Option<fleetdeck_core::config::Location> {
    if let Some(h) = config
        .homes
        .iter()
        .find(|h| collect::home_key(&h.location) == key)
    {
        return Some(h.location.clone());
    }
    let snap = homes.iter().find(|h| h.key == key)?;
    Some(match &snap.host {
        Some(host) => fleetdeck_core::config::Location::Ssh {
            host: host.clone(),
            path: snap.path.clone(),
        },
        None => fleetdeck_core::config::Location::Local(snap.path.clone().into()),
    })
}

fn key_of(code: KeyCode, mods: KeyModifiers) -> Option<Key> {
    Some(match code {
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Key::CtrlC,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => return None,
    })
}

fn apply(app: &mut App, u: Update) {
    match u {
        Update::Home(s) => app.upsert(*s),
        Update::RefreshDone => app.refresh_finished(),
        Update::File { home, path, result } => app.file_done(&home, &path, result),
        Update::Pr(p) => app.pr_done(*p),
    }
}

fn run_tui(config: Config) -> Result<()> {
    let (tx, rx): (Sender<Update>, Receiver<Update>) = mpsc::channel();
    let mut app = App::new(config);
    let mut terminal = ratatui::init();
    // Terminals that support it report focus changes; the others send nothing.
    let _ = execute!(std::io::stdout(), EnableFocusChange);
    let result = (|| -> Result<()> {
        let mut last_start = Instant::now();
        app.request_refresh();
        loop {
            while let Ok(u) = rx.try_recv() {
                apply(&mut app, u);
            }
            if app.refresh_due(last_start.elapsed()) {
                app.request_refresh();
            }
            for req in std::mem::take(&mut app.requests) {
                if req == Request::Refresh {
                    last_start = Instant::now();
                }
                spawn_request(req, &app.config, &app.homes, &tx);
            }
            let mut page = 10;
            let mut targets = Vec::new();
            terminal.draw(|f| {
                page = f.area().height.saturating_sub(12).max(1);
                ui::draw(f, &app);
                let body_w = f.area().width.saturating_sub(24);
                targets = ui::rows(&app, body_w).targets;
            })?;
            if event::poll(Duration::from_millis(200))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if let Some(key) = key_of(k.code, k.modifiers) {
                            app.on_key(key, &targets, page);
                        }
                    }
                    Event::FocusLost => app.focus_lost(),
                    Event::FocusGained => app.focus_gained(),
                    _ => {}
                }
            }
            if app.quit {
                return Ok(());
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableFocusChange);
    ratatui::restore();
    result
}
