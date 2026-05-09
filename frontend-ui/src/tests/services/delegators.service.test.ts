import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    getDelegator: vi.fn(),
  },
}));

const { delegatorsService } = await import('../../services/delegators.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.getDelegator).mockReset();
});

describe('delegatorsService', () => {
  it('delegates to localApi.getDelegator', async () => {
    vi.mocked(localApi.getDelegator).mockResolvedValueOnce({
      delegator_address: '0xd',
      is_active: true,
      first_bond_block: '1',
      last_seen_block: '2',
      delegations: [],
      chain_id: '42161',
    });
    const result = await delegatorsService.fetchDelegator('0xd');
    expect(result.delegator_address).toBe('0xd');
    expect(vi.mocked(localApi.getDelegator)).toHaveBeenCalledWith('0xd');
  });
});
