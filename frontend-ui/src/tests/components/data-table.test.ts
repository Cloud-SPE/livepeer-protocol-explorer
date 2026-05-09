import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import '../../components/ui/data-table.js';
import type { ColumnDef, DataTable } from '../../components/ui/data-table.js';

describe('<data-table>', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    host.remove();
  });

  async function mount(columns: ColumnDef[], rows: Record<string, unknown>[]): Promise<DataTable> {
    const el = document.createElement('data-table') as DataTable;
    el.columns = columns;
    el.rows = rows;
    host.appendChild(el);
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    return el;
  }

  it('renders an empty state when there are no rows', async () => {
    await mount([{ key: 'a', label: 'A' }], []);
    expect(host.textContent).toContain('No rows');
  });

  it('renders a row per item with the right cell count', async () => {
    await mount(
      [
        { key: 'address', label: 'Address', cell: 'address' },
        { key: 'stake', label: 'Stake', cell: 'lpt', align: 'end' },
      ],
      [
        { address: '0x1234567890abcdef1234567890abcdef12345678', stake: '1500.5' },
        { address: '0xabcdefabcdefabcdefabcdefabcdefabcdefabcd', stake: '750' },
      ],
    );
    const rows = host.querySelectorAll('tbody tr');
    expect(rows.length).toBe(2);
    rows.forEach((tr) => {
      expect(tr.querySelectorAll('td').length).toBe(2);
    });
  });

  it('shortens addresses in cell="address" cells', async () => {
    await mount(
      [{ key: 'a', label: 'A', cell: 'address' }],
      [{ a: '0x1234567890abcdef1234567890abcdef12345678' }],
    );
    expect(host.textContent).toMatch(/0x1234…5678/);
  });

  it('formats USD with $ and thousand separator', async () => {
    await mount(
      [{ key: 'usd', label: 'USD', cell: 'usd', align: 'end' }],
      [{ usd: '1234.5' }],
    );
    expect(host.textContent).toContain('$');
    expect(host.textContent).toMatch(/1[,.]?234/);
  });

  it('marks rows linkable when href-template is set', async () => {
    const el = document.createElement('data-table') as DataTable;
    el.columns = [{ key: 'a', label: 'A' }];
    el.rows = [{ a: 'x' }];
    el.hrefTemplate = '#/foo/{a}';
    host.appendChild(el);
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    const tr = host.querySelector('tbody tr');
    expect(tr?.classList.contains('linkable')).toBe(true);
  });
});
