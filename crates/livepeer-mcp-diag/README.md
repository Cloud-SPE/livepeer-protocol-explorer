# livepeer-mcp-diag

A **read-only** [MCP](https://modelcontextprotocol.io) server for debugging the
Livepeer protocol explorer in production. An operator points Claude at it and
asks *"why is indexing stalled / pricing slow / reports late"*; the server
answers by correlating four live surfaces:

1. **Postgres** (read-only `diag_ro` role) — checkpoints, pricing backlog,
   rollup freshness, error tables.
2. **Prometheus `/metrics`** — daemon/enricher/api only (rollups expose none).
3. **Docker container state** — via a GET-only socket proxy.
4. **Docker logs** — via the same proxy.

It never writes: read-only is enforced by the `diag_ro` DB role +
`default_transaction_read_only`, a SELECT-only session guard, and a GET-only
docker proxy. The `raw_sql` string check is only defense-in-depth.

## Tools

| Tool | What it answers |
|---|---|
| `dependency_chain` | **Start here.** Walks indexer→finality→valuation→rollups and names the first stalled stage. |
| `indexer_health` | Per-contract checkpoint lag vs chain head + staleness. |
| `pricing_backlog` | Unpriced count, due retries, terminal-failure breakdown. |
| `report_readiness` | Daily rollup freshness + the upstream blocking newer days. |
| `recent_errors` | Grouped decode/reorg/divergence/pricing-failure counts. |
| `container_state` | `docker ps` — catches silently-crashed workers. |
| `worker_logs` | Tail a container's logs (bounded). |
| `scrape_metrics` | Raw `/metrics` escape hatch. |
| `raw_sql` | SELECT-only query, ≤200 rows, cells truncated. |

## Transports

- **`--transport stdio`** — local dev; the MCP client spawns the binary. Logs go
  to stderr so they don't corrupt the JSON-RPC stream on stdout.
- **`--transport http`** — Streamable HTTP for the co-deployed prod container,
  bearer-protected on `/mcp`, `/health` for the compose healthcheck.

## Local dev

```bash
# Point at a local DB (a diag_ro role is recommended but not required locally).
export DIAG_DATABASE_URL=postgres://diag_ro:pw@localhost:5432/livepeer
cargo run -p livepeer-mcp-diag -- --env-config config/env/dev.yaml --transport stdio
```

Drive it with the MCP Inspector, or add it to a client's MCP config as a stdio
server invoking the same command.

## Production

1. Provision the role once: `scripts/diag_ro_role.sql`.
2. Set `DIAG_DATABASE_URL` and `DIAG_BEARER_TOKEN` in the prod env.
3. `docker compose -f docker-compose.prod.yml up -d livepeer-mcp-diag docker-proxy`.
4. Reach it from a laptop over an SSH tunnel (the port is host-loopback only):

   ```bash
   ssh -L 9200:127.0.0.1:9200 <prod-host>
   # MCP client → http://127.0.0.1:9200/mcp  with  Authorization: Bearer <token>
   ```
