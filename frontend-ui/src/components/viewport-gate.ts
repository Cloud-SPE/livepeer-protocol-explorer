import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';

@customElement('viewport-gate')
export class ViewportGate extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property({ type: Number, attribute: 'min-width' }) minWidth = 800;
  @property() reason = 'This view is best on tablet or desktop.';
  @state() private dismissed = false;

  private _show(): void {
    this.dismissed = true;
  }

  override render() {
    return html`
      <div>
        ${this.dismissed
          ? html`<slot></slot>`
          : html`
              <aside class="banner" role="alert">
                <header>Heads up</header>
                <p>${this.reason}</p>
                <button type="button" @click=${this._show}>Show anyway</button>
              </aside>
              <div hidden><slot></slot></div>
            `}
      </div>
      <style>
        @container (min-width: ${this.minWidth}px) {
          :host > div > .banner { display: none !important; }
          :host > div > [hidden] { display: block !important; }
          :host > div > [hidden] slot { display: contents; }
        }
      </style>
    `;
  }
}
