import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { networkService } from '../services/network.service.js';
import type { RoundIndexRow } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { TimeSeries } from '../components/ui/time-chart.js';
import '../components/ui/data-table.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/time-chart.js';

const COLS: ColumnDef[] = [
  { key: 'round', label: 'Round', cell: 'mono', align: 'end' },
  { key: 'started_at', label: 'Started', cell: 'reltime' },
  { key: 'started_block', label: 'Block', cell: 'mono', align: 'end' },
  { key: 'active_orchestrators', label: 'Active orchs', cell: 'mono', align: 'end' },
  { key: 'total_lpt_staked', label: 'Total stake', cell: 'lpt', align: 'end' },
  { key: 'payouts_usd_on_day', label: 'Payouts (day)', cell: 'usd', align: 'end' },
  { key: 'rewards_usd_on_day', label: 'Rewards (day)', cell: 'usd', align: 'end' },
];

@customElement('view-rounds-list')
export class ViewRoundsList extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private rows: RoundIndexRow[] = [];
  @state() private cursor: string | undefined = undefined;
  @state() private loading = false;
  @state() private error: string | null = null;
  @state() private roundInput = '';

  override connectedCallback(): void {
    super.connectedCallback();
    void this._load(true);
  }

  private async _load(reset: boolean): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.error = null;
    try {
      const cursor = reset ? undefined : this.cursor;
      const r = await networkService.listRounds({
        ...(cursor ? { cursor } : {}),
        limit: 50,
      });
      this.rows = reset ? r.data : [...this.rows, ...r.data];
      this.cursor = r.meta.next_cursor;
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  private _onSearch(e: Event): void {
    e.preventDefault();
    const raw = this.roundInput.trim();
    if (!/^\d+$/.test(raw)) {
      this.error = `Invalid round number: ${raw}`;
      return;
    }
    this.error = null;
    window.location.hash = `/rounds/${raw}`;
  }

  /** Build trend series for the chart from the loaded rows (chronological). */
  private _trendSeries(metric: 'active_orchestrators' | 'total_lpt_staked' | 'payouts_usd_on_day' | 'rewards_usd_on_day'): TimeSeries[] {
    const points = this.rows.slice().reverse(); // chronological
    if (!points.length) return [];
    const data: Array<[string, number]> = points.map(r => [r.started_at, parseFloat(r[metric] as unknown as string) || 0]);
    return [{ name: metric, data }];
  }

  override render() {
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Rounds</h2>
            <p class="lede">Browse rounds, watch network trends, or open one by id.</p>
          </div>
          <div class="controls">
            <form @submit=${this._onSearch}>
              <input
                type="search"
                placeholder="Round number"
                aria-label="Round number"
                .value=${this.roundInput}
                @input=${(e: Event) => (this.roundInput = (e.target as HTMLInputElement).value)}
              />
              <button class="btn btn--primary" type="submit">Open</button>
            </form>
          </div>
        </header>

        ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}

        ${this.rows.length > 1
          ? html`
              <section aria-labelledby="trends-h">
                <header><h3 id="trends-h">Trends across loaded rounds</h3></header>
                <chart-card heading="Total LPT staked per round" storage-key="rounds.total-stake">
                  <time-chart .series=${this._trendSeries('total_lpt_staked')} y-format="number"></time-chart>
                </chart-card>
                <chart-card heading="Active orchestrators per round" storage-key="rounds.active-orchs">
                  <time-chart .series=${this._trendSeries('active_orchestrators')} y-format="count"></time-chart>
                </chart-card>
                <chart-card heading="Payouts (USD) per round" storage-key="rounds.payouts-usd">
                  <time-chart .series=${this._trendSeries('payouts_usd_on_day')} y-format="usd"></time-chart>
                </chart-card>
                <chart-card heading="Rewards (USD) per round" storage-key="rounds.rewards-usd">
                  <time-chart .series=${this._trendSeries('rewards_usd_on_day')} y-format="usd"></time-chart>
                </chart-card>
              </section>
            `
          : ''}

        <section aria-labelledby="leaderboard-h">
          <header><h3 id="leaderboard-h">Recent rounds</h3></header>
          ${this.rows.length === 0 && !this.loading
            ? html`<empty-state heading="No rounds yet" body="The indexer has not surfaced any rounds."></empty-state>`
            : html`
                <data-table
                  caption="Rounds sorted newest first"
                  .columns=${COLS}
                  .rows=${this.rows as unknown as Record<string, unknown>[]}
                  href-template="#/rounds/{round}"
                  empty-text="No rounds"
                ></data-table>
                ${this.cursor
                  ? html`<button
                      class="btn"
                      type="button"
                      ?disabled=${this.loading}
                      @click=${() => this._load(false)}
                    >${this.loading ? 'Loading…' : 'Load more'}</button>`
                  : ''}
              `}
        </section>
      </article>
    `;
  }
}
