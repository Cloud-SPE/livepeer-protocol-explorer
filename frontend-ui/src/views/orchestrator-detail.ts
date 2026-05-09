import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { orchestratorsService } from '../services/orchestrators.service.js';
import { stakeHistoryService } from '../services/stake-history.service.js';
import { configService } from '../services/config.service.js';
import {
  formatNative,
  formatPercent,
  formatTimestamp,
  formatUsd,
  shortAddress,
  todayIso,
} from '../lib/format.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { TimeSeries } from '../components/ui/time-chart.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import type { CutsHistoryResponse, NetEconomicsResponse, StakeHistoryResponse } from '../types/api.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/address-chip.js';
import '../components/ui/tx-chip.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/time-chart.js';
import '../components/ui/bar-chart.js';

const PARAMS_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'block_number', label: 'Block', cell: 'mono' },
  { key: 'reward_cut_percent', label: 'Reward cut', cell: 'percent', align: 'end' },
  { key: 'fee_share_percent', label: 'Fee share (delegators)', cell: 'percent', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

const LIFECYCLE_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'event_name', label: 'Event' },
  { key: 'round', label: 'Round', cell: 'mono', align: 'end' },
  { key: 'is_active', label: 'Active', cell: 'bool', align: 'center' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

const TICKET_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'gateway_address', label: 'Gateway', cell: 'address' },
  { key: 'face_value', label: 'Face value (ETH)', cell: 'eth', align: 'end' },
  { key: 'face_value_usd', label: 'USD', cell: 'usd', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

const CUTS_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'block_number', label: 'Block', cell: 'mono', align: 'end' },
  { key: 'fee_cut_percent', label: 'Fee cut', cell: 'percent', align: 'end' },
  { key: 'reward_cut_percent', label: 'Reward cut', cell: 'percent', align: 'end' },
  { key: 'fee_share_percent', label: 'Fee share', cell: 'percent', align: 'end' },
];

