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

@customElement('view-text-to-image')
export class ViewTextToImage extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private model = '';
  @state() private prompt = '';
  @state() private negative = '';
  @state() private width = 1024;
  @state() private height = 1024;
  @state() private count = 1;
  @state() private steps = 30;
  @state() private guidance = 7.5;
  @state() private safety = true;
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
      const opts = modelsForPipeline(this.capabilities.value?.data ?? null, 'Text-to-image');
      if (opts[0]) this.model = opts[0].name;
    }
  }

  private async _submit(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.prompt.trim()) { this.error = 'Prompt required.'; return; }
    this.error = '';
    this.output = undefined;
    this.loading = true;
    try {
      const seedNum = this.seed ? Number(this.seed) : undefined;
      const payload = {
        prompt: this.prompt,
        model_id: this.model,
        width: this.width,
        height: this.height,
        num_images_per_prompt: this.count,
        num_inference_steps: this.steps,
        guidance_scale: this.guidance,
        safety_check: this.safety,
        ...(this.negative ? { negative_prompt: this.negative } : {}),
        ...(seedNum !== undefined && Number.isFinite(seedNum) ? { seed: seedNum } : {}),
      };
      const res = await aiGateway.textToImage(payload);
      this.output = { kind: 'images', images: res.images ?? [] };
      historyService.push({
        modality: 'text-to-image',
        modelId: this.model,
        prompt: this.prompt,
        summary: `${res.images?.length ?? 0} image(s) — ${this.prompt.slice(0, 100)}`,
        output: this.output,
      });
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  private _reuse(e: CustomEvent<HistoryEntry>): void {
    if (e.detail.prompt) this.prompt = e.detail.prompt;
    if (e.detail.modelId) this.model = e.detail.modelId;
    if (e.detail.output?.kind === 'images') this.output = e.detail.output;
  }

  override render() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, 'Text-to-image');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>Text-to-image</h2>
          <p class="lede">Generate images from a text prompt.</p>
        </header>
        <div class="layout">
          <section aria-labelledby="ti-form-h" class="card stack">
            <h3 id="ti-form-h" class="sr-only">Inputs</h3>
            <form @submit=${this._submit}>
              <label>
                <span>Model</span>
                <select required .value=${this.model} @change=${(e: Event) => (this.model = (e.target as HTMLSelectElement).value)}>
                  ${models.length === 0 ? html`<option value="" disabled selected>—</option>` : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.model}>${m.name}</option>`)}
                </select>
              </label>
              <label>
                <span>Prompt</span>
                <textarea rows="4" required .value=${this.prompt} @input=${(e: Event) => (this.prompt = (e.target as HTMLTextAreaElement).value)}></textarea>
              </label>
              <label>
                <span>Negative prompt</span>
                <textarea rows="2" .value=${this.negative} @input=${(e: Event) => (this.negative = (e.target as HTMLTextAreaElement).value)}></textarea>
              </label>
              <div class="field-row">
                <label><span>Width</span><input type="number" min="1" max="2048" .value=${String(this.width)} @input=${(e: Event) => (this.width = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Height</span><input type="number" min="1" max="2048" .value=${String(this.height)} @input=${(e: Event) => (this.height = Number((e.target as HTMLInputElement).value))} /></label>
              </div>
              <div class="field-row">
                <label><span>Images</span><input type="number" min="1" max="10" .value=${String(this.count)} @input=${(e: Event) => (this.count = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Steps</span><input type="number" min="1" max="200" .value=${String(this.steps)} @input=${(e: Event) => (this.steps = Number((e.target as HTMLInputElement).value))} /></label>
              </div>
              <div class="field-row">
                <label><span>Guidance scale</span><input type="number" min="0" max="50" step="0.1" .value=${String(this.guidance)} @input=${(e: Event) => (this.guidance = Number((e.target as HTMLInputElement).value))} /></label>
                <label><span>Seed (optional)</span><input type="number" .value=${this.seed} @input=${(e: Event) => (this.seed = (e.target as HTMLInputElement).value)} /></label>
              </div>
              <label class="toggle"><input type="checkbox" .checked=${this.safety} @change=${(e: Event) => (this.safety = (e.target as HTMLInputElement).checked)} /> Safety check</label>
              <div class="actions">
                <button class="btn btn--primary" type="submit" ?disabled=${this.loading || !this.model}>${this.loading ? 'Generating…' : 'Generate'}</button>
              </div>
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}
            </form>
            <history-list modality="text-to-image" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuse(e)}></history-list>
          </section>
          <section aria-labelledby="ti-out-h" class="card out-card">
            <h3 id="ti-out-h">Output</h3>
            <ai-result .output=${this.output}></ai-result>
          </section>
        </div>
      </article>
    `;
  }
}
