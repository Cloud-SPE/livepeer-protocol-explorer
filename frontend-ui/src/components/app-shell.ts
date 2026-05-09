import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { until } from 'lit/directives/until.js';
import { defineRoutes, startRouter, withViewTransition, type RouteMatch } from '../lib/router.js';
import { todayIso } from '../lib/format.js';
import './side-nav.js';
import './theme-switcher.js';
import './ui/empty-state.js';

// Lightweight views — statically imported (no heavy deps).
import '../views/dashboard.js';
import '../views/orchestrators-list.js';
import '../views/delegators-list.js';
import '../views/delegator-detail.js';
import '../views/gateways-list.js';
import '../views/gateway-detail.js';
import '../views/governance-proposals.js';
import '../views/governance-proposal-detail.js';
import '../views/governance-votes.js';
import '../views/reports-hub.js';
import '../views/payouts-summary.js';
import '../views/payouts-leaderboard.js';
import '../views/rewards-leaderboard.js';
import '../views/leaderboard-perf.js';
import '../views/network-capabilities.js';
import '../views/rounds-list.js';
import '../views/ai-settings.js';
import '../views/ai-playground/ai-generator.js';
import '../views/ai-playground/llm.js';
import '../views/ai-playground/text-to-image.js';
import '../views/ai-playground/image-to-image.js';
import '../views/ai-playground/image-to-video.js';
import '../views/ai-playground/image-to-text.js';
import '../views/ai-playground/audio-to-text.js';
import '../views/ai-playground/text-to-speech.js';
import '../views/ai-playground/upscale.js';
import '../views/ai-playground/segment-anything.js';

// Heavy views — lazy-loaded so ECharts and the OpenAI SDK only fetch on demand.
const lazyOrchestratorDetail = () => import('../views/orchestrator-detail.js');
const lazyRoundDetail = () => import('../views/round-detail.js');
const lazyTicketsTimeseries = () => import('../views/tickets-timeseries.js');
const lazyStatsPerf = () => import('../views/stats-perf.js');
const lazyByocOpenai = () => import('../views/ai-playground/byoc-openai.js');

const lazyLoading = html`<empty-state heading="Loading view…" body="Fetching the chart bundle."></empty-state>`;

interface ViewState {
  match: RouteMatch | null;
}

@customElement('app-shell')
export class AppShell extends LitElement {

  @state() private viewState: ViewState = { match: null };
  private stopRouter: (() => void) | null = null;

  override createRenderRoot(): HTMLElement {
    return this;
  }

