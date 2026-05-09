import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { rewardsService } from '../services/rewards.service.js';
import { filtersService } from '../services/filters.service.js';
import { todayIso } from '../lib/format.js';
import type { RewardSort } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import { shortAddress } from '../lib/format.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/date-range.js';
import '../components/ui/chart-card.js';
import '../components/ui/bar-chart.js';

const COLS: ColumnDef[] = [
  { key: 'orchestrator_address', label: 'Orchestrator', cell: 'address' },
  { key: 'display_name', label: 'Name' },
  { key: 'reward_event_count', label: 'Events', cell: 'number', align: 'end' },
  { key: 'sum_total_tokens', label: 'Total LPT', cell: 'lpt', align: 'end' },
  { key: 'sum_total_tokens_usd', label: 'Total USD', cell: 'usd', align: 'end' },
  { key: 'sum_orch_tokens', label: 'Orch LPT', cell: 'lpt', align: 'end' },
  { key: 'sum_orch_tokens_usd', label: 'Orch USD', cell: 'usd', align: 'end' },
];

const SORTS: { value: RewardSort; label: string }[] = [
  { value: 'orch_tokens_usd', label: 'Orchestrator tokens USD' },
  { value: 'total_tokens_usd', label: 'Total tokens USD' },
  { value: 'reward_event_count', label: 'Reward event count' },
];

function thirtyDaysAgo(): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() - 30);
  return d.toISOString().slice(0, 10);
}

@customElement('view-rewards-leaderboard')
export class ViewRewardsLeaderboard extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, rewardsService.leaderboard$, rewardsService.leaderboard);

  override connectedCallback(): void {
    super.connectedCallback();
    const s = rewardsService.leaderboard;
    if (s.rows.length === 0 && !s.loading) {
      const f = filtersService.value;
      void rewardsService.refresh({
        from: f.rangeFrom || thirtyDaysAgo(),
        to: f.rangeTo || todayIso(),
        sort: f.rewardSort,
      });
    }
  }

  private _refresh(): void {
    const f = filtersService.value;
    void rewardsService.refresh({
      from: f.rangeFrom || thirtyDaysAgo(),
      to: f.rangeTo || todayIso(),
      sort: f.rewardSort,
    });
  }

  private _onRange(e: CustomEvent<{ from: string; to: string }>): void {
    filtersService.patch({ rangeFrom: e.detail.from, rangeTo: e.detail.to });
    this._refresh();
  }

  private _onSort(e: Event): void {
    const sort = (e.target as HTMLSelectElement).value as RewardSort;
    filtersService.patch({ rewardSort: sort });
    this._refresh();
  }

  /** Top 20 reward leaderboard rows by the active sort key. */
  private _topData(
    rows: { orchestrator_address: string; display_name?: string | null; reward_event_count: string; sum_total_tokens_usd: string; sum_orch_tokens_usd: string }[],
    sort: RewardSort,
  ): BarDatum[] {
    const fieldFor = (r: typeof rows[number]): number => {
      switch (sort) {
        case 'reward_event_count': return parseFloat(r.reward_event_count ?? '0');
        case 'total_tokens_usd':   return parseFloat(r.sum_total_tokens_usd ?? '0');
        case 'orch_tokens_usd':
        default:                   return parseFloat(r.sum_orch_tokens_usd ?? '0');
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
          <h2>Rewards leaderboard</h2>
          <p class="lede">Reward LPT distribution by orchestrator.</p>
          <div class="controls">
            <date-range
              .from=${f.rangeFrom || thirtyDaysAgo()}
              .to=${f.rangeTo || todayIso()}
              @change-range=${(e: CustomEvent<{ from: string; to: string }>) => this._onRange(e)}
            ></date-range>
            <label class="muted">
              Sort by
              <select @change=${this._onSort} .value=${f.rewardSort}>
                ${SORTS.map((o) => html`<option value=${o.value} ?selected=${o.value === f.rewardSort}>${o.label}</option>`)}
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
                storage-key="rewards.leaderboard.top"
              >
                <bar-chart
                  .data=${this._topData(s.rows, s.sort)}
                  horizontal
                  y-format=${s.sort === 'reward_event_count' ? 'count' : 'usd'}
                ></bar-chart>
              </chart-card>
            `
          : ''}
        <section aria-labelledby="rew-tbl-h">
          <h3 id="rew-tbl-h" class="sr-only">Rewards leaderboard</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="${s.from} → ${s.to} · sort by ${s.sort}"
            .columns=${COLS}
            .rows=${s.rows as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{orchestrator_address}"
            empty-text="No reward events in this window"
          ></data-table>
          <div class="row-actions">
            ${s.cursor
              ? html`<button type="button" class="btn" ?disabled=${s.loading} @click=${() => rewardsService.loadMore()}>
                  ${s.loading ? 'Loading…' : 'Load more'}
                </button>`
              : html`<span class="muted">All loaded.</span>`}
          </div>
        </section>
      </article>
    `;
  }
}
