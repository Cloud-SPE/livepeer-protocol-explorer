# TD-028: CSS hygiene cleanup (inline styles, semantic HTML, theme color modernization)

**Status:** Resolved 2026-05-09
**Author:** 2026-05-09
**Severity:** low
**Source:** Frontend audit against [docs/references/modern-css-2026.md](../../references/modern-css-2026.md) (2026-05-09). Rules 2 (Semantic HTML) and 3 (No inline CSS) had isolated violations; the auto/dark themes still use hex while other themes already use `oklch()`.

## Resolution (2026-05-09)

All three phases shipped in one PR.

- **Phase A** — 9 inline `style="..."` attributes removed across `dashboard.ts` (×3 `container-type: inline-size` → folded into `view-dashboard .stat-grid`), `history-list.ts` (×2 → new `.head` + `.head h4` rules), `governance-proposals.ts` (×1 → `view-governance-proposals article.prop > header`), `governance-proposal-detail.ts` (×1 → new `.vote-count-note` class), `stats-perf.ts` (×1 → `.controls form` rule), `data-table.ts` (×1 → `.scroll > .caption` rule). `grep -rn 'style="' frontend-ui/src/` returns zero.
- **Phase B** — semantic tag swaps: 4× `<div class="card stat">` → `<article>` in payouts-summary; 3× same in governance-proposal-detail tally cards; `<div class="card chart-card">` → `<article>` in tickets-timeseries; `<div class="row-actions">` → `<menu aria-label="…">` in history-list (with UA-default reset added). Card-as-button antipattern in governance-proposals replaced with `<a class="card-link">` wrapping `<article class="prop">`; `tabindex/click/keydown` removed. `.card-link` promoted from view-dashboard scope to `base.css` so governance can share it.
- **Phase C** — `themes/auto.css` and `themes/dark.css` rewritten using `oklch()` and `color-mix(in oklab, …)` to match `themes/light.css`'s convention. The remaining `rgb(from var(--accent) r g b / 0.32)` in both files is intentional (relative-color syntax, modern-css-2026.md §24). `base.css` body-gradient `#0f1111` hardcode replaced with `var(--bg-sunken)`.

**Validation:** `npm run typecheck`, `npm run build`, `npm run test` (90/90), `npm run test:visual` (8/8 against committed baselines) all green. Manual visual sweep across themes confirmed by user.

## Background

The audit found `frontend-ui/` is fully compliant on rule 1 (Light DOM — all 36 `LitElement` subclasses override `createRenderRoot()`), with isolated violations on rules 2 and 3, and one localized theming asymmetry (`themes/auto.css` and `themes/dark.css` use hex/rgb while `themes/light.css` uses `oklch()` + `color-mix()`). Larger modern-CSS adoption (native nesting, `light-dark()`, fluid tokens, `:has()`) is tracked separately in [td-029-css-modernization-sprint.md](td-029-css-modernization-sprint.md).

## Scope

Three mechanical buckets, no design decisions, single PR.

1. **Inline-style purge** — 9 known `style="..."` attributes in templates.
2. **Semantic HTML fixes** — 5–6 isolated `<div>` containers that should be semantic tags.
3. **Auto/dark theme color modernization** — migrate from hex/rgb to `oklch()` + `color-mix()` to match `themes/light.css`.

## Phases

### Phase A — Inline-style purge (~45 min)

Each `style="..."` attribute moves to a class in the appropriate stylesheet. Most are flex/gap/padding utility patterns or `container-type: inline-size` declarations.

| File:line | Current inline value | Target |
|---|---|---|
| `frontend-ui/src/views/dashboard.ts:140,217,234` | `style="container-type: inline-size;"` (×3) | New utility class `.container-inline` in `styles/base.css`, or scope to the existing `.stat-grid` rule in `styles/components.css` if always paired |
| `frontend-ui/src/components/ui/history-list.ts:32` | `style="display:flex; gap: var(--sp-2); align-items: center; justify-content: space-between; margin-bottom: var(--sp-2);"` | Component-scoped `.history-list__head` rule in `styles/components.css` |
| `frontend-ui/src/components/ui/history-list.ts:33` | `style="margin:0; font-size: var(--fs-2);"` | Component-scoped `.history-list__title` rule |
| `frontend-ui/src/views/governance-proposals.ts:114` | `style="display:flex; gap: var(--sp-3); align-items: center; flex-wrap: wrap;"` | View-scoped class (e.g. `.prop-row__meta`) in `styles/components.css` |
| `frontend-ui/src/views/governance-proposal-detail.ts:162` | `style="margin-top: var(--sp-3);"` | View-scoped class (e.g. `.prop-detail__section-spacer`) |
| `frontend-ui/src/views/stats-perf.ts:118` | `style="display: contents;"` | New utility class `.contents` in `styles/base.css` (this pattern is reusable) |
| `frontend-ui/src/components/ui/data-table.ts:97` | `style="padding: var(--sp-3) var(--sp-4); font-weight: 600;"` | Component-scoped `.data-table__caption` (or wherever this lives) |

**Implementation rule:** prefer **component-scoped classes** over generic utilities for one-off patterns. Add a utility class only when the pattern repeats ≥3 times across views (the `display: contents` and `container-type: inline-size` cases qualify).

**Acceptance:**
- `grep -rn 'style="' frontend-ui/src/` returns zero results.
- Each affected view renders identically before/after (manual visual check).

### Phase B — Semantic HTML fixes (~45 min)

