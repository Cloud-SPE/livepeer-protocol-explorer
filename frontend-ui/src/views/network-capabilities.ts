import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { networkCapabilitiesService } from '../services/network-capabilities.service.js';
import '../components/ui/refresh-button.js';
import '../components/ui/empty-state.js';
import '../components/ui/address-chip.js';

@customElement('view-network-capabilities')
export class ViewNetworkCapabilities extends LitElement {

  override createRenderRoot(): HTMLElement { return this; }

  @state() private state = new ObservableController(this, networkCapabilitiesService.state$, networkCapabilitiesService.state);

  override connectedCallback(): void {
    super.connectedCallback();
    if (!networkCapabilitiesService.state.data && !networkCapabilitiesService.state.loading) {
      void networkCapabilitiesService.load();
    }
  }

  override render() {
    const s = this.state.value!;
    const orchs = s.data?.orchestrators ?? [];
    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>Network capabilities</h2>
            <p class="lede">Live AI model availability across orchestrators.</p>
          </div>
          <refresh-button
            ?loading=${s.loading}
            .lastUpdated=${s.lastUpdated}
            @refresh=${() => networkCapabilitiesService.load()}
          ></refresh-button>
        </header>
        ${s.error ? html`<p class="error" role="alert">${s.error}</p>` : ''}
        ${orchs.length === 0 && !s.loading
          ? html`<empty-state heading="No capabilities" body="The gateway returned no orchestrator capabilities."></empty-state>`
          : orchs.map(
              (o) => html`
                <article class="card orch">
                  <header>
                    <address-chip address=${o.address} kind="orchestrator" explorer></address-chip>
                    <span class="muted">· ${o.pipelines.length} pipelines</span>
                  </header>
                  <div class="pipelines">
                    ${o.pipelines.map(
                      (p) => html`
                        <div class="pipeline">
                          <h4>${p.type} <span class="muted">(${p.models.length})</span></h4>
                          <ul>
                            ${p.models.map(
                              (m) => html`
                                <li>
                                  <span class="mono">${m.name}</span>
                                  <span>
                                    ${m.status.Warm > 0
                                      ? html`<span class="pill-warm">${m.status.Warm} warm</span>`
                                      : ''}
                                    ${m.status.Cold > 0
                                      ? html`<span class="pill-cold">${m.status.Cold} cold</span>`
                                      : ''}
                                  </span>
                                </li>
                              `,
                            )}
                          </ul>
                        </div>
                      `,
                    )}
                  </div>
                </article>
              `,
            )}
      </article>
    `;
  }
}
