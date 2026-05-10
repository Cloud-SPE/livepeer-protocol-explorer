import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { delegatorsService } from '../services/delegators.service.js';
import type { DelegatorEventRow, DelegatorResponse } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import { shortAddress } from '../lib/format.js';
import '../components/ui/address-chip.js';
import '../components/ui/data-table.js';
import '../components/ui/empty-state.js';

const DELEGATION_COLS: ColumnDef[] = [
  { key: 'delegate_address', label: 'Orchestrator', cell: 'address' },
  { key: 'bonded_principal', label: 'Bonded', cell: 'lpt', align: 'end' },
  { key: 'pending_stake', label: 'Pending stake', cell: 'lpt', align: 'end' },
  { key: 'pending_fees', label: 'Pending fees', cell: 'eth', align: 'end' },
  { key: 'pending_round', label: 'Pending round', cell: 'mono', align: 'end' },
];

const EVENT_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'event_name', label: 'Event' },
  { key: 'counterparty', label: 'Counterparty', cell: 'address' },
  { key: 'amount_display', label: 'Amount', cell: 'mono', align: 'end' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

@customElement('view-delegator-detail')
export class ViewDelegatorDetail extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() address = '';
  @state() private data: DelegatorResponse | null = null;
  @state() private loading = false;
  @state() private error: string | null = null;
  @state() private events: DelegatorEventRow[] = [];
  @state() private eventsCursor: string | undefined = undefined;
  @state() private eventsLoading = false;
  @state() private eventsError: string | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.address) {
      void this._load();
      void this._loadEvents(true);
    }
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('address') && this.address) {
      void this._load();
      void this._loadEvents(true);
    }
  }

  private async _load(): Promise<void> {
    this.loading = true;
    this.error = null;
    try {
      this.data = await delegatorsService.fetchDelegator(this.address);
    } catch (err) {
      this.data = null;
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  private async _loadEvents(reset: boolean): Promise<void> {
    if (this.eventsLoading) return;
    this.eventsLoading = true;
    this.eventsError = null;
    try {
      const cursor = reset ? undefined : this.eventsCursor;
      const r = await delegatorsService.fetchDelegatorEvents(this.address, {
        ...(cursor ? { cursor } : {}),
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

  /** Build display rows for the events table — choose counterparty + format amount. */
  private _eventRows(): Array<Record<string, unknown>> {
    const me = this.address.toLowerCase();
    return this.events.map((e) => {
      const counterparty =
        e.from_address?.toLowerCase() === me
          ? (e.to_address ?? '')
          : (e.from_address ?? '');
      let amount_display = '';
      if (e.amount_normalized) {
        const asset = e.asset ?? '';
        amount_display = `${e.amount_normalized}${asset ? ' ' + asset : ''}`;
      } else if (e.event_name === 'EarningsClaimed' && e.decoded && typeof e.decoded === 'object') {
        const d = e.decoded as Record<string, unknown>;
        const rewards = d['rewards'];
        const fees = d['fees'];
        if (typeof rewards === 'string' || typeof fees === 'string') {
          const parts: string[] = [];
          if (typeof rewards === 'string')
            parts.push(`${(Number(BigInt(rewards) / 10n ** 14n) / 10000).toFixed(4)} LPT rewards`);
          if (typeof fees === 'string')
            parts.push(`${(Number(BigInt(fees) / 10n ** 14n) / 10000).toFixed(6)} ETH fees`);
          amount_display = parts.join(' · ');
        }
      }
      return {
        block_timestamp: e.block_timestamp,
        event_name: e.event_name,
        counterparty,
        amount_display,
        tx_hash: e.tx_hash,
      } as Record<string, unknown>;
    });
  }

  override render() {
    if (this.loading && !this.data) {
      return html`<empty-state heading="Loading…" body="Fetching delegator portfolio."></empty-state>`;
    }
    if (this.error && !this.data) {
      return html`<empty-state heading="Couldn't load delegator" .body=${this.error}></empty-state>`;
    }
    const d = this.data;
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/delegators">← Delegators</a></p>
          <h2>
            ${shortAddress(this.address)}
            ${d?.is_active ? html`<span class="pill pill--pos">Active</span>` : html`<span class="pill">Inactive</span>`}
          </h2>
          <p class="lede">
            <address-chip .address=${this.address} kind="delegator" .link=${false} explorer></address-chip>
          </p>
        </header>

        <section aria-labelledby="portfolio-h">
          <header><h3 id="portfolio-h">Portfolio</h3></header>
          <div class="card">
            <dl class="kv">
              <dt>First bond block</dt>
              <dd>${d?.first_bond_block ?? '—'}</dd>
              <dt>Last seen block</dt>
              <dd>${d?.last_seen_block ?? '—'}</dd>
              <dt>Delegations</dt>
              <dd>${d?.delegations.length ?? 0}</dd>
            </dl>
          </div>
        </section>

        <section aria-labelledby="delegations-h">
          <header><h3 id="delegations-h">Delegations</h3></header>
          <data-table
            caption="Current delegations"
            .columns=${DELEGATION_COLS}
            .rows=${(d?.delegations ?? []) as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{delegate_address}"
            empty-text="No current delegations"
          ></data-table>
        </section>

        <section aria-labelledby="events-h">
          <header><h3 id="events-h">Activity (${this.events.length}${this.eventsCursor ? '+' : ''})</h3></header>
          ${this.eventsError ? html`<p class="error" role="alert">${this.eventsError}</p>` : ''}
          ${this.events.length === 0 && !this.eventsLoading
            ? html`<empty-state heading="No activity" body="No Bond, Unbond, Rebond, EarningsClaimed, or Withdraw events for this delegator."></empty-state>`
            : html`
                <data-table
                  caption="Bond / Unbond / Rebond / EarningsClaimed / TransferBond / Withdraw events"
                  .columns=${EVENT_COLS}
                  .rows=${this._eventRows()}
                  empty-text="No activity"
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
      </article>
    `;
  }
}
