import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import '../../components/ui/empty-state.js';

describe('<empty-state>', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    host.remove();
  });

  it('renders the default heading', async () => {
    host.innerHTML = '<empty-state></empty-state>';
    const el = host.querySelector('empty-state');
    expect(el).toBeTruthy();
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    expect(host.textContent).toContain('Nothing here yet');
  });

  it('honors the heading and body properties', async () => {
    host.innerHTML = '<empty-state heading="No payouts" body="Try a different window"></empty-state>';
    const el = host.querySelector('empty-state');
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    expect(host.textContent).toContain('No payouts');
    expect(host.textContent).toContain('Try a different window');
  });

  it('renders into light DOM (no shadowRoot)', async () => {
    host.innerHTML = '<empty-state></empty-state>';
    const el = host.querySelector('empty-state') as Element & { shadowRoot: ShadowRoot | null };
    expect(el.shadowRoot).toBeNull();
  });
});
