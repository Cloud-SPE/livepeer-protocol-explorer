import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { ticketsService } from '../services/tickets.service.js';
import { filtersService } from '../services/filters.service.js';
import { todayIso } from '../lib/format.js';
import type { JobType, TicketSeriesRow } from '../types/api.js';
import '../components/ui/refresh-button.js';
import '../components/ui/date-range.js';
import '../components/ui/job-type-toggle.js';
import '../components/ui/time-chart.js';
import type { TimeSeries } from '../components/ui/time-chart.js';

function thirtyDaysAgo(): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() - 30);
  return d.toISOString().slice(0, 10);
}

function rowsToSeries(rows: TicketSeriesRow[]): Array<[string, number]> {
  return rows.map((r) => [r.date, Number(r.count)]);
}

@customElement('view-tickets-timeseries')
export class ViewTicketsTimeseries extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, ticketsService.state$, ticketsService.state);

  override connectedCallback(): void {
    super.connectedCallback();
    const s = ticketsService.state;
    if (!s.data && !s.loading) {
      const f = filtersService.value;
      void ticketsService.refresh({
        start: f.rangeFrom || thirtyDaysAgo(),
        end: f.rangeTo || todayIso(),
        jobType: f.jobType,
      });
    }
  }

  private _refresh(): void {
    const f = filtersService.value;
    void ticketsService.refresh({
      start: f.rangeFrom || thirtyDaysAgo(),
      end: f.rangeTo || todayIso(),
      jobType: f.jobType,
    });
  }

  private _onRange(e: CustomEvent<{ from: string; to: string }>): void {
    filtersService.patch({ rangeFrom: e.detail.from, rangeTo: e.detail.to });
    this._refresh();
  }

  private _onJobType(e: CustomEvent<JobType>): void {
    filtersService.patch({ jobType: e.detail });
    this._refresh();
  }

  override render() {
    const s = this.state.value!;
    const f = filtersService.value;
    const series: TimeSeries[] = [];
    if (s.data) {
      if (f.jobType !== 'ai' && s.data.transcoding.length) {
        series.push({ name: 'Transcoding', data: rowsToSeries(s.data.transcoding), area: true });
      }
      if (f.jobType !== 'transcoding' && s.data.ai.length) {
        series.push({ name: 'AI', data: rowsToSeries(s.data.ai), area: true });
      }
    }
    return html`
      <article class="page">
        <header class="page-head">
          <h2>Daily tickets</h2>
          <p class="lede">Ticket redemption counts per day, AI vs transcoding.</p>
          <div class="controls">
            <date-range
              .from=${f.rangeFrom || thirtyDaysAgo()}
              .to=${f.rangeTo || todayIso()}
              @change-range=${(e: CustomEvent<{ from: string; to: string }>) => this._onRange(e)}
            ></date-range>
            <job-type-toggle
              .value=${f.jobType}
              @change-job-type=${(e: CustomEvent<JobType>) => this._onJobType(e)}
            ></job-type-toggle>
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() => this._refresh()}
            ></refresh-button>
          </div>
        </header>
        <section aria-labelledby="tckt-h">
          <h3 id="tckt-h" class="sr-only">Tickets time-series chart</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <article class="card chart-card">
            <time-chart .series=${series} y-format="count" chart-heading="Daily tickets"></time-chart>
          </article>
        </section>
      </article>
    `;
  }
}
