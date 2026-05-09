import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';
import '../components/ui/empty-state.js';

@customElement('view-rounds-list')
export class ViewRoundsList extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  override render() {
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Rounds</h2>
            <p class="lede">Open a round directly by id.</p>
          </div>
        </header>
        <empty-state
          heading="Open a round by id"
          body="Paste a round number into the URL bar, for example #/rounds/4192, or use the current round link on the dashboard."
        ></empty-state>
      </article>
    `;
  }
}
