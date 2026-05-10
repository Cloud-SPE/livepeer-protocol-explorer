import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { orchestratorsService } from '../services/orchestrators.service.js';
import { gatewaysService } from '../services/gateways.service.js';
import { governanceService, proposalTitle } from '../services/governance.service.js';
import { payoutsService } from '../services/payouts.service.js';
import { networkCapabilitiesService } from '../services/network-capabilities.service.js';
import { networkService } from '../services/network.service.js';
import type { NetworkStatsResponse } from '../types/api.js';
import {
  formatNative,
  formatRelative,
  formatTimestamp,
  formatUsd,
  todayIso,
} from '../lib/format.js';
import '../components/ui/empty-state.js';
import '../components/ui/refresh-button.js';
import '../components/ui/address-chip.js';

@customElement('view-dashboard')
export class ViewDashboard extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private orchs = new ObservableController(this, orchestratorsService.list$, orchestratorsService.list);
  @state() private gws = new ObservableController(this, gatewaysService.list$, gatewaysService.list);
  @state() private props = new ObservableController(this, governanceService.proposals$, governanceService.proposals);
  @state() private summary = new ObservableController(this, payoutsService.summary$, payoutsService.summary);
  @state() private caps = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private networkStats: NetworkStatsResponse | null = null;
  @state() private networkLoading = false;
  @state() private networkError: string | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    if (orchestratorsService.list.rows.length === 0 && !orchestratorsService.list.loading) {
      void orchestratorsService.refreshList();
    }
    if (gatewaysService.list.rows.length === 0 && !gatewaysService.list.loading) {
      void gatewaysService.refreshList();
    }
    if (governanceService.proposals.rows.length === 0 && !governanceService.proposals.loading) {
      void governanceService.refreshProposals();
    }
    if (
      (!payoutsService.summary.summary || payoutsService.summary.date !== todayIso()) &&
      !payoutsService.summary.loading
    ) {
      void payoutsService.loadSummary('daily', todayIso(), 'both');
    }
    if (!networkCapabilitiesService.state.data && !networkCapabilitiesService.state.loading) {
      void networkCapabilitiesService.load();
    }
    if (!this.networkStats && !this.networkLoading) {
      void this._loadNetworkStats();
    }
  }

  private _refreshAll(): void {
    void orchestratorsService.refreshList();
    void gatewaysService.refreshList();
    void governanceService.refreshProposals();
    void payoutsService.loadSummary('daily', todayIso(), 'both');
    void networkCapabilitiesService.load();
    void this._loadNetworkStats();
  }

  private async _loadNetworkStats(): Promise<void> {
    this.networkLoading = true;
    this.networkError = null;
    try {
      this.networkStats = await networkService.fetchNetworkStats();
    } catch (err) {
      this.networkError = err instanceof Error ? err.message : String(err);
    } finally {
      this.networkLoading = false;
    }
  }

  private _aiStats(): { orchs: number; models: number; warmModels: number } {
    const data = this.caps.value?.data;
    if (!data) return { orchs: 0, models: 0, warmModels: 0 };
    const modelNames = new Set<string>();
    const warmModels = new Set<string>();
    for (const o of data.orchestrators) {
      for (const p of o.pipelines) {
        for (const m of p.models) {
          const id = `${p.type}:${m.name}`;
          modelNames.add(id);
          if (m.status.Warm > 0) warmModels.add(id);
        }
      }
    }
    return { orchs: data.orchestrators.length, models: modelNames.size, warmModels: warmModels.size };
  }

  private _lastUpdated(): string | null {
    const stamps = [
      this.orchs.value?.lastUpdated,
      this.gws.value?.lastUpdated,
      this.props.value?.lastUpdated,
      this.summary.value?.lastUpdated,
      this.caps.value?.lastUpdated,
      this._networkRefreshAge(),
    ].filter((s): s is string => Boolean(s));
    if (stamps.length === 0) return null;
    return stamps.reduce((a, b) => (a < b ? a : b));
  }

  private _anyLoading(): boolean {
    return Boolean(
      this.orchs.value?.loading ||
        this.gws.value?.loading ||
        this.props.value?.loading ||
        this.summary.value?.loading ||
        this.caps.value?.loading ||
        this.networkLoading,
    );
  }

  private _networkRefreshAge(): string | null {
    const stats = this.networkStats;
    if (!stats) return null;
    const stamps = [
      stats.orchestrator_profile_refreshed_at,
      stats.broadcaster_profile_refreshed_at,
    ].filter((s): s is string => Boolean(s));
    if (stamps.length === 0) return null;
    return stamps.reduce((a, b) => (a < b ? a : b));
  }

  override render() {
    const orchs = this.orchs.value;
    const gws = this.gws.value;
    const props = this.props.value;
    const summary = this.summary.value?.summary;
    const ai = this._aiStats();
    const network = this.networkStats;
    const top5 = (orchs?.rows ?? []).slice(0, 5);
    const recentProps = (props?.rows ?? []).slice(0, 3);
    const refreshAge = this._networkRefreshAge();

    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Dashboard</h2>
            <p class="lede">A snapshot of the Livepeer network from the local indexer.</p>
          </div>
          <refresh-button
            ?loading=${this._anyLoading()}
            .lastUpdated=${this._lastUpdated()}
            label="Refresh all"
            @refresh=${() => this._refreshAll()}
          ></refresh-button>
        </header>

        <section class="grid" aria-label="Overview cards">
          <article class="card span-3">
            <h3>Network totals</h3>
            <p class="meta">
              ${network?.latest_round
                ? html`<a href="#/rounds/${network.latest_round}">Round ${network.latest_round}</a>`
                : 'Round —'}
              · Block ${network?.latest_round_started_block ?? '—'}
              ${network?.latest_round_started_at
                ? html`· <span title=${formatTimestamp(network.latest_round_started_at)}>${formatRelative(network.latest_round_started_at)}</span>`
                : ''}
            </p>
            ${this.networkError ? html`<p class="error" role="alert">${this.networkError}</p>` : ''}
            <div class="stat-grid">
              <div class="stat">
                <div class="label">Active orchs</div>
                <div class="value">${network?.active_orchestrators ?? '—'}</div>
                <div class="sub">latest round snapshot</div>
              </div>
              <div class="stat">
                <div class="label">Total stake</div>
                <div class="value">${formatNative(network?.total_lpt_staked, 18, { digits: 0, compact: true })} LPT</div>
                <div class="sub">network total</div>
              </div>
              <div class="stat">
                <div class="label">Gateways</div>
                <div class="value">${network?.gateways_known ?? '—'}</div>
                <div class="sub">known broadcasters</div>
              </div>
              <a class="card-link" href="#/delegators">
                <div class="stat">
                  <div class="label">Active delegators</div>
                  <div class="value">${network?.active_delegators?.toLocaleString() ?? '—'}</div>
                  <div class="sub">${network?.total_delegations?.toLocaleString() ?? '—'} delegations</div>
                </div>
              </a>
              <div class="stat">
                <div class="label">Last refresh</div>
                <div class="value">${refreshAge ? formatRelative(refreshAge) : '—'}</div>
                <div class="sub" title="Materialized views refresh on an approximately 30 second cadence.">matview age</div>
              </div>
            </div>
            <div class="stat-grid">
              <div class="stat">
                <div class="label">24h payouts</div>
                <div class="value">${formatUsd(network?.payouts_usd_24h)}</div>
              </div>
              <div class="stat">
                <div class="label">24h rewards</div>
                <div class="value">${formatUsd(network?.rewards_usd_24h)}</div>
              </div>
              <div class="stat">
                <div class="label">24h gas burned</div>
                <div class="value">${formatNative(network?.gas_burned_eth_24h, 18, { digits: 4 })} ETH</div>
              </div>
            </div>
            <div class="footer-links">
              <a href="#/orchestrators">Browse orchestrators →</a>
              ·
              <a href="#/gateways">Browse gateways →</a>
              ·
              <a href="#/reports">Open reports →</a>
            </div>
          </article>

          <a class="card-link" href="#/orchestrators">
            <article class="card">
              <h3>Top 5 orchestrators</h3>
              ${top5.length === 0
                ? html`<p class="muted">Loading…</p>`
                : html`
                    <ol>
                      ${top5.map(
                        (o, i) => html`
                          <li>
                            <span class="name">${i + 1}. ${o.display_name || o.address.slice(0, 10) + '…'}</span>
                            <span class="meta mono">${formatNative(o.total_stake, 18, { digits: 0, compact: true })} LPT</span>
                          </li>
                        `,
                      )}
                    </ol>
                  `}
              <div class="footer-links"><a href="#/orchestrators">View all →</a></div>
            </article>
          </a>

          <a class="card-link" href="#/governance/proposals">
            <article class="card">
              <h3>Recent governance</h3>
              ${recentProps.length === 0
                ? html`<p class="muted">Loading…</p>`
                : html`
                    <ol>
                      ${recentProps.map(
                        (p) => html`
                          <li>
                            <span class="name">${proposalTitle(p)}</span>
                            <span class="meta" title=${formatTimestamp(p.created_at)}>${formatRelative(p.created_at)}</span>
                          </li>
                        `,
                      )}
                    </ol>
                  `}
              <div class="footer-links"><a href="#/governance/proposals">All proposals →</a></div>
            </article>
          </a>

          <a class="card-link" href="#/reports/tickets/daily">
            <article class="card">
              <h3>Activity charts</h3>
              <p class="muted">7-day ticket volume, payouts leaderboard, rewards over time.</p>
              <div class="stat-grid">
                <div class="pill-tile">
                  <div class="num">${summary?.ticket_count ?? '—'}</div>
                  <div class="meta">Tickets today</div>
                </div>
                <div class="pill-tile">
                  <div class="num">${formatNative(summary?.sum_face_value_native, 18, { digits: 2, compact: true })} ETH</div>
                  <div class="meta">Face value today</div>
                </div>
              </div>
              <div class="footer-links"><a href="#/reports/tickets/daily">Open chart →</a></div>
            </article>
          </a>

          <a class="card-link" href="#/ai/network-capabilities">
            <article class="card">
              <h3>AI capabilities</h3>
              <div class="stat-grid">
                <div class="stat">
                  <div class="label">Orchestrators</div>
                  <div class="value">${ai.orchs || '—'}</div>
                </div>
                <div class="stat">
                  <div class="label">Distinct models</div>
                  <div class="value">${ai.models || '—'}</div>
                  <div class="sub">${ai.warmModels} warm</div>
                </div>
              </div>
              <div class="footer-links"><a href="#/ai/network-capabilities">Browse capabilities →</a></div>
            </article>
          </a>
        </section>
      </article>
    `;
  }
}
