import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { payoutsService } from '../services/payouts.service.js';
import { filtersService } from '../services/filters.service.js';
import { formatNative, formatTimestamp, formatUsd } from '../lib/format.js';
import type { JobType, SummaryPeriod } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/job-type-toggle.js';
import '../components/ui/empty-state.js';

const COLS: ColumnDef[] = [
  { key: 'orchestrator_address', label: 'Orchestrator', cell: 'address' },
  { key: 'display_name', label: 'Name' },
  { key: 'ticket_count', label: 'Tickets', cell: 'number', align: 'end' },
  { key: 'sum_face_value_native', label: 'Face value (ETH)', cell: 'eth', align: 'end' },
  { key: 'sum_face_value_usd', label: 'Face value USD', cell: 'usd', align: 'end' },
  { key: 'sum_commission_native', label: 'Commission (ETH)', cell: 'eth', align: 'end' },
  { key: 'sum_commission_usd', label: 'Commission USD', cell: 'usd', align: 'end' },
];

const PERIOD_LABEL: Record<SummaryPeriod, string> = {
  daily: 'Daily',
  weekly: 'Weekly',
  monthly: 'Monthly',
};

@customElement('view-payouts-summary')
export class ViewPayoutsSummary extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @property() period: SummaryPeriod = 'daily';
  @property() date: string = '';
  @state() private summary = new ObservableController(this, payoutsService.summary$, payoutsService.summary);

  override connectedCallback(): void {
    super.connectedCallback();
    this._maybeLoad();
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('period') || changed.has('date')) this._maybeLoad();
  }

  private _maybeLoad(): void {
    if (!this.date) return;
    const cur = payoutsService.summary;
    if (cur.period !== this.period || cur.date !== this.date) {
      void payoutsService.loadSummary(this.period, this.date, filtersService.value.jobType);
    }
  }

  private _setDate(e: Event): void {
    const next = (e.target as HTMLInputElement).value;
    if (next) window.location.hash = `/reports/${this.period}/${next}`;
  }

  private _setJobType(e: CustomEvent<JobType>): void {
    filtersService.patch({ jobType: e.detail });
    void payoutsService.loadSummary(this.period, this.date, e.detail);
  }

  override render() {
    const s = this.summary.value!;
    const sum = s.summary;
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>${PERIOD_LABEL[this.period]} payouts · ${this.date}</h2>
            ${sum
              ? html`<p class="lede">${formatTimestamp(sum.period_start)} → ${formatTimestamp(sum.period_end)} · valuation ${sum.valuation_version}</p>`
              : html`<p class="lede">Loading…</p>`}
          </div>
          <div class="controls">
            <nav class="dates" aria-label="Date">
              <label class="muted">Date <input type="date" .value=${this.date} @change=${this._setDate} /></label>
            </nav>
            <job-type-toggle
              .value=${filtersService.value.jobType}
              @change-job-type=${(e: CustomEvent<JobType>) => this._setJobType(e)}
            ></job-type-toggle>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => payoutsService.loadSummary(this.period, this.date, filtersService.value.jobType)}
            ></refresh-button>
          </div>
        </header>

        ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}

        <section aria-labelledby="totals-h">
          <h3 id="totals-h" class="sr-only">Period totals</h3>
          <div class="totals">
            <article class="card stat">
              <div class="label">Tickets</div>
              <div class="value">${sum?.ticket_count ?? '—'}</div>
              <div class="sub">${sum?.distinct_gateways ?? '—'} distinct gateways</div>
            </article>
            <article class="card stat">
              <div class="label">Face value</div>
              <div class="value">${formatNative(sum?.sum_face_value_native, 18, { digits: 4 })} ETH</div>
              <div class="sub">${formatUsd(sum?.sum_face_value_usd)}</div>
            </article>
            <article class="card stat">
              <div class="label">Commission</div>
              <div class="value">${formatNative(sum?.sum_commission_native, 18, { digits: 4 })} ETH</div>
              <div class="sub">${formatUsd(sum?.sum_commission_usd)}</div>
            </article>
            <article class="card stat">
              <div class="label">Delegators' share</div>
              <div class="value">${formatNative(sum?.sum_delegators_share_native, 18, { digits: 4 })} ETH</div>
              <div class="sub">${formatUsd(sum?.sum_delegators_share_usd)}</div>
            </article>
          </div>
        </section>

        <section aria-labelledby="leaderboard-h">
          <h3 id="leaderboard-h">Per-orchestrator breakdown</h3>
          <data-table
            caption="Top orchestrators in this window"
            .columns=${COLS}
            .rows=${s.rows as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{orchestrator_address}"
            empty-text="No payouts in this window"
          ></data-table>
        </section>
      </article>
    `;
  }
}
