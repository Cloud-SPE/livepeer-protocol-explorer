import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { networkService } from '../services/network.service.js';
import type { RoundSummaryResponse } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import { formatNative, formatTimestamp, formatUsd } from '../lib/format.js';
import '../components/ui/data-table.js';
import '../components/ui/empty-state.js';

const ROUND_COLS: ColumnDef[] = [
  { key: 'address', label: 'Orchestrator', cell: 'address' },
  { key: 'total_stake', label: 'Stake', cell: 'lpt', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'reward_cut_percent', label: 'Reward cut', cell: 'percent', align: 'end' },
  { key: 'is_active', label: 'Active', cell: 'bool', align: 'center' },
];

@customElement('view-round-detail')
export class ViewRoundDetail extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() roundId = '';
  @state() private data: RoundSummaryResponse | null = null;
  @state() private loading = false;
  @state() private error: string | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.roundId) void this._load();
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('roundId') && this.roundId) {
      void this._load();
    }
  }

  private async _load(): Promise<void> {
    this.loading = true;
    this.error = null;
    try {
      this.data = await networkService.fetchRound(this.roundId);
    } catch (err) {
      this.data = null;
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  override render() {
    if (this.loading && !this.data) {
      return html`<empty-state heading="Loading…" body="Fetching round summary."></empty-state>`;
    }
    if (this.error && !this.data) {
      return html`<empty-state heading="Couldn't load round" .body=${this.error}></empty-state>`;
    }
    const round = this.data;
    const id = Number(this.roundId || round?.round || '0');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/rounds">← Rounds</a></p>
          <h2>Round ${round?.round ?? this.roundId}</h2>
          <p class="lede">
            Started block ${round?.round_started_block ?? '—'} · ${round?.round_started_at ? formatTimestamp(round.round_started_at) : '—'}
          </p>
        </header>

        <section aria-labelledby="summary-h">
          <header><h3 id="summary-h">Summary</h3></header>
          <div class="card">
            <dl class="kv">
              <dt>Active orchestrators</dt>
              <dd>${round?.active_orchestrators ?? '—'}</dd>
              <dt>Total LPT staked</dt>
              <dd>${formatNative(round?.total_lpt_staked, 18, { digits: 0, compact: true })} LPT</dd>
              <dt>Payouts on day</dt>
              <dd>${formatUsd(round?.payouts_usd_on_day)}</dd>
              <dt>Rewards on day</dt>
              <dd>${formatUsd(round?.rewards_usd_on_day)}</dd>
            </dl>
          </div>
        </section>

        <section aria-labelledby="top-h">
          <header><h3 id="top-h">Top orchestrators by stake</h3></header>
          <data-table
            caption="Round stake snapshot"
            .columns=${ROUND_COLS}
            .rows=${(round?.top_orchs ?? []) as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{address}"
            empty-text="No orchestrators for this round"
          ></data-table>
        </section>

        <p class="row-actions">
          <a class="btn" href="#/rounds/${Math.max(id - 1, 0)}">Previous round</a>
          <a class="btn" href="#/rounds/${id + 1}">Next round</a>
        </p>
      </article>
    `;
  }
}
