# Backlog

## In flight
- [ ] checkout-retry - Retry declined card payments once with a fresh idempotency key (repo: shop) (kind: ship) (since @D-2@)
  Mode no-mistakes, yolo off. The repro is in data/checkout-retry/notes.md.
- [ ] search-latency - Cut p95 search latency on the catalog page below 300 ms (repo: shop) (kind: ship) (since @D-1@)
  Mode direct-PR. Budget: one cache layer, no new service.
- [ ] release-notes-sept - Draft the September release notes from the merged PRs (repo: docs-site) (kind: ship) (since @D-0@)
- [ ] consent-banner - Cookie consent banner with the new legal copy (repo: shop) (kind: ship) (since @D-1@)
- [ ] web-mate - Second mate for the storefront and the design system (kind: secondmate) (since @D-9@)
## Queued
- [ ] flaky-e2e-quarantine - Quarantine the three flaky checkout E2E specs and file follow-ups (repo: shop) (kind: ship) (priority: 1) (since @D-3@)
- [ ] dependency-audit - Audit the npm packages that the weekly scan flagged (repo: shop) (kind: scout) (since @D-4@)
- [ ] pricing-page-copy - Rewrite the pricing page copy for the new plans (repo: docs-site) (kind: ship) (since @D-6@) (hold: waiting for the captain's pricing decision) (hold-kind: captain)
  Captain hold set: @ISO-432000@
## Done
- [x] cdn-cache-headers - Long cache headers on hashed assets https://github.com/acme/shop/pull/405 (repo: shop) (kind: ship) (merged @D-1@)
- [x] slow-query-report - Find the slowest catalog queries data/slow-query-report/report.md (repo: shop) (kind: scout) (reported @D-2@)
