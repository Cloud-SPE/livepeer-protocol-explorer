import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import { configService } from '../../services/config.service.js';
import { shortAddress } from '../../lib/format.js';

@customElement('tx-chip')
export class TxChip extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() hash: string = '';
  private cfg = new ObservableController(this, configService.config$, configService.value);

  override render() {
    const cfg = this.cfg.value;
    const explorer = `${cfg?.explorerTxBase ?? ''}${this.hash}`;
    return html`
      <a href="${explorer}" target="_blank" rel="noopener" title="${this.hash}">${shortAddress(this.hash, 8, 6)}</a>
      <span class="ext">↗</span>
    `;
  }
}
