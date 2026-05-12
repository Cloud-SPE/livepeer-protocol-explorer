# Cloudflare Tunnel

How the `livepeer-api` container is exposed to the public internet via
Cloudflare Tunnel — hostname split, ingress rules, container wiring.

Reference: <https://developers.cloudflare.com/tunnel/advanced/local-management/configuration-file/#how-traffic-is-matched>

## 1. Architecture

`livepeer-api` serves the user-facing surfaces (API + SPA + its own
operational endpoints). Two other containers expose their own metrics
on separate ports:

| Container | Port | Endpoints |
|---|---|---|
| `livepeer-api` | `8080` | `/api/v1/*`, `/`, `/assets/*`, `/config.json`, `/docs`, `/openapi.json`, `/health`, `/metrics`, `/backfills/status` |
| `livepeer-daemon` | `9107` | `/health`, `/metrics` |
| `livepeer-enricher` | `9112` | `/health`, `/metrics` |

The other workers (`livepeer-rollups-*`, `livepeer-staker-tx-receipts`)
don't expose HTTP — they're purely DB-driven, observed via Postgres-side
queries.

Cloudflared **does not do path rewriting**, so each metrics-emitting
container needs its own hostname (you can't fold them all under one
hostname with sub-paths without an extra reverse proxy).

```
                ┌─────────────────────────────┐
                │   Cloudflare Tunnel         │
                │   (cloudflared sidecar)     │
                └───────┬─────────────────────┘
                        │  http://livepeer-api:8080
                        ▼
                ┌─────────────────────────────┐
                │   livepeer-api (axum)       │
                │   /api/v1, /health,         │
                │   /metrics, SPA, /docs      │
                └─────────────────────────────┘
```

Why not split into three containers?

- The api binary already serves all three surfaces. Splitting would mean
  building a separate FE-only image (nginx + bundled SPA) and a metrics
  proxy. Wasted complexity for no operational gain — same deploy cadence,
  same code, same observability.
- Path-based ingress on a single backend gives you the same domain
  separation without the deploy/version skew risk.

## 2. Hostname → surface mapping

| Hostname | Purpose | Audience | Backend |
|---|---|---|---|
| `app.example.com` | SPA + same-origin API | End users in browsers | `livepeer-api:8080` |
| `api.example.com` | Public API surface | Third-party UIs, scripts, partners | `livepeer-api:8080` |
| `metrics-api.example-metrics.com` | api process metrics | Prometheus | `livepeer-api:8080` |
| `metrics-daemon.example-metrics.com` | daemon metrics (indexer/finality/reorg/valuator/staker loops) | Prometheus | `livepeer-daemon:9107` |
| `metrics-enricher.example-metrics.com` | enricher metrics (ENS resolution) | Prometheus | `livepeer-enricher:9112` |

### Why this split

- **`app.example.com` serves both SPA and `/api/v1/*`** so the browser
  fetches API data **same-origin**. No CORS preflight on every request,
  cookies/auth headers work natively, faster (one less roundtrip).
  The FE config (`frontend-ui/src/services/config.service.ts`) uses
  `baseApiUrl: ''` (relative URLs), so the SPA picks up whatever origin
  it's loaded from.
