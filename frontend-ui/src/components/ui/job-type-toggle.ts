import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { JobType } from '../../types/api.js';

const OPTIONS: { value: JobType; label: string }[] = [
  { value: 'both', label: 'Both' },
  { value: 'transcoding', label: 'Transcoding' },
  { value: 'ai', label: 'AI' },
];

@customElement('job-type-toggle')
export class JobTypeToggle extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property() value: JobType = 'both';

  private _set(v: JobType): void {
    if (v === this.value) return;
    this.value = v;
    this.dispatchEvent(
      new CustomEvent<JobType>('change-job-type', { detail: v, bubbles: true, composed: true }),
    );
  }

  override render() {
    return html`
      <span class="label">Job type</span>
      <div class="group" role="group" aria-label="Filter by job type">
        ${OPTIONS.map(
          (opt) => html`
            <button
              type="button"
              aria-pressed=${this.value === opt.value ? 'true' : 'false'}
              @click=${() => this._set(opt.value)}
            >
              ${opt.label}
            </button>
          `,
        )}
      </div>
    `;
  }
}
