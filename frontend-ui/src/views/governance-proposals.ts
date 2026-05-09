import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import {
  governanceService,
  proposalOutcome,
  proposalTitle,
  type ProposalOutcome,
} from '../services/governance.service.js';
import { formatNative, formatRelative, formatTimestamp } from '../lib/format.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';

type UiFilter = 'all' | ProposalOutcome;

const FILTERS: { value: UiFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'passed', label: 'Passed' },
  { value: 'defeated', label: 'Defeated' },
];

@customElement('view-governance-proposals')
export class ViewGovernanceProposals extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, governanceService.proposals$, governanceService.proposals);
  @state() private filter: UiFilter = 'all';

  override connectedCallback(): void {
    super.connectedCallback();
    // Always fetch the full list — filtering happens client-side using
    // proposalOutcome, because the backend's `status` query semantics
    // ("active" = "not executed") don't match the user-facing notion of
    // active / passed / defeated.
    if (governanceService.proposals.rows.length === 0 && !governanceService.proposals.loading) {
      void governanceService.refreshProposals('all');
    }
  }

  private _setFilter(f: UiFilter): void {
    this.filter = f;
  }

  override render() {
    const s = this.state.value!;
    const all = s.rows;
    const filtered = this.filter === 'all'
      ? all
      : all.filter((p) => proposalOutcome(p) === this.filter);

    // Counts for each filter so the tab labels can show how much each yields.
    const counts: Record<UiFilter, number> = { all: all.length, active: 0, passed: 0, defeated: 0 };
    for (const p of all) counts[proposalOutcome(p)] += 1;

    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Governance proposals</h2>
            <p class="lede">Treasury proposals from the Livepeer Governor.</p>
          </div>
          <div class="controls">
            <div class="group" role="group" aria-label="Filter by outcome">
              ${FILTERS.map(
                (opt) => html`
                  <button
                    type="button"
                    aria-pressed=${this.filter === opt.value ? 'true' : 'false'}
                    @click=${() => this._setFilter(opt.value)}
                  >
                    ${opt.label} <span class="muted">(${counts[opt.value]})</span>
                  </button>
                `,
              )}
            </div>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => governanceService.refreshProposals('all')}
            ></refresh-button>
          </div>
        </header>
        <section aria-labelledby="prop-list-h">
          <h3 id="prop-list-h" class="sr-only">Proposals list</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          ${filtered.length === 0 && !s.loading
            ? html`<empty-state
                heading="No ${this.filter === 'all' ? '' : this.filter} proposals"
                body=${this.filter === 'all'
                  ? 'The Governor has no recorded proposals yet.'
                  : `No proposals match the "${this.filter}" filter. Try another tab.`}
              ></empty-state>`
            : html`
                <div class="list">
                  ${filtered.map((p) => {
                    const href = `#/governance/proposals/${p.proposal_id}`;
                    const outcome = proposalOutcome(p);
                    const statusPill =
                      outcome === 'passed'
                        ? html`<span class="pill pill--pos">Passed</span>`
                        : outcome === 'defeated'
                          ? html`<span class="pill pill--neg">Defeated</span>`
                          : html`<span class="pill">Active</span>`;
                    return html`
                      <a class="card-link" href=${href}>
                        <article class="prop">
                          <header>
                            <h3>${proposalTitle(p)}</h3>
                            ${statusPill}
                          </header>
                          <div class="meta">
                            <span title=${formatTimestamp(p.created_at)}>Created ${formatRelative(p.created_at)}</span>
                            <span class="mono">#${p.proposal_id.slice(0, 12)}…</span>
                            ${p.executed_at
                              ? html`<span title=${formatTimestamp(p.executed_at)}>Executed ${formatRelative(p.executed_at)}</span>`
                              : ''}
                          </div>
                          <div class="tally" aria-label="Vote tally">
                            <span class="for">For ${formatNative(p.vote_tally.for_weight, 18, { digits: 0, compact: true })}</span>
                            <span class="against">Against ${formatNative(p.vote_tally.against_weight, 18, { digits: 0, compact: true })}</span>
                            <span class="abstain">Abstain ${formatNative(p.vote_tally.abstain_weight, 18, { digits: 0, compact: true })}</span>
                          </div>
                        </article>
                      </a>
                    `;
                  })}
                </div>
              `}
        </section>
      </article>
    `;
  }
}
