import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';
import { todayIso } from '../lib/format.js';

@customElement('view-reports-hub')
export class ViewReportsHub extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  override render() {
    const today = todayIso();
    return html`
      <article class="page">
        <header class="page-head">
          <h2>Reports</h2>
          <p class="lede">Network payout, reward, and ticket analytics.</p>
        </header>
        <section class="grid" aria-label="Report categories">
          <a class="tile" href="#/reports/daily/${today}">
            <h3>Daily payouts</h3>
            <p>Per-orchestrator commissions and ticket counts for a single day.</p>
          </a>
          <a class="tile" href="#/reports/weekly/${today}">
            <h3>Weekly payouts</h3>
            <p>Mon–Sun rolled-up payouts and contributor leaderboard.</p>
          </a>
          <a class="tile" href="#/reports/monthly/${today}">
            <h3>Monthly payouts</h3>
            <p>Calendar-month payouts and contributor leaderboard.</p>
          </a>
          <a class="tile" href="#/reports/top/payout">
            <h3>Top payouts</h3>
            <p>Cursor-paginated leaderboard over a chosen window.</p>
          </a>
          <a class="tile" href="#/reports/tickets/daily">
            <h3>Daily tickets</h3>
            <p>AI vs transcoding ticket counts over a date range.</p>
          </a>
          <a class="tile" href="#/rewards/leaderboard">
            <h3>Rewards leaderboard</h3>
            <p>Reward LPT distribution by orchestrator.</p>
          </a>
        </section>
      </article>
    `;
  }
}
