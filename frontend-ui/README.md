# frontend-ui

A static-hosted, Lit + TypeScript SPA that consumes the Rust `livepeer-api`
crate from this repo (and a handful of Livepeer-operated external services)
and replaces the legacy `livepeer-tools-ui` React app feature-for-feature.

- Web components via **Lit 3**
- Build via **Vite**
- State via **RxJS BehaviorSubject** services bridged into Lit through a small
  `ObservableController` (Lit ↔ RxJS, ~30 LOC)
- Hash routing
- Vanilla CSS only — cascade layers, OKLCH, `light-dark()`, `color-mix()`,
  `@property`, container queries, view transitions
- Charts via **Apache ECharts** (lazy-loaded)
- BYOC via the official **`openai`** SDK (lazy-loaded)

## Requirements

- Node 20 LTS or 22 LTS
- npm

The Rust backend at `127.0.0.1:8080` (this repo's `crates/livepeer-api`)
must be reachable for any view backed by the local API. CORS-allowed
origins are required for production deployment.

## Quick start

```bash
cd frontend-ui
npm install
npm run dev          # Vite at http://localhost:5173
```

In **dev**, Vite proxies every Rust API surface used by the SPA
(`health`, `metrics`, `docs`, `openapi.json`, `backfills`, `events`,
`valuations`, `aggregations`, `delegators`, `governance`, `network`,
`prices`, `payouts`, `rewards`, `rounds`, `tickets`, `reports`,
`stake`, `transcoders`, `orchestrators`, `gateways`) to
`http://127.0.0.1:8080`, so the SPA runs same-origin and CORS isn't
needed. The dev `public/config.json` ships with `baseApiUrl: ""`
(relative) for that reason.

For **production**, edit `dist/config.json` so `baseApiUrl` points at the
deployed Rust API. CORS must be enabled there for your hosting origin.
A reference value lives in `public/config.example.json`.

## Scripts

| Script         | Description                                                    |
| -------------- | -------------------------------------------------------------- |
| `npm run dev`  | Vite dev server with HMR                                       |
| `npm run build`| Type-check + production build into `dist/`                     |
| `npm run preview` | Preview the built `dist/`                                  |
| `npm test`     | Vitest (unit tests, jsdom)                                     |
| `npm run typecheck` | `tsc --noEmit`                                            |
| `npm run lint` | ESLint flat config                                             |
| `npm run fmt`  | Prettier rewrite                                               |

## Runtime configuration

The SPA fetches `/config.json` on boot **before** mounting any view.
This file lives next to `index.html` in the deployed `dist/`, so a single
build can target many backends — just ship a different `config.json`
per deployment without rebuilding.

Schema (see `public/config.example.json`):

```json
{
  "baseApiUrl": "http://127.0.0.1:8080",
  "gatewayUrl": "https://dream-gateway.livepeer.cloud",
  "gatewayBearer": "",
  "byocGatewayUrl": "https://openai-gateway.livepeer.cloud/v1",
  "perfStatsUrl": "https://leaderboard-serverless.vercel.app/api/raw_stats",
  "aiPerfStatsUrl": "https://lpc-leaderboard-serverless.vercel.app/api/raw_stats",
  "leaderboardStatsUrl": "https://leaderboard-serverless.vercel.app/api/aggregated_stats",
  "aiLeaderboardStatsUrl": "https://lpc-leaderboard-serverless.vercel.app/api/aggregated_stats",
  "regionsUrl": "https://lpc-leaderboard-serverless.vercel.app/api/regions",
  "pipelineUrl": "https://lpc-leaderboard-serverless.vercel.app/api/pipelines",
  "explorerTxBase": "https://arbiscan.io/tx/",
  "explorerAddressBase": "https://arbiscan.io/address/"
}
```

`config.json` overrides any `VITE_*` build-time defaults. Missing keys
fall back to the defaults baked at build time, then to in-code defaults.

The AI gateway URL and bearer can additionally be overridden per-browser
in **AI → Settings** (stored in `localStorage`).

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
│   └── sources/                  swappable per-upstream HTTP clients
│       ├── local-api.ts          → VITE_BASE_API_URL (Rust crate)
│       ├── leaderboard.ts        → external aggregated_stats
│       ├── perf-stats.ts         → external raw_stats
│       ├── catalog.ts            → external regions + pipelines
│       ├── ai-gateway.ts         → AI playground (9 modalities)
│       └── openai-sdk.ts         → BYOC OpenAI gateway
├── services/                     RxJS BehaviorSubject services
│   ├── config.service.ts
│   ├── theme.service.ts
│   ├── filters.service.ts
│   ├── history.service.ts        (10-deep AI prompt history per modality)
│   ├── orchestrators.service.ts
│   ├── gateways.service.ts
│   ├── governance.service.ts
│   ├── payouts.service.ts
│   ├── rewards.service.ts
│   ├── tickets.service.ts
│   ├── leaderboard.service.ts
│   ├── perf.service.ts
│   ├── catalog.service.ts
│   ├── network-capabilities.service.ts
│   └── byoc.service.ts
├── components/
│   ├── app-shell.ts              header + side-nav + main + footer
│   ├── side-nav.ts
│   ├── theme-switcher.ts
│   ├── viewport-gate.ts          desktop-only banner with dismiss
│   └── ui/                       data-table, time-chart, bar-chart,
│                                  address-chip, tx-chip, money-cell,
│                                  date-range, job-type-toggle, ai-result,
│                                  history-list, markdown-view, …
└── views/                        page-level Lit components
    ├── dashboard.ts              5-card overview
    ├── orchestrators-list.ts
    ├── orchestrator-detail.ts
    ├── delegators-list.ts
    ├── delegator-detail.ts
    ├── gateways-list.ts          (legacy `/broadcasters/*` aliased)
    ├── gateway-detail.ts
    ├── rounds-list.ts
    ├── round-detail.ts
    ├── governance-proposals.ts
    ├── governance-proposal-detail.ts
    ├── governance-votes.ts
    ├── reports-hub.ts
    ├── payouts-summary.ts        (daily / weekly / monthly via path)
    ├── payouts-leaderboard.ts
    ├── rewards-leaderboard.ts
    ├── tickets-timeseries.ts     (lazy-imported)
    ├── leaderboard-perf.ts
    ├── stats-perf.ts             (lazy-imported)
    ├── network-capabilities.ts
    ├── ai-settings.ts
    └── ai-playground/
        ├── ai-generator.ts
        ├── llm.ts
        ├── text-to-image.ts
        ├── image-to-image.ts
        ├── image-to-video.ts
        ├── image-to-text.ts
        ├── audio-to-text.ts
        ├── text-to-speech.ts
        ├── upscale.ts
        ├── segment-anything.ts
        └── byoc-openai.ts        (lazy-imported)
```

## Bundle layout

`npm run build` produces these chunks (sizes after gzip):

| Chunk                  | Size      | Loaded when                                           |
| ---------------------- | --------- | ----------------------------------------------------- |
| `index-*.js`           | ~58 KB    | Always (entry)                                        |
| `index-*.css`          | ~2.6 KB   | Always                                                |
| `echarts-*.js`         | ~344 KB   | First mounted chart component on a chart-bearing view |
| `openai-*.js`          | ~31 KB    | `/ai/byoc/openai` only                                |
| `orchestrator-detail-*`| small app chunk | `/orchestrators/:address` only                    |
| `round-detail-*`       | small app chunk | `/rounds/:round_id` only                           |
| `tickets-timeseries-*` | ~1.6 KB   | `/reports/tickets/daily` only                         |
| `stats-perf-*`         | ~2.9 KB   | `/performance/stats` only                             |
| `byoc-openai-*`        | ~4.4 KB   | `/ai/byoc/openai` only                                |

First paint stays on the lightweight shell and non-chart views. Chart-bearing
detail/report views fetch their own route chunks on navigation, then pull the
`echarts` vendor chunk only when a chart component mounts. The BYOC/OpenAI
surface remains lazy as its own route chunk.

## Deployment (Cloudflare Pages or any static host)

```bash
npm run build
# upload dist/ to your static host
```

Two non-default knobs:

1. **`config.json`** — edit `dist/config.json` to point at the right backends.
2. **CORS** — the Rust backend must respond to the deployed origin. Confirm
   `Access-Control-Allow-Origin` includes your Pages URL.

For SPA routing, hash-based URLs (e.g. `#/orchestrators`) need no host config.
The skip-link on every page sends focus to `<main>` for keyboard users.

## Architecture notes

- All on-chain-derived data flows through the local Rust API at `baseApiUrl`.
  Arbitrum RPC and The Graph are **never** called.
- External services (perf stats, leaderboard, regions, pipelines, AI gateway,
  BYOC gateway) each live behind a single `lib/sources/*` module so a future
  switch to a backend-proxied route is a one-file change.
- Operator subset: only `debounceTime`, `switchMap`, `combineLatest`,
  `interval`/`timer`, and `shareReplay(1)` from RxJS. Anything heavier is
  out of scope for this app.
- Numbers from the API arrive as decimal strings (e.g. `"1252157.250…"`);
  formatting goes through `dnum` so we never lose precision on large stake
  values.
- Tests are plain Vitest with mocked source modules; they assert against
  service `value` after action (and `firstValueFrom(state$)` where stream
  semantics matter).
