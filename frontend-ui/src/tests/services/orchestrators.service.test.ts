import { describe, it, expect, vi, beforeEach } from 'vitest';
import { firstValueFrom } from 'rxjs';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    listOrchestrators: vi.fn(),
    getOrchestrator: vi.fn(),
    getTranscoderProfileAtBlock: vi.fn(),
    getTranscoderParamsHistory: vi.fn(),
    getTranscoderLifecycleHistory: vi.fn(),
    getOrchestratorTickets: vi.fn(),
  },
}));

const { orchestratorsService } = await import('../../services/orchestrators.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.listOrchestrators).mockReset();
  vi.mocked(localApi.getOrchestrator).mockReset();
  vi.mocked(localApi.getTranscoderProfileAtBlock).mockReset();
  vi.mocked(localApi.getTranscoderParamsHistory).mockReset();
  vi.mocked(localApi.getTranscoderLifecycleHistory).mockReset();
  vi.mocked(localApi.getOrchestratorTickets).mockReset();
  orchestratorsService.reset();
});

describe('orchestratorsService.refreshList', () => {
  it('populates list state from /orchestrators', async () => {
    vi.mocked(localApi.listOrchestrators).mockResolvedValueOnce({
      data: [
        {
          address: '0xa',
          total_stake: '100',
          fee_cut_percent: '10',
          fee_share_percent: '90',
          reward_cut_percent: '5',
          is_active: true,
          as_of_block: '1',
        },
      ],
      meta: { chain_id: '42161', next_cursor: 'next' },
    });
    await orchestratorsService.refreshList();
    const s = orchestratorsService.list;
    expect(s.rows).toHaveLength(1);
    expect(s.cursor).toBe('next');
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
    expect(s.lastUpdated).toBeTruthy();
  });

  it('preserves activeOnly toggle on refresh', async () => {
    vi.mocked(localApi.listOrchestrators).mockResolvedValueOnce({ data: [], meta: { chain_id: '1' } });
    await orchestratorsService.refreshList(true);
    expect(orchestratorsService.list.activeOnly).toBe(true);
    expect(vi.mocked(localApi.listOrchestrators).mock.calls[0]?.[0]).toMatchObject({ activeOnly: true });
  });

  it('records errors without losing existing state', async () => {
    vi.mocked(localApi.listOrchestrators).mockRejectedValueOnce(new Error('boom'));
    await orchestratorsService.refreshList();
    expect(orchestratorsService.list.error).toBe('boom');
    expect(orchestratorsService.list.loading).toBe(false);
  });
});

describe('orchestratorsService.loadMore', () => {
  it('appends rows and updates cursor', async () => {
    vi.mocked(localApi.listOrchestrators).mockResolvedValueOnce({
      data: [{ address: '0xa', total_stake: '1', fee_cut_percent: '0', fee_share_percent: '100', reward_cut_percent: '0', is_active: true, as_of_block: '1' }],
      meta: { chain_id: '1', next_cursor: 'c1' },
    });
    await orchestratorsService.refreshList();
    vi.mocked(localApi.listOrchestrators).mockResolvedValueOnce({
      data: [{ address: '0xb', total_stake: '2', fee_cut_percent: '0', fee_share_percent: '100', reward_cut_percent: '0', is_active: false, as_of_block: '2' }],
      meta: { chain_id: '1' },
    });
    await orchestratorsService.loadMore();
    const s = orchestratorsService.list;
    expect(s.rows).toHaveLength(2);
    expect(s.cursor).toBeNull();
  });

  it('does nothing when no cursor', async () => {
    await orchestratorsService.loadMore();
    expect(vi.mocked(localApi.listOrchestrators)).not.toHaveBeenCalled();
  });
});

describe('orchestratorsService.loadDetail', () => {
  it('aggregates parallel calls', async () => {
    vi.mocked(localApi.getOrchestrator).mockResolvedValueOnce({
      address: '0xa',
      total_stake: '1',
      fee_cut_percent: '0',
      fee_share_percent: '100',
      reward_cut_percent: '0',
      is_active: true,
      as_of_block: '1',
    });
    vi.mocked(localApi.getTranscoderProfileAtBlock).mockResolvedValueOnce({
      transcoder_address: '0xa',
      block_number: '1',
    });
    vi.mocked(localApi.getTranscoderParamsHistory).mockResolvedValueOnce({ data: [] });
    vi.mocked(localApi.getTranscoderLifecycleHistory).mockResolvedValueOnce({ data: [] });
    vi.mocked(localApi.getOrchestratorTickets).mockResolvedValueOnce({ data: [] });

    await orchestratorsService.loadDetail('0xa');
    const d = orchestratorsService.detail;
    expect(d.address).toBe('0xa');
    expect(d.profile?.address).toBe('0xa');
    expect(d.error).toBeNull();
  });
});

describe('orchestratorsService streams', () => {
  it('emits initial state on subscribe', async () => {
    const v = await firstValueFrom(orchestratorsService.list$);
    expect(v.rows).toEqual([]);
    expect(v.loading).toBe(false);
  });
});
