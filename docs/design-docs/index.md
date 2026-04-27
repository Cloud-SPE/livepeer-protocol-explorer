# Design Docs Index

Each design doc captures a single architectural decision with rationale, alternatives, and verification status.

| Doc | Status | Owner | Last verified |
|---|---|---|---|
| _none yet_ | — | — | — |

## Conventions

- One `.md` file per decision.
- Frontmatter: `status: { draft, accepted, superseded }`, `verified: YYYY-MM-DD`, `superseded_by: <path>`.
- A doc is **accepted** only after its claims have been validated against the code or runtime.
- A doc is **superseded** when a newer decision overrides it; never delete — link forward.

## Pointers

- Load-bearing operating principles: [core-beliefs.md](core-beliefs.md)
- The product spec: [../product-specs/v1-livepeer-indexer.md](../product-specs/v1-livepeer-indexer.md)
