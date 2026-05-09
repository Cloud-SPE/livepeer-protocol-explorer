# CSS patterns

Curated patterns the project uses, with examples that are already live
in the codebase. Adding a new pattern? Update this file in the same PR.

The full CSS standard for the project is
[`docs/references/modern-css-2026.md`](../../../docs/references/modern-css-2026.md).
This file is the practical "where do we use these in `frontend-ui`"
companion.

---

## When to use `:has()`

Use `:has()` when a parent should react to the state or presence of a
descendant — and the alternative is JavaScript toggling a modifier
class on the parent. The descendant becomes the source of truth; the
parent responds to it.

**Good fits:**

- **Card tinting based on contained outcome pills.** See
  `styles/components.css` — `view-governance-proposals article.prop:has(.pill--pos)`
  tints passed proposals green based on the pill the template renders.
  No `prop--passed` modifier class to maintain.
- **Form field groups reacting to input validity.** See `styles/base.css` —
  `.field:has(:user-invalid) > label` turns the label red when the
  enclosed input is `:user-invalid`. Wrap a label + input + hint in
  `<div class="field">…</div>`.
- **Cards adapting to contained content.** `.card:has(img) { padding: 0; }`
  is the canonical example from the modern-CSS standard.
- **Nav highlighting based on route state.** `nav-group:has(a[aria-current="page"])`
  lets a parent group highlight when one of its links is active.

**Avoid for:**

- One-off styling that already has a class — leave existing patterns
  alone unless you're actively touching the file.
- Performance-critical selectors against very large DOM trees
  (e.g. `:has()` against the entire document body inside an animation
  loop). For our tree sizes this is not a concern.

---

## When to use `light-dark()`

Use `light-dark()` when a token has a light-mode value and a dark-mode
value and you want the browser to pick based on `color-scheme`.

**Where it lives:** `styles/themes/auto.css` — every token uses
`light-dark(lightVal, darkVal)`. The auto theme sets
`color-scheme: light dark`, so the browser resolves each token from
`prefers-color-scheme`.

**Branded themes (`midnight`, `solarized`, `high-contrast`) do NOT use
`light-dark()`.** They are not light-dark variants — they are explicit
themes. Each defines its own token values directly under
`[data-theme="..."]`.

The explicit `light` and `dark` themes also define tokens directly
(no `light-dark()`); their `[data-theme]` selectors take precedence
over `[data-theme="auto"]` when picked.

---

## Fluid sizing with `clamp()`

The token scale is partially fluid:

- `--fs-1` through `--fs-5` (in `styles/themes/_shape.css`) use
  `clamp()` so heading sizes grow with viewport width while body text
  stays near-fixed.
- `--sp-1` through `--sp-5` are **fixed** for visual predictability
  at small scales.
- `--sp-6`, `--sp-7`, `--sp-8` are **fluid** so layout-scale spacing
  grows on wide displays.
- `--sp-section` and `--sp-page` are aliases callers can opt into for
  fluid section padding and page gutters.

**When to reach for the fluid tokens:** page-level containers, hero
sections, large gaps between major regions. **When to keep things
fixed:** anything inside a card, table, list, or form — predictability
matters more than scale-responsiveness.

---

## Native nesting

The project's CSS supports native `&` nesting (Vite passes CSS through
unchanged; all evergreen browsers support it since 2023).

**Style guide:**

- **Cap nesting depth at 2.** A third level signals an over-broad
  parent selector — flatten and move on.
- Nest `&:hover`, `&:focus-visible`, `&[aria-current="page"]`, and
  immediate-child selectors freely. Don't nest unrelated rules just
  because they share a prefix.
- Don't combine semantically unrelated selectors under one parent
  block just to use nesting.

Example:

```css
.card {
  padding: var(--sp-3);

  & .title { font-weight: 600; }
  &:hover { background: var(--bg-elev); }
  &:has(img) { padding: 0; }
}
```

---

## `@container` queries vs. `@media` queries

Default to `@container` for component-level responsive behavior.
`@media` is for true viewport-level changes (e.g.
`prefers-reduced-motion`, `prefers-color-scheme`).

The codebase uses `container-type: inline-size` extensively (e.g.
`view-dashboard .stat-grid`, `view-governance-proposals .list`). When
adding a new component grid, follow the same pattern: declare
`container-type: inline-size` on the parent, then use
`@container (min-width: …)` to switch layouts.
