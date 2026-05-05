# Design Docs Index

Each design doc captures a single architectural decision with rationale, alternatives, and verification status.

| Doc | Status | Resolves | Last verified |
|---|---|---|---|
| [sqlite-seed-mapping.md](sqlite-seed-mapping.md) | accepted | Q-OD-1, Q-OD-2, Q-OD-3, Q-OD-4 | 2026-04-27 |
| [bonding-manager-event-fields.md](bonding-manager-event-fields.md) | accepted | Q-OD-7 | 2026-04-27 |
| [gateway-ticketbroker-data-model.md](gateway-ticketbroker-data-model.md) | accepted | gateway sender balances and payout/funding API shape | 2026-05-05 |
| [on-chain-references.md](on-chain-references.md) | accepted | Q-OD-5, Q-OD-8, Q-OD-9, Q-OD-10 | 2026-04-27 |
| [continuous-catchup-architecture.md](continuous-catchup-architecture.md) | draft | TD-012 follow-mode architecture | 2026-05-04 |

## Conventions

- One `.md` file per decision.
- Frontmatter: `status: { draft, accepted, superseded }`, `verified: YYYY-MM-DD`, `superseded_by: <path>`.
- A doc is **accepted** only after its claims have been validated against the code or runtime.
- A doc is **superseded** when a newer decision overrides it; never delete — link forward.

## Pointers

- Load-bearing operating principles: [core-beliefs.md](core-beliefs.md)
- The product spec: [../product-specs/v1-livepeer-indexer.md](../product-specs/v1-livepeer-indexer.md)
