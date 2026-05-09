import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';

export interface TabDef {
  href: string;          // hash-prefixed (e.g. "#/governance/proposals")
  label: string;
  match: RegExp;         // tested against current path (without "#")
}

/**
 * A horizontal sub-navigation bar for grouping related views (e.g. Proposals /
 * Votes inside the Governance section). Each tab is a real anchor so it works
 * with browser back/forward and keyboard navigation; the active tab is decided
 * from `location.hash` so this component stays state-free.
 */
@customElement('section-tabs')
export class SectionTabs extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @property({ attribute: false }) tabs: TabDef[] = [];
  @property() label = 'Section';

  @state() private path = location.hash.slice(1) || '/';

  private _onHash = (): void => {
    this.path = location.hash.slice(1) || '/';
  };

  override connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener('hashchange', this._onHash);
  }
  override disconnectedCallback(): void {
    super.disconnectedCallback();
    window.removeEventListener('hashchange', this._onHash);
  }

  override render() {
    const current = this.path.split('?')[0] ?? '/';
    return html`
      <nav class="subnav" aria-label="${this.label}">
        ${this.tabs.map(
          (t) => html`
            <a
              href=${t.href}
              aria-current=${t.match.test(current) ? 'page' : 'false'}
            >${t.label}</a>
          `,
        )}
      </nav>
    `;
  }
}
