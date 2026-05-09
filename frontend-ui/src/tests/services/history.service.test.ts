import { describe, it, expect, beforeEach } from 'vitest';
import { historyService } from '../../services/history.service.js';

beforeEach(() => {
  localStorage.clear();
  historyService.clearAll();
});

describe('historyService.push', () => {
  it('prepends new entries and assigns id + timestamp', () => {
    const e = historyService.push({
      modality: 'llm',
      modelId: 'm1',
      prompt: 'hi',
      summary: 'hi',
    });
    expect(e.id).toBeTruthy();
    expect(e.timestamp).toBeTruthy();
    expect(historyService.list('llm')).toHaveLength(1);
    expect(historyService.list('text-to-image')).toHaveLength(0);
  });

  it('caps each modality at 10 entries', () => {
    for (let i = 0; i < 15; i++) {
      historyService.push({ modality: 'llm', summary: `entry ${i}` });
    }
    const list = historyService.list('llm');
    expect(list).toHaveLength(10);
    expect(list[0]?.summary).toBe('entry 14');
  });
});

describe('historyService.remove', () => {
  it('removes a specific entry by id', () => {
    historyService.push({ modality: 'llm', summary: 'a' });
    const b = historyService.push({ modality: 'llm', summary: 'b' });
    historyService.remove('llm', b.id);
    expect(historyService.list('llm').map((e) => e.summary)).toEqual(['a']);
  });
});

describe('historyService persistence', () => {
  it('round-trips through localStorage', () => {
    historyService.push({ modality: 'llm', summary: 'persisted' });
    expect(localStorage.getItem('lp-tools:history')).toContain('persisted');
  });
});
