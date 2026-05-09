import { LitElement, html, type TemplateResult } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { ifDefined } from 'lit/directives/if-defined.js';
import {
  formatDecimal,
  formatNative,
  formatPercent,
  formatRelative,
  formatTimestamp,
  formatUsd,
  shortAddress,
} from '../../lib/format.js';

export type CellKind =
  | 'text'
  | 'mono'
  | 'address'
  | 'tx'
  | 'lpt'
  | 'eth'
  | 'usd'
  | 'percent'
  | 'bool'
  | 'time'
  | 'reltime'
  | 'pill'
  | 'number';

export interface ColumnDef {
  key: string;
  label: string;
  cell?: CellKind;
  align?: 'start' | 'end' | 'center';
  width?: string;
  decimals?: number;
  /** Clip overflowing text with an ellipsis; full content stays in the
   *  cell's `title` attribute so hover reveals the rest. Combine with
   *  `width` to set the clip threshold. */
  truncate?: boolean;
}

@customElement('data-table')
export class DataTable extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() caption: string = '';
  @property({ attribute: false }) columns: ColumnDef[] = [];
  @property({ attribute: false }) rows: Record<string, unknown>[] = [];
  @property({ attribute: 'href-template' }) hrefTemplate: string = '';
  @property() emptyText = 'No rows';

  private _hrefFor(row: Record<string, unknown>): string | undefined {
    if (!this.hrefTemplate) return undefined;
    return this.hrefTemplate.replace(/\{(\w+)\}/g, (_m, k: string) => String(row[k] ?? ''));
  }

  private _renderCell(col: ColumnDef, row: Record<string, unknown>): TemplateResult | string {
    const v = row[col.key];
    switch (col.cell ?? 'text') {
      case 'address':
        return html`<span class="mono" title="${String(v ?? '')}">${shortAddress(v == null ? null : String(v))}</span>`;
      case 'tx':
        return html`<span class="mono" title="${String(v ?? '')}">${shortAddress(v == null ? null : String(v), 8, 6)}</span>`;
      case 'lpt':
        return formatNative(v as string | null | undefined, 18, { digits: col.decimals ?? 4 });
      case 'eth':
        return formatNative(v as string | null | undefined, 18, { digits: col.decimals ?? 6 });
      case 'usd':
        return formatUsd(v as string | null | undefined, col.decimals !== undefined ? { digits: col.decimals } : {});
      case 'percent':
        return formatPercent(v as string | null | undefined, col.decimals ?? 2);
      case 'bool':
        return v
          ? html`<span class="pill pill--pos">Yes</span>`
          : html`<span class="pill">No</span>`;
      case 'time':
        return html`<time datetime="${String(v ?? '')}">${formatTimestamp(v as string)}</time>`;
      case 'reltime':
        return html`<time datetime="${String(v ?? '')}" title="${formatTimestamp(v as string)}">${formatRelative(v as string)}</time>`;
      case 'pill':
        return v == null ? '' : html`<span class="pill">${String(v)}</span>`;
      case 'mono':
        return html`<span class="mono">${v == null ? '—' : String(v)}</span>`;
      case 'number':
        return formatDecimal(v as string | number | null | undefined, { digits: col.decimals ?? 0 });
      case 'text':
      default:
        return v == null || v === '' ? '—' : String(v);
    }
  }

  override render() {
    if (!this.rows.length) {
      return html`
        <div class="scroll">
          ${this.caption ? html`<div class="caption">${this.caption}</div>` : ''}
          <p class="empty">${this.emptyText}</p>
        </div>
      `;
    }
    return html`
      <div class="scroll">
        <table>
          ${this.caption ? html`<caption>${this.caption}</caption>` : ''}
          <thead>
            <tr>
              ${this.columns.map(
                (c) => html`<th class="${c.align ?? 'start'}" style=${ifDefined(c.width ? `width:${c.width}` : undefined)}>${c.label}</th>`,
              )}
            </tr>
          </thead>
          <tbody>
            ${this.rows.map((row) => {
              const href = this._hrefFor(row);
              const click = href
                ? () => {
                    window.location.hash = href.startsWith('#') ? href.slice(1) : href;
                  }
                : undefined;
              return html`
                <tr class=${href ? 'linkable' : ''} @click=${click}>
                  ${this.columns.map((c) => {
                    const align = c.align ?? 'start';
                    const cls = c.truncate ? `${align} truncate` : align;
                    const styleAttr = c.truncate && c.width ? `max-width:${c.width}` : undefined;
                    const raw = row[c.key];
                    // For truncated cells, drop the full string into title so
                    // hover surfaces the clipped tail.
                    const titleAttr =
                      c.truncate && raw != null && raw !== '' ? String(raw) : undefined;
                    return html`
                      <td
                        class="${cls}"
                        data-label="${c.label}"
                        style=${ifDefined(styleAttr)}
                        title=${ifDefined(titleAttr)}
                      >
                        ${this._renderCell(c, row)}
                      </td>
                    `;
                  })}
                </tr>
              `;
            })}
          </tbody>
        </table>
      </div>
    `;
  }
}
