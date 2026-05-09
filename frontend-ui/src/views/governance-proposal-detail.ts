import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import {
  governanceService,
  proposalTitle,
  supportLabel,
} from '../services/governance.service.js';
import { orchestratorsService } from '../services/orchestrators.service.js';
import { formatNative, formatRelative, formatTimestamp } from '../lib/format.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import type { BarDatum } from '../components/ui/bar-chart.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/markdown-view.js';
import '../components/ui/address-chip.js';
import '../components/ui/tx-chip.js';
import '../components/ui/empty-state.js';
import '../components/ui/chart-card.js';
import '../components/ui/bar-chart.js';

type Tab = 'description' | 'votes';

const VOTE_COLS: ColumnDef[] = [
  { key: 'block_timestamp', label: 'When', cell: 'reltime' },
  { key: 'voter', label: 'Voter', cell: 'address' },
  { key: 'support_label', label: 'Support', cell: 'pill' },
  { key: 'weight', label: 'Weight (LPT)', cell: 'lpt', align: 'end' },
  { key: 'reason', label: 'Reason', truncate: true, width: '52ch' },
  { key: 'tx_hash', label: 'Tx', cell: 'tx' },
];

@customElement('view-governance-proposal-detail')
export class ViewGovernanceProposalDetail extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property() override id: string = '';
  @state() private detail = new ObservableController(this, governanceService.detail$, governanceService.detail);
  @state() private tab: Tab = this._tabFromHash();

  private _tabFromHash(): Tab {
    const hash = location.hash;
    const qIdx = hash.indexOf('?');
    if (qIdx === -1) return 'description';
    return new URLSearchParams(hash.slice(qIdx + 1)).get('tab') === 'votes' ? 'votes' : 'description';
  }

  private _onHash = (): void => {
    this.tab = this._tabFromHash();
  };

  override connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener('hashchange', this._onHash);
    if (this.id && governanceService.detail.id !== this.id) {
      void governanceService.loadDetail(this.id);
    }
    // Single bulk preload of the orchestrators list so voters who are
    // orchestrators show with their ENS name + avatar. Idempotent.
    void orchestratorsService.warmEnsCache();
  }
  override disconnectedCallback(): void {
    super.disconnectedCallback();
    window.removeEventListener('hashchange', this._onHash);
  }
  override updated(changed: Map<string, unknown>): void {
    if (changed.has('id') && this.id && governanceService.detail.id !== this.id) {
      void governanceService.loadDetail(this.id);
    }
  }

  private _hrefFor(tab: Tab): string {
    return `#/governance/proposals/${this.id}${tab === 'votes' ? '?tab=votes' : ''}`;
  }

  /** Vote tally as a 3-bar dataset, scaled by 1e18. */
  private _tallyData(): BarDatum[] {
    const t = this.detail.value?.proposal?.vote_tally;
    if (!t) return [];
    const scale = (v: string | undefined): number => Number(BigInt(v ?? '0') / 10n ** 14n) / 10000;
    return [
      { label: 'For',     value: scale(t.for_weight) },
      { label: 'Against', value: scale(t.against_weight) },
      { label: 'Abstain', value: scale(t.abstain_weight) },
    ];
  }

  override render() {
    const s = this.detail.value!;
    if (s.loading && !s.proposal) {
      return html`<empty-state heading="Loading…" body="Fetching proposal."></empty-state>`;
    }
    if (s.error && !s.proposal) {
      return html`<empty-state heading="Couldn't load proposal" .body=${s.error}></empty-state>`;
    }
    const p = s.proposal!;
    const t = p.vote_tally;
    const voteRows =
      s.votes?.data.map((v) => ({
        ...v,
        support_label: supportLabel(v.support),
      })) ?? [];
    const voteCount = voteRows.length;

    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/governance/proposals">← Proposals</a></p>
          <h2>
            ${proposalTitle(p)}
            ${p.executed
              ? html`<span class="pill pill--pos">Executed</span>`
              : html`<span class="pill">Not executed</span>`}
          </h2>
        </header>

        <section aria-labelledby="meta-h">
          <header>
            <h3 id="meta-h">Details</h3>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => governanceService.loadDetail(this.id)}
            ></refresh-button>
          </header>
          <div class="card">
            <dl class="kv">
              <dt>Proposal ID</dt>
              <dd>${p.proposal_id}</dd>
              <dt>Proposer</dt>
              <dd>${p.proposer ? html`<address-chip address=${p.proposer} kind="unknown" explorer></address-chip>` : '—'}</dd>
              <dt>Created</dt>
              <dd>${formatTimestamp(p.created_at)} · ${formatRelative(p.created_at)}</dd>
              <dt>Vote window</dt>
              <dd>${p.vote_start ?? '—'} → ${p.vote_end ?? '—'}</dd>
              ${p.executed_at
                ? html`<dt>Executed</dt>
                    <dd>${formatTimestamp(p.executed_at)} · ${formatRelative(p.executed_at)}</dd>`
                : ''}
              <dt>Created tx</dt>
              <dd><tx-chip hash=${p.created_tx_hash}></tx-chip></dd>
            </dl>
          </div>
        </section>

        <section aria-labelledby="tally-h">
          <header><h3 id="tally-h">Vote tally</h3></header>
          <div class="tally">
            <article class="card stat for">
              <div class="label">For</div>
              <div class="value">${formatNative(t.for_weight, 18, { digits: 0, compact: true })}</div>
            </article>
            <article class="card stat against">
              <div class="label">Against</div>
              <div class="value">${formatNative(t.against_weight, 18, { digits: 0, compact: true })}</div>
            </article>
            <article class="card stat">
              <div class="label">Abstain</div>
              <div class="value">${formatNative(t.abstain_weight, 18, { digits: 0, compact: true })}</div>
            </article>
          </div>
          <p class="muted vote-count-note">
            ${t.vote_count} vote${t.vote_count === '1' ? '' : 's'} cast.
          </p>
          <chart-card
            heading="Vote tally (LPT weight)"
            storage-key=${`proposal.${this.id}.tally`}
          >
            <bar-chart .data=${this._tallyData()} horizontal y-format="number"></bar-chart>
          </chart-card>
        </section>

        <nav class="proposal-tabs" role="tablist" aria-label="Proposal content">
          <a
            role="tab"
            href=${this._hrefFor('description')}
            aria-selected=${this.tab === 'description' ? 'true' : 'false'}
          >Description</a>
          <a
            role="tab"
            href=${this._hrefFor('votes')}
            aria-selected=${this.tab === 'votes' ? 'true' : 'false'}
          >Votes
            <span class="count" aria-label="${voteCount} votes loaded">${voteCount}</span>
          </a>
        </nav>

        ${this.tab === 'description'
          ? html`
              <section aria-labelledby="desc-h" role="tabpanel">
                <h3 id="desc-h" class="sr-only">Description</h3>
                <div class="card">
                  <markdown-view .source=${p.description ?? ''}></markdown-view>
                </div>
              </section>
            `
          : html`
              <section aria-labelledby="votes-h" role="tabpanel">
                <h3 id="votes-h" class="sr-only">Voters</h3>
                <data-table
                  caption="Vote events for this proposal"
                  .columns=${VOTE_COLS}
                  .rows=${voteRows as unknown as Record<string, unknown>[]}
                  empty-text="No votes cast yet"
                ></data-table>
              </section>
            `}
      </article>
    `;
  }
}
