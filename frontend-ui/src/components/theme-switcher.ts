import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';
import { ObservableController } from '../lib/observable-controller.js';
import { themeService } from '../services/theme.service.js';
import type { ThemeName } from '../types/config.js';

const LABELS: Record<ThemeName, string> = {
  auto: 'Auto',
  light: 'Light',
  dark: 'Dark',
  midnight: 'Midnight',
  solarized: 'Solarized',
  'high-contrast': 'High contrast',
};

@customElement('theme-switcher')
export class ThemeSwitcher extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  private theme = new ObservableController(this, themeService.theme$, themeService.value);

  private _change(e: Event): void {
    const t = (e.target as HTMLSelectElement).value as ThemeName;
    themeService.set(t);
  }

  override render() {
    const current = this.theme.value ?? 'auto';
    return html`
      <label for="theme-select">Theme</label>
      <select id="theme-select" @change=${this._change} .value=${current}>
        ${themeService.themes.map(
          (t) => html`<option value=${t} ?selected=${t === current}>${LABELS[t]}</option>`,
        )}
      </select>
    `;
  }
}
