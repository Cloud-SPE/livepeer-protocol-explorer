import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { gatewaysService } from '../services/gateways.service.js';
import { configService } from '../services/config.service.js';
import { formatNative, formatTimestamp, formatUsd, shortAddress, todayIso } from '../lib/format.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { TimeSeries } from '../components/ui/time-chart.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/address-chip.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/time-chart.js';
import '../components/ui/bar-chart.js';

const BALANCE_COLS: ColumnDef[] = [
  { key: 'block_number', label: 'Block', cell: 'mono' },
  { key: 'deposit', label: 'Deposit (ETH)', cell: 'eth', align: 'end' },
  { key: 'reserve_funds_remaining', label: 'Reserve (ETH)', cell: 'eth', align: 'end' },
  { key: 'reserve_claimed_in_current_round', label: 'Claimed (ETH)', cell: 'eth', align: 'end' },
  { key: 'unlock_in_progress', label: 'Unlocking', cell: 'bool', align: 'center' },
  { key: 'source', label: 'Source', cell: 'pill' },
];

const RECIPIENTS_COLS: ColumnDef[] = [
  { key: 'recipient_address', label: 'Recipient', cell: 'address' },
  { key: 'payout_event_count', label: 'Payouts', cell: 'number', align: 'end' },
  { key: 'total_amount_native', label: 'Total (ETH)', cell: 'eth', align: 'end' },
  { key: 'total_amount_usd', label: 'Total USD', cell: 'usd', align: 'end' },
  { key: 'ticket_redeemed_count', label: 'Tickets', cell: 'number', align: 'end' },
];

