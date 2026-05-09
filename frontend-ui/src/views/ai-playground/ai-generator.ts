import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import { configService } from '../../services/config.service.js';
import { networkCapabilitiesService } from '../../services/network-capabilities.service.js';

interface Tile {
  href: string;
  title: string;
  body: string;
  tag: string;
}

const TILES: Tile[] = [
  { href: '#/ai/llm', title: 'LLM', body: 'Chat completions with streaming.', tag: 'text → text' },
  { href: '#/ai/text-to-image', title: 'Text-to-image', body: 'Diffusion image generation from prompts.', tag: 'text → image' },
  { href: '#/ai/image-to-image', title: 'Image-to-image', body: 'Image variation and style transfer.', tag: 'image → image' },
  { href: '#/ai/image-to-video', title: 'Image-to-video', body: 'Animate a still image into a short clip.', tag: 'image → video' },
  { href: '#/ai/image-to-text', title: 'Image-to-text', body: 'Caption or OCR an uploaded image.', tag: 'image → text' },
  { href: '#/ai/audio-to-text', title: 'Audio-to-text', body: 'Speech-to-text transcription.', tag: 'audio → text' },
  { href: '#/ai/text-to-speech', title: 'Text-to-speech', body: 'Synthesize speech from a prompt.', tag: 'text → audio' },
  { href: '#/ai/upscale', title: 'Upscale', body: 'Increase image resolution.', tag: 'image → image' },
  { href: '#/ai/segment-anything-2', title: 'Segment Anything 2', body: 'Object segmentation masks.', tag: 'image → image' },
  { href: '#/ai/byoc/openai', title: 'BYOC OpenAI', body: 'Bring your own OpenAI-compatible gateway.', tag: 'BYOC' },
  { href: '#/ai/network-capabilities', title: 'Network capabilities', body: 'Live model availability per orchestrator.', tag: 'reference' },
  { href: '#/ai/settings', title: 'Settings', body: 'Override gateway URL and bearer locally.', tag: 'config' },
];

@customElement('view-ai-generator')
export class ViewAiGenerator extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private cfg = new ObservableController(this, configService.config$, configService.value);
  @state() private caps = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data && !networkCapabilitiesService.state.loading) {
      void networkCapabilitiesService.load();
    }
  }

  private _connectivity(): { tone: 'pos' | 'warn' | 'neg'; label: string; detail: string } {
    const cfg = this.cfg.value!;
    const caps = this.caps.value!;
    if (!cfg.gatewayUrl) {
      return { tone: 'neg', label: 'Gateway URL not configured', detail: 'Set it in AI Settings or in config.json before submitting jobs.' };
    }
    if (caps.loading) {
      return { tone: 'warn', label: 'Probing gateway…', detail: `Loading capabilities from ${cfg.gatewayUrl}.` };
    }
    if (caps.error) {
      const hint = !cfg.gatewayBearer
        ? 'No bearer token configured — many gateways require one. Set it in AI Settings.'
        : 'The gateway returned an error. Check the URL and bearer in AI Settings.';
      return { tone: 'neg', label: 'Gateway unreachable', detail: `${caps.error}. ${hint}` };
    }
    const orchs = caps.data?.orchestrators.length ?? 0;
    if (orchs === 0) {
      return { tone: 'warn', label: 'No orchestrators advertised', detail: 'The gateway responded but reports zero orchestrators. The network may be idle or the bearer may lack scope.' };
    }
    return { tone: 'pos', label: `Connected · ${orchs} orchestrator${orchs === 1 ? '' : 's'}`, detail: `Capabilities loaded from ${cfg.gatewayUrl}.` };
  }

  override render() {
    const conn = this._connectivity();
    return html`
      <article class="page">
        <header class="page-head">
          <h2>AI playground</h2>
          <p class="lede">Submit jobs to the Livepeer AI gateway from your browser.</p>
        </header>
        <aside
          class="banner banner--${conn.tone}"
          role=${conn.tone === 'neg' ? 'alert' : 'status'}
          aria-live="polite"
        >
          <strong>${conn.label}</strong>
          <span>${conn.detail}</span>
          ${conn.tone === 'neg' || conn.tone === 'warn'
            ? html`<a class="btn" href="#/ai/settings">Open settings</a>`
            : ''}
        </aside>
        <section class="grid" aria-label="Modalities">
          ${TILES.map(
            (t) => html`
              <a class="tile" href=${t.href}>
                <h3>${t.title} <span class="tag">${t.tag}</span></h3>
                <p>${t.body}</p>
              </a>
            `,
          )}
        </section>
      </article>
    `;
  }
}
