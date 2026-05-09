import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getTicketsTimeseries: vi.fn(),
  },
}));

const { ticketsService } = await import('../../services/tickets.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getTicketsTimeseries).mockReset();
  ticketsService.reset();
});

describe('ticketsService.refresh', () => {
  it('captures the timeseries response', async () => {
    vi.mocked(localApi.getTicketsTimeseries).mockResolvedValueOnce({
      start: '2026-01-01',
      end: '2026-01-07',
      job_type: 'both',
      ai: [{ date: '2026-01-01', count: '5' }],
      transcoding: [{ date: '2026-01-01', count: '12' }],
    });
    await ticketsService.refresh({ start: '2026-01-01', end: '2026-01-07' });
    const s = ticketsService.state;
    expect(s.data?.ai).toHaveLength(1);
    expect(s.data?.transcoding).toHaveLength(1);
    expect(s.error).toBeNull();
  });

  it('records errors', async () => {
    vi.mocked(localApi.getTicketsTimeseries).mockRejectedValueOnce(new Error('400'));
    await ticketsService.refresh({ start: 'a', end: 'b' });
    expect(ticketsService.state.error).toBe('400');
  });
});
