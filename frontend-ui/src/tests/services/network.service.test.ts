import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getNetworkStats: vi.fn(),
    getRound: vi.fn(),
  },
}));

const { networkService } = await import('../../services/network.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getNetworkStats).mockReset();
  vi.mocked(localApi.getRound).mockReset();
});

describe('networkService', () => {
  it('fetches network stats', async () => {
    vi.mocked(localApi.getNetworkStats).mockResolvedValueOnce({
      chain_id: '42161',
      active_orchestrators: 0,
      total_lpt_staked: '0',
      gateways_known: 0,
      payouts_usd_24h: '0',
      rewards_usd_24h: '0',
      gas_burned_eth_24h: '0',
    });
    const result = await networkService.fetchNetworkStats();
    expect(result.chain_id).toBe('42161');
  });

  it('fetches one round', async () => {
    vi.mocked(localApi.getRound).mockResolvedValueOnce({
      round: '10',
      round_started_block: '100',
      round_started_at: '2026-05-09T00:00:00Z',
      active_orchestrators: 1,
      total_lpt_staked: '1',
      top_orchs: [],
      payouts_usd_on_day: '0',
      rewards_usd_on_day: '0',
      new_round_events: 1,
    });
    const result = await networkService.fetchRound(10);
    expect(result.round).toBe('10');
    expect(vi.mocked(localApi.getRound)).toHaveBeenCalledWith(10);
  });
});
