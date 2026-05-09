import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { governanceService, supportLabel } from '../services/governance.service.js';
import { orchestratorsService } from '../services/orchestrators.service.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';

const COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'proposal_short', label: 'Proposal', cell: 'mono' },
  { key: 'voter', label: 'Voter', cell: 'address' },
  { key: 'support_label', label: 'Support', cell: 'pill' },
  { key: 'weight', label: 'Weight (LPT)', cell: 'lpt', align: 'end' },
  { key: 'reason', label: 'Reason', truncate: true, width: '52ch' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

@customElement('view-governance-votes')
export class ViewGovernanceVotes extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private votes = new ObservableController(this, governanceService.votes$, governanceService.votes);

  override connectedCallback(): void {
    super.connectedCallback();
    if (governanceService.votes.rows.length === 0 && !governanceService.votes.loading) {
      void governanceService.refreshVotes();
    }
    // Single bulk preload of orchestrators so voter rows render with ENS
    // names + avatars. Voters who aren't orchestrators stay as truncated
    // hex — the bulk fetch covers everyone in one call, so there's no
    // 404 noise from per-address probes.
    void orchestratorsService.warmEnsCache();
  }

  override render() {
    const s = this.votes.value!;
    // Keep the full proposal_id on the row so href-template can navigate to
    // the matching proposal detail; expose a truncated copy for display.
    const rows = s.rows.map((v) => ({
      ...v,
      support_label: supportLabel(v.support),
      proposal_short: `#${v.proposal_id.slice(0, 10)}…`,
    }));
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <p class="crumb"><a href="#/governance/proposals">← Proposals</a></p>
            <h2>All governance votes</h2>
            <p class="lede">
              Every Governor VoteCast event across all proposals. Click a row to open the proposal it was cast on.
            </p>
          </div>
          <refresh-button
            ?loading=${s.loading}
            .lastUpdated=${s.lastUpdated}
            @refresh=${() => governanceService.refreshVotes()}
          ></refresh-button>
        </header>
        <section aria-labelledby="votes-tbl-h">
          <h3 id="votes-tbl-h" class="sr-only">Votes table</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="Recent votes (${s.rows.length}${s.cursor ? '+' : ''} loaded)"
            .columns=${COLS}
            .rows=${rows as unknown as Record<string, unknown>[]}
            href-template="#/governance/proposals/{proposal_id}"
            empty-text="No votes yet"
          ></data-table>
          <div class="row-actions">
            ${s.cursor
              ? html`<button type="button" class="btn" ?disabled=${s.loading} @click=${() => governanceService.loadMoreVotes()}>
                  ${s.loading ? 'Loading…' : 'Load more'}
                </button>`
              : html`<span class="muted">All loaded.</span>`}
          </div>
        </section>
      </article>
    `;
  }
}
