import './styles/layers.css';
import './styles/reset.css';
import './styles/themes/_shape.css';
import './styles/themes/auto.css';
import './styles/themes/light.css';
import './styles/themes/dark.css';
import './styles/themes/midnight.css';
import './styles/themes/solarized.css';
import './styles/themes/high-contrast.css';
import './styles/base.css';
import './styles/components.css';

import { loadRuntimeConfig } from './services/config.service.js';
import './services/theme.service.js';

async function boot(): Promise<void> {
  await loadRuntimeConfig();
  await import('./components/app-shell.js');
  const mount = document.getElementById('app-mount');
  if (!mount) return;
  mount.removeAttribute('aria-busy');
  mount.replaceChildren(document.createElement('app-shell'));
}

void boot();
