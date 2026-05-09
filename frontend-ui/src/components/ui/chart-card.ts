import { LitElement, html, css } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';

/**
 * Reusable wrapper for chart components (`time-chart`, `bar-chart`, etc.)
 * with a built-in collapsible header and per-card hide/show persistence.
 *
 * Uses shadow DOM (default) so the `<slot>` mechanism preserves the
 * caller's slotted chart child. Most other components in this app use
 * light DOM (`createRenderRoot(){return this;}`) but a wrapper that
 * accepts arbitrary slotted content **must** use shadow DOM — Lit's
 * light-DOM render path wipes out child nodes, which is what was
 * causing slotted bar/time charts to never reach the DOM.
 *
 * Usage:
 *
 * ```html
 * <chart-card heading="Fee cut over time" storage-key="orch.0xabc.fee-cut">
 *   <time-chart .series=${cutSeries} y-format="number"></time-chart>
 * </chart-card>
 * ```
 *
 * Behaviour:
 * - Default state is **expanded** (`default-collapsed` to opt out).
 * - On first visit the localStorage entry is missing → `default-collapsed`
 *   wins. After the user toggles, the choice is persisted under
 *   `chart-card.<storage-key>.collapsed = "true" | "false"`.
 * - When collapsed the slotted child is hidden via `display: none` (kept
 *   mounted so echarts state is preserved across collapse/expand cycles).
 * - When `storage-key` is omitted the card still toggles, just without
 *   persistence.
 */
@customElement('chart-card')
export class ChartCard extends LitElement {
  static override styles = css`
    :host { display: block; }

    article {
      background: var(--bg-elev);
      border: var(--bw-1) solid var(--border);
      border-radius: var(--r-3);
      box-shadow: var(--shadow-1);
      overflow: hidden;
    }

    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: var(--sp-3);
      padding: var(--sp-3) var(--sp-4);
      border-bottom: var(--bw-1) solid var(--border);
    }
    :host([compact]) header { padding: var(--sp-2) var(--sp-3); }

    h3 {
      margin: 0;
      font-size: var(--fs-3);
      font-weight: 600;
      line-height: 1.2;
      color: var(--fg);
    }

    button {
      display: inline-flex;
      align-items: center;
      gap: var(--sp-1);
      padding: 2px var(--sp-2);
      font: inherit;
      font-size: var(--fs-1);
      color: var(--fg-muted);
      background: transparent;
      border: var(--bw-1) solid var(--border);
      border-radius: var(--r-pill);
      cursor: pointer;
      transition: background 100ms, border-color 100ms, color 100ms;
    }
    button:hover { background: var(--bg-sunken); color: var(--fg); }
    button:focus-visible { outline: 2px solid var(--accent-ring); outline-offset: 2px; }

    .body { padding: var(--sp-4); }
    :host([compact]) .body { padding: var(--sp-3); }

    /* Hide-but-keep-mounted: keeps the slotted echarts instance alive
       across collapse/expand cycles so we don't pay the re-init cost. */
    .body[hidden] { display: none; }

    /* When the body is hidden, drop the head's bottom border so the head
       sits flush with the rounded card edge. */
    article.is-collapsed header { border-bottom: 0; }
  `;

  @property() heading = '';
  @property({ attribute: 'storage-key' }) storageKey = '';
  @property({ type: Boolean, attribute: 'default-collapsed' }) defaultCollapsed = false;
  @property({ type: Boolean, reflect: true }) compact = false;

  @state() private collapsed = false;

  override connectedCallback(): void {
    super.connectedCallback();
    this.collapsed = this._readStoredState();
  }

  private _storageKey(): string | null {
    return this.storageKey ? `chart-card.${this.storageKey}.collapsed` : null;
  }

  private _readStoredState(): boolean {
    const key = this._storageKey();
    if (!key) return this.defaultCollapsed;
    try {
      const stored = localStorage.getItem(key);
      if (stored === null) return this.defaultCollapsed;
      return stored === 'true';
    } catch {
      return this.defaultCollapsed;
    }
  }

  private _toggle(): void {
    this.collapsed = !this.collapsed;
    const key = this._storageKey();
    if (!key) return;
    try {
      localStorage.setItem(key, String(this.collapsed));
    } catch {
      // ignore — see _readStoredState comment.
    }
  }

  override render() {
    const expanded = !this.collapsed;
    const toggleLabel = this.collapsed ? 'Show' : 'Hide';
    const arrow = this.collapsed ? '▾' : '▴';
    return html`
      <article class=${this.collapsed ? 'is-collapsed' : ''}>
        <header>
          ${this.heading ? html`<h3>${this.heading}</h3>` : html`<h3>Chart</h3>`}
          <button
            type="button"
            @click=${this._toggle}
            aria-expanded=${String(expanded)}
            aria-label=${`${toggleLabel} ${this.heading || 'chart'}`}
          >
            <span aria-hidden="true">${arrow}</span>
            <span>${toggleLabel}</span>
          </button>
        </header>
        <div class="body" ?hidden=${this.collapsed}>
          <slot></slot>
        </div>
      </article>
    `;
  }
}
