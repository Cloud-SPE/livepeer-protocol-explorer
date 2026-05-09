# TD-029: CSS modernization sprint (native nesting, light-dark, fluid tokens, :has PoC)

**Status:** Resolved 2026-05-09
**Author:** 2026-05-09
**Severity:** medium
**Source:** Frontend audit against [docs/references/modern-css-2026.md](../../references/modern-css-2026.md) (2026-05-09). Rule 4 (Modern CSS adherence) is the weakest area — the project has adopted modern foundations (`@layer`, `@container`, `oklch`, `:where`) but underuses native nesting, `light-dark()`, `clamp()`, `:has()`, and `:user-invalid`.

## Resolution (2026-05-09)

All five phases shipped in one session — Phase A (the file-by-file native nesting refactor that was originally deferred to per-PR follow-up) was completed once Phase 0's harness was operational and producing zero-diff baselines.

- **Phase 0 — visual harness:** `playwright.config.ts` + `tests/visual/routes.spec.ts` cover 8 routes with API mocked to empty responses. Chromium-only, fixed 1280×800 viewport, animations disabled, `maxDiffPixelRatio: 0.01`. `@playwright/test` added to devDependencies; `test:visual` and `test:visual:update` scripts wired up. Baselines in `tests/visual/routes.spec.ts-snapshots/` (8 PNGs committed).
- **Phase A — native nesting:** `base.css`, `components.css` (1000+ lines), and `reset.css` (single `&:focus-visible` opportunity) all converted. Cap depth 2 observed throughout. `@container` and `@media` queries nested inside their target rules where possible (data-table mobile breakpoint, AI playground 1000px split, side-nav 900px aside reveal, etc.). Theme files and `_shape.css` left flat (no nesting opportunity in flat var declarations).
- **Phase B — `light-dark()` in auto theme:** `themes/auto.css` rewritten so each token uses `light-dark(lightVal, darkVal)`; `color-scheme: light dark` so it inherits OS preference. Branded themes (midnight/solarized/high-contrast/explicit-light/explicit-dark) untouched — they take precedence via their own `[data-theme]` selectors and define tokens directly. No theme service changes needed.
- **Phase C — fluid spacing tokens:** `themes/_shape.css` updated. `--sp-1`..`--sp-5` stay fixed. `--sp-6`/`--sp-7`/`--sp-8` now `clamp()`-fluid with `min` matching the previous fixed sizes (zero regression at narrow viewports). New opt-in aliases: `--sp-section`, `--sp-page`. Font scale `--fs-1`..`--fs-5` was already fluid in `_shape.css` — unchanged.
- **Phase D — `:has()` PoC + patterns doc:** `view-governance-proposals article.prop.prop--passed/--defeated` migrated to `:has(.pill--pos)` / `:has(.pill--neg)`. Modifier classes dropped from the template — pill is now the source of truth. `.field` group pattern added to `base.css` (`.field:has(:user-invalid) > label, .field:has(:user-invalid) > .hint { color: var(--neg); }`). `frontend-ui/src/styles/PATTERNS.md` written, documents `:has()`, `light-dark()`, `clamp()`, native nesting, and `@container` vs `@media`.

**Bonus — npm audit fix:** addressed in the same session. `happy-dom` 15.11.7 → 20.9.0 (critical RCE), `vite` 5.4.11 → 6.4.2 (esbuild moderate CVE), `vitest` 2.1.6 → 3.2.4 (vitest 2 ships its own internal vite ≤6.4.1). Result: 6 → 0 vulnerabilities, all tests still pass with no test changes.

**Validation:** `npm run typecheck`, `npm run build`, `npm run test` (90/90), `npm run test:visual` (8/8 against committed baselines) all green throughout the sprint. Manual visual sweep across themes confirmed by user.

## Background

The audit found that `frontend-ui/src/styles/`:
- Uses `@layer` and `@container` extensively (good).
- Uses flat selectors throughout — no native `&` nesting.
- Themes via `[data-theme="..."]` selectors only — no `light-dark()`.
- Uses fixed `--fs-N` and `--sp-N` tokens with no `clamp()` fluid sizing.
- Has zero `:has()` selectors despite obvious candidates (e.g. proposal-outcome tinting in `components.css:762-775`).
- Doesn't use `:user-invalid` for form validation styling.

This plan adopts all five in a coordinated sprint with a visual regression harness as Phase 0.

Hygiene-level cleanup (inline styles, semantic HTML, hex→oklch in remaining themes) is out of scope here and tracked in [td-028-css-hygiene-cleanup.md](td-028-css-hygiene-cleanup.md).

## Architecture decisions

These are baked-in choices, not open questions. They were debated and resolved before this plan was written.

