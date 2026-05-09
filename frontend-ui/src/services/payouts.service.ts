import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import { ensService } from './ens.service.js';
import type {
  JobType,
  PayoutLeaderboardRow,
  PayoutSort,
  PayoutSummaryResponse,
  SummaryPeriod,
} from '../types/api.js';

function recordLeaderboard(rows: readonly PayoutLeaderboardRow[]): void {
  // Leaderboard rows carry orchestrator address + display_name + avatar_url —
  // perfect food for the ENS cache, just under a different key name.
  ensService.recordMany(
    rows.map((r) => ({
      address: r.orchestrator_address,
      display_name: r.display_name ?? null,
      avatar_url: r.avatar_url ?? null,
    })),
  );
}

interface LeaderboardState {
  rows: PayoutLeaderboardRow[];
  cursor: string | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
  from: string;
  to: string;
  jobType: JobType;
  sort: PayoutSort;
}

interface SummaryState {
  period: SummaryPeriod;
  date: string;
  jobType: JobType;
  summary: PayoutSummaryResponse | null;
  rows: PayoutLeaderboardRow[];
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const leaderboardInitial: LeaderboardState = {
  rows: [],
  cursor: null,
  loading: false,
  error: null,
  lastUpdated: null,
  from: '',
  to: '',
  jobType: 'both',
  sort: 'commission_usd',
};

const summaryInitial: SummaryState = {
  period: 'daily',
  date: '',
  jobType: 'both',
  summary: null,
  rows: [],
  loading: false,
  error: null,
  lastUpdated: null,
};

const _leaderboard$ = new BehaviorSubject<LeaderboardState>(leaderboardInitial);
const _summary$ = new BehaviorSubject<SummaryState>(summaryInitial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const payoutsService = {
  leaderboard$: _leaderboard$.asObservable() as Observable<LeaderboardState>,
  summary$: _summary$.asObservable() as Observable<SummaryState>,
  get leaderboard(): LeaderboardState { return _leaderboard$.getValue(); },
  get summary(): SummaryState { return _summary$.getValue(); },

  async refreshLeaderboard(params: {
    from: string;
    to: string;
    jobType?: JobType;
    sort?: PayoutSort;
  }): Promise<void> {
    const jobType = params.jobType ?? _leaderboard$.getValue().jobType;
    const sort = params.sort ?? _leaderboard$.getValue().sort;
    _leaderboard$.next({
      ...leaderboardInitial,
      from: params.from,
      to: params.to,
      jobType,
      sort,
      loading: true,
    });
    try {
      const { data, meta } = await localApi.getPayoutLeaderboard({
        from: params.from,
        to: params.to,
        job_type: jobType,
        sort,
        limit: 50,
      });
      recordLeaderboard(data);
      _leaderboard$.next({
        rows: data,
        cursor: meta.next_cursor ?? null,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
        from: params.from,
        to: params.to,
        jobType,
        sort,
      });
    } catch (err) {
      _leaderboard$.next({ ..._leaderboard$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  async loadMoreLeaderboard(): Promise<void> {
    const previous = _leaderboard$.getValue();
    if (!previous.cursor || previous.loading) return;
    _leaderboard$.next({ ...previous, loading: true, error: null });
    try {
      const { data, meta } = await localApi.getPayoutLeaderboard({
        from: previous.from,
        to: previous.to,
        job_type: previous.jobType,
        sort: previous.sort,
        cursor: previous.cursor,
        limit: 50,
      });
      recordLeaderboard(data);
      _leaderboard$.next({
        ...previous,
        rows: [...previous.rows, ...data],
        cursor: meta.next_cursor ?? null,
        loading: false,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _leaderboard$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  async loadSummary(period: SummaryPeriod, date: string, jobType: JobType = 'both'): Promise<void> {
    _summary$.next({ ...summaryInitial, period, date, jobType, loading: true });
    try {
      const [summary, leaderboard] = await Promise.all([
        localApi.getPayoutSummary(period, date, jobType).catch(() => null),
        localApi
          .getPayoutLeaderboard({ from: date, to: date, job_type: jobType, sort: 'commission_usd', limit: 50 })
          .catch(() => null),
      ]);
      if (leaderboard?.data) recordLeaderboard(leaderboard.data);
      _summary$.next({
        period,
        date,
        jobType,
        summary,
        rows: leaderboard?.data ?? [],
        loading: false,
        error: summary ? null : 'Failed to load summary',
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _summary$.next({ ...summaryInitial, period, date, jobType, loading: false, error: errMsg(err) });
    }
  },

  reset(): void {
    _leaderboard$.next(leaderboardInitial);
    _summary$.next(summaryInitial);
  },
};
