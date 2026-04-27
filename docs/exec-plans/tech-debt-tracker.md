# Tech Debt Tracker

Known shortcuts, deferred work, and TODOs that are too small to warrant their own plan but should not be lost.

| ID | Item | Source | Severity | Plan to resolve |
|---|---|---|---|---|
| TD-001 | `BondingVotes` ABI not vendored — only `LivepeerGovernor.json` available. SPEC §6.7 only lists Governor events, so v1 is unblocked, but BondingVotes proxy at `0x0B9C25...` has no registry entry. | Scaffold | low | Fetch from Arbiscan when delegation events are needed (post-v1). |
| TD-002 | All SPEC §22 open data items (Q-OD-1 through Q-OD-10) marked `TODO(Q-OD-N)` in code/config — not yet resolved. | SPEC §22 | medium | Resolve during implementation kickoff before first real backfill. |
