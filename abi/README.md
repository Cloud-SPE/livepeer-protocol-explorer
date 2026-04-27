# Contract ABIs

Vendored ABI JSON files for every contract the system decodes against. Sourced from `livepeer-backend-rs` repo. Each file's sha256 is recorded in `contract_abi_registry.abi_hash` (SPEC §5.5) and verified at service boot — any mismatch refuses to start.

## Files

| File | Used for | SPEC §5.1 address |
|---|---|---|
| `Controller.json` | Boot-time address resolution | `0xD8E8328501E9645d16Cf49539efC04f734606ee4` |
| `BondingManager.json` | BondingManager proxy event decoding | `0x35Bcf3c30594191d53231E4FF333E8A770453e40` |
| `BondingManagerTarget.json` | BondingManager implementation reference | (target, resolved via Controller) |
| `TicketBroker.json` | TicketBroker proxy event decoding | `0xa8bB618B1520E284046F3dFc448851A1Ff26e41B` |
| `TicketBrokerTarget.json` | TicketBroker implementation reference | (target) |
| `RoundsManager.json` | RoundsManager proxy event decoding | `0xdd6f56DcC28D3F5f27084381fE8Df634985cc39f` |
| `RoundsManagerTarget.json` | RoundsManager implementation reference | (target) |
| `LivepeerToken.json` | LPT ERC-20 event decoding | `0x289ba1701C2F088cf0faf8B3705246331cB8A839` |
| `Minter.json` | Minter event decoding | `0xc20DE37170B45774e6CD3d2304017fc962f27252` |
| `LivepeerGovernor.json` | Governor + governance events (`ProposalCreated`, `VoteCast`, `ProposalExecuted`) per §6.7 | `0xD9dEd6f9959176F0A04dcf88a0d2306178A736a6` |
| `ServiceRegistry.json` | Service URI updates | (Controller-resolved) |
| `L2Migrator.json` | L2 migration events (informational) | (Controller-resolved) |
| `AggregatorV3Interface.json` | Chainlink ETH/USD aggregator (`latestRoundData`) per §7.3.3 | (config) |
| `UniswapV3Pool.json` | LPT/WETH pool (`observe`, `slot0`, `increaseObservationCardinalityNext`) per §7.3.1 | `0x4fd47e5102dfbf95541f64ed6fe13d4ed26d2546` |

## Missing

- **`BondingVotes.json`** — proxy at `0x0B9C254837E72Ebe9Fe04960C43B69782E68169A` (SPEC §5.1). Tracked in `docs/exec-plans/tech-debt-tracker.md` (TD-001). v1 event catalog (§6.7) only covers Governor events, so this is unblocked for v1; fetch from Arbiscan when delegation events are added.

## Discipline

- One ABI file per contract version. When a contract upgrades, add `{ContractName}_v{N}.json` and a new `contract_abi_registry` row covering the new block range (RUNBOOK §6).
- Never edit a file in place. Replace = new version + registry entry.
- The registry, not these filenames, is the source of truth for `(proxy, block_range) -> abi`.
