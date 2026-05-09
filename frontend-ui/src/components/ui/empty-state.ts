import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';

@customElement('empty-state')
export class EmptyState extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() heading = 'Nothing here yet';
  @property() body: string = '';

  override render() {
    return html`
      <div>
        <div class="title">${this.heading}</div>
        ${this.body ? html`<p class="body">${this.body}</p>` : ''}
        <slot></slot>
      </div>
    `;
  }
}
