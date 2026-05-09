import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    listGateways: vi.fn(),
    getGatewayProfile: vi.fn(),
    getGatewayBalanceLatest: vi.fn(),
    getGatewayBalanceHistory: vi.fn(),
    getGatewayRecipients: vi.fn(),
    getGatewayAnalyticsSummary: vi.fn(),
  },
}));

const { gatewaysService } = await import('../../services/gateways.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

beforeEach(() => {
  vi.mocked(localApi.listGateways).mockReset();
  vi.mocked(localApi.getGatewayProfile).mockReset();
  vi.mocked(localApi.getGatewayBalanceLatest).mockReset();
  vi.mocked(localApi.getGatewayBalanceHistory).mockReset();
  vi.mocked(localApi.getGatewayRecipients).mockReset();
  vi.mocked(localApi.getGatewayAnalyticsSummary).mockReset();
  gatewaysService.reset();
});

describe('gatewaysService.refreshList', () => {
  it('hydrates list state', async () => {
    vi.mocked(localApi.listGateways).mockResolvedValueOnce({
      data: [
        {
          address: '0xg',
          kind: 'gateway',
          latest_deposit: '1',
          latest_reserve: '2',
          unlock_in_progress: false,
          as_of_block: '10',
        },
      ],
      meta: { chain_id: '42161' },
    });
    await gatewaysService.refreshList();
    expect(gatewaysService.list.rows).toHaveLength(1);
    expect(gatewaysService.list.error).toBeNull();
  });

  it('captures errors', async () => {
    vi.mocked(localApi.listGateways).mockRejectedValueOnce(new Error('http_500'));
    await gatewaysService.refreshList();
    expect(gatewaysService.list.error).toBe('http_500');
  });
});

describe('gatewaysService.loadDetail', () => {
  it('aggregates parallel responses', async () => {
    vi.mocked(localApi.getGatewayProfile).mockResolvedValueOnce({
      address: '0xg',
      kind: 'gateway',
      latest_deposit: '1',
      latest_reserve: '2',
      unlock_in_progress: false,
      as_of_block: '1',
    });
    vi.mocked(localApi.getGatewayBalanceLatest).mockResolvedValueOnce({
      gateway_address: '0xg',
      block_number: '1',
      deposit: '1',
      reserve_funds_remaining: '2',
      reserve_claimed_in_current_round: '0',
      withdraw_round: '0',
      unlock_in_progress: false,
      source: 'indexed',
    });
    vi.mocked(localApi.getGatewayBalanceHistory).mockResolvedValueOnce({
      gateway_address: '0xg',
      data: [],
    });
    vi.mocked(localApi.getGatewayRecipients).mockResolvedValueOnce({
      gateway_address: '0xg',
      semantics: 'net',
      data: [],
    });
    vi.mocked(localApi.getGatewayAnalyticsSummary).mockResolvedValueOnce({
      gateway_address: '0xg',
      days: '7',
      semantics: 'net',
      from_timestamp: '2026-01-01T00:00:00Z',
      to_timestamp: '2026-01-08T00:00:00Z',
      funding: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      payouts: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      withdrawals: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      distinct_recipients: '0',
      data: [],
    });
    await gatewaysService.loadDetail('0xg');
    const d = gatewaysService.detail;
    expect(d.address).toBe('0xg');
    expect(d.profile?.address).toBe('0xg');
    expect(d.balance?.deposit).toBe('1');
  });

  it('survives a missing profile gracefully', async () => {
    vi.mocked(localApi.getGatewayProfile).mockRejectedValueOnce(new Error('404'));
    vi.mocked(localApi.getGatewayBalanceLatest).mockResolvedValueOnce(null as never);
    vi.mocked(localApi.getGatewayBalanceHistory).mockResolvedValueOnce({ gateway_address: '0xg', data: [] });
    vi.mocked(localApi.getGatewayRecipients).mockResolvedValueOnce({ gateway_address: '0xg', semantics: 'net', data: [] });
    vi.mocked(localApi.getGatewayAnalyticsSummary).mockResolvedValueOnce({
      gateway_address: '0xg',
      days: '7',
      semantics: 'net',
      from_timestamp: '',
      to_timestamp: '',
      funding: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      payouts: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      withdrawals: { count: '0', total_amount_native: '0', total_amount_usd: '0', usd_rows_priced: '0' },
      distinct_recipients: '0',
      data: [],
    });
    await gatewaysService.loadDetail('0xg');
    expect(gatewaysService.detail.profile).toBeNull();
    expect(gatewaysService.detail.error).toBe('Failed to load gateway');
  });
});
