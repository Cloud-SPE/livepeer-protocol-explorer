import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/leaderboard.js', () => ({
  leaderboardSource: {
    fetchTranscoding: vi.fn(),
    fetchAi: vi.fn(),
  },
}));

const { leaderboardService, summarize } = await import('../../services/leaderboard.service.js');
const { leaderboardSource } = await import('../../lib/sources/leaderboard.js');

beforeEach(() => {
  vi.mocked(leaderboardSource.fetchTranscoding).mockReset();
  vi.mocked(leaderboardSource.fetchAi).mockReset();
  leaderboardService.reset();
});

describe('summarize', () => {
  it('averages scores across regions per orchestrator', () => {
    const rows = summarize({
      '0xa': {
        FRA: { success_rate: 1, round_trip_score: 0.9, score: 0.9 },
        NYC: { success_rate: 0.8, round_trip_score: 0.7, score: 0.56 },
      },
      '0xb': {
        FRA: { success_rate: 0.5, round_trip_score: 0.5, score: 0.25 },
      },
    });
    expect(rows).toHaveLength(2);
    expect(rows[0]?.orchestrator).toBe('0xa');
    expect(rows[0]?.region_count).toBe(2);
    expect(rows[0]?.avg_score).toBeCloseTo((0.9 + 0.56) / 2, 5);
  });

  it('returns empty for null', () => {
    expect(summarize(null)).toEqual([]);
  });

  it('skips orchestrators with no regions', () => {
    expect(summarize({ '0xa': {} })).toEqual([]);
  });

  it('sorts by avg_score descending', () => {
    const rows = summarize({
      '0xa': { FRA: { success_rate: 1, round_trip_score: 0.5, score: 0.5 } },
      '0xb': { FRA: { success_rate: 1, round_trip_score: 0.9, score: 0.9 } },
    });
    expect(rows[0]?.orchestrator).toBe('0xb');
  });
});

describe('leaderboardService.refresh', () => {
  it('calls the transcoding source by default', async () => {
    vi.mocked(leaderboardSource.fetchTranscoding).mockResolvedValueOnce({
      '0xa': { FRA: { success_rate: 1, round_trip_score: 1, score: 1 } },
    });
    await leaderboardService.refresh({ kind: 'transcoding', region: 'GLOBAL' });
    expect(leaderboardService.state.kind).toBe('transcoding');
    expect(leaderboardService.state.data).not.toBeNull();
    expect(leaderboardService.state.error).toBeNull();
  });

  it('calls the AI source when kind=ai', async () => {
    vi.mocked(leaderboardSource.fetchAi).mockResolvedValueOnce({});
    await leaderboardService.refresh({ kind: 'ai', region: 'GLOBAL', pipeline: 'llm', model: 'm1' });
    expect(vi.mocked(leaderboardSource.fetchAi)).toHaveBeenCalledWith({
      pipeline: 'llm',
      model: 'm1',
      region: undefined,
    });
  });

  it('records errors', async () => {
    vi.mocked(leaderboardSource.fetchTranscoding).mockRejectedValueOnce(new Error('500'));
    await leaderboardService.refresh({ kind: 'transcoding', region: 'GLOBAL' });
    expect(leaderboardService.state.error).toBe('500');
  });
});
