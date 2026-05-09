import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { formatRelative } from '../../lib/format.js';

@customElement('refresh-button')
export class RefreshButton extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property({ type: Boolean }) loading = false;
  @property() lastUpdated: string | null = null;
  @property() label = 'Refresh';

  private _click(): void {
    this.dispatchEvent(new CustomEvent('refresh', { bubbles: true, composed: true }));
  }

  override render() {
    return html`
      <button type="button" ?disabled=${this.loading} @click=${this._click} aria-label="${this.label}">
        <span class=${this.loading ? 'spin' : ''}>↻</span>
        <span>${this.label}</span>
      </button>
      ${this.lastUpdated
        ? html`<span class="stamp" title="${this.lastUpdated}">Updated ${formatRelative(this.lastUpdated)}</span>`
        : html`<span class="stamp">Not loaded yet</span>`}
    `;
  }
}
