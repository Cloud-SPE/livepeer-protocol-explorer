import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Mock the local-api source so the dashboard's auto-fetch doesn't blow up
// against jsdom's lack of a real network.
vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    listOrchestrators: vi.fn().mockResolvedValue({ data: [], meta: { chain_id: '42161' } }),
    listGateways: vi.fn().mockResolvedValue({ data: [], meta: { chain_id: '42161' } }),
    listProposals: vi.fn().mockResolvedValue({ data: [] }),
    getPayoutSummary: vi.fn().mockResolvedValue({
      period_start: '', period_end: '', valuation_version: 'v1', job_type: 'both',
      ticket_count: '0', sum_face_value_native: '0', sum_face_value_usd: '0',
      sum_commission_native: '0', sum_commission_usd: '0',
      sum_delegators_share_native: '0', sum_delegators_share_usd: '0',
      distinct_gateways: '0', usd_rows_priced: '0',
    }),
    getNetworkStats: vi.fn().mockResolvedValue({
      chain_id: '42161',
      latest_round: '4192',
      latest_round_started_block: '460704938',
      latest_round_started_at: '2026-05-08T14:52:00Z',
      active_orchestrators: 101,
      total_lpt_staked: '27970000',
      gateways_known: 50,
      payouts_usd_24h: '1120',
      rewards_usd_24h: '9253',
      gas_burned_eth_24h: '0.005',
      orchestrator_profile_refreshed_at: '2026-05-09T08:00:00Z',
      broadcaster_profile_refreshed_at: '2026-05-09T07:59:45Z',
    }),
    getPayoutLeaderboard: vi.fn().mockResolvedValue({ data: [], meta: {} }),
  },
}));
vi.mock('../../lib/sources/ai-gateway.js', () => ({
  aiGateway: {
    networkCapabilities: vi.fn().mockResolvedValue({ orchestrators: [] }),
  },
}));

await import('../../views/dashboard.js');

describe('<view-dashboard>', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    host.remove();
    vi.clearAllMocks();
  });

  it('mounts and renders the five overview cards', async () => {
    const el = document.createElement('view-dashboard');
    host.appendChild(el);
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;

    const headings = Array.from(host.querySelectorAll('h3')).map((h) => h.textContent?.trim());
    expect(headings).toContain('Network totals');
    expect(headings).toContain('Top 5 orchestrators');
    expect(headings).toContain('Recent governance');
    expect(headings).toContain('Activity charts');
    expect(headings).toContain('AI capabilities');
  });

  it('renders a refresh-all button', async () => {
    const el = document.createElement('view-dashboard');
    host.appendChild(el);
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    const button = host.querySelector('refresh-button');
    expect(button).toBeTruthy();
    expect(button?.getAttribute('label')).toBe('Refresh all');
  });

  it('renders into light DOM', async () => {
    const el = document.createElement('view-dashboard');
    host.appendChild(el);
    await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
    expect((el as unknown as { shadowRoot: ShadowRoot | null }).shadowRoot).toBeNull();
  });
});
