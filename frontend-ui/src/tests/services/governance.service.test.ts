import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/local-api.js', () => ({
  localApi: {
    listProposals: vi.fn(),
    getProposal: vi.fn(),
    listVotes: vi.fn(),
  },
}));

const { governanceService, proposalTitle, supportLabel } = await import('../../services/governance.service.js');
const { localApi } = await import('../../lib/sources/local-api.js');

const sampleProposal = {
  proposal_id: '0xabc',
  proposer: '0xdeadbeef',
  vote_start: '100',
  vote_end: '200',
  description: '# Proposal title\nDetails go here',
  created_block: '90',
  created_at: '2026-01-15T10:00:00Z',
  created_tx_hash: '0xtx',
  executed: true,
  executed_block: '210',
  executed_at: '2026-01-20T10:00:00Z',
  vote_tally: {
    against_weight: '1000000000000000000',
    for_weight: '5000000000000000000',
    abstain_weight: '500000000000000000',
    vote_count: '7',
  },
};

beforeEach(() => {
  vi.mocked(localApi.listProposals).mockReset();
  vi.mocked(localApi.getProposal).mockReset();
  vi.mocked(localApi.listVotes).mockReset();
  governanceService.reset();
});

describe('proposalTitle', () => {
  it('returns the first markdown line stripped of #', () => {
    expect(proposalTitle({ ...sampleProposal })).toBe('Proposal title');
  });

  it('falls back to the proposal id when description is missing', () => {
    expect(proposalTitle({ ...sampleProposal, description: '' })).toBe('Proposal 0xabc');
  });
});

describe('supportLabel', () => {
  it('maps the Solidity enum strings', () => {
    expect(supportLabel('1')).toBe('For');
    expect(supportLabel('0')).toBe('Against');
    expect(supportLabel('2')).toBe('Abstain');
  });
});

describe('governanceService.refreshProposals', () => {
  it('hydrates proposals state', async () => {
    vi.mocked(localApi.listProposals).mockResolvedValueOnce({ data: [sampleProposal] });
    await governanceService.refreshProposals('all');
    const s = governanceService.proposals;
    expect(s.rows).toHaveLength(1);
    expect(s.status).toBe('all');
    expect(s.error).toBeNull();
  });

  it('preserves status across refresh', async () => {
    vi.mocked(localApi.listProposals).mockResolvedValueOnce({ data: [] });
    await governanceService.refreshProposals('active');
    expect(governanceService.proposals.status).toBe('active');
    expect(vi.mocked(localApi.listProposals).mock.calls[0]?.[0]).toMatchObject({ status: 'active' });
  });

  it('records errors', async () => {
    vi.mocked(localApi.listProposals).mockRejectedValueOnce(new Error('500'));
    await governanceService.refreshProposals();
    expect(governanceService.proposals.error).toBe('500');
  });
});

describe('governanceService.loadDetail', () => {
  it('fetches proposal + votes in parallel', async () => {
    vi.mocked(localApi.getProposal).mockResolvedValueOnce(sampleProposal);
    vi.mocked(localApi.listVotes).mockResolvedValueOnce({
      data: [],
      meta: { domain: 'governor', backfill_complete: true },
    });
    await governanceService.loadDetail('0xabc');
    const s = governanceService.detail;
    expect(s.id).toBe('0xabc');
    expect(s.proposal?.proposal_id).toBe('0xabc');
    expect(s.error).toBeNull();
  });
});

describe('governanceService.refreshVotes', () => {
  it('paginates with cursor', async () => {
    vi.mocked(localApi.listVotes).mockResolvedValueOnce({
      data: [
        {
          event_id: 'e1',
          event_name: 'VoteCast',
          proposal_id: '0xabc',
          voter: '0xv',
          support: '1',
          weight: '1',
          block_number: '1',
          block_timestamp: '2026-01-15T10:00:00Z',
          tx_hash: '0xtx',
        },
      ],
      next_cursor: 'cur',
      meta: { domain: 'governor', backfill_complete: true },
    });
    await governanceService.refreshVotes();
    expect(governanceService.votes.rows).toHaveLength(1);
    expect(governanceService.votes.cursor).toBe('cur');

    vi.mocked(localApi.listVotes).mockResolvedValueOnce({
      data: [
        {
          event_id: 'e2',
          event_name: 'VoteCast',
          proposal_id: '0xdef',
          voter: '0xv2',
          support: '0',
          weight: '2',
          block_number: '2',
          block_timestamp: '2026-01-16T10:00:00Z',
          tx_hash: '0xtx2',
        },
      ],
      meta: { domain: 'governor', backfill_complete: true },
    });
    await governanceService.loadMoreVotes();
    expect(governanceService.votes.rows).toHaveLength(2);
    expect(governanceService.votes.cursor).toBeNull();
  });
});
