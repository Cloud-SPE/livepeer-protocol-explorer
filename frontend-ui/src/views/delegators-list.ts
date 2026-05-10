import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { delegatorsService } from '../services/delegators.service.js';
import type { DelegatorIndexRow } from '../types/api.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import '../components/ui/data-table.js';
import '../components/ui/empty-state.js';

const COLS: ColumnDef[] = [
  { key: 'delegator_address', label: 'Delegator', cell: 'address' },
  { key: 'total_bonded', label: 'Total bonded', cell: 'lpt', align: 'end' },
  { key: 'delegation_count', label: 'Delegations', cell: 'mono', align: 'end' },
  { key: 'is_active', label: 'Active', cell: 'bool', align: 'center' },
];

@customElement('view-delegators-list')
export class ViewDelegatorsList extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private rows: DelegatorIndexRow[] = [];
  @state() private cursor: string | undefined = undefined;
  @state() private loading = false;
  @state() private error: string | null = null;
  @state() private addressInput = '';

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
      const r = await delegatorsService.listDelegators({
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
    const raw = this.addressInput.trim().toLowerCase();
    if (!/^0x[a-f0-9]{40}$/.test(raw)) {
      this.error = `Invalid address: ${raw}`;
      return;
    }
    this.error = null;
    window.location.hash = `/delegators/${raw}`;
  }

  override render() {
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Delegators</h2>
            <p class="lede">Browse delegators ranked by total bonded LPT, or look one up by address.</p>
          </div>
          <div class="controls">
            <form @submit=${this._onSearch}>
              <input
                type="search"
                placeholder="0x… delegator address"
                aria-label="Delegator address"
                .value=${this.addressInput}
                @input=${(e: Event) => (this.addressInput = (e.target as HTMLInputElement).value)}
              />
              <button class="btn btn--primary" type="submit">Open</button>
            </form>
          </div>
        </header>

        ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}

        <section aria-labelledby="leaderboard-h">
          <header><h3 id="leaderboard-h">Top delegators by bonded LPT</h3></header>
          ${this.rows.length === 0 && !this.loading
            ? html`<empty-state heading="No delegators yet" body="The indexer has not surfaced any delegations."></empty-state>`
            : html`
                <data-table
                  caption="Delegators sorted by total bonded principal"
                  .columns=${COLS}
                  .rows=${this.rows as unknown as Record<string, unknown>[]}
                  href-template="#/delegators/{delegator_address}"
                  empty-text="No delegators"
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
