import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { configService } from '../services/config.service.js';
import { networkCapabilitiesService } from '../services/network-capabilities.service.js';
import { STORAGE_KEYS, getItem, setItem, removeItem } from '../lib/storage.js';
import type { AppConfig, PartialAppConfig } from '../types/config.js';

interface GatewayOverride {
  url?: string;
  bearer?: string;
  byocUrl?: string;
}

/** Official Livepeer-operated AI gateway endpoints. Mirrors the dropdown in
 *  the legacy livepeer-tools-ui Settings page. */
const OFFICIAL_GATEWAYS: { value: string; label: string }[] = [
  { value: 'https://dream-gateway.livepeer.cloud', label: 'dream-gateway.livepeer.cloud (default — closest)' },
  { value: 'https://dream-gateway-us-west.livepeer.cloud', label: 'dream-gateway-us-west.livepeer.cloud' },
  { value: 'https://dream-gateway-us-east.livepeer.cloud', label: 'dream-gateway-us-east.livepeer.cloud' },
  { value: 'https://dream-gateway-eu-central.livepeer.cloud', label: 'dream-gateway-eu-central.livepeer.cloud' },
];
const DEFAULT_GATEWAY = OFFICIAL_GATEWAYS[0]!.value;
const OFFICIAL_BYOC = 'https://openai-gateway.livepeer.cloud/v1';

@customElement('view-ai-settings')
export class ViewAiSettings extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }

  @state() private cfg = new ObservableController(this, configService.config$, configService.value);
  @state() private url = '';
  @state() private bearer = '';
  @state() private byocUrl = '';
  @state() private useCustom = false;
  @state() private message: string = '';

  override connectedCallback(): void {
    super.connectedCallback();
    const stored = getItem<GatewayOverride>(STORAGE_KEYS.GATEWAY_OVERRIDE, {});
    const initialUrl = stored.url ?? this.cfg.value?.gatewayUrl ?? DEFAULT_GATEWAY;
    this.url = initialUrl;
    this.bearer = stored.bearer ?? this.cfg.value?.gatewayBearer ?? '';
    this.byocUrl = stored.byocUrl ?? this.cfg.value?.byocGatewayUrl ?? OFFICIAL_BYOC;
    this.useCustom = !OFFICIAL_GATEWAYS.some((g) => g.value === initialUrl);
  }

  private _selectOfficial(e: Event): void {
    this.url = (e.target as HTMLSelectElement).value;
  }

  private _toggleCustom(e: Event): void {
    this.useCustom = (e.target as HTMLInputElement).checked;
    if (!this.useCustom) this.url = DEFAULT_GATEWAY;
  }

  private _save(e: Event): void {
    e.preventDefault();
    if (!this.url.trim()) {
      this.message = 'Gateway URL is required.';
      return;
    }
    const override: GatewayOverride = {
      url: this.url.trim(),
      bearer: this.bearer.trim(),
      byocUrl: this.byocUrl.trim(),
    };
    setItem(STORAGE_KEYS.GATEWAY_OVERRIDE, override);
    const patch: PartialAppConfig = {
      gatewayUrl: override.url ?? '',
      gatewayBearer: override.bearer ?? '',
      byocGatewayUrl: override.byocUrl ?? '',
    };
    configService.patch(patch);
    // Reprobe the gateway so every AI playground view sees fresh model lists
    // without the user needing to hard-reload.
    networkCapabilitiesService.reset();
    void networkCapabilitiesService.load();
    this.message = 'Saved. Capabilities are reloading from the new gateway.';
  }

  private _reset(): void {
    removeItem(STORAGE_KEYS.GATEWAY_OVERRIDE);
    const defaults: AppConfig = configService.apply(null);
    this.url = defaults.gatewayUrl;
    this.bearer = defaults.gatewayBearer;
    this.byocUrl = defaults.byocGatewayUrl;
    this.useCustom = !OFFICIAL_GATEWAYS.some((g) => g.value === defaults.gatewayUrl);
    networkCapabilitiesService.reset();
    void networkCapabilitiesService.load();
    this.message = 'Reverted to baked defaults. Capabilities are reloading.';
  }

  override render() {
    const selectedOfficial = OFFICIAL_GATEWAYS.find((g) => g.value === this.url)?.value ?? DEFAULT_GATEWAY;

    return html`
      <article class="page">
        <header class="page-head">
          <div>
            <h2>AI settings</h2>
            <p class="lede">
              Choose a Livepeer-operated regional gateway, or point at a custom gateway URL.
              Settings are stored in <code>localStorage</code> and applied on save.
            </p>
          </div>
        </header>
        <form @submit=${this._save}>
          <fieldset>
            <legend>Gateway</legend>
            <label>
              <span>Region</span>
              <select
                ?disabled=${this.useCustom}
                .value=${selectedOfficial}
                @change=${this._selectOfficial}
              >
                ${OFFICIAL_GATEWAYS.map(
                  (g) => html`<option value=${g.value} ?selected=${g.value === selectedOfficial}>${g.label}</option>`,
                )}
              </select>
            </label>
            <label class="toggle">
              <input
                type="checkbox"
                .checked=${this.useCustom}
                @change=${this._toggleCustom}
              />
              <span>Use a custom gateway URL instead</span>
            </label>
            ${this.useCustom
              ? html`
                  <label>
                    <span>Custom gateway URL</span>
                    <input
                      type="url"
                      required
                      placeholder="https://my-gateway.example"
                      .value=${this.url}
                      @input=${(e: Event) => (this.url = (e.target as HTMLInputElement).value)}
                    />
                  </label>
                `
              : ''}
          </fieldset>

          <label>
            <span>Bearer token (optional — many gateways require one)</span>
            <input
              type="text"
              placeholder="lp_…"
              .value=${this.bearer}
              @input=${(e: Event) => (this.bearer = (e.target as HTMLInputElement).value)}
            />
          </label>

          <label>
            <span>BYOC OpenAI-compatible gateway URL</span>
            <input
              type="url"
              placeholder="https://openai-gateway.livepeer.cloud/v1"
              .value=${this.byocUrl}
              @input=${(e: Event) => (this.byocUrl = (e.target as HTMLInputElement).value)}
            />
          </label>

          <div class="row">
            <button class="btn btn--primary" type="submit">Save</button>
            <button class="btn" type="button" @click=${this._reset}>Reset to defaults</button>
            ${this.message ? html`<span class="ok">${this.message}</span>` : ''}
          </div>
        </form>
      </article>
    `;
  }
}
