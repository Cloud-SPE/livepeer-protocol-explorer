# frontend-ui

A static-hosted, Lit + TypeScript SPA that consumes the Rust `livepeer-api`
crate from this repo. The bundle is served at the same origin as the API
by the Axum process in production (via `FE_STATIC_DIR`), or behind Vite's
dev proxy locally.

- Web components via **Lit 3**
- Build via **Vite**
- State via **RxJS BehaviorSubject** services bridged into Lit through a small
  `ObservableController` (Lit ↔ RxJS, ~30 LOC)
- Hash routing
- Vanilla CSS only — cascade layers, OKLCH, `light-dark()`, `color-mix()`,
  `@property`, container queries, view transitions
- Charts via **Apache ECharts** (lazy-loaded)

## Requirements

- Node 20 LTS or 22 LTS
- npm

The Rust backend at `127.0.0.1:8080` (this repo's `crates/livepeer-api`)
must be reachable for any view that hits the local API. CORS-allowed
origins are required only if the SPA is served from a different origin
than the API.

## Quick start

```bash
cd frontend-ui
npm install
npm run dev          # Vite at http://localhost:5173
```

In **dev**, Vite proxies every Rust API surface used by the SPA — the
versioned business prefix (`/api`) plus the operational endpoints
(`/health`, `/metrics`, `/backfills`, `/config.json`, `/docs`,
`/openapi.json`) — to `http://127.0.0.1:8080`, so the SPA runs
same-origin and CORS isn't needed. The dev `public/config.json` ships
with `baseApiUrl: ""` (relative) for that reason.

For **production**, the Rust API serves the built bundle from
`FE_STATIC_DIR` at the same origin as `/api/v1/*`, so the relative
`baseApiUrl: ""` keeps working with no further config. If you instead
host `dist/` on a separate static host (Cloudflare Pages, S3, etc.),
edit `dist/config.json` so `baseApiUrl` points at the deployed Rust API
and make sure that API responds with CORS headers for your host.

## Scripts

| Script               | Description                                                    |
| -------------------- | -------------------------------------------------------------- |
| `npm run dev`        | Vite dev server with HMR                                       |
| `npm run build`      | Type-check + production build into `dist/`                     |
| `npm run preview`    | Preview the built `dist/`                                      |
| `npm test`           | Vitest (unit tests, jsdom)                                     |
| `npm run typecheck`  | `tsc --noEmit`                                                 |
| `npm run lint`       | ESLint flat config                                             |
| `npm run fmt`        | Prettier rewrite                                               |
| `npm run test:visual`| Playwright visual smoke tests                                  |

## Runtime configuration

The SPA fetches `/config.json` on boot **before** mounting any view.
This file lives next to `index.html` in the deployed `dist/`, so a single
build can target many backends — just ship a different `config.json`
per deployment without rebuilding.

Schema (see `public/config.example.json`):

```json
{
  "baseApiUrl": "http://127.0.0.1:8080",
  "explorerTxBase": "https://arbiscan.io/tx/",
  "explorerAddressBase": "https://arbiscan.io/address/"
}
```

When the API serves the bundle in production, the same shape is produced
dynamically by `GET /config.json` from `FE_*` env vars (see
`crates/livepeer-api/src/routes/operational.rs::frontend_config` for the
env-var mapping). Missing keys fall back to the defaults baked into the
Rust handler.

## Themes

Six themes ship out of the box (toggle in the header):

| `data-theme`   | Notes                                                      |
| -------------- | ---------------------------------------------------------- |
| `auto`         | Follows OS via `light-dark()` — default                    |
| `light`        | OKLCH cool palette                                         |
| `dark`         | OKLCH cool palette, dark surfaces                          |
| `midnight`     | Deep purple-blue, higher chroma accents                    |
| `solarized`    | Warm Solarized-inspired light theme                        |
| `high-contrast`| WCAG-AAA-friendly black/white with thicker borders         |

Theme files live at `src/styles/themes/*.css`. Each defines `:root[data-theme="…"]`
and overrides only color tokens — structural tokens (spacing, type, radii)
are shared in `_shape.css`.

Adding a theme:
1. Create `src/styles/themes/<name>.css` with `:root[data-theme="<name>"] { … }`
2. Import it in `src/main.ts`
3. Add `<name>` to `THEME_NAMES` in `src/types/config.ts`

## Project structure

```
src/
├── main.ts                       fetch config.json → mount <app-shell>
├── styles/
│   ├── layers.css                @layer ordering
│   ├── reset.css
│   ├── base.css                  semantic element defaults
│   └── themes/                   _shape + 6 palettes
├── lib/
│   ├── api-base.ts               createApi<T>() generic factory
│   ├── observable-controller.ts  Lit ↔ RxJS bridge
│   ├── router.ts                 hash router (~80 LOC)
│   ├── format.ts                 dnum-backed number formatters
│   ├── storage.ts                typed localStorage wrapper
│   └── sources/
│       └── local-api.ts          → baseApiUrl (Rust crate, /api/v1/…)
├── services/                     RxJS BehaviorSubject services
│   ├── config.service.ts
│   ├── theme.service.ts
│   ├── filters.service.ts
│   ├── orchestrators.service.ts
│   ├── gateways.service.ts
│   ├── governance.service.ts
│   ├── payouts.service.ts
│   ├── rewards.service.ts
│   ├── tickets.service.ts
│   ├── network.service.ts
│   └── stake-history.service.ts
├── components/
│   ├── app-shell.ts              header + side-nav + main + footer
│   ├── side-nav.ts
│   ├── theme-switcher.ts
│   ├── viewport-gate.ts          desktop-only banner with dismiss
│   └── ui/                       data-table, time-chart, bar-chart,
│                                  address-chip, tx-chip, money-cell,
│                                  date-range, job-type-toggle,
│                                  markdown-view, …
└── views/                        page-level Lit components
    ├── dashboard.ts              overview cards
    ├── orchestrators-list.ts
    ├── orchestrator-detail.ts    (lazy-imported)
    ├── delegators-list.ts
    ├── delegator-detail.ts
    ├── gateways-list.ts          (legacy `/broadcasters/*` aliased)
    ├── gateway-detail.ts
    ├── rounds-list.ts
    ├── round-detail.ts           (lazy-imported)
    ├── governance-proposals.ts
    ├── governance-proposal-detail.ts
    ├── governance-votes.ts
    ├── reports-hub.ts
    ├── payouts-summary.ts        (daily / weekly / monthly via path)
    ├── payouts-leaderboard.ts
    ├── rewards-leaderboard.ts
    └── tickets-timeseries.ts     (lazy-imported)
```

## Bundle layout

`npm run build` produces these chunks (sizes after gzip):

| Chunk                   | Size      | Loaded when                                           |
| ----------------------- | --------- | ----------------------------------------------------- |
| `index-*.js`            | ~4 KB     | Always (entry)                                        |
| `app-shell-*.js`        | ~48 KB    | Always (shell + lightweight views)                    |
| `index-*.css`           | ~6 KB     | Always                                                |
| `echarts-*.js`          | ~347 KB   | First mounted chart component on a chart-bearing view |
| `orchestrator-detail-*` | ~4 KB     | `/orchestrators/:address` only                        |
| `round-detail-*`        | ~3 KB     | `/rounds/:round_id` only                              |
| `tickets-timeseries-*`  | ~1.4 KB   | `/reports/tickets/daily` only                         |

First paint stays on the lightweight shell and non-chart views. Chart-bearing
detail/report views fetch their own route chunks on navigation, then pull the
`echarts` vendor chunk only when a chart component mounts.

## Deployment

The recommended deployment is **bundled with the Rust API**: `cargo build
--release` produces the API binary, `npm run build` produces `dist/`, and
the API serves `dist/` via `FE_STATIC_DIR`. No separate static host
needed.

If you want to host the SPA on a static CDN instead:

```bash
npm run build
# upload dist/ to your static host
```

Two non-default knobs:

1. **`config.json`** — edit `dist/config.json` to point `baseApiUrl` at
   the deployed Rust API.
2. **CORS** — the Rust API must respond to the deployed origin. Confirm
   `Access-Control-Allow-Origin` includes your CDN URL.

For SPA routing, hash-based URLs (e.g. `#/orchestrators`) need no host
config. The skip-link on every page sends focus to `<main>` for keyboard
users.

## Architecture notes

- All on-chain-derived data flows through the local Rust API at
  `baseApiUrl`. Arbitrum RPC and The Graph are **never** called from the
  browser.
- The single upstream source (`lib/sources/local-api.ts`) prepends
  `/api/v1` once at client construction, so callers keep un-prefixed
  paths.
- Operator subset: only `debounceTime`, `switchMap`, `combineLatest`,
  `interval`/`timer`, and `shareReplay(1)` from RxJS. Anything heavier is
  out of scope.
- Numbers from the API arrive as decimal strings (e.g. `"1252157.250…"`);
  formatting goes through `dnum` so we never lose precision on large
  stake values.
- Tests are plain Vitest with mocked source modules; they assert against
  service `value` after action (and `firstValueFrom(state$)` where stream
  semantics matter). Integration-style tests that need a backend live in
  `tests/visual/` (Playwright).