  override connectedCallback(): void {
    super.connectedCallback();
    this._registerRoutes();
    this.stopRouter = startRouter((m) => {
      withViewTransition(() => {
        this.viewState = { match: m };
        this.requestUpdate();
        const main = this.querySelector('main');
        main?.focus({ preventScroll: false });
      });
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopRouter?.();
  }

  private _registerRoutes(): void {
    defineRoutes([
      { pattern: '/' },
      { pattern: '/orchestrators' },
      { pattern: '/orchestrators/:address' },
      { pattern: '/delegators' },
      { pattern: '/delegators/:address' },
      { pattern: '/gateways' },
      { pattern: '/gateways/:address' },
      { pattern: '/rounds' },
      { pattern: '/rounds/:round_id' },
      { pattern: '/broadcasters', redirect: () => '/gateways' },
      { pattern: '/broadcasters/:address', redirect: (p) => `/gateways/${p['address']}` },
      { pattern: '/reports' },
      { pattern: '/reports/daily', redirect: () => `/reports/daily/${todayIso()}` },
      { pattern: '/reports/weekly', redirect: () => `/reports/weekly/${todayIso()}` },
      { pattern: '/reports/monthly', redirect: () => `/reports/monthly/${todayIso()}` },
      { pattern: '/reports/daily/:date' },
      { pattern: '/reports/weekly/:date' },
      { pattern: '/reports/monthly/:date' },
      { pattern: '/reports/top/payout' },
      { pattern: '/reports/tickets/daily' },
      { pattern: '/rewards/leaderboard' },
      { pattern: '/vote/history', redirect: () => '/governance/proposals' },
      { pattern: '/governance/proposals' },
      { pattern: '/governance/proposals/:id' },
      { pattern: '/governance/votes' },
      { pattern: '/performance/leaderboard' },
      { pattern: '/performance/stats' },
      { pattern: '/ai/generator' },
      { pattern: '/ai/llm' },
      { pattern: '/ai/text-to-image' },
      { pattern: '/ai/image-to-image' },
      { pattern: '/ai/image-to-video' },
      { pattern: '/ai/image-to-text' },
      { pattern: '/ai/upscale' },
      { pattern: '/ai/audio-to-text' },
      { pattern: '/ai/text-to-speech' },
      { pattern: '/ai/segment-anything-2' },
      { pattern: '/ai/network-capabilities' },
      { pattern: '/ai/settings' },
      { pattern: '/ai/byoc/openai' },
    ]);
  }

  private _renderView(): unknown {
    const m = this.viewState.match;
    if (!m) {
      return html`
        <article class="page">
          <header class="page-head">
            <div>
              <h2>404 — page not found</h2>
              <p class="lede">No view matches <code>${window.location.hash || '#/'}</code>.</p>
            </div>
          </header>
          <p>
            <a class="btn btn--primary" href="#/">Back to dashboard</a>
            <a class="btn" href="#/orchestrators">Browse orchestrators</a>
            <a class="btn" href="#/reports">Browse reports</a>
          </p>
        </article>
      `;
    }
    switch (m.pattern) {
      case '/':
        return html`<view-dashboard></view-dashboard>`;
      case '/orchestrators':
        return html`<view-orchestrators-list></view-orchestrators-list>`;
      case '/orchestrators/:address':
        return until(
          lazyOrchestratorDetail().then(
            () => html`<view-orchestrator-detail .address=${m.params['address'] ?? ''}></view-orchestrator-detail>`,
          ),
          lazyLoading,
        );
      case '/delegators':
        return html`<view-delegators-list></view-delegators-list>`;
      case '/delegators/:address':
        return html`<view-delegator-detail .address=${m.params['address'] ?? ''}></view-delegator-detail>`;
      case '/gateways':
        return html`<view-gateways-list></view-gateways-list>`;
      case '/gateways/:address':
        return html`<view-gateway-detail .address=${m.params['address'] ?? ''}></view-gateway-detail>`;
      case '/rounds':
        return html`<view-rounds-list></view-rounds-list>`;
      case '/rounds/:round_id':
        return until(
          lazyRoundDetail().then(
            () => html`<view-round-detail .roundId=${m.params['round_id'] ?? ''}></view-round-detail>`,
          ),
          lazyLoading,
        );
      case '/governance/proposals':
        return html`<view-governance-proposals></view-governance-proposals>`;
      case '/governance/proposals/:id':
        return html`<view-governance-proposal-detail .id=${m.params['id'] ?? ''}></view-governance-proposal-detail>`;
      case '/governance/votes':
        return html`<view-governance-votes></view-governance-votes>`;
      case '/reports':
        return html`<view-reports-hub></view-reports-hub>`;
      case '/reports/daily/:date':
        return html`<view-payouts-summary period="daily" .date=${m.params['date'] ?? ''}></view-payouts-summary>`;
      case '/reports/weekly/:date':
        return html`<view-payouts-summary period="weekly" .date=${m.params['date'] ?? ''}></view-payouts-summary>`;
      case '/reports/monthly/:date':
        return html`<view-payouts-summary period="monthly" .date=${m.params['date'] ?? ''}></view-payouts-summary>`;
      case '/reports/top/payout':
        return html`<view-payouts-leaderboard></view-payouts-leaderboard>`;
      case '/reports/tickets/daily':
        return until(
          lazyTicketsTimeseries().then(() => html`<view-tickets-timeseries></view-tickets-timeseries>`),
          lazyLoading,
        );
      case '/rewards/leaderboard':
        return html`<view-rewards-leaderboard></view-rewards-leaderboard>`;
      case '/performance/leaderboard':
        return html`<view-leaderboard-perf></view-leaderboard-perf>`;
      case '/performance/stats':
        return until(
          lazyStatsPerf().then(() => html`<view-stats-perf></view-stats-perf>`),
          lazyLoading,
        );
      case '/ai/generator':
        return html`<view-ai-generator></view-ai-generator>`;
      case '/ai/llm':
        return html`<view-llm></view-llm>`;
      case '/ai/text-to-image':
        return html`<view-text-to-image></view-text-to-image>`;
      case '/ai/image-to-image':
        return html`<view-image-to-image></view-image-to-image>`;
      case '/ai/image-to-video':
        return html`<view-image-to-video></view-image-to-video>`;
      case '/ai/image-to-text':
        return html`<view-image-to-text></view-image-to-text>`;
      case '/ai/upscale':
        return html`<view-upscale></view-upscale>`;
      case '/ai/audio-to-text':
        return html`<view-audio-to-text></view-audio-to-text>`;
      case '/ai/text-to-speech':
        return html`<view-text-to-speech></view-text-to-speech>`;
      case '/ai/segment-anything-2':
        return html`<view-segment-anything></view-segment-anything>`;
      case '/ai/network-capabilities':
        return html`<view-network-capabilities></view-network-capabilities>`;
      case '/ai/settings':
        return html`<view-ai-settings></view-ai-settings>`;
      case '/ai/byoc/openai':
        return until(
          lazyByocOpenai().then(() => html`<view-byoc-openai></view-byoc-openai>`),
          lazyLoading,
        );
      default:
        // Reaching this means a pattern is registered in `defineRoutes` but
        // the switch above forgot to map it to a view component. That's a
        // developer error — show enough detail to fix it quickly.
        console.warn(`No view bound for route pattern ${m.pattern}`);
        return html`
          <empty-state
            heading="View not bound"
            .body=${`The router matched pattern ${m.pattern} but no component is wired for it. This is a frontend bug.`}
          >
          </empty-state>
        `;
    }
  }

  override render() {
    return html`
      <header class="app-bar" role="banner">
        <a href="#/" class="brand">Livepeer Tools</a>
        <div class="spacer"></div>
        <theme-switcher></theme-switcher>
      </header>
      <div class="body">
        <aside aria-label="Sections">
          <side-nav></side-nav>
        </aside>
        <main id="main" tabindex="-1">${this._renderView()}</main>
      </div>
      <footer class="footer">Livepeer Tools · backed by livepeer-api</footer>
    `;
  }
}
