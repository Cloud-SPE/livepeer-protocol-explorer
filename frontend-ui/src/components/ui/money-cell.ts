import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { formatNative, formatUsd, formatDecimal } from '../../lib/format.js';

@customElement('money-cell')
export class MoneyCell extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() native?: string;
  @property() usd?: string;
  @property() symbol = 'LPT';
  @property({ type: Number }) decimals = 18;
  @property({ type: Boolean }) raw = false;

  override render() {
    const nativeText = this.raw
      ? formatDecimal(this.native, { digits: 4 })
      : formatNative(this.native, this.decimals, { digits: 4 });
    return html`
      <span class="native">${nativeText}<span class="symbol">${this.symbol}</span></span>
      ${this.usd !== undefined && this.usd !== null && this.usd !== ''
        ? html`<span class="usd">${formatUsd(this.usd)}</span>`
        : ''}
    `;
  }
}
