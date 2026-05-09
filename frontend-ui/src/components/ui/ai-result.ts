import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { resolveMediaUrl } from '../../lib/sources/ai-gateway.js';
import type { HistoryOutput } from '../../types/playground.js';
import './markdown-view.js';

@customElement('ai-result')
export class AiResult extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property({ attribute: false }) output?: HistoryOutput;
  @property({ type: Boolean }) renderMarkdown = false;

  override render() {
    const out = this.output;
    if (!out) return html`<p class="muted">No output yet.</p>`;
    if (out.kind === 'text') {
      return this.renderMarkdown
        ? html`<div class="text-block"><markdown-view .source=${out.text}></markdown-view></div>`
        : html`<pre class="text-block">${out.text}</pre>`;
    }
    if (out.kind === 'audio') {
      const url = resolveMediaUrl(out.audio.url);
      return html`<figure>
        <audio controls src=${url}></audio>
        <figcaption><a href=${url} target="_blank" rel="noopener" download>Open audio ↗</a></figcaption>
      </figure>`;
    }
    return html`
      <div class="images">
        ${out.images.map((img) => {
          const url = resolveMediaUrl(img.url);
          const isVideo = /\.(mp4|webm|mov)(\?|$)/i.test(url);
          return html`
            <figure>
              ${isVideo
                ? html`<video controls src=${url} preload="metadata"></video>`
                : html`<img src=${url} alt="" loading="lazy" />`}
              <figcaption>
                ${img.seed !== undefined ? html`<span>seed ${img.seed}</span>` : ''}
                <a href=${url} target="_blank" rel="noopener" download>Open ↗</a>
              </figcaption>
            </figure>
          `;
        })}
      </div>
    `;
  }
}
