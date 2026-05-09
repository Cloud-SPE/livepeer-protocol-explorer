import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/perf-stats.js', () => ({
  perfStatsSource: {
    fetchTranscoding: vi.fn(),
    fetchAi: vi.fn(),
  },
}));

const { perfService } = await import('../../services/perf.service.js');
const { perfStatsSource } = await import('../../lib/sources/perf-stats.js');

beforeEach(() => {
  vi.mocked(perfStatsSource.fetchTranscoding).mockReset();
  vi.mocked(perfStatsSource.fetchAi).mockReset();
  perfService.reset();
});

describe('perfService.refresh', () => {
  it('does nothing without an orchestrator', async () => {
    await perfService.refresh({ kind: 'transcoding', orchestrator: '' });
    expect(vi.mocked(perfStatsSource.fetchTranscoding)).not.toHaveBeenCalled();
  });

  it('fetches transcoding stats by default', async () => {
    vi.mocked(perfStatsSource.fetchTranscoding).mockResolvedValueOnce({});
    await perfService.refresh({ kind: 'transcoding', orchestrator: '0xa' });
    expect(perfService.state.orchestrator).toBe('0xa');
    expect(perfService.state.kind).toBe('transcoding');
    expect(perfService.state.error).toBeNull();
  });

  it('fetches AI stats when kind=ai', async () => {
    vi.mocked(perfStatsSource.fetchAi).mockResolvedValueOnce({});
    await perfService.refresh({ kind: 'ai', orchestrator: '0xa', pipeline: 'llm', model: 'm1' });
    expect(vi.mocked(perfStatsSource.fetchAi)).toHaveBeenCalledWith({
      orchestrator: '0xa',
      pipeline: 'llm',
      model: 'm1',
    });
  });

  it('records errors', async () => {
    vi.mocked(perfStatsSource.fetchTranscoding).mockRejectedValueOnce(new Error('boom'));
    await perfService.refresh({ kind: 'transcoding', orchestrator: '0xa' });
    expect(perfService.state.error).toBe('boom');
  });
});
