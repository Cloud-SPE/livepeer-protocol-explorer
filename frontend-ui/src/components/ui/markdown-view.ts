import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { marked } from 'marked';

marked.setOptions({ gfm: true, breaks: false });

@customElement('markdown-view')
export class MarkdownView extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() source: string = '';

  override render() {
    if (!this.source) return html`<p class="muted">No description.</p>`;
    const rendered = marked.parse(this.source, { async: false }) as string;
    return html`${unsafeHTML(rendered)}`;
  }
}