- **`api.example.com` is the public API contract.** Stable hostname for
  external consumers and partner integrations to point at. CORS is
  configured here for cross-origin browser callers (set
  `CORS_ALLOWED_ORIGINS` to include callers' origins).
- **One metrics hostname per container.** `livepeer-api`, `livepeer-daemon`,
  and `livepeer-enricher` each run their own HTTP server on different
  ports — no way to fold them under one hostname without an extra reverse
  proxy. Standard Prometheus pattern: one scrape target per hostname.
- **Apply Cloudflare Access (SSO) or IP allowlists** to all three
  `metrics-*` hostnames. `/metrics` exposes internals (request counts,
  DB pool state, RPC call timings) and has no app-layer auth — never
  reachable from the open internet.
- **`/config.json` only on the SPA hostname.** It can carry per-deploy
  values like `FE_GATEWAY_BEARER`. Keeping it off `api.example.com`
  prevents incidental disclosure when somebody shares a curl example.

## 3. cloudflared config

Save as `cloudflared/config.yml` next to your tunnel credentials JSON.

```yaml
tunnel: <TUNNEL_UUID>
credentials-file: /etc/cloudflared/<TUNNEL_UUID>.json

# How matching works (per Cloudflare docs):
#   - Rules are evaluated TOP-TO-BOTTOM; the first match wins.
#   - `path` values are Go RE2 regular expressions. Anchor with ^ to
#     avoid accidental substring matches.
#   - The final entry MUST be a service (no hostname/path) catch-all,
#     otherwise cloudflared refuses to start.
ingress:
  # ─── app.example.com — SPA + /config.json + same-origin /api/v1/* ──
  # Block operational endpoints from the user-facing domain. /metrics
  # in particular shouldn't leak from a public host.
  - hostname: app.example.com
    path: ^/(metrics|health|backfills/status|openapi\.json|docs($|/))
    service: http_status:404
  # Everything else hits the backend, which serves /api/v1/*,
  # /config.json, /assets/*, and falls back to index.html for any
  # unmatched SPA deep-link path (e.g. /orchestrators/0xabc).
  - hostname: app.example.com
    service: http://livepeer-api:8080

  # ─── api.example.com — public API + docs ───────────────────────────
  # Single regex covers all three allowed prefixes:
  #   - /api/v1 and /api/v1/...   (versioned business surface)
  #   - /openapi.json             (machine-readable spec)
  #   - /docs and /docs/...       (Swagger UI + its assets)
  # The ($|/) terminator prevents /api/v10, /api/v1foo, /docsbar from
  # matching. The trailing $ on openapi\.json prevents /openapi.jsonfoo.
  - hostname: api.example.com
    path: ^/(api/v1($|/)|openapi\.json$|docs($|/))
    service: http://livepeer-api:8080
  # Anything else on api.example.com (including /, /metrics, /health,
  # /config.json, /assets/*) is rejected.
  - hostname: api.example.com
    service: http_status:404

  # ─── metrics-api.example-metrics.com — livepeer-api metrics ────────
  # Lock all three metrics-* hostnames down with Cloudflare Access or an
  # IP allowlist — /metrics has no auth at the application layer.
  - hostname: metrics-api.example-metrics.com
    path: ^/(health|metrics|backfills/status)$
    service: http://livepeer-api:8080
  - hostname: metrics-api.example-metrics.com
    service: http_status:404

  # ─── metrics-daemon.example-metrics.com — daemon metrics ───────────
  # Daemon runs indexer/finality/reorg/valuator/staker loops + matview
  # refresh. /backfills/status is api-only, so it's not allowed here.
  - hostname: metrics-daemon.example-metrics.com
    path: ^/(health|metrics)$
    service: http://livepeer-daemon:9107
  - hostname: metrics-daemon.example-metrics.com
    service: http_status:404

  # ─── metrics-enricher.example-metrics.com — enricher metrics ───────
  # ENS name + avatar resolution worker. Same /health + /metrics shape.
  - hostname: metrics-enricher.example-metrics.com
    path: ^/(health|metrics)$
    service: http://livepeer-enricher:9112
  - hostname: metrics-enricher.example-metrics.com
    service: http_status:404

  # ─── required catch-all ────────────────────────────────────────────
  - service: http_status:404
```

### Path-matching gotchas

- **Always anchor with `^`.** Without it, `path: api/v1` matches
  `/somepath/api/v1/anything` because RE2 patterns are unanchored by
  default.
- **Use `($|/)` to terminate path segments.** `^/api/v1` would match
  `/api/v10`. `^/api/v1($|/)` matches only `/api/v1` and `/api/v1/...`.
- **Path matching ignores query string.** `/health?check=foo` matches
  `^/health$` because `?check=foo` isn't part of the path.
- **Hostname is exact match (no regex)** unless you use `*.example.com`
  for wildcards. The placeholders above are all literal.

## 4. Container & network setup

cloudflared needs to resolve `livepeer-api` by hostname, which means it
must share Docker's `livepeer-valuation` network with the api container.

Add to `docker-compose.prod.yml` (or split into a sidecar
`docker-compose.tunnel.yml` that joins the same external network):

```yaml
cloudflared:
  image: cloudflare/cloudflared:latest
  command: tunnel --no-autoupdate --config /etc/cloudflared/config.yml run
  restart: unless-stopped
  hostname: cloudflared
  container_name: cloudflared
  volumes:
    # Mount config.yml + <TUNNEL_UUID>.json credentials. Read-only.
    - ./cloudflared:/etc/cloudflared:ro
  depends_on:
    livepeer-api:
      condition: service_started
```

The api container does NOT need to publish port 8080 — cloudflared
reaches it over the internal docker network at `http://livepeer-api:8080`.
Keep the `# ports: - "8080:8080"` line commented in `docker-compose.prod.yml`.

### DNS records

For each hostname, add a CNAME pointing at the tunnel. Easiest via
cloudflared CLI (run once per hostname):

```bash
cloudflared tunnel route dns <tunnel-name> app.example.com
cloudflared tunnel route dns <tunnel-name> api.example.com
cloudflared tunnel route dns <tunnel-name> metrics-api.example-metrics.com
cloudflared tunnel route dns <tunnel-name> metrics-daemon.example-metrics.com
cloudflared tunnel route dns <tunnel-name> metrics-enricher.example-metrics.com
```

Or manually in the Cloudflare dashboard: each hostname → CNAME →
`<TUNNEL_UUID>.cfargotunnel.com` (proxied/orange cloud).

## 5. CORS

Same-origin SPA traffic on `app.example.com` does not require CORS.

Cross-origin third-party callers hitting `api.example.com` from their
own browsers do. Set on the api container:

```yaml
# in docker-compose.prod.yml under livepeer-api environment:
CORS_ALLOWED_ORIGINS: "https://partner-ui.example.com,https://other-app.example.com"
```

Default is `*` (open). For a public-facing API you generally want this
restricted to known origins.

## 6. Verification

After bringing up cloudflared, smoke-test each hostname.

### `app.example.com` — SPA + same-origin API
```bash
# SPA shell loads
curl -sSI https://app.example.com/                | head -3
# /config.json reachable
curl -sS  https://app.example.com/config.json     | jq .
# Same-origin API works
curl -sS  https://app.example.com/api/v1/network/stats | jq .
# Ops endpoints are blocked (expect 404)
curl -sSI https://app.example.com/metrics         | head -1
curl -sSI https://app.example.com/health          | head -1
```

### `api.example.com` — public API + docs
```bash
# API works
curl -sS  https://api.example.com/api/v1/network/stats | jq .
# OpenAPI spec
curl -sS  https://api.example.com/openapi.json    | jq '.info, .servers'
# Docs UI loads
curl -sSI https://api.example.com/docs/           | head -3
# Operational and SPA paths are blocked (expect 404)
curl -sSI https://api.example.com/                | head -1
curl -sSI https://api.example.com/health          | head -1
curl -sSI https://api.example.com/metrics         | head -1
curl -sSI https://api.example.com/config.json     | head -1
```

### `metrics-api.example-metrics.com` — api process metrics
```bash
curl -sS  https://metrics-api.example-metrics.com/health
curl -sSI https://metrics-api.example-metrics.com/metrics          | head -1
curl -sS  https://metrics-api.example-metrics.com/backfills/status | jq .
# Business + SPA paths blocked (expect 404)
curl -sSI https://metrics-api.example-metrics.com/                       | head -1
curl -sSI https://metrics-api.example-metrics.com/api/v1/network/stats   | head -1
```

### `metrics-daemon.example-metrics.com` — indexer/finality/staker metrics
```bash
curl -sS  https://metrics-daemon.example-metrics.com/health
curl -sSI https://metrics-daemon.example-metrics.com/metrics       | head -1
# api-only paths blocked (expect 404)
curl -sSI https://metrics-daemon.example-metrics.com/backfills/status | head -1
```

### `metrics-enricher.example-metrics.com` — ENS resolver metrics
```bash
curl -sS  https://metrics-enricher.example-metrics.com/health
curl -sSI https://metrics-enricher.example-metrics.com/metrics     | head -1
```

### Prometheus scrape config snippet

```yaml
scrape_configs:
  - job_name: livepeer-api
    scheme: https
    static_configs:
      - targets: [metrics-api.example-metrics.com]
  - job_name: livepeer-daemon
    scheme: https
    static_configs:
      - targets: [metrics-daemon.example-metrics.com]
  - job_name: livepeer-enricher
    scheme: https
    static_configs:
      - targets: [metrics-enricher.example-metrics.com]
```

## 7. Operational notes

- **First-request latency for new tunnels** can be ~1s. Cloudflare keeps
  warm connections to the origin; if cloudflared restarts, the next
  request from each cold edge POP pays a re-establish cost.
- **`cloudflared` health is independent of `livepeer-api`.** If the api
  container is down, requests still hit cloudflared and return Cloudflare
  error pages — make sure your monitoring distinguishes "tunnel down"
  (cloudflared dead) vs "origin down" (livepeer-api dead). Both have
  Cloudflare-side and origin-side `/health` to compare.
- **Changing ingress requires a cloudflared restart** to pick up the new
  config. `docker compose restart cloudflared` is enough — no api restart
  needed.
- **Logs**: `docker logs -f cloudflared` shows per-request matching
  decisions at info level. Crank to debug for full path-matching trace
  when debugging "why is this 404'ing": add `loglevel: debug` to the top
  of `config.yml`.

## 8. Future hardening

When you're ready:

- **Cloudflare Access on `metrics.example-metrics.com`** — require SSO
  (Google Workspace, GitHub, etc.) before any request reaches cloudflared.
- **Rate limiting** at Cloudflare for `api.example.com` — protects the
  origin DB pool from abusive clients without code changes.
- **WAF rules** for `api.example.com` — drop obviously-malformed query
  strings before they reach the api process.
- **Custom error pages** per hostname — Cloudflare dashboard. Useful so
  the 404s on `api.example.com/` say "this is an API; see /docs" rather
  than a generic Cloudflare page.
