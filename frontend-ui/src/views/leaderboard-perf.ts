import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { catalogService } from '../services/catalog.service.js';
import { leaderboardService, summarize, type LeaderboardKind } from '../services/leaderboard.service.js';
import type { ColumnDef } from '../components/ui/data-table.js';
import '../components/ui/data-table.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';

const COLS: ColumnDef[] = [
  { key: 'orchestrator', label: 'Orchestrator', cell: 'address' },
  { key: 'avg_score', label: 'Avg score', cell: 'number', decimals: 3, align: 'end' },
  { key: 'avg_success_rate', label: 'Success rate', cell: 'number', decimals: 3, align: 'end' },
  { key: 'avg_round_trip', label: 'Round-trip score', cell: 'number', decimals: 3, align: 'end' },
  { key: 'region_count', label: 'Regions', cell: 'number', align: 'end' },
];

@customElement('view-leaderboard-perf')
export class ViewLeaderboardPerf extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, leaderboardService.state$, leaderboardService.state);
  @state() private catalog = new ObservableController(this, catalogService.state$, catalogService.state);

  override connectedCallback(): void {
    super.connectedCallback();
    if (catalogService.state.regions.length === 0 && !catalogService.state.loading) {
      void catalogService.load();
    }
    if (!leaderboardService.state.data && !leaderboardService.state.loading) {
      void leaderboardService.refresh({ kind: 'transcoding', region: 'GLOBAL' });
    }
  }

  private _setKind(kind: LeaderboardKind): void {
    const cur = leaderboardService.state;
    if (cur.kind === kind) return;
    if (kind === 'ai') {
      const firstPipeline = catalogService.state.pipelines[0];
      const firstModel = firstPipeline?.models[0];
      void leaderboardService.refresh({
        kind,
        region: 'GLOBAL',
        pipeline: firstPipeline?.id ?? '',
        model: firstModel ?? '',
      });
    } else {
      void leaderboardService.refresh({ kind, region: 'GLOBAL' });
    }
  }

  private _setRegion(e: Event): void {
    const region = (e.target as HTMLSelectElement).value;
    const cur = leaderboardService.state;
    void leaderboardService.refresh({
      kind: cur.kind,
      region,
      pipeline: cur.pipeline,
      model: cur.model,
    });
  }

  private _setPipeline(e: Event): void {
    const pipeline = (e.target as HTMLSelectElement).value;
    const def = catalogService.state.pipelines.find((p) => p.id === pipeline);
    void leaderboardService.refresh({
      kind: 'ai',
      region: leaderboardService.state.region,
      pipeline,
      model: def?.models[0] ?? '',
    });
  }

  private _setModel(e: Event): void {
    const model = (e.target as HTMLSelectElement).value;
    const cur = leaderboardService.state;
    void leaderboardService.refresh({
      kind: 'ai',
      region: cur.region,
      pipeline: cur.pipeline,
      model,
    });
  }

  override render() {
    const s = this.state.value!;
    const c = this.catalog.value!;
    const rows = summarize(s.data);
    const aiPipeline = c.pipelines.find((p) => p.id === s.pipeline);
    const regions =
      s.kind === 'ai' && aiPipeline?.regions?.length
        ? c.regions.filter((r) => aiPipeline.regions.includes(r.id))
        : c.regions;

    return html`
      <article class="page">
        <header class="page-head">
          <h2>Performance leaderboard</h2>
          <p class="lede">Aggregated success rate and round-trip score by orchestrator.</p>
          <div class="controls">
            <div class="group" role="group" aria-label="Pipeline kind">
              <button type="button" aria-pressed=${s.kind === 'transcoding'} @click=${() => this._setKind('transcoding')}>Transcoding</button>
              <button type="button" aria-pressed=${s.kind === 'ai'} @click=${() => this._setKind('ai')}>AI</button>
            </div>
            <label class="muted">
              Region
              <select @change=${this._setRegion} .value=${s.region}>
                <option value="GLOBAL" ?selected=${s.region === 'GLOBAL'}>Global</option>
                ${regions.map(
                  (r) => html`<option value=${r.id} ?selected=${r.id === s.region}>${r.name} (${r.id})</option>`,
                )}
              </select>
            </label>
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
                leaderboardService.refresh({
                  kind: s.kind,
                  region: s.region,
                  pipeline: s.pipeline,
                  model: s.model,
                })}
            ></refresh-button>
          </div>
        </header>
        <section aria-labelledby="lb-perf-h">
          <h3 id="lb-perf-h" class="sr-only">Leaderboard</h3>
          ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          <data-table
            caption="${rows.length} orchestrators · region ${s.region}${s.kind === 'ai' ? ` · pipeline ${s.pipeline} · model ${s.model}` : ''}"
            .columns=${COLS}
            .rows=${rows as unknown as Record<string, unknown>[]}
            href-template="#/orchestrators/{orchestrator}"
            empty-text="No leaderboard data for this filter"
          ></data-table>
        </section>
      </article>
    `;
  }
}