1. **Native nesting** — adopt unconditionally. Vite passes CSS through unchanged; Chrome 112+, Safari 16.5+, Firefox 117+ all support `&` natively. **Cap nesting depth at 2.** Anything deeper signals an over-broad parent selector.
2. **`light-dark()` vs `[data-theme]`** — hybrid. The `auto` theme adopts `light-dark()` and becomes truly OS-preference-driven. The branded themes (`midnight`, `solarized`, `high-contrast`) keep their `[data-theme="..."]` selectors because they are not light-dark variants. The explicit `light` and `dark` themes also keep selector-based blocks but are written so a user picking them sets `color-scheme: only light` / `only dark` on `:root`, which `light-dark()` honors.
3. **`clamp()` for fluid sizing** — fluid-ize **token values**, not call sites. Headings (`--fs-3` and up) and section spacing (`--sp-section`, `--sp-page`) become fluid; body text (`--fs-1`, `--fs-2`) and micro-spacing (`--sp-1`–`--sp-3`) stay fixed. No component CSS changes — every existing `var(--fs-4)` automatically picks up the fluid behavior.
4. **`:has()` adoption** — proof-of-concept first, then opportunistic. One PR migrates the proposal-outcome tinting and adds a `:has(:user-invalid)` example on a form field. A patterns doc lists when `:has()` is appropriate. **No global sweep** — let future PRs reach for it organically.

## Phases

### Phase 0 — Visual regression harness (~2 hours)

Without a tripwire, the nesting refactor (Phase A) and `light-dark()` migration (Phase B) both require a manual sweep of every route to feel safe. With one, every subsequent CSS PR gets automatic pixel-diff coverage.

