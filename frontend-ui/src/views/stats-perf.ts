import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { perfService } from '../services/perf.service.js';
import { catalogService } from '../services/catalog.service.js';
import type { LeaderboardKind } from '../services/leaderboard.service.js';
import { getCurrentPath } from '../lib/router.js';
import { shortAddress } from '../lib/format.js';
import type { TimeSeries } from '../components/ui/time-chart.js';
import '../components/ui/time-chart.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';
import '../components/ui/address-chip.js';

@customElement('view-stats-perf')
export class ViewStatsPerf extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, perfService.state$, perfService.state);
  @state() private catalog = new ObservableController(this, catalogService.state$, catalogService.state);
  @state() private orchInput = '';

  override connectedCallback(): void {
    super.connectedCallback();
    if (catalogService.state.regions.length === 0 && !catalogService.state.loading) {
      void catalogService.load();
    }
    const { query } = getCurrentPath();
    const orch = query.get('orch') ?? perfService.state.orchestrator;
    this.orchInput = orch;
    if (orch && perfService.state.orchestrator !== orch) {
      void perfService.refresh({ kind: 'transcoding', orchestrator: orch });
    }
  }

  private _setKind(kind: LeaderboardKind): void {
    const cur = perfService.state;
    if (cur.kind === kind || !cur.orchestrator) {
      const previousModel = catalogService.state.pipelines[0]?.models[0] ?? '';
      const previousPipeline = catalogService.state.pipelines[0]?.id ?? '';
      void perfService.refresh({
        kind,
        orchestrator: cur.orchestrator || this.orchInput,
        pipeline: previousPipeline,
        model: previousModel,
      });
      return;
    }
    void perfService.refresh({
      kind,
      orchestrator: cur.orchestrator,
      pipeline: cur.pipeline || catalogService.state.pipelines[0]?.id || '',
      model: cur.model || catalogService.state.pipelines[0]?.models[0] || '',
    });
  }

  private _onSubmit(e: Event): void {
    e.preventDefault();
    const orch = this.orchInput.trim();
    if (!orch) return;
    window.location.hash = `/performance/stats?orch=${encodeURIComponent(orch)}`;
    void perfService.refresh({ kind: perfService.state.kind, orchestrator: orch });
  }

  private _setPipeline(e: Event): void {
    const pipeline = (e.target as HTMLSelectElement).value;
    const def = catalogService.state.pipelines.find((p) => p.id === pipeline);
    void perfService.refresh({
      kind: 'ai',
      orchestrator: perfService.state.orchestrator,
      pipeline,
      model: def?.models[0] ?? '',
    });
  }

  private _setModel(e: Event): void {
    const model = (e.target as HTMLSelectElement).value;
    void perfService.refresh({
      kind: 'ai',
      orchestrator: perfService.state.orchestrator,
      pipeline: perfService.state.pipeline,
      model,
    });
  }

  private _seriesByMetric(metric: 'success_rate' | 'round_trip_time'): TimeSeries[] {
    const data = this.state.value?.data;
    if (!data) return [];
    return Object.entries(data)
      .filter(([, points]) => points.length > 0)
      .map(([region, points]) => ({
        name: region,
        data: points
          .slice()
          .sort((a, b) => a.timestamp - b.timestamp)
          .map((p): [string, number] => [new Date(p.timestamp * 1000).toISOString(), p[metric]]),
      }));
  }

  override render() {
    const s = this.state.value!;
    const c = this.catalog.value!;
    const aiPipeline = c.pipelines.find((p) => p.id === s.pipeline);
    return html`
      <article class="page">
        <header class="page-head">
          <h2>Performance stats</h2>
          <p class="lede">
            Per-orchestrator raw round-trip and success-rate samples, broken down by region.
            ${s.orchestrator
              ? html` Currently viewing
                  <address-chip address=${s.orchestrator} kind="orchestrator" .link=${false} explorer></address-chip>
                  (${shortAddress(s.orchestrator)}).`
              : ''}
          </p>
          <div class="controls">
            <form @submit=${this._onSubmit}>
              <input
                type="search"
                placeholder="0x… orchestrator address"
                .value=${this.orchInput}
                @input=${(e: Event) => (this.orchInput = (e.target as HTMLInputElement).value)}
              />
              <button class="btn btn--primary" type="submit">Load</button>
            </form>
            <div class="group" role="group" aria-label="Pipeline kind">
              <button type="button" aria-pressed=${s.kind === 'transcoding'} @click=${() => this._setKind('transcoding')}>Transcoding</button>
              <button type="button" aria-pressed=${s.kind === 'ai'} @click=${() => this._setKind('ai')}>AI</button>
            </div>
            ${s.kind === 'ai'
              ? html`
                  <label class="muted">
                    Pipeline
                    <select @change=${this._setPipeline} .value=${s.pipeline}>
                      ${c.pipelines.map(
                        (p) => html`<option value=${p.id} ?selected=${p.id === s.pipeline}>${p.id}</option>`,
                      )}
                    </select>
                  </label>
                  <label class="muted">
                    Model
                    <select @change=${this._setModel} .value=${s.model}>
                      ${(aiPipeline?.models ?? []).map(
                        (m) => html`<option value=${m} ?selected=${m === s.model}>${m}</option>`,
                      )}
                    </select>
                  </label>
                `
              : ''}
            <refresh-button
              ?loading=${s.loading}
              .lastUpdated=${s.lastUpdated}
              @refresh=${() =>
                perfService.refresh({
                  kind: s.kind,
                  orchestrator: s.orchestrator,
                  pipeline: s.pipeline,
                  model: s.model,
                })}
            ></refresh-button>
          </div>
        </header>

        ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
        ${!s.orchestrator
          ? html`<empty-state heading="Enter an orchestrator address" body="Performance stats are loaded per orchestrator."></empty-state>`
          : html`
              <section aria-labelledby="rt-h">
                <h3 id="rt-h" class="sr-only">Round-trip time chart</h3>
                <div class="charts">
                  <div class="card chart-card">
                    <h3>Round-trip time (s)</h3>
                    <time-chart .series=${this._seriesByMetric('round_trip_time')} y-format="number"></time-chart>
                  </div>
                  <div class="card chart-card">
                    <h3>Success rate</h3>
                    <time-chart .series=${this._seriesByMetric('success_rate')} y-format="number"></time-chart>
                  </div>
                </div>
              </section>
            `}
      </article>
    `;
  }
}
