import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getPayoutLeaderboard: vi.fn(),
    getPayoutSummary: vi.fn(),
  },
}));

const { payoutsService } = await import('../../services/payouts.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getPayoutLeaderboard).mockReset();
  vi.mocked(localApi.getPayoutSummary).mockReset();
  payoutsService.reset();
});

const sampleRow = {
  orchestrator_address: '0xa',
  ticket_count: '5',
  sum_face_value_native: '1.5',
  sum_face_value_usd: '4500',
  sum_commission_native: '0.15',
  sum_commission_usd: '450',
  sum_delegators_share_native: '1.35',
  sum_delegators_share_usd: '4050',
  distinct_gateways: '3',
  usd_rows_priced: '5',
};

describe('payoutsService.refreshLeaderboard', () => {
  it('hydrates leaderboard state with cursor', async () => {
    vi.mocked(localApi.getPayoutLeaderboard).mockResolvedValueOnce({
      data: [sampleRow],
      meta: {
        chain_id: '42161',
        from: '2026-01-01',
        to: '2026-01-31',
        valuation_version: 'v1',
        job_type: 'both',
        sort: 'commission_usd',
        next_cursor: 'c1',
      },
    });
    await payoutsService.refreshLeaderboard({ from: '2026-01-01', to: '2026-01-31' });
    const s = payoutsService.leaderboard;
    expect(s.rows).toHaveLength(1);
    expect(s.cursor).toBe('c1');
    expect(s.error).toBeNull();
  });

  it('records errors', async () => {
    vi.mocked(localApi.getPayoutLeaderboard).mockRejectedValueOnce(new Error('500'));
    await payoutsService.refreshLeaderboard({ from: '2026-01-01', to: '2026-01-02' });
    expect(payoutsService.leaderboard.error).toBe('500');
  });
});

describe('payoutsService.loadMoreLeaderboard', () => {
  it('appends rows and updates cursor', async () => {
    vi.mocked(localApi.getPayoutLeaderboard).mockResolvedValueOnce({
      data: [sampleRow],
      meta: { chain_id: '1', from: 'a', to: 'b', valuation_version: 'v1', job_type: 'both', sort: 'commission_usd', next_cursor: 'c1' },
    });
    await payoutsService.refreshLeaderboard({ from: 'a', to: 'b' });
    vi.mocked(localApi.getPayoutLeaderboard).mockResolvedValueOnce({
      data: [{ ...sampleRow, orchestrator_address: '0xb' }],
      meta: { chain_id: '1', from: 'a', to: 'b', valuation_version: 'v1', job_type: 'both', sort: 'commission_usd' },
    });
    await payoutsService.loadMoreLeaderboard();
    expect(payoutsService.leaderboard.rows).toHaveLength(2);
    expect(payoutsService.leaderboard.cursor).toBeNull();
  });
});

describe('payoutsService.loadSummary', () => {
  it('parallel-fetches summary and per-row leaderboard', async () => {
    vi.mocked(localApi.getPayoutSummary).mockResolvedValueOnce({
      period_start: '2026-01-15T00:00:00Z',
      period_end: '2026-01-16T00:00:00Z',
      valuation_version: 'v1',
      job_type: 'both',
      ticket_count: '10',
      sum_face_value_native: '2',
      sum_face_value_usd: '6000',
      sum_commission_native: '0.2',
      sum_commission_usd: '600',
      sum_delegators_share_native: '1.8',
      sum_delegators_share_usd: '5400',
      distinct_gateways: '5',
      usd_rows_priced: '10',
    });
    vi.mocked(localApi.getPayoutLeaderboard).mockResolvedValueOnce({
      data: [sampleRow],
      meta: { chain_id: '1', from: '2026-01-15', to: '2026-01-15', valuation_version: 'v1', job_type: 'both', sort: 'commission_usd' },
    });
    await payoutsService.loadSummary('daily', '2026-01-15', 'both');
    const s = payoutsService.summary;
    expect(s.summary?.ticket_count).toBe('10');
    expect(s.rows).toHaveLength(1);
    expect(s.error).toBeNull();
  });
});
