import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import type {
  ProposalRow,
  ProposalStatus,
  VoteListResponse,
  VoteRow,
} from '../types/api.js';

interface ProposalsState {
  rows: ProposalRow[];
  status: ProposalStatus;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

interface ProposalDetailState {
  id: string | null;
  proposal: ProposalRow | null;
  votes: VoteListResponse | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

interface VotesState {
  rows: VoteRow[];
  cursor: string | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const proposalsInitial: ProposalsState = {
  rows: [],
  status: 'all',
  loading: false,
  error: null,
  lastUpdated: null,
};

const proposalDetailInitial: ProposalDetailState = {
  id: null,
  proposal: null,
  votes: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const votesInitial: VotesState = {
  rows: [],
  cursor: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _proposals$ = new BehaviorSubject<ProposalsState>(proposalsInitial);
const _detail$ = new BehaviorSubject<ProposalDetailState>(proposalDetailInitial);
const _votes$ = new BehaviorSubject<VotesState>(votesInitial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const governanceService = {
  proposals$: _proposals$.asObservable() as Observable<ProposalsState>,
  detail$: _detail$.asObservable() as Observable<ProposalDetailState>,
  votes$: _votes$.asObservable() as Observable<VotesState>,
  get proposals(): ProposalsState { return _proposals$.getValue(); },
  get detail(): ProposalDetailState { return _detail$.getValue(); },
  get votes(): VotesState { return _votes$.getValue(); },

  async refreshProposals(status?: ProposalStatus): Promise<void> {
    const previous = _proposals$.getValue();
    const s = status ?? previous.status;
    _proposals$.next({ ...proposalsInitial, status: s, loading: true });
    try {
      const { data } = await localApi.listProposals({ status: s, limit: 100 });
      _proposals$.next({
        rows: data,
        status: s,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _proposals$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  async loadDetail(id: string): Promise<void> {
    _detail$.next({ ...proposalDetailInitial, id, loading: true });
    try {
      const [proposal, votes] = await Promise.all([
        localApi.getProposal(id).catch(() => null),
        localApi.listVotes({ proposal_id: id, limit: 200 }).catch(() => null),
      ]);
      _detail$.next({
        id,
        proposal,
        votes,
        loading: false,
        error: proposal ? null : 'Failed to load proposal',
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _detail$.next({ ...proposalDetailInitial, id, loading: false, error: errMsg(err) });
    }
  },

  async refreshVotes(): Promise<void> {
    _votes$.next({ ...votesInitial, loading: true });
    try {
      const { data, next_cursor } = await localApi.listVotes({ limit: 100 });
      _votes$.next({
        rows: data,
        cursor: next_cursor ?? null,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _votes$.next({ ..._votes$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  async loadMoreVotes(): Promise<void> {
    const previous = _votes$.getValue();
    if (!previous.cursor || previous.loading) return;
    _votes$.next({ ...previous, loading: true, error: null });
    try {
      const { data, next_cursor } = await localApi.listVotes({ cursor: previous.cursor, limit: 100 });
      _votes$.next({
        ...previous,
        rows: [...previous.rows, ...data],
        cursor: next_cursor ?? null,
        loading: false,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _votes$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  reset(): void {
    _proposals$.next(proposalsInitial);
    _detail$.next(proposalDetailInitial);
    _votes$.next(votesInitial);
  },
};

export function supportLabel(support: string): string {
  switch (support) {
    case '1': return 'For';
    case '0': return 'Against';
    case '2': return 'Abstain';
    default: return support;
  }
}

export function proposalTitle(p: ProposalRow): string {
  if (!p.description) return `Proposal ${p.proposal_id}`;
  const firstLine = p.description.split('\n')[0]?.trim() ?? '';
  if (!firstLine) return `Proposal ${p.proposal_id}`;
  return firstLine.replace(/^#+\s*/, '').slice(0, 140);
}

export type ProposalOutcome = 'passed' | 'defeated' | 'active';

/** Parse a uint256-shaped weight (raw integer or decimal-formatted string)
 *  into a BigInt for exact comparison. Returns 0n if unparseable. */
function toBigIntWeight(value: string | undefined | null): bigint {
  if (!value) return 0n;
  const intPart = value.includes('.') ? value.split('.')[0] : value;
  if (!intPart) return 0n;
  try { return BigInt(intPart); } catch { return 0n; }
}

/**
 * Categorize a proposal as passed / defeated / active.
 *   - executed=true                                          → passed
 *   - votes cast AND against > for (strict)                  → defeated
 *   - otherwise                                              → active
 * Abstain-only or for=against=0 cases stay 'active', not 'defeated'.
 */
export function proposalOutcome(p: ProposalRow): ProposalOutcome {
  if (p.executed) return 'passed';
  const t = p.vote_tally;
  const forW = toBigIntWeight(t?.for_weight);
  const againstW = toBigIntWeight(t?.against_weight);
  if (forW > 0n || againstW > 0n) {
    if (againstW > forW) return 'defeated';
  }
  return 'active';
}