@customElement('view-gateway-detail')
export class ViewGatewayDetail extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @property() address: string = '';
  @state() private detail = new ObservableController(this, gatewaysService.detail$, gatewaysService.detail);

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.address && gatewaysService.detail.address !== this.address) {
      void gatewaysService.loadDetail(this.address);
    }
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('address') && this.address && gatewaysService.detail.address !== this.address) {
      void gatewaysService.loadDetail(this.address);
    }
  }

  private _csvUrl(): string {
    const base = configService.value.baseApiUrl.replace(/\/$/, '');
    const today = todayIso();
    const start = new Date();
    start.setUTCDate(start.getUTCDate() - 30);
    const startIso = start.toISOString().slice(0, 10);
    return `${base}/reports/gateway-payouts.csv?gateway=${encodeURIComponent(this.address)}&start=${startIso}&end=${today}`;
  }

  /** Deposit + reserve across balance snapshots, X axis = block number. */
  private _balanceSeries(): TimeSeries[] {
    const points = (this.detail.value?.balanceHistory?.data ?? []).slice().reverse();
    if (!points.length) return [];
    const x = (b: string): number => Number(b ?? 0);
    return [
      {
        name: 'Deposit (ETH)',
        data: points.map(p => [x(p.block_number), parseFloat(p.deposit ?? '0')]),
        type: 'line',
        area: true,
      },
      {
        name: 'Reserve remaining (ETH)',
        data: points.map(p => [x(p.block_number), parseFloat(p.reserve_funds_remaining ?? '0')]),
        type: 'line',
      },
      {
        name: 'Claimed this round (ETH)',
        data: points.map(p => [x(p.block_number), parseFloat(p.reserve_claimed_in_current_round ?? '0')]),
        type: 'line',
      },
    ];
  }

  /** Top recipient orchs by total USD paid by this gateway. */
  private _topRecipientsData(): BarDatum[] {
    const rows = this.detail.value?.recipients?.data ?? [];
    return rows
      .map(r => ({ label: shortAddress(r.recipient_address), value: parseFloat(r.total_amount_usd ?? '0') }))
      .filter(d => d.value > 0)
      .sort((a, b) => b.value - a.value)
      .slice(0, 10);
  }


  override render() {
    const s = this.detail.value!;
    if (s.loading && !s.profile) {
      return html`<empty-state heading="Loading…" body="Fetching gateway profile."></empty-state>`;
    }
    if (s.error && !s.profile) {
      return html`<empty-state heading="Couldn't load gateway" .body=${s.error}></empty-state>`;
    }
    const p = s.profile;
    const a = s.analytics;

    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/gateways">← Gateways</a></p>
          <h2>
            ${p?.display_name ?? shortAddress(this.address)}
            ${p?.kind ? html`<span class="pill pill--accent">${p.kind}</span>` : ''}
            ${p?.unlock_in_progress ? html`<span class="pill pill--neg">Unlocking</span>` : ''}
          </h2>
          <p class="lede">
            <address-chip address="${this.address}" kind="gateway" .link=${false} explorer></address-chip>
          </p>
        </header>

        <section aria-labelledby="profile-h">
          <header>
            <h3 id="profile-h">Balance</h3>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => gatewaysService.loadDetail(this.address)}
            ></refresh-button>
          </header>
          <div class="card">
            <dl class="kv">
              <dt>Latest deposit</dt>
              <dd>${formatNative(s.balance?.deposit, 18, { digits: 6 })} ETH</dd>
              <dt>Reserve remaining</dt>
              <dd>${formatNative(s.balance?.reserve_funds_remaining, 18, { digits: 6 })} ETH</dd>
              <dt>Claimed this round</dt>
              <dd>${formatNative(s.balance?.reserve_claimed_in_current_round, 18, { digits: 6 })} ETH</dd>
              <dt>Withdraw round</dt>
              <dd>${s.balance?.withdraw_round ?? '—'}</dd>
              <dt>As of block</dt>
              <dd>${s.balance?.block_number ?? p?.as_of_block ?? '—'}</dd>
              <dt>Source</dt>
              <dd>${s.balance?.source ?? '—'}</dd>
            </dl>
          </div>
        </section>

        ${a
          ? html`
              <section aria-labelledby="totals-h">
                <header><h3 id="totals-h">7-day totals</h3></header>
                <p class="muted">${formatTimestamp(a.from_timestamp)} → ${formatTimestamp(a.to_timestamp)} · semantics: ${a.semantics}</p>
                <div class="totals">
                  <div class="card stat">
                    <div class="label">Funding</div>
                    <div class="value">${formatNative(a.funding.total_amount_native, 18, { digits: 4 })} ETH</div>
                    <div class="muted">${formatUsd(a.funding.total_amount_usd)}</div>
                  </div>
                  <div class="card stat">
                    <div class="label">Payouts</div>
                    <div class="value">${formatNative(a.payouts.total_amount_native, 18, { digits: 4 })} ETH</div>
                    <div class="muted">${formatUsd(a.payouts.total_amount_usd)}</div>
                  </div>
                  <div class="card stat">
                    <div class="label">Withdrawals</div>
                    <div class="value">${formatNative(a.withdrawals.total_amount_native, 18, { digits: 4 })} ETH</div>
                    <div class="muted">${formatUsd(a.withdrawals.total_amount_usd)}</div>
                  </div>
                </div>
              </section>
            `
          : ''}

        <section aria-labelledby="downloads-h">
          <header><h3 id="downloads-h">CSV reports</h3></header>
          <p><a class="btn" href=${this._csvUrl()} download>Download gateway payouts CSV</a></p>
        </section>

        <section aria-labelledby="recipients-h">
          <header><h3 id="recipients-h">Top recipients</h3></header>
          <data-table
            caption="Orchestrators receiving payouts"
            .columns=${RECIPIENTS_COLS}
            .rows=${(s.recipients?.data ?? []) as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{recipient_address}"
            empty-text="No payouts in window"
          ></data-table>
          ${(s.recipients?.data?.length ?? 0) > 0
            ? html`
                <chart-card
                  heading="Top recipients (USD)"
                  storage-key=${`gw.${this.address}.top-recipients`}
                >
                  <bar-chart
                    .data=${this._topRecipientsData()}
                    horizontal
                    y-format="usd"
                  ></bar-chart>
                </chart-card>
              `
            : ''}
        </section>

        <section aria-labelledby="balance-h">
          <header><h3 id="balance-h">Balance history</h3></header>
          <data-table
            caption="Balance snapshots"
            .columns=${BALANCE_COLS}
            .rows=${(s.balanceHistory?.data ?? []) as unknown as Record<string, unknown>[]}
            empty-text="No balance history"
          ></data-table>
          ${(s.balanceHistory?.data?.length ?? 0) > 0
            ? html`
                <chart-card
                  heading="Balance across snapshots"
                  storage-key=${`gw.${this.address}.balance`}
                >
                  <time-chart
                    .series=${this._balanceSeries()}
                    y-format="number"
                    chart-heading="Deposit / reserve / claimed (ETH) by block number"
                  ></time-chart>
                </chart-card>
              `
            : ''}
        </section>
      </article>
    `;
  }
}
