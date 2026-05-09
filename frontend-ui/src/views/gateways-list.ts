import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { gatewaysService } from '../services/gateways.service.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import { shortAddress } from '../lib/format.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/bar-chart.js';

const COLUMNS: ColumnDef[] = [
  { key: 'address', label: 'Address', cell: 'address' },
  { key: 'display_name', label: 'Name' },
  { key: 'kind', label: 'Kind', cell: 'pill' },
  { key: 'latest_deposit', label: 'Deposit (ETH)', cell: 'eth', align: 'end' },
  { key: 'latest_reserve', label: 'Reserve (ETH)', cell: 'eth', align: 'end' },
  { key: 'unlock_in_progress', label: 'Unlocking', cell: 'bool', align: 'center' },
];

@customElement('view-gateways-list')
export class ViewGatewaysList extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private listState = new ObservableController(this, gatewaysService.list$, gatewaysService.list);

  override connectedCallback(): void {
    super.connectedCallback();
    if (gatewaysService.list.rows.length === 0 && !gatewaysService.list.loading) {
      void gatewaysService.refreshList();
    }
  }

  /** Top loaded gateways by latest_deposit (ETH). */
  private _topByDepositData(): BarDatum[] {
    const rows = gatewaysService.list.rows ?? [];
    return rows
      .map(r => ({
        label: r.display_name || shortAddress(r.address),
        value: parseFloat(r.latest_deposit ?? '0'),
      }))
      .filter(d => d.value > 0)
      .sort((a, b) => b.value - a.value)
      .slice(0, 20);
  }

  override render() {
    const s = this.listState.value!;
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Gateways</h2>
            <p class="lede">Broadcasters and gateways funding orchestrators on the network.</p>
          </div>
          <refresh-button
            ?loading=${s.loading}
            .lastUpdated=${s.lastUpdated}
            @refresh=${() => gatewaysService.refreshList()}
          ></refresh-button>
        </header>
        ${s.rows.length > 0
          ? html`
              <chart-card
                heading="Top by deposit (ETH)"
                storage-key="gateways.list.top-deposit"
              >
                <bar-chart .data=${this._topByDepositData()} horizontal y-format="number"></bar-chart>
              </chart-card>
            `
          : ''}
        <section aria-labelledby="gw-tbl-h">
          <h3 id="gw-tbl-h" class="sr-only">Gateways table</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="Gateways (${s.rows.length}${s.cursor ? '+' : ''} loaded)"
            .columns=${COLUMNS}
            .rows=${s.rows as unknown as Record<string, unknown>[]}
            href-template="#/gateways/{address}"
            empty-text="No gateways yet"
          ></data-table>
          <div class="row-actions">
            ${s.cursor
              ? html`<button type="button" class="btn" ?disabled=${s.loading} @click=${() => gatewaysService.loadMore()}>
                  ${s.loading ? 'Loading…' : 'Load more'}
                </button>`
              : html`<span class="muted">All loaded.</span>`}
          </div>
        </section>
      </article>
    `;
  }
}
