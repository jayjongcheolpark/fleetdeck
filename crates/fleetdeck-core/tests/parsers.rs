//! Parser tests against samples shaped like real firstmate homes, with
//! made-up names, links and prose. The line shapes and edge cases (stamp
//! order, keyless resolutions, torn ledger lines, indented headings) follow
//! what firstmate writes.

use fleetdeck_core::backlog::{self, Section};
use fleetdeck_core::ledger;
use fleetdeck_core::meta::Meta;
use fleetdeck_core::projects;
use fleetdeck_core::routes;
use fleetdeck_core::status;

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn open_keys(name: &str, kind: &str) -> Vec<String> {
    let events = status::parse(&fixture(name));
    status::open_decisions(&events, Some(kind))
        .into_iter()
        .map(|d| d.key)
        .collect()
}

#[test]
fn legacy_ship_log_closes_everything_on_done() {
    let events = status::parse(&fixture("ship-legacy.status"));
    assert_eq!(events.len(), 11);
    assert!(events.iter().all(|e| e.at.is_none()));
    assert_eq!(events[2].verb, "needs-decision");
    assert_eq!(
        events[2].key.as_deref(),
        Some("nm-01HZW2B4C6D8E0F2G4H6J8K0MN-review")
    );
    let open = status::open_decisions(&events, Some("ship"));
    assert!(open.is_empty());
    assert_eq!(status::log_state(&events, &open), "done");
}

#[test]
fn stamped_ship_log_keeps_its_last_review_gate_open() {
    let events = status::parse(&fixture("ship-stamped.status"));
    assert_eq!(events.len(), 17);
    assert!(events.iter().all(|e| e.at.is_some()));
    // `resolved [key=…] [at=…]` puts the stamp after the key.
    assert_eq!(events[3].verb, "resolved");
    assert_eq!(events[3].at, Some(1790796410));
    assert_eq!(
        open_keys("ship-stamped.status", "ship"),
        ["nm-01HZY3C5D7E9F1G3H5J7K9M1NP-review"]
    );
    let open = status::open_decisions(&events, Some("ship"));
    // A needs-decision is reported as parked, like fm-crew-state.sh.
    assert_eq!(status::log_state(&events, &open), "parked");
}

#[test]
fn secondmate_done_lines_do_not_close_decisions() {
    let events = status::parse(&fixture("secondmate.status"));
    assert!(events.iter().any(|e| e.verb == "done"));
    assert!(
        events
            .iter()
            .any(|e| e.corr.as_deref() == Some("0e50454f31af3176"))
    );
    assert_eq!(
        open_keys("secondmate.status", "secondmate"),
        ["pr438-439-endpoint-mismatch", "pr480-staging-qa-checklist"]
    );
    // The same log read as a ship task: done clears what came before it.
    assert!(open_keys("secondmate.status", "ship").len() < 2);
}

#[test]
fn keyless_resolutions_close_the_default_key() {
    let events = status::parse(&fixture("resolved-last.status"));
    assert!(status::open_decisions(&events, Some("ship")).is_empty());
}

#[test]
fn metas_from_three_spawn_shapes() {
    let sm = Meta::parse(&fixture("secondmate.meta"));
    assert_eq!(sm.kind(), "secondmate");
    assert_eq!(sm.backend(), "herdr");
    assert_eq!(sm.get("herdr_pane_id"), Some("w74:p2"));
    assert_eq!(sm.endpoint(), Some("default:w74:p2"));
    assert_eq!(sm.get("projects"), Some("shop"));
    assert_eq!(sm.pr(), None);

    let ship = Meta::parse(&fixture("ship-direct-pr.meta"));
    assert_eq!(ship.kind(), "ship");
    assert_eq!(ship.get("mode"), Some("direct-PR"));
    assert_eq!(ship.get("branch"), Some("fm/fleetdeck-v1"));
    assert_eq!(ship.get("model"), Some("claude-opus-5-5"));

    let pr = Meta::parse(&fixture("ship-with-pr.meta"));
    assert_eq!(pr.pr(), Some("https://github.com/acme/shop/pull/424"));
    assert_eq!(
        pr.get("pr_head"),
        Some("0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c")
    );
    assert!(!pr.is_remote());
}

