import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';

interface NavItem {
  label: string;
  href: string;
  match: RegExp;
  group?: string;
}

const NAV: NavItem[] = [
  { label: 'Dashboard', href: '#/', match: /^\/$/ },
  { label: 'Orchestrators', href: '#/orchestrators', match: /^\/orchestrators/ },
  { label: 'Gateways', href: '#/gateways', match: /^\/(gateways|broadcasters)/ },
  { label: 'Reports', href: '#/reports', match: /^\/reports/ },
  { label: 'Rewards', href: '#/rewards/leaderboard', match: /^\/rewards/ },
  { label: 'Governance', href: '#/governance/proposals', match: /^\/(governance|vote)/ },
  { label: 'Delegators', href: '#/delegators', match: /^\/delegators/ },
  { label: 'Rounds', href: '#/rounds', match: /^\/rounds/ },
];

@customElement('side-nav')
export class SideNav extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @state() private path = location.hash.slice(1) || '/';

  override connectedCallback(): void {
    super.connectedCallback();
    this._onHash = this._onHash.bind(this);
    window.addEventListener('hashchange', this._onHash);
  }
  override disconnectedCallback(): void {
    super.disconnectedCallback();
    window.removeEventListener('hashchange', this._onHash);
  }

  private _onHash(): void {
    this.path = location.hash.slice(1) || '/';
  }

  override render() {
    const current = this.path.split('?')[0] ?? '/';
    return html`
      <nav aria-label="Primary">
        ${NAV.map(
          (item) => html`
            <a
              href="${item.href}"
              aria-current=${item.match.test(current) ? 'page' : 'false'}
              >${item.label}</a
            >
          `,
        )}
      </nav>
    `;
  }
}
