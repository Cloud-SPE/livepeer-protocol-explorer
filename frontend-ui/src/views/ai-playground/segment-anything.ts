import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import {
  modelsForPipeline,
  networkCapabilitiesService,
} from '../../services/network-capabilities.service.js';
import { historyService } from '../../services/history.service.js';
import { aiGateway } from '../../lib/sources/ai-gateway.js';
import type { HistoryEntry, HistoryOutput } from '../../types/playground.js';
import '../../components/ui/ai-result.js';
import '../../components/ui/history-list.js';
import '../../components/viewport-gate.js';

@customElement('view-segment-anything')
export class ViewSegmentAnything extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private model = '';
  @state() private file: File | null = null;
  @state() private points = '';
  @state() private labels = '';
  @state() private loading = false;
  @state() private error = '';
  @state() private output: HistoryOutput | undefined = undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data) void networkCapabilitiesService.load();
  }

  override updated(): void {
    if (!this.model) {
      const opts = modelsForPipeline(this.capabilities.value?.data ?? null, 'Segment-anything-2');
      if (opts[0]) this.model = opts[0].name;
    }
  }

  private async _submit(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.file) { this.error = 'Image required.'; return; }
    this.error = '';
    this.output = undefined;
    this.loading = true;
    try {
      const res = await aiGateway.segmentAnything2({
        image: this.file,
        model_id: this.model,
        ...(this.points ? { point_coords: this.points } : {}),
        ...(this.labels ? { point_labels: this.labels } : {}),
      });
      this.output = { kind: 'images', images: res.images ?? [] };
      historyService.push({
        modality: 'segment-anything-2',
        modelId: this.model,
        summary: `Segmented ${this.file.name}`,
        output: this.output,
      });
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  private _reuse(e: CustomEvent<HistoryEntry>): void {
    if (e.detail.modelId) this.model = e.detail.modelId;
  }

  override render() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, 'Segment-anything-2');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>Segment Anything 2</h2>
          <p class="lede">Generate segmentation masks. Optional prompt points/labels are JSON arrays.</p>
        </header>
        <viewport-gate min-width="800" reason="The mask preview is best viewed on tablet or desktop.">
          <div class="layout">
            <section class="card stack">
              <form @submit=${this._submit}>
                <label>
                  <span>Model</span>
                  <select required .value=${this.model} @change=${(e: Event) => (this.model = (e.target as HTMLSelectElement).value)}>
                    ${models.length === 0 ? html`<option value="" disabled selected>—</option>` : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.model}>${m.name}</option>`)}
                  </select>
                </label>
                <label>
                  <span>Image</span>
                  <input type="file" accept="image/*" required @change=${(e: Event) => (this.file = (e.target as HTMLInputElement).files?.[0] ?? null)} />
                </label>
                <label>
                  <span>Point coords JSON (optional)</span>
                  <input type="text" placeholder="[[100,200],[400,300]]" .value=${this.points} @input=${(e: Event) => (this.points = (e.target as HTMLInputElement).value)} />
                </label>
                <label>
                  <span>Point labels JSON (optional)</span>
                  <input type="text" placeholder="[1,0]" .value=${this.labels} @input=${(e: Event) => (this.labels = (e.target as HTMLInputElement).value)} />
                </label>
                <div class="actions">
                  <button class="btn btn--primary" type="submit" ?disabled=${this.loading || !this.model}>${this.loading ? 'Segmenting…' : 'Segment'}</button>
                </div>
                ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}
              </form>
              <history-list modality="segment-anything-2" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuse(e)}></history-list>
            </section>
            <section class="card out-card">
              <h3>Output</h3>
              <ai-result .output=${this.output}></ai-result>
            </section>
          </div>
        </viewport-gate>
      </article>
    `;
  }
}
