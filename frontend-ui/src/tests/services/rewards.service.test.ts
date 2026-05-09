import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getRewardLeaderboard: vi.fn(),
  },
}));

const { rewardsService } = await import('../../services/rewards.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getRewardLeaderboard).mockReset();
  rewardsService.reset();
});

const row = {
  orchestrator_address: '0xa',
  reward_event_count: '7',
  sum_total_tokens: '10',
  sum_total_tokens_usd: '40',
  sum_orch_tokens: '0.5',
  sum_orch_tokens_usd: '2',
  sum_delegators_tokens: '9.5',
  sum_delegators_tokens_usd: '38',
  usd_rows_priced: '7',
};

describe('rewardsService.refresh', () => {
  it('captures rows and meta', async () => {
    vi.mocked(localApi.getRewardLeaderboard).mockResolvedValueOnce({
      data: [row],
      meta: { chain_id: '1', from: 'a', to: 'b', valuation_version: 'v1', sort: 'orch_tokens_usd', next_cursor: 'c' },
    });
    await rewardsService.refresh({ from: 'a', to: 'b' });
    expect(rewardsService.leaderboard.rows).toHaveLength(1);
    expect(rewardsService.leaderboard.cursor).toBe('c');
  });

  it('records errors', async () => {
    vi.mocked(localApi.getRewardLeaderboard).mockRejectedValueOnce(new Error('boom'));
    await rewardsService.refresh({ from: 'a', to: 'b' });
    expect(rewardsService.leaderboard.error).toBe('boom');
  });
});

describe('rewardsService.loadMore', () => {
  it('appends and updates cursor', async () => {
    vi.mocked(localApi.getRewardLeaderboard).mockResolvedValueOnce({
      data: [row],
      meta: { chain_id: '1', from: 'a', to: 'b', valuation_version: 'v1', sort: 'orch_tokens_usd', next_cursor: 'c' },
    });
    await rewardsService.refresh({ from: 'a', to: 'b' });
    vi.mocked(localApi.getRewardLeaderboard).mockResolvedValueOnce({
      data: [{ ...row, orchestrator_address: '0xb' }],
      meta: { chain_id: '1', from: 'a', to: 'b', valuation_version: 'v1', sort: 'orch_tokens_usd' },
    });
    await rewardsService.loadMore();
    expect(rewardsService.leaderboard.rows).toHaveLength(2);
    expect(rewardsService.leaderboard.cursor).toBeNull();
  });
});
