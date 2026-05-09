import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { ObservableController } from '../../lib/observable-controller.js';
import { historyService } from '../../services/history.service.js';
import { formatRelative, formatTimestamp } from '../../lib/format.js';
import type { HistoryEntry, Modality } from '../../types/playground.js';

@customElement('history-list')
export class HistoryList extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() modality: Modality = 'llm';

  @state() private state = new ObservableController(this, historyService.state$, historyService.state);

  private _entries(): HistoryEntry[] {
    return this.state.value?.[this.modality] ?? [];
  }

  private _emit(entry: HistoryEntry): void {
    this.dispatchEvent(new CustomEvent<HistoryEntry>('reuse', { detail: entry, bubbles: true, composed: true }));
  }

  private _remove(entry: HistoryEntry): void {
    historyService.remove(this.modality, entry.id);
  }

  override render() {
    const entries = this._entries();
    return html`
      <header class="head">
        <h4>Recent (${entries.length})</h4>
        ${entries.length
          ? html`<button class="clear" type="button" @click=${() => historyService.clear(this.modality)}>Clear</button>`
          : ''}
      </header>
      ${entries.length === 0
        ? html`<p class="empty">No history yet. Submissions will be saved here (last 10).</p>`
        : html`
            <ol>
              ${entries.map(
                (e) => html`
                  <li>
                    <div>
                      <div class="meta" title=${formatTimestamp(e.timestamp)}>
                        ${formatRelative(e.timestamp)}${e.modelId ? ` · ${e.modelId}` : ''}
                      </div>
                      <div class="summary" title=${e.summary}>${e.summary}</div>
                    </div>
                    <menu class="row-actions" aria-label="Entry actions">
                      <button type="button" @click=${() => this._emit(e)}>Reuse</button>
                      <button type="button" @click=${() => this._remove(e)} aria-label="Remove">✕</button>
                    </menu>
                  </li>
                `,
              )}
            </ol>
          `}
    `;
  }
}
