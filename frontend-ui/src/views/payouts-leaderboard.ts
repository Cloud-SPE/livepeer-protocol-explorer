import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { payoutsService } from '../services/payouts.service.js';
import { filtersService } from '../services/filters.service.js';
import { todayIso } from '../lib/format.js';
import type { JobType, PayoutSort } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import { shortAddress } from '../lib/format.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/job-type-toggle.js';
import '../components/ui/date-range.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/bar-chart.js';

const COLS: ColumnDef[] = [
  { key: 'orchestrator_address', label: 'Orchestrator', cell: 'address' },
  { key: 'display_name', label: 'Name' },
  { key: 'ticket_count', label: 'Tickets', cell: 'number', align: 'end' },
  { key: 'sum_face_value_usd', label: 'Face value USD', cell: 'usd', align: 'end' },
  { key: 'sum_commission_usd', label: 'Commission USD', cell: 'usd', align: 'end' },
  { key: 'distinct_gateways', label: 'Gateways', cell: 'number', align: 'end' },
];

const SORTS: { value: PayoutSort; label: string }[] = [
  { value: 'commission_usd', label: 'Commission USD' },
  { value: 'face_value_usd', label: 'Face value USD' },
  { value: 'ticket_count', label: 'Ticket count' },
];

function thirtyDaysAgo(): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() - 30);
  return d.toISOString().slice(0, 10);
}

@customElement('view-payouts-leaderboard')
export class ViewPayoutsLeaderboard extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, payoutsService.leaderboard$, payoutsService.leaderboard);

  override connectedCallback(): void {
    super.connectedCallback();
    const s = payoutsService.leaderboard;
    if (s.rows.length === 0 && !s.loading) {
      const f = filtersService.value;
      void payoutsService.refreshLeaderboard({
        from: f.rangeFrom || thirtyDaysAgo(),
        to: f.rangeTo || todayIso(),
        jobType: f.jobType,
        sort: f.payoutSort,
      });
    }
  }

  private _refresh(): void {
    const f = filtersService.value;
    void payoutsService.refreshLeaderboard({
      from: f.rangeFrom || thirtyDaysAgo(),
      to: f.rangeTo || todayIso(),
      jobType: f.jobType,
      sort: f.payoutSort,
    });
  }

  private _onRange(e: CustomEvent<{ from: string; to: string }>): void {
    filtersService.patch({ rangeFrom: e.detail.from, rangeTo: e.detail.to });
    this._refresh();
  }

  private _onJobType(e: CustomEvent<JobType>): void {
    filtersService.patch({ jobType: e.detail });
    this._refresh();
  }

  private _onSort(e: Event): void {
    const sort = (e.target as HTMLSelectElement).value as PayoutSort;
    filtersService.patch({ payoutSort: sort });
    this._refresh();
  }

  /** Top 20 leaderboard rows by the active sort key. */
  private _topData(
    rows: { orchestrator_address: string; display_name?: string | null; ticket_count: string; sum_face_value_usd: string; sum_commission_usd: string }[],
    sort: PayoutSort,
  ): BarDatum[] {
    const fieldFor = (r: typeof rows[number]): number => {
      switch (sort) {
        case 'ticket_count':    return parseFloat(r.ticket_count ?? '0');
        case 'face_value_usd':  return parseFloat(r.sum_face_value_usd ?? '0');
        case 'commission_usd':
        default:                return parseFloat(r.sum_commission_usd ?? '0');
      }
    };
    return rows
      .map(r => ({ label: r.display_name || shortAddress(r.orchestrator_address), value: fieldFor(r) }))
      .filter(d => d.value > 0)
      .sort((a, b) => b.value - a.value)
      .slice(0, 20);
  }

  override render() {
    const s = this.state.value!;
    const f = filtersService.value;
    return html`
      <article class="page">
        <header class="page-head">
          <h2>Top payouts</h2>
          <p class="lede">Cursor-paginated leaderboard over the chosen window.</p>
          <div class="controls">
            <date-range
              .from=${f.rangeFrom || thirtyDaysAgo()}
              .to=${f.rangeTo || todayIso()}
              @change-range=${(e: CustomEvent<{ from: string; to: string }>) => this._onRange(e)}
            ></date-range>
            <job-type-toggle
              .value=${f.jobType}
              @change-job-type=${(e: CustomEvent<JobType>) => this._onJobType(e)}
            ></job-type-toggle>
            <label class="muted">
              Sort by
              <select @change=${this._onSort} .value=${f.payoutSort}>
                ${SORTS.map((o) => html`<option value=${o.value} ?selected=${o.value === f.payoutSort}>${o.label}</option>`)}
              </select>
            </label>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => this._refresh()}
            ></refresh-button>
          </div>
        </header>
        ${s.rows.length > 0
          ? html`
              <chart-card
                heading=${`Top by ${s.sort}`}
                storage-key="payouts.leaderboard.top"
              >
                <bar-chart
                  .data=${this._topData(s.rows, s.sort)}
                  horizontal
                  y-format=${s.sort === 'ticket_count' ? 'count' : 'usd'}
                ></bar-chart>
              </chart-card>
            `
          : ''}
        <section aria-labelledby="lb-tbl-h">
          <h3 id="lb-tbl-h" class="sr-only">Payouts leaderboard</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="${s.from} → ${s.to} · ${s.jobType} · sort by ${s.sort}"
            .columns=${COLS}
            .rows=${s.rows as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{orchestrator_address}"
            empty-text="No payouts in this window"
          ></data-table>
          <div class="row-actions">
            ${s.cursor
              ? html`<button type="button" class="btn" ?disabled=${s.loading} @click=${() => payoutsService.loadMoreLeaderboard()}>
                  ${s.loading ? 'Loading…' : 'Load more'}
                </button>`
              : html`<span class="muted">All loaded.</span>`}
          </div>
        </section>
      </article>
    `;
  }
}
