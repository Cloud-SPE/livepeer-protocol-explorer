# Visual regression harness

Tripwire for CSS regressions. Runs Playwright against `vite preview`,
visits ~8 representative routes with the API mocked to empty responses,
and snapshots the rendered DOM.

## First-time setup

After `npm install`, install the Chromium browser binary:

```sh
npx playwright install chromium
```

## Running locally

```sh
npm run test:visual            # run + diff against committed snapshots
npm run test:visual:update     # accept current rendering as the new baseline
```

The `webServer` block in `playwright.config.ts` builds and previews the
app automatically; you do not need to start `vite` separately.

## What it covers

The 8 routes in `routes.spec.ts` are a smoke set, not exhaustive. They
exercise:
- the dashboard (most card variants)
- list views with `data-table` (orchestrators, gateways)
- governance (the proposal-card pattern)
- reports hub
- performance leaderboard (custom layout)
- one AI playground view (form-heavy)

API responses are mocked to empty `{ data: [], meta: ... }` shapes so
snapshots don't depend on a running backend or live data. The empty
states are themselves what regresses most often when CSS changes.

## When a snapshot fails

1. Look at the diff PNG in `test-results/` to see what changed.
2. If the change is intentional, run `npm run test:visual:update` and
   commit the new snapshot files.
3. If the change is unintentional, fix the CSS that caused it.

## Out of scope

- Cross-browser visual diffs (Chromium only — this is an internal tool)
- Themed snapshots (light, dark, midnight, etc.) — add as separate
  `test.use({ ... })` blocks once a theme-cookie or query-param
  override exists
- Authenticated routes (none of the snapshotted routes require auth)
