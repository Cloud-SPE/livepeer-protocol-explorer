import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import {
  modelsForPipeline,
  networkCapabilitiesService,
} from '../../services/network-capabilities.service.js';
import { historyService } from '../../services/history.service.js';
import { aiGateway, streamLlm } from '../../lib/sources/ai-gateway.js';
import type { ChatChoiceMessage, HistoryEntry, HistoryOutput } from '../../types/playground.js';
import '../../components/ui/ai-result.js';
import '../../components/ui/history-list.js';
import '../../components/ui/refresh-button.js';

@customElement('view-llm')
export class ViewLlm extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private capabilities = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);
  @state() private model = '';
  @state() private system = 'You are a helpful assistant.';
  @state() private prompt = '';
  @state() private maxTokens = 512;
  @state() private temperature = 0.7;
  @state() private stream = true;
  @state() private loading = false;
  @state() private error = '';
  @state() private output: HistoryOutput | undefined = undefined;
  @state() private reasoning = '';

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data && !networkCapabilitiesService.state.loading) {
      void networkCapabilitiesService.load();
    }
  }

  override updated(): void {
    if (!this.model) {
      const opts = modelsForPipeline(this.capabilities.value?.data ?? null, 'Llm');
      if (opts[0]) this.model = opts[0].name;
    }
  }

  private async _submit(e: Event): Promise<void> {
    e.preventDefault();
    if (!this.prompt.trim()) {
      this.error = 'Prompt is required.';
      return;
    }
    this.error = '';
    this.output = undefined;
    this.reasoning = '';
    this.loading = true;
    try {
      const payload = {
        model: this.model,
        messages: [
          { role: 'system' as const, content: this.system || '' },
          { role: 'user' as const, content: this.prompt },
        ],
        max_tokens: this.maxTokens,
        temperature: this.temperature,
      };
      let text = '';
      let reasoning = '';
      if (this.stream) {
        for await (const delta of streamLlm(payload)) {
          text += stripHeader(delta.content);
          reasoning += stripHeader(delta.reasoning) + stripHeader(delta.reasoning_content);
          this.output = { kind: 'text', text };
          this.reasoning = reasoning;
        }
      } else {
        const res = await aiGateway.llm(payload);
        const msg: ChatChoiceMessage | undefined = res.choices[0]?.message ?? res.choices[0]?.delta;
        text = stripHeader(msg?.content);
        reasoning = stripHeader(msg?.reasoning) + stripHeader(msg?.reasoning_content);
        this.output = { kind: 'text', text };
        this.reasoning = reasoning;
      }
      historyService.push({
        modality: 'llm',
        modelId: this.model,
        prompt: this.prompt,
        summary: text.slice(0, 140) || '(empty response)',
        output: { kind: 'text', text },
      });
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }

  private _reuse(e: CustomEvent<HistoryEntry>): void {
    if (e.detail.modelId) this.model = e.detail.modelId;
    if (e.detail.prompt) this.prompt = e.detail.prompt;
    if (e.detail.output?.kind === 'text') this.output = e.detail.output;
  }

  override render() {
    const models = modelsForPipeline(this.capabilities.value?.data ?? null, 'Llm');
    return html`
      <article class="page">
        <header class="page-head">
          <p class="crumb"><a href="#/ai/generator">← AI playground</a></p>
          <h2>LLM</h2>
          <p class="lede">Chat completions via the AI gateway.</p>
        </header>
        <div class="layout">
          <section aria-labelledby="form-h" class="card stack">
            <h3 id="form-h" class="sr-only">Inputs</h3>
            <form @submit=${this._submit}>
              <label>
                <span>Model${models.length === 0 ? ' (waiting on capabilities…)' : ''}</span>
                <select required .value=${this.model} @change=${(e: Event) => (this.model = (e.target as HTMLSelectElement).value)}>
                  ${models.length === 0
                    ? html`<option value="" disabled selected>—</option>`
                    : models.map((m) => html`<option value=${m.name} ?selected=${m.name === this.model}>${m.name}</option>`)}
                </select>
              </label>
              <label>
                <span>System</span>
                <textarea rows="2" .value=${this.system} @input=${(e: Event) => (this.system = (e.target as HTMLTextAreaElement).value)}></textarea>
              </label>
              <label>
                <span>Prompt</span>
                <textarea rows="6" required .value=${this.prompt} @input=${(e: Event) => (this.prompt = (e.target as HTMLTextAreaElement).value)}></textarea>
              </label>
              <div class="field-row">
                <label>
                  <span>Max tokens</span>
                  <input type="number" min="1" max="32000" .value=${String(this.maxTokens)} @input=${(e: Event) => (this.maxTokens = Number((e.target as HTMLInputElement).value))} />
                </label>
                <label>
                  <span>Temperature</span>
                  <input type="number" min="0" max="2" step="0.1" .value=${String(this.temperature)} @input=${(e: Event) => (this.temperature = Number((e.target as HTMLInputElement).value))} />
                </label>
              </div>
              <label class="toggle">
                <input type="checkbox" .checked=${this.stream} @change=${(e: Event) => (this.stream = (e.target as HTMLInputElement).checked)} />
                Stream response
              </label>
              <div class="actions">
                <button class="btn btn--primary" type="submit" ?disabled=${this.loading || !this.model}>${this.loading ? 'Generating…' : 'Generate'}</button>
                <refresh-button label="Reload models" ?loading=${this.capabilities.value?.loading ?? false} .lastUpdated=${this.capabilities.value?.lastUpdated ?? null} @refresh=${() => networkCapabilitiesService.load()}></refresh-button>
              </div>
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : ''}
            </form>
            <history-list modality="llm" @reuse=${(e: CustomEvent<HistoryEntry>) => this._reuse(e)}></history-list>
          </section>
          <section aria-labelledby="out-h" class="card out-card">
            <h3 id="out-h">Output</h3>
            <ai-result .output=${this.output} render-markdown></ai-result>
            ${this.reasoning ? html`<div class="reasoning">${this.reasoning}</div>` : ''}
          </section>
        </div>
      </article>
    `;
  }
}

function stripHeader(value: unknown): string {
  if (typeof value === 'string') {
    return value.replace(/<\|start_header_id\|>assistant<\|end_header_id\|>/g, '');
  }
  if (Array.isArray(value)) return value.map((v) => stripHeader(v)).join('');
  if (value && typeof value === 'object') {
    const o = value as { text?: unknown; content?: unknown };
    if (typeof o.text === 'string') return stripHeader(o.text);
    if (typeof o.content === 'string') return stripHeader(o.content);
  }
  return '';
}
