import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { networkService } from '../services/network.service.js';
import type {
  RoundEventCountsResponse,
  RoundEventRow,
  RoundSummaryResponse,
} from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import { formatNative, formatTimestamp, formatUsd, shortAddress } from '../lib/format.js';
import '../components/ui/data-table.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/bar-chart.js';

const ROUND_COLS: ColumnDef[] = [
  { key: 'address', label: 'Orchestrator', cell: 'address' },
  { key: 'total_stake', label: 'Stake', cell: 'lpt', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'reward_cut_percent', label: 'Reward cut', cell: 'percent', align: 'end' },
  { key: 'is_active', label: 'Active', cell: 'bool', align: 'center' },
];

const EVENT_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'event_name', label: 'Event' },
  { key: 'contract_name', label: 'Contract' },
  { key: 'from_address', label: 'From', cell: 'address' },
  { key: 'to_address', label: 'To', cell: 'address' },
  { key: 'amount_display', label: 'Amount', cell: 'mono', align: 'end' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

@customElement('view-round-detail')
export class ViewRoundDetail extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() roundId = '';
  @state() private data: RoundSummaryResponse | null = null;
  @state() private loading = false;
  @state() private error: string | null = null;
  @state() private counts: RoundEventCountsResponse | null = null;
  @state() private events: RoundEventRow[] = [];
  @state() private eventsCursor: string | undefined = undefined;
  @state() private eventsLoading = false;
  @state() private eventsError: string | null = null;
  @state() private eventsKindFilter = '';

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.roundId) {
      void this._load();
      void this._loadCounts();
    }
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('roundId') && this.roundId) {
      void this._load();
      void this._loadCounts();
      this.events = [];
      this.eventsCursor = undefined;
      this.eventsKindFilter = '';
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

  private async _loadCounts(): Promise<void> {
    try {
      this.counts = await networkService.fetchRoundEventCounts(this.roundId);
    } catch {
      this.counts = null;
    }
  }

  private async _loadEvents(reset: boolean): Promise<void> {
    if (this.eventsLoading) return;
    this.eventsLoading = true;
    this.eventsError = null;
    try {
      const cursor = reset ? undefined : this.eventsCursor;
      const r = await networkService.fetchRoundEvents(this.roundId, {
        ...(cursor ? { cursor } : {}),
        ...(this.eventsKindFilter ? { kinds: this.eventsKindFilter } : {}),
        limit: 50,
      });
      this.events = reset ? r.data : [...this.events, ...r.data];
      this.eventsCursor = r.meta.next_cursor;
    } catch (err) {
      this.eventsError = err instanceof Error ? err.message : String(err);
    } finally {
      this.eventsLoading = false;
    }
  }

  /** Top-orchs stake distribution as a bar chart. */
  private _stakeDistributionData(): BarDatum[] {
    return (this.data?.top_orchs ?? []).map(o => ({
      label: shortAddress(o.address),
      value: parseFloat(o.total_stake) || 0,
    }));
  }

  /** Format event rows for the table — stringify amount/asset. */
  private _eventRows(): Array<Record<string, unknown>> {
    return this.events.map(e => ({
      ...e,
      amount_display: e.amount_normalized
        ? `${e.amount_normalized}${e.asset ? ' ' + e.asset : ''}`
        : '',
    }));
  }

  /** Render a +/- delta against prev_round, signed. */
  private _renderDelta(curr: number, prev: number, opts?: { compact?: boolean; digits?: number; suffix?: string }): unknown {
    const diff = curr - prev;
    if (diff === 0 || !Number.isFinite(diff)) return html`<span class="muted">no change</span>`;
    const sign = diff > 0 ? '+' : '';
    const display = opts?.compact
      ? formatNative(String(diff), 0, { digits: opts.digits ?? 0, compact: true })
      : diff.toLocaleString(undefined, { maximumFractionDigits: opts?.digits ?? 2 });
    const klass = diff > 0 ? 'pos' : 'neg';
    return html`<span class="${klass}">${sign}${display}${opts?.suffix ?? ''}</span>`;
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
    const prev = round?.prev_round;
    const eventCountsList = this.counts
      ? Object.entries(this.counts.counts).sort((a, b) => b[1] - a[1])
      : [];
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
              <dd>
                ${round?.active_orchestrators ?? '—'}
                ${prev ? html` · vs r${prev.round}: ${this._renderDelta(round!.active_orchestrators, prev.active_orchestrators)}` : ''}
              </dd>
              <dt>Total LPT staked</dt>
              <dd>
                ${formatNative(round?.total_lpt_staked, 18, { digits: 0, compact: true })} LPT
                ${prev
                  ? html` · ${this._renderDelta(parseFloat(round!.total_lpt_staked), parseFloat(prev.total_lpt_staked), { compact: true, suffix: ' LPT' })}`
                  : ''}
              </dd>
              <dt>Payouts on day</dt>
              <dd>
                ${formatUsd(round?.payouts_usd_on_day)}
                ${round?.payouts_usd_30round_avg
                  ? html` · 30-round avg ${formatUsd(round.payouts_usd_30round_avg)}`
                  : ''}
                ${prev ? html` · ${this._renderDelta(parseFloat(round!.payouts_usd_on_day), parseFloat(prev.payouts_usd_on_day), { digits: 2, suffix: ' USD' })}` : ''}
              </dd>
              <dt>Rewards on day</dt>
              <dd>
                ${formatUsd(round?.rewards_usd_on_day)}
                ${round?.rewards_usd_30round_avg
                  ? html` · 30-round avg ${formatUsd(round.rewards_usd_30round_avg)}`
                  : ''}
                ${prev ? html` · ${this._renderDelta(parseFloat(round!.rewards_usd_on_day), parseFloat(prev.rewards_usd_on_day), { digits: 2, suffix: ' USD' })}` : ''}
              </dd>
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
          ${(round?.top_orchs?.length ?? 0) > 0
            ? html`
                <chart-card
                  heading="Top-10 stake distribution"
                  storage-key=${`round.${this.roundId}.stake-dist`}
                >
                  <bar-chart
                    .data=${this._stakeDistributionData()}
                    horizontal
                    y-format="number"
                  ></bar-chart>
                </chart-card>
              `
            : ''}
        </section>

        <section aria-labelledby="counts-h">
          <header><h3 id="counts-h">Event counts in round window</h3></header>
          ${this.counts
            ? html`
                <p class="muted">
                  ${this.counts.total.toLocaleString()} total events between block
                  ${this.counts.from_block} and ${this.counts.to_block ?? 'chain head'}.
                </p>
                <div class="stat-grid">
                  ${eventCountsList.slice(0, 8).map(([name, n]) => html`
                    <div class="stat">
                      <div class="label">${name}</div>
                      <div class="value">${n.toLocaleString()}</div>
                    </div>
                  `)}
                </div>
              `
            : html`<p class="muted">Loading event counts…</p>`}
        </section>

        <section aria-labelledby="events-h">
          <header><h3 id="events-h">Activity (${this.events.length}${this.eventsCursor ? '+' : ''})</h3></header>
          <p class="muted">
            Filter:
            <select @change=${(e: Event) => { this.eventsKindFilter = (e.target as HTMLSelectElement).value; void this._loadEvents(true); }}>
              <option value="">Default (high-volume)</option>
              <option value="Reward">Reward only</option>
              <option value="WinningTicketRedeemed,WinningTicketTransfer">Tickets</option>
              <option value="Bond,Unbond,Rebond,TransferBond">Stake actions</option>
              <option value="EarningsClaimed">EarningsClaimed</option>
              <option value="TranscoderUpdate,TranscoderActivated,TranscoderDeactivated">Transcoder lifecycle</option>
            </select>
          </p>
          ${this.eventsError ? html`<p class="error" role="alert">${this.eventsError}</p>` : ''}
          ${this.events.length === 0 && !this.eventsLoading
            ? html`
                <empty-state heading="No events loaded yet" body="Click the button below to load this round's activity."></empty-state>
                <p class="row-actions">
                  <button class="btn btn--primary" type="button" @click=${() => this._loadEvents(true)}>Load events</button>
                </p>
              `
            : html`
                <data-table
                  caption="Events that fired during the round window"
                  .columns=${EVENT_COLS}
                  .rows=${this._eventRows()}
                  empty-text="No events match the filter"
                ></data-table>
                ${this.eventsCursor
                  ? html`<button
                      class="btn"
                      type="button"
                      ?disabled=${this.eventsLoading}
                      @click=${() => this._loadEvents(false)}
                    >${this.eventsLoading ? 'Loading…' : 'Load more'}</button>`
                  : ''}
              `}
        </section>

        <p class="row-actions">
          <a class="btn" href="#/rounds/${Math.max(id - 1, 0)}">Previous round</a>
          <a class="btn" href="#/rounds/${id + 1}">Next round</a>
        </p>
      </article>
    `;
  }
}