| File:line | Current | Target | Rationale |
|---|---|---|---|
| `frontend-ui/src/views/payouts-summary.ts:99,104,109,114` | `<div class="card stat">` | `<article class="card stat">` | Stat cards are self-contained content; `<article>` matches dashboard's existing pattern |
| `frontend-ui/src/views/governance-proposals.ts:95` | `<div class="list">` wrapping clickable `<article>` cards | `<section class="list" aria-label="Proposals">` | List is a labeled landmark, not a generic div |
| `frontend-ui/src/views/governance-proposals.ts:106-131` | `<article tabindex="0" @click @keydown>` mimicking a button | Either wrap the article body in an inner `<a href="...">` (preserve article semantics, get native click), **or** keep the article and remove the keydown handler in favor of a child link. Pick whichever preserves the styling | Card-as-button is an anti-pattern; native `<a>` gets keyboard, focus, middle-click, ctrl-click for free |
| `frontend-ui/src/views/tickets-timeseries.ts:100` | `<div class="card chart-card">` | `<article class="card chart-card">` | Same reasoning as payouts-summary |
| `frontend-ui/src/components/ui/history-list.ts:51` | `<div class="row-actions">` | `<menu class="row-actions">` (if action buttons) **or** `<div class="row-actions" role="group">` (if grouping unrelated controls) | Pick based on what the row-actions actually contains |
| `frontend-ui/src/views/ai-playground/image-to-text.ts:79,102` | `<section class="card ...">` used as cards | `<article class="card ...">` | `<section>` should be a thematic grouping of an outer article; standalone cards are articles |

**Acceptance:**
- Every flagged file uses the target tag.
- No visual regression (the swap should be a no-op when classes carry styling).
- Lighthouse / axe pass on the affected views (no new accessibility errors).

### Phase C — Auto + dark theme color modernization (~30 min)

Both `themes/auto.css` and `themes/dark.css` currently use hex (`#111111`, `#f5f7f7`, etc.) and `rgb(from #... r g b / α)`. The other themes (`light.css`, and likely `midnight.css` / `solarized.css` / `high-contrast.css` — verify) already use `oklch()` + `color-mix(in oklab, …)`.

**Mechanical translation:**
- `#111111`           → `oklch(15% 0 0)`
- `#181818`           → `oklch(20% 0 0)`
- `#0f1111`           → `oklch(13% 0.005 200)`
- `#f5f7f7`           → `oklch(96% 0.003 200)`
- `rgb(from #f5f7f7 r g b / 0.78)` → `color-mix(in oklab, var(--fg), transparent 22%)`
- `rgb(255 255 255 / 0.08)` → `color-mix(in oklab, white, transparent 92%)`
- `#28a36a` (`--pos`) → `oklch(62% 0.16 145)` (matches `light.css` `--pos` chroma/hue)
- `#d35c5c` (`--neg`) → `oklch(60% 0.20 25)` (matches `light.css` `--neg` chroma/hue)
- `#d9a441` (`--warn`) → `oklch(70% 0.16 75)`
- `#3f8fcb` (`--info`) → `oklch(60% 0.14 220)`
- `#18794e` (`--accent`) → `oklch(50% 0.16 155)` (slightly punchier in oklch space; verify against the existing brand)

**`*-soft` and `*-ring` derived tokens:** convert the `rgb(from …)` form to `color-mix(in oklab, var(--token), var(--bg) 82%)` to match `light.css`'s pattern.

**Verification step:** before/after screenshot of dashboard + payouts-summary + AI playground in **both** the auto and dark themes. Acceptable shifts are minor brightness/saturation differences from the gamut change; obviously-wrong shifts (colors reading as different brands) fail the phase. Adjust the `oklch` lightness/chroma until the result reads the same.

**Out of scope here:** restructuring `auto.css` to actually respond to OS preference via `light-dark()`. Today it's a dark-only alias; making it adaptive is **TD-029 Phase B**.

**Acceptance:**
- `grep -nE '#[0-9a-fA-F]{3,6}|rgb\(' themes/auto.css themes/dark.css` returns zero results.
- The other themes were spot-checked and confirmed already on `oklch()`.
- Visual diff against the four named themes shows no perceptible regression on a 5-route sweep.

## Validation

No automated visual regression coverage exists yet — that's the first phase of TD-029. For TD-028, validation is a **manual sweep** of the affected views in each theme:

- `/` (dashboard)
- `/orchestrators` + an orch detail page
- `/governance/proposals` + a proposal detail
- `/reports/daily/<today>`
- `/performance/stats`
- `/reports/tickets/daily`
- One AI playground view (`/ai/llm` or `/ai/image-to-text`)

Themes to check: `auto`, `dark`, `light`. Other themes only need a quick spot-check on the dashboard.

## Risks

| Risk | Mitigation |
|---|---|
| `oklch()` brightness shifts subtly from hex equivalents — the auto/dark themes might read as slightly different shades | Pick lightness/chroma values empirically against side-by-side screenshots; the goal is "same brand," not "same RGB" |
| Inline-style extraction creates orphaned utility classes that no one knows when to use | Prefer component-scoped class names; promote to utilities only when a pattern repeats ≥3 times |
| Card-as-button refactor in `governance-proposals.ts` could change focus order or break keyboard nav | Test with Tab navigation through the proposal list before/after; the inner-`<a>` approach is the safer of the two options |
| Visual sweep is human-driven and easy to miss subtle regressions | Acceptable for low-severity TD; TD-029 introduces the harness |

## Estimated effort

- Phase A: 0.75 h
- Phase B: 0.75 h
- Phase C: 0.5 h
- **Total: ~2 h** (single PR)

## Dependencies

- None. Independent of TD-027 and TD-029.

## Future-proofing

Once shipped, the codebase is fully compliant on rules 1, 2, and 3 of `modern-css-2026.md`. Rule 4 (modern CSS adoption — nesting, `light-dark()`, `clamp()`, `:has()`) remains open in TD-029.
