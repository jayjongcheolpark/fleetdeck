//! Builds a fake main home with a local second mate, collects it through
//! the real gather script, and checks the model and that nothing changed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fleetdeck_core::backlog::Section;
use fleetdeck_core::collect::{self, collect_fleet, read_file};
use fleetdeck_core::config::{Config, HomeSpec, Location};
use fleetdeck_core::model::{HomeSnapshot, LockState, PrSource, Reach, Role};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A pid that is not running: the largest pid on any supported system is
/// far below this.
const DEAD_PID: u32 = 99_999_999;

fn build(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("main home");
    let mate = root.join("mate");

    put(&main, "data/backlog.md", &fixture("backlog-main.md"));
    put(&main, "data/projects.md", &fixture("projects.md"));
    put(
        &main,
        "data/secondmates.md",
        &format!(
            "# Second mates\n- shop-mate - Own one product area. (home: {}; scope: app work; projects: shop; added 2026-08-05)\n",
            mate.display()
        ),
    );
    put(
        &main,
        "data/captain.md",
        "# Captain preferences\n- keep it short\n",
    );
    put(&main, "data/learnings.md", &"x".repeat(3000));
    put(&main, "data/scout-notes.md", "# notes\n");
    put(&main, "config/startup-memory-budget", "7500\n");
    put(&main, "config/backend", "herdr\n");
    put(&main, "config/cmux-socket-password", "do-not-read\n");
    put(&main, "config/fleet-ledger", "");

    put(&main, "state/.lock", &format!("{DEAD_PID}\n"));
    put(
        &main,
        "state/.lock-session",
        "0d3c2b1a-9e8f-4a7b-8c6d-5e4f3a2b1c0d\n",
    );
    put(&main, "state/.last-watcher-beat", "");
    put(
        &main,
        "state/.watcher-down",
        "acked:handling:12345.1790997179.Ab3dEf\n",
    );
    put(
        &main,
        "state/.wake-queue",
        "1790997000\t1\tsignal\tk\tpayload\n1790997001\t2\tcheck\tk2\tp\nshort row\n",
    );
    put(&main, "state/fleet-ledger.jsonl", &fixture("ledger.jsonl"));
    put(
        &main,
        "state/home-summary.json",
        &fixture("home-summary-clean.json"),
    );

    put(
        &main,
        "state/fleetdeck-v1.meta",
        &fixture("ship-direct-pr.meta"),
    );
    put(
        &main,
        "state/fleetdeck-v1.status",
        "working [at=1790996832]: setup done\nblocked [at=1790996900] [key=ci]: CI is red on https://github.com/acme/fleetdeck/pull/1\n",
    );
    put(
        &main,
        "state/fleetdeck-v1.busy-state",
        "v1 gen=g1 seq=2 state=busy source=claude-hook event=user-prompt-submit ts=1790996748\n",
    );
    put(&main, "state/fleetdeck-v1.busy-gen", "g1\n");
    put(
        &main,
        "state/fleetdeck-v1.inbox/001.msg",
        "schema=fm-task-inbox.v1\n--\nhi\n",
    );
    put(&main, "state/fleetdeck-v1.inbox/handled/000.msg", "old\n");

    put(&main, "state/shop-mate.meta", &fixture("secondmate.meta"));
    put(
        &main,
        "state/shop-mate.status",
        &fixture("secondmate.status"),
    );
    put(
        &main,
        "state/remote-ship.meta",
        &fixture("ship-with-pr.meta"),
    );
    put(
        &main,
        "state/remote-ship.status",
        &fixture("ship-stamped.status"),
    );
    put(
        &main,
        "state/remote-ship.busy-state",
        &fixture("busy-state"),
    );
    put(&main, "state/remote-ship.busy-gen", "g-other\n");
    put(
        &main,
        "state/old-task.status",
        &fixture("ship-legacy.status"),
    );

    put(
        &main,
        "state/operational-inbox/1790996745-1a2b3c4d5e6f7a8b.msg",
        "op",
    );
    put(
        &main,
        "state/operational-inbox/1790996746-0000000000000000.msg",
        "op",
    );
    put(
        &main,
        "state/pending-replies/aaaa",
        "schema=fm-pending-reply.v1\nphase=awaiting_report\nphase=resolved\n",
    );
    put(
        &main,
        "state/pending-replies/bbbb",
        "schema=fm-pending-reply.v1\nphase=awaiting_report\n",
    );
    put(&main, "state/terminal-outcomes/abc.pending", "state=done\n");

    put(&mate, ".fm-secondmate-home", "shop-mate\n");
    put(
        &mate,
        ".fm-secondmate-parent",
        &format!(
            "schema=fm-secondmate-parent.v1\nroute=local\nparent_home={}\n",
            main.display()
        ),
    );
    put(&mate, "data/backlog.md", &fixture("backlog-secondmate.md"));
    put(&mate, "data/charter.md", "# Charter\n");
    put(
        &mate,
        "state/install-date-match-direction.meta",
        "kind=ship\nmode=no-mistakes\n",
    );
    put(
        &mate,
        "state/install-date-match-direction.status",
        &fixture("resolved-last.status"),
    );
    (main, mate)
}