@customElement('view-orchestrator-detail')
export class ViewOrchestratorDetail extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @property() address: string = '';
  @state() private detail = new ObservableController(this, orchestratorsService.detail$, orchestratorsService.detail);
  @state() private stakeHistory: StakeHistoryResponse | null = null;
  @state() private cutsHistory: CutsHistoryResponse | null = null;
  @state() private netEconomics: NetEconomicsResponse | null = null;
  @state() private historyLoading = false;
  @state() private historyError: string | null = null;
  @state() private historyRounds = 100;
  @state() private economicsDays = 30;

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.address && orchestratorsService.detail.address !== this.address) {
      void orchestratorsService.loadDetail(this.address);
    }
    if (this.address) {
      void this._loadHistoryPanels();
    }
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('address') && this.address && orchestratorsService.detail.address !== this.address) {
      void orchestratorsService.loadDetail(this.address);
      void this._loadHistoryPanels();
    }
  }

  private _csvUrl(report: 'rewards' | 'payouts'): string {
    const base = configService.value.baseApiUrl.replace(/\/$/, '');
    const today = todayIso();
    const start = new Date();
    start.setUTCDate(start.getUTCDate() - 30);
    const startIso = start.toISOString().slice(0, 10);
    return `${base}/reports/${report}.csv?orchestrator=${encodeURIComponent(this.address)}&start=${startIso}&end=${today}`;
  }

  /** Cuts-over-time series from params history. Reverses to chronological. */
  private _cutsSeries(): TimeSeries[] {
    const points = (this.detail.value?.paramsHistory?.data ?? []).slice().reverse();
    if (!points.length) return [];
    return [
      {
        name: 'Reward cut',
        data: points.map(p => [p.block_timestamp, parseFloat(p.reward_cut_percent ?? '0')]),
        type: 'line',
      },
      {
        name: 'Fee cut (orch keep)',
        data: points.map(p => [p.block_timestamp, parseFloat(p.fee_cut_percent ?? '0')]),
        type: 'line',
      },
      {
        name: 'Fee share (to delegators)',
        data: points.map(p => [p.block_timestamp, parseFloat(p.fee_share_percent ?? '0')]),
        type: 'line',
      },
    ];
  }

  /** Daily ticket volume + USD totals derived from recent tickets. */
  private _ticketActivitySeries(): { count: TimeSeries[]; usd: TimeSeries[] } {
    const tickets = this.detail.value?.tickets?.data ?? [];
    const map = new Map<string, { count: number; usd: number }>();
    for (const t of tickets) {
      const day = (t.block_timestamp ?? '').slice(0, 10);
      if (!day) continue;
      const e = map.get(day) ?? { count: 0, usd: 0 };
      e.count += 1;
      e.usd += parseFloat(t.face_value_usd ?? '0');
      map.set(day, e);
    }
    const days = [...map.keys()].sort();
    return {
      count: [
        { name: 'Tickets', data: days.map(d => [d, map.get(d)!.count]), type: 'bar' },
      ],
      usd: [
        { name: 'USD', data: days.map(d => [d, map.get(d)!.usd]), type: 'line', area: true },
      ],
    };
  }

  /** Top N gateways by total USD paid to this orch (within ticket window). */
  private _topGatewaysData(): BarDatum[] {
    const tickets = this.detail.value?.tickets?.data ?? [];
    const map = new Map<string, number>();
    for (const t of tickets) {
      const addr = t.gateway_address ?? '';
      if (!addr) continue;
      map.set(addr, (map.get(addr) ?? 0) + parseFloat(t.face_value_usd ?? '0'));
    }
    return [...map.entries()]
      .map(([addr, usd]) => ({ label: shortAddress(addr), value: usd }))
      .sort((a, b) => b.value - a.value)
      .slice(0, 10);
  }

  private _stakeSeries(): TimeSeries[] {
    const points = this.stakeHistory?.data ?? [];
    if (points.length === 0) return [];
    return [
      {
        name: 'Total stake',
        data: points.map((p) => [p.block_timestamp, Number(p.total_stake)]),
        type: 'line',
      },
    ];
  }

  private async _loadHistoryPanels(): Promise<void> {
    this.historyLoading = true;
    this.historyError = null;
    try {
      const stakeHistory =
        this.historyRounds === 0
          ? await stakeHistoryService.fetchStakeHistory(this.address, 0, undefined)
          : await stakeHistoryService.fetchStakeHistory(this.address, undefined, undefined);
      const [cutsHistory, netEconomics] = await Promise.all([
        orchestratorsService.fetchCutsHistory(this.address),
        orchestratorsService.fetchNetEconomics(this.address, this.economicsDays),
      ]);
      this.stakeHistory = this.historyRounds === 0
        ? stakeHistory
        : {
            ...stakeHistory,
            data: stakeHistory.data.slice(Math.max(stakeHistory.data.length - this.historyRounds, 0)),
          };
      this.cutsHistory = cutsHistory;
      this.netEconomics = netEconomics;
    } catch (err) {
      this.historyError = err instanceof Error ? err.message : String(err);
    } finally {
      this.historyLoading = false;
    }
  }

  private _setHistoryRounds(rounds: number): void {
    this.historyRounds = rounds;
    if (this.address) void this._loadHistoryPanels();
  }

  private _setEconomicsDays(days: number): void {
    this.economicsDays = days;
    if (this.address) void this._loadHistoryPanels();
  }

  override render() {
    const s = this.detail.value!;
    if (s.loading && !s.profile) {
      return html`<empty-state heading="Loading…" body="Fetching orchestrator profile."></empty-state>`;
    }
    if (s.error && !s.profile) {
      return html`<empty-state heading="Couldn't load orchestrator" .body=${s.error}></empty-state>`;
    }
    const p = s.profile;
    const params = s.blockProfile?.params ?? s.paramsHistory?.data?.[0];
    const lifecycle = s.blockProfile?.lifecycle ?? s.lifecycleHistory?.data?.[0];

    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/orchestrators">← Orchestrators</a></p>
          <h2>
            ${p?.display_name ?? shortAddress(this.address)}
            ${p?.is_active ? html`<span class="pill pill--pos">Active</span>` : html`<span class="pill">Inactive</span>`}
          </h2>
          <p class="lede">
            <address-chip address="${this.address}" kind="orchestrator" .link=${false} explorer></address-chip>
          </p>
        </header>

        <section aria-labelledby="profile-h">
          <header>
            <h3 id="profile-h">Profile</h3>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => orchestratorsService.loadDetail(this.address)}
            ></refresh-button>
          </header>
          <div class="card">
            <dl class="kv">
              <dt>Total stake</dt>
              <dd>${formatNative(p?.total_stake, 18, { digits: 4 })} LPT</dd>
              <dt>Reward cut</dt>
              <dd>${formatPercent(p?.reward_cut_percent)}</dd>
              <dt>Fee cut</dt>
              <dd>${formatPercent(p?.fee_cut_percent)}</dd>
              <dt>Fee share (delegators)</dt>
              <dd>${formatPercent(p?.fee_share_percent)}</dd>
              <dt>Service URI</dt>
              <dd>${p?.service_uri ?? '—'}</dd>
              <dt>Last lifecycle event</dt>
              <dd>${formatTimestamp(p?.last_lifecycle_event_at)}</dd>
              <dt>As of block</dt>
              <dd>${p?.as_of_block ?? '—'}</dd>
              ${params
                ? html`
                    <dt>Latest params block</dt>
                    <dd>${params.block_number}</dd>
                  `
                : ''}
              ${lifecycle
                ? html`
                    <dt>Latest lifecycle</dt>
                    <dd>${lifecycle.event_name} @ round ${lifecycle.round}</dd>
                  `
                : ''}
            </dl>
          </div>
        </section>

        <section aria-labelledby="downloads-h">
          <header><h3 id="downloads-h">CSV reports</h3></header>
          <p class="muted">Last 30 days. Adjust the URL to set custom date ranges.</p>
          <p>
            <a class="btn" href=${this._csvUrl('rewards')} download>Download rewards CSV</a>
            <a class="btn" href=${this._csvUrl('payouts')} download>Download payouts/tickets CSV</a>
          </p>
        </section>

        <section aria-labelledby="lifecycle-h">
          <header><h3 id="lifecycle-h">Lifecycle history</h3></header>
          <data-table
            caption="Activations and deactivations"
            .columns=${LIFECYCLE_COLS}
            .rows=${(s.lifecycleHistory?.data ?? []) as unknown as Record<string, unknown>[]}
            empty-text="No lifecycle events"
          ></data-table>
        </section>

        <section aria-labelledby="params-h">
          <header><h3 id="params-h">Params history</h3></header>
          <data-table
            caption="Reward cut / fee share changes"
            .columns=${PARAMS_COLS}
            .rows=${(s.paramsHistory?.data ?? []) as unknown as Record<string, unknown>[]}
            empty-text="No param changes recorded"
          ></data-table>
          ${(s.paramsHistory?.data?.length ?? 0) > 0
            ? html`
                <chart-card
                  heading="Cuts over time"
                  storage-key=${`orch.${this.address}.cuts`}
                >
                  <time-chart
                    .series=${this._cutsSeries()}
                    y-format="number"
                    chart-heading="Reward cut, fee cut, fee share (%)"
                  ></time-chart>
                </chart-card>
              `
            : ''}
        </section>

        <section aria-labelledby="stake-history-h">
          <header>
            <h3 id="stake-history-h">Stake history</h3>
            <div class="controls">
              ${[
                { rounds: 30, label: '30' },
                { rounds: 100, label: '100' },
                { rounds: 0, label: 'All' },
              ].map(({ rounds, label }) => html`
                <button type="button" class="btn" ?disabled=${this.historyRounds === rounds} @click=${() => this._setHistoryRounds(rounds)}>
                  ${label}
                </button>
              `)}
            </div>
          </header>
          ${this.historyError ? html`<p class="error" role="alert">${this.historyError}</p>` : ''}
          ${(this.stakeHistory?.data.length ?? 0) > 0
            ? html`
                <chart-card heading="Total stake by round" storage-key=${`orch.${this.address}.stake-history`}>
                  <time-chart
                    .series=${this._stakeSeries()}
                    y-format="number"
                    chart-heading="Total stake by round"
                  ></time-chart>
                </chart-card>
              `
            : html`<empty-state heading="No data yet" body="No stake history is available for this orchestrator yet."></empty-state>`}
        </section>

        <section aria-labelledby="cuts-history-h">
          <header><h3 id="cuts-history-h">Cuts history</h3></header>
          <data-table
            caption="TranscoderUpdate history"
            .columns=${CUTS_COLS}
            .rows=${(this.cutsHistory?.data ?? []) as unknown as Record<string, unknown>[]}
            empty-text=${this.historyLoading ? 'Loading…' : 'No cut changes recorded'}
          ></data-table>
        </section>

        <section aria-labelledby="economics-h">
          <header>
            <h3 id="economics-h">Net economics</h3>
            <div class="controls">
              ${[7, 30, 90, 365].map((days) => html`
                <button type="button" class="btn" ?disabled=${this.economicsDays === days} @click=${() => this._setEconomicsDays(days)}>
                  ${days}d
                </button>
              `)}
            </div>
          </header>
          ${this.netEconomics
            ? html`
                <div class="card">
                  <dl class="kv">
                    <dt>Gross payouts</dt>
                    <dd>${formatUsd(this.netEconomics.gross_payouts_usd)}</dd>
                    <dt>Gross rewards</dt>
                    <dd>${formatUsd(this.netEconomics.gross_rewards_usd)}</dd>
                    <dt>Gross total</dt>
                    <dd>${formatUsd(this.netEconomics.gross_total_usd)}</dd>
                    <dt>Gas cost</dt>
                    <dd>${formatNative(this.netEconomics.gas_cost_native_eth, 18, { digits: 6 })} ETH</dd>
                    <dt>Window</dt>
                    <dd>${formatTimestamp(this.netEconomics.period_start)} → ${formatTimestamp(this.netEconomics.period_end)}</dd>
                  </dl>
                </div>
              `
            : html`<empty-state heading="No data yet" body="No economics summary is available for this orchestrator yet."></empty-state>`}
        </section>

        <section aria-labelledby="tickets-h">
          <header><h3 id="tickets-h">Recent tickets</h3></header>
          <data-table
            caption="Latest ticket redemptions"
            .columns=${TICKET_COLS}
            .rows=${(s.tickets?.data ?? []) as unknown as Record<string, unknown>[]}
            empty-text="No tickets in window"
          ></data-table>
          ${(s.tickets?.data?.length ?? 0) > 0
            ? html`
                <chart-card
                  heading="Daily ticket activity"
                  storage-key=${`orch.${this.address}.tickets-daily`}
                >
                  <time-chart
                    .series=${this._ticketActivitySeries().count}
                    y-format="count"
                    chart-heading="Tickets per day"
                  ></time-chart>
                </chart-card>
                <chart-card
                  heading="Daily ticket value (USD)"
                  storage-key=${`orch.${this.address}.tickets-usd`}
                >
                  <time-chart
                    .series=${this._ticketActivitySeries().usd}
                    y-format="usd"
                    chart-heading="Face value per day (USD)"
                  ></time-chart>
                </chart-card>
                <chart-card
                  heading="Top paying gateways"
                  storage-key=${`orch.${this.address}.top-gateways`}
                >
                  <bar-chart
                    .data=${this._topGatewaysData()}
                    horizontal
                    y-format="usd"
                  ></bar-chart>
                </chart-card>
              `
            : ''}
        </section>
      </article>
    `;
  }
}
