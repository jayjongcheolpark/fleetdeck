# Backlog

## In flight
- [ ] fleetdeck-v1 - fleetdeck: first end-to-end version of the read-only fleet TUI (local + remote homes) (repo: fleetdeck) (kind: ship) (since 2026-10-02)
  Mode direct-PR, yolo on. Rust + ratatui terminal UI, read-only, local and remote homes from the start.
- [ ] worktree-pool-slot-guard - Worktree pool hands slots to new spawns while another task's live meta still claims them (repo: firstmate). Observed twice in the shop-mate secondmate home (trimmed) (kind: ship) (since 2026-08-17) (hold: captain call on the upstream fix) (hold-kind: captain)
  Captain hold set: 2026-09-10T02:52:57Z
## Queued
- [ ] nm-field-scan-order-dependent - fm_nm_field's whole-output scan is order-dependent and can read the wrong branch (repo: firstmate) (kind: ship) (since 2026-08-08) (hold: deferred from PR 58; pre-existing defect) (hold-kind: parked)
  Found by review during PR 58 on 2026-08-07 and deliberately deferred (trimmed).
- [ ] pool-return-branch-deletion-gap - Report upstream: pool return deletes a task's local branch even when told to preserve it (kind: ops) (since 2026-08-19)
  2026-08-19: cleanup of the release-notes task returned its pooled working copy (trimmed).
- [ ] watcher-beacon-alarm-gap - Investigate: secondmate home watcher beacon stale ~5h with no failure alarm firing (trimmed)
## Done
- [x] status-lamp-main-only - status-lamp: follow only the main home https://github.com/acme/fleet-tools/pull/9 (repo: fleet-tools) (kind: ship) (merged 2026-10-02)
