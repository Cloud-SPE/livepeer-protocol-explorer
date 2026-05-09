import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getStakeHistory: vi.fn(),
  },
}));

const { stakeHistoryService } = await import('../../services/stake-history.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getStakeHistory).mockReset();
});

describe('stakeHistoryService', () => {
  it('delegates to localApi.getStakeHistory', async () => {
    vi.mocked(localApi.getStakeHistory).mockResolvedValueOnce({
      address: '0xa',
      data: [],
      meta: { chain_id: '42161' },
    });
    const result = await stakeHistoryService.fetchStakeHistory('0xa', 10, 20);
    expect(result.address).toBe('0xa');
    expect(vi.mocked(localApi.getStakeHistory)).toHaveBeenCalledWith('0xa', { fromRound: 10, toRound: 20 });
  });
});
