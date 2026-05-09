import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { delegatorsService } from '../services/delegators.service.js';
import type { DelegatorResponse } from '../types/api.js';
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

@customElement('view-delegator-detail')
export class ViewDelegatorDetail extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() address = '';
  @state() private data: DelegatorResponse | null = null;
  @state() private loading = false;
  @state() private error: string | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    if (this.address) void this._load();
  }

  override updated(changed: Map<string, unknown>): void {
    if (changed.has('address') && this.address) {
      void this._load();
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
      </article>
    `;
  }
}