/// Every path under `root` with its bytes, and its mtime in nanoseconds.
fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u128)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let md = fs::symlink_metadata(&p).unwrap();
            let mtime = md
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            if md.is_dir() {
                out.insert(p.clone(), (Vec::new(), mtime));
                stack.push(p);
            } else {
                out.insert(p.clone(), (fs::read(&p).unwrap(), mtime));
            }
        }
    }
    out
}

fn collect(config: &Config) -> Vec<HomeSnapshot> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&out);
    collect_fleet(config, move |s| sink.lock().unwrap().push(s));

    out.lock().unwrap().clone()
}

#[test]
fn collects_a_fake_fleet_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let (main, mate) = build(dir.path());
    let before = tree(dir.path());

    let config = Config::single_local(main.clone());
    let snaps = collect(&config);
    assert_eq!(snaps.len(), 2, "main home plus its discovered second mate");
    let m = snaps
        .iter()
        .find(|s| s.path == main.display().to_string())
        .unwrap();
    let s = snaps
        .iter()
        .find(|s| s.path == mate.display().to_string())
        .unwrap();

    // Homes and liveness.
    assert_eq!(m.reach, Reach::Ok);
    assert_eq!(m.role, Role::Main);
    assert_eq!(m.session.lock, LockState::Stale);
    assert_eq!(m.session.pid, Some(DEAD_PID));
    assert_eq!(
        m.session.session_id.as_deref(),
        Some("0d3c2b1a-9e8f-4a7b-8c6d-5e4f3a2b1c0d")
    );
    assert!(m.watcher.beat_fresh());
    assert!(!m.watcher.alive);
    assert_eq!(
        m.watcher.down_marker.as_deref(),
        Some("acked:handling:12345.1790997179.Ab3dEf")
    );
    assert_eq!(s.discovered_from.as_deref(), Some(m.key.as_str()));
    assert_eq!(s.name, "shop-mate");
    match &s.role {
        Role::Secondmate {
            id,
            route,
            parent_home,
        } => {
            assert_eq!(id, "shop-mate");
            assert_eq!(route.as_deref(), Some("local"));
            assert_eq!(parent_home.as_deref(), Some(main.to_str().unwrap()));
        }
        other => panic!("expected a second mate, got {other:?}"),
    }
    assert_eq!(s.session.lock, LockState::Free);

    // Work.
    let ids: Vec<&str> = m.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        ["fleetdeck-v1", "remote-ship", "shop-mate", "old-task"]
    );
    let fd = &m.tasks[0];
    assert_eq!(fd.state, "blocked");
    assert_eq!(fd.open.len(), 1);
    assert_eq!(fd.open[0].key, "ci");
    assert_eq!(
        fd.pr,
        Some((
            "https://github.com/acme/fleetdeck/pull/1".to_string(),
            PrSource::StatusEvent
        ))
    );
    assert_eq!(fd.busy.as_ref().unwrap().state, "busy");
    assert!(fd.busy.as_ref().unwrap().trusted);
    assert_eq!((fd.inbox_pending, fd.inbox_handled), (1, 1));
    assert_eq!(fd.backlog.as_ref().unwrap().0, Section::InFlight);

    let cm = &m.tasks[2];
    assert_eq!(cm.meta.kind(), "secondmate");
    assert_eq!(cm.open.len(), 2);
    assert_eq!(
        cm.pr, None,
        "a second mate's relayed PR links are not its PR"
    );

    let rs = &m.tasks[1];
    assert_eq!(rs.state, "parked");
    assert_eq!(
        rs.pr.as_ref().map(|(u, s)| (u.as_str(), *s)),
        Some(("https://github.com/acme/shop/pull/424", PrSource::Meta))
    );
    let busy = rs.busy.as_ref().unwrap();
    assert!(!busy.trusted);
    assert_eq!(busy.state, "unknown", "a stale generation is never trusted");

    let old = &m.tasks[3];
    assert!(old.orphan);
    assert_eq!(old.state, "done");

    assert_eq!(m.backlog.count(Section::Queued), 3);
    assert_eq!(m.captain_holds().count(), 1);
    assert_eq!(s.backlog.count(Section::Queued), 3);
    assert_eq!(s.tasks.len(), 1);
    assert_eq!(s.tasks[0].state, "working");
    assert!(
        collect::in_flight_without_task(m)
            .iter()
            .any(|i| i.id == "worktree-pool-slot-guard")
    );

    // Queues are counted, never drained.
    assert_eq!(m.queues.wake, Some(2));
    assert_eq!(m.queues.operational_inbox, 2);
    assert_eq!(m.queues.task_inbox_pending, 1);
    assert_eq!(m.queues.pending_replies_open, 1);
    assert_eq!(m.queues.terminal_outcomes_pending, 1);

    // Context and config.
    let ctx: Vec<&str> = m.context.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(
        ctx,
        [
            "data/projects.md",
            "data/secondmates.md",
            "data/captain.md",
            "data/learnings.md"
        ]
    );
    let learnings = m
        .context
        .iter()
        .find(|d| d.path == "data/learnings.md")
        .unwrap();
    assert_eq!(learnings.est_tokens(), 1000);
    assert_eq!(s.context.last().unwrap().path, "data/charter.md");
    let secret = m
        .config
        .iter()
        .find(|d| d.path == "config/cmux-socket-password")
        .unwrap();
    assert_eq!(secret.content, None, "secret-looking config is never read");
    assert_eq!(secret.size, 12);
    let backend = m
        .config
        .iter()
        .find(|d| d.path == "config/backend")
        .unwrap();
    assert_eq!(backend.content.as_deref(), Some("herdr\n"));
    assert!(m.data_docs.iter().any(|d| d.path == "data/scout-notes.md"));
    assert_eq!(m.projects.len(), 4);

    // Activity and summary.
    assert_eq!(m.ledger.len(), 7);
    let sum = m.summary.as_ref().unwrap();
    assert_eq!(sum.schema, "fm-secondmate-home-summary.v1");
    assert_eq!(sum.state.as_deref(), Some("no_active_work"));

    // The detail view reads one file on demand, inside the home only.
    let loc = Location::Local(main.clone());
    let f = read_file(&loc, "data/scout-notes.md", 1024, Duration::from_secs(10)).unwrap();
    assert_eq!(f.content, "# notes\n");
    assert!(
        read_file(
            &loc,
            "../mate/data/charter.md",
            1024,
            Duration::from_secs(10)
        )
        .is_err()
    );
    assert!(read_file(&loc, "/etc/hosts", 1024, Duration::from_secs(10)).is_err());

    // Nothing in either home changed: same paths, bytes and mtimes.
    assert_eq!(before, tree(dir.path()));
}

#[test]
fn missing_and_unreachable_homes_are_reported_not_hung() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        homes: vec![
            HomeSpec {
                name: "gone".into(),
                location: Location::Local(dir.path().join("nope")),
                discover: true,
            },
            HomeSpec {
                name: "nowhere".into(),
                location: Location::Ssh {
                    host: "fleetdeck-test.invalid".into(),
                    path: "~/firstmate".into(),
                },
                discover: true,
            },
        ],
        refresh_secs: 30,
        timeout_secs: 10,
    };
    let started = std::time::Instant::now();
    let snaps = collect(&config);
    assert!(started.elapsed() < Duration::from_secs(15));
    let gone = snaps.iter().find(|s| s.name == "gone").unwrap();
    assert_eq!(gone.reach, Reach::Missing);
    let nowhere = snaps.iter().find(|s| s.name == "nowhere").unwrap();
    assert!(
        matches!(&nowhere.reach, Reach::Unreachable { error } if error.starts_with("ssh fleetdeck-test.invalid")),
        "{:?}",
        nowhere.reach
    );
}