**Setup:**
- Add Playwright as a dev dependency: `npm i -D @playwright/test`.
- New file `playwright.config.ts` at the frontend root, configured to run `vite preview` on a fixed port and target Chromium only (CI doesn't need cross-browser visual diffs for an internal tool).
- New `frontend-ui/tests/visual/` directory with one spec (`routes.spec.ts`) that visits each route in the list below and calls `await expect(page).toHaveScreenshot()`.
- Screenshots committed under `frontend-ui/tests/visual/__screenshots__/`.

**Routes covered (initial set, ~8):**
- `/` (dashboard)
- `/orchestrators`
- `/orchestrators/<known-addr>` (pick a stable test address)
- `/gateways/<known-addr>`
- `/governance/proposals`
- `/governance/proposals/<known-id>`
- `/reports/daily/<fixed-date>` (fixed-date param so the snapshot is deterministic)
- `/performance/leaderboard`
- `/ai/llm` (one AI playground page is enough)

**Determinism considerations:**
- Mock the API at the network layer with a fixed JSON fixture per route, **or** point the harness at a frozen snapshot of the dev backend. Live data → flaky snapshots.
- Set a fixed viewport (1280×800) and disable animations via `page.addStyleTag` injecting `* { animation: none !important; transition: none !important; }`.

**CI wiring:**
- Add a `test:visual` script in `package.json`.
- Add a workflow step (or extend the existing test workflow) to run it. Failures upload the diff PNGs as artifacts.

**Acceptance:**
- `npm run test:visual` runs locally and passes.
- Snapshots committed for the 8 routes.
- A deliberate breaking change (e.g. setting `body { background: red }`) causes the suite to fail with a useful diff artifact.

### Phase A — Native nesting refactor (~5 hours)

Mechanical pass through every file in `frontend-ui/src/styles/`. No behavior change; computed styles should match before/after.

**Approach:**
- File by file. `base.css`, `components.css`, the `components/` subdirectory, the theme files. Skip `reset.css` and `_shape.css` if they're already minimal.
- For each repeated selector prefix, lift to a parent block with `&` nesting:
  ```css
  /* Before */
  .card { padding: var(--sp-3); }
  .card .title { font-weight: 600; }
  .card:hover { background: var(--bg-elev); }

  /* After */
  .card {
    padding: var(--sp-3);
    & .title { font-weight: 600; }
    &:hover { background: var(--bg-elev); }
  }
  ```
- **Cap depth at 2.** Three or more levels of nesting signals an over-broad parent selector — flatten and move on.
- Don't combine unrelated selectors just for the sake of nesting. If two rules share a prefix but are conceptually unrelated, leave them flat.

**Per-PR strategy:** ship file-by-file (or in groups of 2–3 closely related files). Massive single-PR refactors are hard to review and easy to break. Each PR runs Phase 0's harness; expect zero diff per PR.

**Acceptance:**
- Phase 0 visual harness passes on every PR.
- `grep -c '^\.' styles/components.css` (or similar selector-line count) drops meaningfully (~30%+) in files that had repeated prefixes.
- No nesting block exceeds depth 2.

### Phase B — `light-dark()` adoption in the auto theme (~1 hour)

**Today:** `themes/auto.css` is functionally a dark-only alias (`color-scheme: dark` regardless of OS preference).

**Target:** `auto` becomes the actually-adaptive theme, driven by OS `prefers-color-scheme` via `light-dark()`. The user keeps the ability to force light or dark explicitly through the existing theme switcher.

**Implementation:**

1. Set `:root { color-scheme: light dark; }` as the default in `base.css` (or wherever the root reset lives). This enables `light-dark()` throughout.

2. Rewrite `themes/auto.css` to use `light-dark()` per token:
   ```css
   @layer tokens {
     :root[data-theme="auto"] {
       /* color-scheme inherits "light dark" from :root → adaptive */
       --bg:        light-dark(oklch(98% 0.005 250), oklch(15% 0 0));
       --bg-elev:   light-dark(oklch(100% 0 0),     oklch(20% 0 0));
       --fg:        light-dark(oklch(20% 0.02 250), oklch(96% 0.003 200));
       /* ... etc, using the same token values that themes/light.css and
          themes/dark.css already define ... */
     }
   }
   ```
   The values for each side of `light-dark()` come from the existing `themes/light.css` and `themes/dark.css` definitions — this is a refactor, not a redesign.

3. Update the **theme switcher** logic (likely `theme-switcher.ts` or `services/theme.service.ts` — verify) so that when a user picks **"Light"** or **"Dark"** explicitly:
   - It sets `data-theme="light"` or `data-theme="dark"` (already does this).
   - It also sets `:root.style.colorScheme = 'only light'` or `'only dark'` so any `light-dark()` calls elsewhere in the page resolve correctly.
   - When the user picks **"Auto"**, set `data-theme="auto"` and clear the inline `colorScheme` override (defaults back to `light dark`).

4. Verify the existing `themes/light.css` and `themes/dark.css` still work as explicit-pick overrides. They should — those selectors take precedence over `[data-theme="auto"]` when the user picks them.

**Out of scope:** migrating `midnight`, `solarized`, `high-contrast` to `light-dark()`. These are branded themes, not light-dark axes; they stay as `[data-theme="..."]` selector blocks.

**Acceptance:**
- With OS in light mode and `data-theme="auto"`, the app renders in light tokens.
- With OS in dark mode and `data-theme="auto"`, the app renders in dark tokens.
- Switching to `data-theme="light"` or `data-theme="dark"` overrides the OS preference.
- Switching to `data-theme="midnight"` etc. still works unchanged.
- Phase 0 visual harness covers both light and dark snapshots of `data-theme="auto"`.

### Phase C — Fluid tokens via `clamp()` (~1 hour)

Single file change: `styles/base.css` (or wherever `--fs-N` and `--sp-N` are declared). Zero call-site edits.

**Target token shape (Utopia-style fluid scale):**

```css
:root {
  /* Type scale — fixed at small, fluid at large */
  --fs-1: 0.875rem;                                   /* small text — fixed */
  --fs-2: 1rem;                                       /* body — fixed */
  --fs-3: clamp(1.125rem, 1.05rem + 0.4vw, 1.375rem); /* h4 / lede — slightly fluid */
  --fs-4: clamp(1.375rem, 1.2rem  + 0.9vw, 1.875rem); /* h3 — fluid */
  --fs-5: clamp(1.75rem,  1.4rem  + 1.7vw, 2.625rem); /* h2 — fluid */
  --fs-6: clamp(2.25rem,  1.7rem  + 2.7vw, 3.75rem);  /* h1 / hero — fully fluid */

  /* Spacing scale — micro-spacing fixed, layout spacing fluid */
  --sp-1: 0.25rem;
  --sp-2: 0.5rem;
  --sp-3: 0.75rem;                                    /* fixed up through here */
  --sp-4: clamp(1rem, 0.875rem + 0.5vw, 1.5rem);
  --sp-5: clamp(1.5rem, 1.25rem + 1vw, 2.5rem);
  --sp-section: clamp(2rem, 1rem + 4vw, 4rem);
  --sp-page:    clamp(1rem, 0.5rem + 2vw, 2.5rem);
}
```

Exact values are starting points — the implementer should tune them against the visual harness on representative breakpoints (375px / 768px / 1280px / 1920px).

**Why this approach:**
- Predictability at small scales (body text doesn't reflow as the window resizes).
- Page-level rhythm scales naturally with viewport.
- Zero call-site changes — every `var(--fs-5)` and `var(--sp-section)` automatically becomes fluid.
- Easy to tune or revert (one file).

**Acceptance:**
- Tokens edited; no other file changes.
- Phase 0 visual harness passes at the default 1280×800; manually verify at 375px and 1920px (or add those as additional viewports to the harness).
- Headings visibly scale with viewport; body text does not reflow size.

### Phase D — `:has()` proof of concept + patterns doc (~1 hour)

Two concrete migrations + a short patterns guide. **Not a sweep.**

**D1 — Migrate proposal-outcome tinting (~20 min)**

Today, `styles/components.css:762-775` styles `.prop.prop--passed` / `.prop.prop--failed` based on a class that JS applies to the parent based on the contained pill state.

Target:
```css
.prop:has(.pill--pos) {
  background: color-mix(in oklab, var(--pos-soft), transparent 50%);
}
.prop:has(.pill--neg) {
  background: color-mix(in oklab, var(--neg-soft), transparent 50%);
}
```

Then delete the JS that toggles `.prop--passed` / `.prop--failed` on the article, and remove those modifier classes from the template.

**D2 — Add `:has(:user-invalid)` to one form (~20 min)**

Pick a form (likely `views/ai-settings.ts` or similar — verify which views have inputs). Add a field-group style:

```css
.field:has(input:user-invalid) {
  & > label { color: var(--neg); }
  & > .hint { color: var(--neg); }
}
```

This is a demonstration of the pattern. The goal is to give future PRs an example to imitate.

**D3 — Patterns doc (~20 min)**

Add `frontend-ui/src/styles/PATTERNS.md` (or extend an existing docs file) with a short section:

```
## When to use :has()

Use :has() when a parent should react to the state or presence of a
descendant — and the alternative is JS toggling a modifier class on
the parent.

Good fits:
- Form field groups reacting to input validity (:has(:user-invalid))
- Cards adapting to contained content (:has(img), :has(video))
- Nav groups highlighting when they contain the current page (:has(a[aria-current="page"]))
- Cards tinting based on contained outcome pills (the proposals example)

Avoid for:
- One-off styling that already has a class — leave it alone
- Performance-critical selectors against very large DOM trees
```

**Acceptance:**
- Proposal-outcome tinting works without the `.prop--passed` / `.prop--failed` JS path.
- Form-field example renders red label/hint when input is `:user-invalid`.
- Patterns doc exists and is linked from the styles README (if there is one) or from `CLAUDE.md`.

## Validation

Phase 0 produces the harness; every subsequent phase runs it. Phases A, B, C should ideally show **zero visual diff** (refactors). Phase D produces small intentional diffs in the proposal list and the touched form view — review those by eye.

For light/dark/auto theme verification, the harness should snapshot at least the dashboard and one detail page in each of the three modes. (This adds ~3× the snapshot count for those routes — acceptable.)

## Risks

| Risk | Mitigation |
|---|---|
| Native nesting refactor introduces subtle specificity changes | Cap depth at 2; the visual harness will catch any rendering change |
| `light-dark()` doesn't behave the way the theme switcher expects (e.g. inline `color-scheme` not honored) | Test the four switcher modes (light, dark, auto, branded) in both OS preferences before declaring Phase B done |
| Fluid tokens make some existing layouts look wrong at small viewports (e.g. headings overlapping nav) | Tune the `clamp()` `min` value upward; the visual harness should add a 375px viewport snapshot to catch this |
| `:has()` migration in D1 changes the rendering order of background tints if the JS toggle was doing something subtle | Spot-check the proposal list before/after; if the JS was setting more than just the class (e.g. ARIA attributes), preserve those |
| Visual harness becomes a maintenance burden (snapshots flake on every legitimate change) | Keep the harness deliberately small (~8 routes); if it flakes, fix the determinism source (mocked API, fixed dates) rather than expanding tolerances |

## Estimated effort

- Phase 0: 2 h
- Phase A: 5 h (spread across multiple small PRs)
- Phase B: 1 h
- Phase C: 1 h
- Phase D: 1 h
- **Total: ~10 hours** (~1.5 days)

## Dependencies

- TD-028 (CSS hygiene cleanup) — not a strict blocker, but landing TD-028 first means the inline-style purge and theme oklch conversion don't conflict with this sprint's diffs. Recommend TD-028 first, TD-029 after.

## Future-proofing

After this lands, `frontend-ui/` is fully aligned with `modern-css-2026.md` rules 1–4. Subsequent CSS work inherits the visual harness as a regression gate. The `:has()` patterns doc seeds a culture of opportunistic adoption — future features that would otherwise reach for a JS class toggle should default to `:has()` instead.

Optional follow-ups, not in this plan:
- **`@scope`** for component-local styling without BEM-style class prefixing.
- **Anchor positioning** for tooltips and popovers (currently likely positioned via JS or absolute math).
- **`@property`** for typed custom properties on animated tokens.
- **View Transitions** beyond the existing `withViewTransition` use — coordinated cross-route transitions.
