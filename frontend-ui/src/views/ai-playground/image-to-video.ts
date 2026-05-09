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

@customElement('view-image-to-video')
export class ViewImageToVideo extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private model = '';
  @state() private file: File | null = null;
  @state() private width = 1024;
  @state() private height = 576;
  @state() private fps = 6;
  @state() private motion = 127;
  @state() private noise = 0.02;
  @state() private seed = '';
  @state() private loading = false;
  @state() private error = '';
  @state() private output: HistoryOutput | undefined = undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data) void networkCapabilitiesService.load();
  }

  override updated(): void {
    if (!this.model) {
      const opts = modelsForPipeline(this.capabilities.value?.data ?? null, 'Image-to-video');
      if (opts[0]) this.model = opts[0].name;
    }
  }

  private async _submit(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.file) { this.error = 'Image file required.'; return; }
    this.error = '';
    this.output = undefined;
    this.loading = true;
    try {
      const seedNum = this.seed ? Number(this.seed) : undefined;
      const res = await aiGateway.imageToVideo({
        image: this.file,
        model_id: this.model,
        width: this.width,
        height: this.height,
        fps: this.fps,
        motion_bucket_id: this.motion,
        noise_aug_strength: this.noise,
        ...(seedNum !== undefined && Number.isFinite(seedNum) ? { seed: seedNum } : {}),
      });
      this.output = { kind: 'images', images: res.images ?? [] };
      historyService.push({
        modality: 'image-to-video',
        modelId: this.model,
        summary: `Video from ${this.file.name}`,
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
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, 'Image-to-video');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>Image-to-video</h2>
          <p class="lede">Animate a still image into a short video.</p>
        </header>
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
                <span>Source image</span>
                <input type="file" accept="image/*" required @change=${(e: Event) => (this.file = (e.target as HTMLInputElement).files?.[0] ?? null)} />
              </label>
              <div class="field-row">
                <label><span>Width</span><input type="number" min="1" max="2048" .value=${String(this.width)} @input=${(e: Event) => (this.width = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Height</span><input type="number" min="1" max="2048" .value=${String(this.height)} @input=${(e: Event) => (this.height = Number((e.target as HTMLInputElement).value))} /></label>
              </div>
              <div class="field-row">
                <label><span>FPS</span><input type="number" min="1" max="30" .value=${String(this.fps)} @input=${(e: Event) => (this.fps = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Motion bucket</span><input type="number" min="1" max="255" .value=${String(this.motion)} @input=${(e: Event) => (this.motion = Number((e.target as HTMLInputElement).value))} /></label>
              </div>
              <div class="field-row">
                <label><span>Noise aug strength</span><input type="number" min="0" max="1" step="0.01" .value=${String(this.noise)} @input=${(e: Event) => (this.noise = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Seed (optional)</span><input type="number" .value=${this.seed} @input=${(e: Event) => (this.seed = (e.target as HTMLInputElement).value)} /></label>
              </div>
              <div class="actions">
                <button class="btn btn--primary" type="submit" ?disabled=${this.loading || !this.model}>${this.loading ? 'Rendering…' : 'Generate'}</button>
              </div>
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}
            </form>
            <history-list modality="image-to-video" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuse(e)}></history-list>
          </section>
          <section class="card out-card">
            <h3>Output</h3>
            <ai-result .output=${this.output}></ai-result>
          </section>
        </div>
      </article>
    `;
  }
}
