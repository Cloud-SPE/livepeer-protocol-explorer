import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { orchestratorsService } from '../services/orchestrators.service.js';
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
  { key: 'total_stake', label: 'Stake', cell: 'lpt', align: 'end' },
  { key: 'reward_cut_percent', label: 'Reward cut', cell: 'percent', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'is_active', label: 'Active', cell: 'bool', align: 'center' },
];

@customElement('view-orchestrators-list')
export class ViewOrchestratorsList extends LitElement {

  override createRenderRoot(): HTMLElement {
    return this;
  }

  @state() private listState = new ObservableController(this, orchestratorsService.list$, orchestratorsService.list);

  override connectedCallback(): void {
    super.connectedCallback();
    if (orchestratorsService.list.rows.length === 0 && !orchestratorsService.list.loading) {
      void orchestratorsService.refreshList();
    }
  }

  private _toggleActive(e: Event): void {
    const checked = (e.target as HTMLInputElement).checked;
    void orchestratorsService.refreshList(checked);
  }

  /** Top 20 currently-loaded orchs by total_stake (decimal LPT string). */
  private _topByStakeData(): BarDatum[] {
    const rows = orchestratorsService.list.rows ?? [];
    return rows
      .map(r => ({
        label: r.display_name || shortAddress(r.address),
        value: parseFloat(r.total_stake ?? '0'),
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
            <h2>Orchestrators</h2>
            <p class="lede">Operators on the network, sorted by total stake.</p>
          </div>
          <div class="controls">
            <label class="toggle">
              <input type="checkbox" .checked=${s.activeOnly} @change=${this._toggleActive} />
              Active only
            </label>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => orchestratorsService.refreshList()}
            ></refresh-button>
          </div>
        </header>
        ${s.rows.length > 0
          ? html`
              <chart-card
                heading="Top by stake (LPT)"
                storage-key="orchestrators.list.top-stake"
              >
                <bar-chart .data=${this._topByStakeData()} horizontal y-format="number"></bar-chart>
              </chart-card>
            `
          : ''}
        <section aria-labelledby="orch-tbl-h">
          <h3 id="orch-tbl-h" class="sr-only">Orchestrators table</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="Orchestrators (${s.rows.length}${s.cursor ? '+' : ''} loaded)"
            .columns=${COLUMNS}
            .rows=${s.rows as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{address}"
            empty-text="No orchestrators yet"
          ></data-table>
          <div class="row-actions">
            ${s.cursor
              ? html`<button type="button" class="btn" ?disabled=${s.loading} @click=${() => orchestratorsService.loadMore()}>
                  ${s.loading ? 'Loading…' : 'Load more'}
                </button>`
              : html`<span class="muted">All loaded.</span>`}
          </div>
        </section>
      </article>
    `;
  }
}