#[test]
fn main_backlog_sections_holds_and_links() {
    let b = backlog::parse(&fixture("backlog-main.md"));
    assert_eq!(b.count(Section::InFlight), 2);
    assert_eq!(b.count(Section::Queued), 3);
    assert_eq!(b.count(Section::Done), 1);
    let (sec, guard) = b.find("worktree-pool-slot-guard").unwrap();
    assert!(guard.captain_hold(sec));
    assert_eq!(guard.hold_set.as_deref(), Some("2026-09-10T02:52:57Z"));
    assert!(guard.title.ends_with("(trimmed)"));
    let (_, ops) = b.find("pool-return-branch-deletion-gap").unwrap();
    assert_eq!(ops.kind.as_deref(), Some("ops"));
    assert_eq!(ops.repo, None);
    let (sec, parked) = b.find("nm-field-scan-order-dependent").unwrap();
    assert_eq!(parked.hold_kind.as_deref(), Some("parked"));
    assert!(parked.held(sec, "2026-10-02"));
    assert!(!parked.captain_hold(sec));
    let (_, done) = b.find("status-lamp-main-only").unwrap();
    assert_eq!(done.pr_urls, ["https://github.com/acme/fleet-tools/pull/9"]);
}

#[test]
fn secondmate_backlog_with_queued_first_and_resolution_blocks() {
    let b = backlog::parse(&fixture("backlog-secondmate.md"));
    let order: Vec<Section> = b.sections.iter().map(|s| s.section).collect();
    assert_eq!(order, [Section::Queued, Section::InFlight, Section::Done]);
    let (_, exp) = b.find("export-history-and-retention").unwrap();
    assert!(exp.body.contains("## PRIORITY CHANGED"));
    assert_eq!(exp.hold_kind.as_deref(), Some("captain"));
    let (_, dep) = b.find("order-audit-remaining-writers").unwrap();
    assert_eq!(dep.blocked_by, ["order-audit-region-missing"]);
    assert_eq!(dep.priority, Some(2));
    assert_eq!(
        dep.title,
        "Record the region in the remaining OrderAudit writer paths"
    );
    let (sec, done) = b.find("invoice-pdf-attachments").unwrap();
    assert_eq!(sec, Section::Done);
    assert_eq!(done.hold_until.as_deref(), Some("2026-10-01"));
    assert_eq!(
        done.closed,
        Some(("reported".to_string(), "2026-10-01".to_string()))
    );
    // Hold tags survive on done rows, but a done row is never an open hold.
    assert!(!done.held(sec, "2026-09-01"));
    assert!(!done.captain_hold(sec));
    let captain: Vec<_> = b
        .sections
        .iter()
        .flat_map(|s| s.items.iter().map(move |i| (s.section, i)))
        .filter(|(s, i)| i.captain_hold(*s))
        .map(|(_, i)| i.id.as_str())
        .collect();
    assert_eq!(captain, ["export-history-and-retention"]);
}

#[test]
fn secondmate_routes_local_on_each_machine_and_remote_form() {
    let local = routes::parse(&fixture("secondmates-local.md"));
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].id, "shop-mate");
    assert_eq!(
        local[0].home,
        "/Users/me/.worktrees/firstmate-0a1b2c/1/firstmate"
    );
    assert!(local[0].scope.contains("store address/region/warehouse"));
    let devbox = routes::parse(&fixture("secondmates-devbox.md"));
    assert_eq!(devbox[0].id, "bot-mate");
    assert_eq!(devbox[0].home, "/home/dev/secondmates/bot-mate");
    assert_eq!(devbox[0].projects, ["chat-bot"]);
    // No real remote route exists yet; this line follows the documented
    // `host:`/`root:` form from the secondmate-provisioning skill.
    let remote = routes::parse(
        "- bot-mate - Own the bot. (host: buildbox; root: /home/dev/firstmate; home: /home/dev/secondmates/bot-mate; scope: bot; projects: chat-bot; added 2026-09-30)\n",
    );
    assert_eq!(remote[0].host.as_deref(), Some("buildbox"));
}

#[test]
fn projects_registry() {
    let ps = projects::parse(&fixture("projects.md"));
    let names: Vec<_> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["dotfiles", "field-guide", "fleet-tools", "fleetdeck"]
    );
    assert!(ps.iter().all(|p| p.mode == "direct-PR"));
    assert!(ps[3].yolo);
    assert!(!ps[0].yolo);
}

#[test]
fn ledger_events_and_torn_last_line() {
    let ev = ledger::parse(&fixture("ledger.jsonl"));
    let kinds: Vec<_> = ev.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "task.merged",
            "task.pr_ready",
            "task.cleaned_up",
            "task.dispatched",
            "task.status",
            "task.status",
            "task.status"
        ]
    );
    assert_eq!(ev[3].summary, "ship · fleetdeck · claude · claude-opus-5-5");
    assert!(
        ev[4]
            .summary
            .starts_with("resolved [pr480-french-notes]: owner chose (a)")
    );
}
