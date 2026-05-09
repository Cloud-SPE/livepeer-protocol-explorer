import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';

function diffDays(from: string, to: string): number {
  if (!from || !to) return 0;
  const a = new Date(`${from}T00:00:00Z`);
  const b = new Date(`${to}T00:00:00Z`);
  if (Number.isNaN(a.valueOf()) || Number.isNaN(b.valueOf())) return 0;
  return Math.round((b.getTime() - a.getTime()) / 86_400_000);
}

@customElement('date-range')
export class DateRange extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() from: string = '';
  @property() to: string = '';

  private _change(part: 'from' | 'to', e: Event): void {
    const value = (e.target as HTMLInputElement).value;
    if (part === 'from') this.from = value;
    else this.to = value;
    // Swap if the user inverted the range — but never clamp the width.
    if (this.from && this.to && diffDays(this.from, this.to) < 0) {
      [this.from, this.to] = [this.to, this.from];
    }
    this.dispatchEvent(
      new CustomEvent('change-range', {
        detail: { from: this.from, to: this.to },
        bubbles: true,
        composed: true,
      }),
    );
  }

  override render() {
    const days = this.from && this.to ? diffDays(this.from, this.to) : 0;
    return html`
      <label>
        <span>From</span>
        <input type="date" .value=${this.from} @change=${(e: Event) => this._change('from', e)} />
      </label>
      <label>
        <span>To</span>
        <input type="date" .value=${this.to} @change=${(e: Event) => this._change('to', e)} />
      </label>
      ${days > 0 ? html`<span class="hint">${days} day${days === 1 ? '' : 's'}</span>` : ''}
    `;
  }
}
