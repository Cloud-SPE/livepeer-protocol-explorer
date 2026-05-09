import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';
import '../components/ui/empty-state.js';

@customElement('view-delegators-list')
export class ViewDelegatorsList extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  override render() {
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Delegators</h2>
            <p class="lede">Open a delegator directly by address.</p>
          </div>
        </header>
        <empty-state
          heading="Open a delegator by address"
          body="Paste a delegator address into the URL bar, for example #/delegators/0x58b9...0716, or follow a delegator link elsewhere in the app."
        ></empty-state>
      </article>
    `;
  }
}
