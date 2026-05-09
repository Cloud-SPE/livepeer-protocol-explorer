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

@customElement('view-audio-to-text')
export class ViewAudioToText extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private model = '';
  @state() private file: File | null = null;
  @state() private loading = false;
  @state() private error = '';
  @state() private output: HistoryOutput | undefined = undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data) void networkCapabilitiesService.load();
  }

  override updated(): void {
    if (!this.model) {
      const opts = modelsForPipeline(this.capabilities.value?.data ?? null, 'Audio-to-text');
      if (opts[0]) this.model = opts[0].name;
    }
  }

  private async _submit(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.file) { this.error = 'Audio file required.'; return; }
    this.error = '';
    this.output = undefined;
    this.loading = true;
    try {
      const res = await aiGateway.audioToText({ audio: this.file, model_id: this.model });
      this.output = { kind: 'text', text: res.text };
      historyService.push({
        modality: 'audio-to-text',
        modelId: this.model,
        summary: res.text.slice(0, 140),
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
    if (e.detail.output?.kind === 'text') this.output = e.detail.output;
  }

  override render() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, 'Audio-to-text');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>Audio-to-text</h2>
          <p class="lede">Transcribe spoken audio.</p>
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
                <span>Audio file</span>
                <input type="file" accept="audio/*" required @change=${(e: Event) => (this.file = (e.target as HTMLInputElement).files?.[0] ?? null)} />
              </label>
              <div class="actions">
                <button class="btn btn--primary" type="submit" ?disabled=${this.loading || !this.model}>${this.loading ? 'Transcribing…' : 'Transcribe'}</button>
              </div>
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}
            </form>
            <history-list modality="audio-to-text" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuse(e)}></history-list>
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
