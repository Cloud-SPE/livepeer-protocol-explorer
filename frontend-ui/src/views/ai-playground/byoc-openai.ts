import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import {
  modelsForPipeline,
  networkCapabilitiesService,
} from '../../services/network-capabilities.service.js';
import { byocService } from '../../services/byoc.service.js';
import { historyService } from '../../services/history.service.js';
import type { HistoryEntry } from '../../types/playground.js';
import '../../components/ui/refresh-button.js';
import '../../components/ui/markdown-view.js';
import '../../components/ui/history-list.js';

type Tab = 'chat' | 'images' | 'embeddings';

const PIPELINE_FOR: Record<Tab, string> = {
  chat: 'openai-chat-completions',
  images: 'openai-image-generation',
  embeddings: 'openai-text-embeddings',
};

@customElement('view-byoc-openai')
export class ViewByocOpenai extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private chatState = new ObservableController(this, byocService.chat$, byocService.chat);
  @state() private imagesState = new ObservableController(this, byocService.images$, byocService.images);
  @state() private embeddingsState = new ObservableController(this, byocService.embeddings$, byocService.embeddings);

  @state() private tab: Tab = 'chat';
  @state() private chatModel = '';
  @state() private chatSystem = 'You are a helpful assistant.';
  @state() private chatPrompt = '';
  @state() private chatTemp = 0.7;
  @state() private chatMaxTokens = 512;
  @state() private chatStream = true;

  @state() private imageModel = '';
  @state() private imagePrompt = '';
  @state() private imageSize: '1024x1024' | '1024x1792' | '1792x1024' = '1024x1024';
  @state() private imageN = 1;

  @state() private embeddingModel = '';
  @state() private embeddingInput = '';

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data && !networkCapabilitiesService.state.loading) {
      void networkCapabilitiesService.load();
    }
  }

  override updated(): void {
    const caps = this.capabilities.value?.data ?? null;
    if (!this.chatModel) {
      const m = modelsForPipeline(caps, PIPELINE_FOR.chat)[0];
      if (m) this.chatModel = m.name;
    }
    if (!this.imageModel) {
      const m = modelsForPipeline(caps, PIPELINE_FOR.images)[0];
      if (m) this.imageModel = m.name;
    }
    if (!this.embeddingModel) {
      const m = modelsForPipeline(caps, PIPELINE_FOR.embeddings)[0];
      if (m) this.embeddingModel = m.name;
    }
  }

  private async _submitChat(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.chatPrompt.trim()) return;
    await byocService.runChat(
      {
        model: this.chatModel,
        messages: [
          { role: 'system', content: this.chatSystem },
          { role: 'user', content: this.chatPrompt },
        ],
        max_tokens: this.chatMaxTokens,
        temperature: this.chatTemp,
      },
      { stream: this.chatStream },
    );
    const out = byocService.chat;
    if (!out.error) {
      historyService.push({
        modality: 'llm',
        modelId: `byoc:${this.chatModel}`,
        prompt: this.chatPrompt,
        summary: out.output.slice(0, 140) || '(empty response)',
        output: { kind: 'text', text: out.output },
      });
    }
  }

  private async _submitImages(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.imagePrompt.trim()) return;
    await byocService.runImages({
      model: this.imageModel,
      prompt: this.imagePrompt,
      size: this.imageSize,
      n: this.imageN,
    });
    const out = byocService.images;
    if (!out.error) {
      historyService.push({
        modality: 'text-to-image',
        modelId: `byoc:${this.imageModel}`,
        prompt: this.imagePrompt,
        summary: `${out.images.length} image(s) — ${this.imagePrompt.slice(0, 100)}`,
        output: { kind: 'images', images: out.images.map((url) => ({ url })) },
      });
    }
  }

  private async _submitEmbeddings(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.embeddingInput.trim()) return;
    await byocService.runEmbeddings({ model: this.embeddingModel, input: this.embeddingInput });
  }

  private _downloadEmbedding(): void {
    const e = byocService.embeddings.embedding;
    if (!e) return;
    const blob = new Blob([JSON.stringify({ embedding: e }, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `embedding-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  private _reuseChat(e: CustomEvent<HistoryEntry>): void {
    if (e.detail.prompt) this.chatPrompt = e.detail.prompt;
  }

  override render() {
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>BYOC OpenAI</h2>
          <p class="lede">Hits an OpenAI-compatible BYOC gateway via the official SDK.</p>
        </header>
        <div class="tabs" role="tablist">
          ${(['chat', 'images', 'embeddings'] as Tab[]).map(
            (t) => html`
              <button
                role="tab"
                aria-selected=${this.tab === t ? 'true' : 'false'}
                @click=${() => (this.tab = t)}
              >
                ${t === 'chat' ? 'Chat' : t === 'images' ? 'Images' : 'Embeddings'}
              </button>
            `,
          )}
        </div>
        ${this.tab === 'chat' ? this._renderChat() : ''}
        ${this.tab === 'images' ? this._renderImages() : ''}
        ${this.tab === 'embeddings' ? this._renderEmbeddings() : ''}
      </article>
    `;
  }

  private _renderChat() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, PIPELINE_FOR.chat);
    const s = this.chatState.value!;
    return html`
      <div class="layout">
        <section class="card stack">
          <form @submit=${this._submitChat}>
            <label>
              <span>Model</span>
              <select required .value=${this.chatModel} @change=${(e: Event) => (this.chatModel = (e.target as HTMLSelectElement).value)}>
                ${models.length === 0
                  ? html`<option value="" disabled selected>—</option>`
                  : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.chatModel}>${m.name}</option>`)}
              </select>
            </label>
            <label>
              <span>System</span>
              <textarea rows="2" .value=${this.chatSystem} @input=${(e: Event) => (this.chatSystem = (e.target as HTMLTextAreaElement).value)}></textarea>
            </label>
            <label>
              <span>Prompt</span>
              <textarea rows="6" required .value=${this.chatPrompt} @input=${(e: Event) => (this.chatPrompt = (e.target as HTMLTextAreaElement).value)}></textarea>
            </label>
            <div class="field-row">
              <label><span>Max tokens</span><input type="number" min="1" max="32000" .value=${String(this.chatMaxTokens)} @input=${(e: Event) => (this.chatMaxTokens = Number((e.target as HTMLInputElement).value))} /></label>
              <label><span>Temperature</span><input type="number" min="0" max="2" step="0.1" .value=${String(this.chatTemp)} @input=${(e: Event) => (this.chatTemp = Number((e.target as HTMLInputElement).value))} /></label>
            </div>
            <label class="toggle"><input type="checkbox" .checked=${this.chatStream} @change=${(e: Event) => (this.chatStream = (e.target as HTMLInputElement).checked)} /> Stream response</label>
            <div class="actions">
              <button class="btn btn--primary" type="submit" ?disabled=${s.loading || !this.chatModel}>${s.loading ? 'Generating…' : 'Generate'}</button>
            </div>
            ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          </form>
          <history-list modality="llm" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuseChat(e)}></history-list>
        </section>
        <section class="card out-card">
          <h3>Output</h3>
          ${s.output
            ? html`<div class="text-block"><markdown-view .source=${s.output}></markdown-view></div>`
            : html`<p class="muted">No output yet.</p>`}
          ${s.reasoning ? html`<div class="reasoning">${s.reasoning}</div>` : ''}
        </section>
      </div>
    `;
  }

  private _renderImages() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, PIPELINE_FOR.images);
    const s = this.imagesState.value!;
    return html`
      <div class="layout">
        <section class="card stack">
          <form @submit=${this._submitImages}>
            <label>
              <span>Model</span>
              <select required .value=${this.imageModel} @change=${(e: Event) => (this.imageModel = (e.target as HTMLSelectElement).value)}>
                ${models.length === 0
                  ? html`<option value="" disabled selected>—</option>`
                  : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.imageModel}>${m.name}</option>`)}
              </select>
            </label>
            <label>
              <span>Prompt</span>
              <textarea rows="4" required .value=${this.imagePrompt} @input=${(e: Event) => (this.imagePrompt = (e.target as HTMLTextAreaElement).value)}></textarea>
            </label>
            <div class="field-row">
              <label>
                <span>Size</span>
                <select .value=${this.imageSize} @change=${(e: Event) => (this.imageSize = (e.target as HTMLSelectElement).value as '1024x1024' | '1024x1792' | '1792x1024')}>
                  <option value="1024x1024" ?selected=${this.imageSize === '1024x1024'}>1024 × 1024</option>
                  <option value="1024x1792" ?selected=${this.imageSize === '1024x1792'}>1024 × 1792</option>
                  <option value="1792x1024" ?selected=${this.imageSize === '1792x1024'}>1792 × 1024</option>
                </select>
              </label>
              <label><span>Count</span><input type="number" min="1" max="4" .value=${String(this.imageN)} @input=${(e: Event) => (this.imageN = Number((e.target as HTMLInputElement).value))} /></label>
            </div>
            <div class="actions">
              <button class="btn btn--primary" type="submit" ?disabled=${s.loading || !this.imageModel}>${s.loading ? 'Generating…' : 'Generate'}</button>
            </div>
            ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          </form>
        </section>
        <section class="card out-card">
          <h3>Output</h3>
          ${s.images.length === 0
            ? html`<p class="muted">No images yet.</p>`
            : html`
                <div class="images-grid">
                  ${s.images.map(
                    (url) => html`
                      <figure>
                        <img src=${url} alt="" loading="lazy" />
                      </figure>
                    `,
                  )}
                </div>
              `}
        </section>
      </div>
    `;
  }

  private _renderEmbeddings() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, PIPELINE_FOR.embeddings);
    const s = this.embeddingsState.value!;
    return html`
      <div class="layout">
        <section class="card stack">
          <form @submit=${this._submitEmbeddings}>
            <label>
              <span>Model</span>
              <select required .value=${this.embeddingModel} @change=${(e: Event) => (this.embeddingModel = (e.target as HTMLSelectElement).value)}>
                ${models.length === 0
                  ? html`<option value="" disabled selected>—</option>`
                  : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.embeddingModel}>${m.name}</option>`)}
              </select>
            </label>
            <label>
              <span>Input text</span>
              <textarea rows="6" required .value=${this.embeddingInput} @input=${(e: Event) => (this.embeddingInput = (e.target as HTMLTextAreaElement).value)}></textarea>
            </label>
            <div class="actions">
              <button class="btn btn--primary" type="submit" ?disabled=${s.loading || !this.embeddingModel}>${s.loading ? 'Computing…' : 'Compute'}</button>
            </div>
            ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
          </form>
        </section>
        <section class="card out-card">
          <h3>Output</h3>
          ${s.embedding
            ? html`
                <p class="muted">Vector dimensions: <strong class="mono">${s.dims}</strong></p>
                <p class="emb-preview">[${s.embedding.slice(0, 12).map((v) => v.toFixed(5)).join(', ')}${s.embedding.length > 12 ? ', …' : ''}]</p>
                <button class="btn" type="button" @click=${this._downloadEmbedding}>Download JSON</button>
              `
            : html`<p class="muted">No embedding yet.</p>`}
        </section>
      </div>
    `;
  }
}
