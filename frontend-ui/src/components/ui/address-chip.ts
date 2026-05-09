import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import { configService } from '../../services/config.service.js';
import { ensService } from '../../services/ens.service.js';
import { shortAddress } from '../../lib/format.js';

@customElement('address-chip')
export class AddressChip extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() address: string = '';
  @property() kind: 'orchestrator' | 'gateway' | 'delegator' | 'unknown' = 'unknown';
  @property({ type: Boolean }) link = true;
  @property({ type: Boolean }) explorer = false;
  @property({ type: Number }) head = 6;
  @property({ type: Number }) tail = 4;
  @state() private copied = false;

  private cfg = new ObservableController(this, configService.config$, configService.value);
  // Re-render when any cached ENS entry changes. Reading a single address
  // out of the map is cheap, so subscribing to the whole cache$ is fine.
  private ens = new ObservableController(this, ensService.cache$, ensService.cache);

  private async _copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.address);
      this.copied = true;
      setTimeout(() => (this.copied = false), 1200);
    } catch {
      /* ignore */
    }
  }

  private _avatarBroken = (): void => {
    ensService.forgetAvatar(this.address);
  };

  override render() {
    const entry = ensService.lookup(this.address);
    const display = entry.name ?? shortAddress(this.address, this.head, this.tail);
    const isName = Boolean(entry.name);
    const href = this.link
      ? this.kind === 'orchestrator'
        ? `#/orchestrators/${this.address}`
        : this.kind === 'gateway'
          ? `#/gateways/${this.address}`
          : `#/delegators/${this.address}`
      : '';
    const explorerHref = `${this.cfg.value?.explorerAddressBase ?? ''}${this.address}`;

    const avatar = entry.avatar
      ? html`<img class="avatar" src=${entry.avatar} alt="" loading="lazy" decoding="async" @error=${this._avatarBroken} />`
      : '';
    const textClass = isName ? 'text name' : 'text mono';

    return html`
      ${avatar}
      ${this.link
        ? html`<a class="${textClass}" href="${href}" title="${this.address}">${display}</a>`
        : html`<span class="${textClass}" title="${this.address}">${display}</span>`}
      <button class="copy" type="button" title="Copy address" @click=${this._copy} aria-label="Copy address">
        ${this.copied ? html`<span class="ok">✓</span>` : '⧉'}
      </button>
      ${this.explorer
        ? html`<a class="ext" target="_blank" rel="noopener" href="${explorerHref}" title="Open in Arbiscan">↗</a>`
        : ''}
    `;
  }
}
