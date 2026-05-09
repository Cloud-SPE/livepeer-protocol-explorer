import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import { ensService } from './ens.service.js';
import type { RewardLeaderboardRow, RewardSort } from '../types/api.js';

function recordLeaderboard(rows: readonly RewardLeaderboardRow[]): void {
  ensService.recordMany(
    rows.map((r) => ({
      address: r.orchestrator_address,
      display_name: r.display_name ?? null,
      avatar_url: r.avatar_url ?? null,
    })),
  );
}

interface LeaderboardState {
  rows: RewardLeaderboardRow[];
  cursor: string | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
  from: string;
  to: string;
  sort: RewardSort;
}

const initial: LeaderboardState = {
  rows: [],
  cursor: null,
  loading: false,
  error: null,
  lastUpdated: null,
  from: '',
  to: '',
  sort: 'orch_tokens_usd',
};

const _state$ = new BehaviorSubject<LeaderboardState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const rewardsService = {
  leaderboard$: _state$.asObservable() as Observable<LeaderboardState>,
  get leaderboard(): LeaderboardState { return _state$.getValue(); },

  async refresh(params: { from: string; to: string; sort?: RewardSort }): Promise<void> {
    const sort = params.sort ?? _state$.getValue().sort;
    _state$.next({ ...initial, from: params.from, to: params.to, sort, loading: true });
    try {
      const { data, meta } = await localApi.getRewardLeaderboard({
        from: params.from,
        to: params.to,
        sort,
        limit: 50,
      });
      recordLeaderboard(data);
      _state$.next({
        rows: data,
        cursor: meta.next_cursor ?? null,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
        from: params.from,
        to: params.to,
        sort,
      });
    } catch (err) {
      _state$.next({ ..._state$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  async loadMore(): Promise<void> {
    const previous = _state$.getValue();
    if (!previous.cursor || previous.loading) return;
    _state$.next({ ...previous, loading: true, error: null });
    try {
      const { data, meta } = await localApi.getRewardLeaderboard({
        from: previous.from,
        to: previous.to,
        sort: previous.sort,
        cursor: previous.cursor,
        limit: 50,
      });
      recordLeaderboard(data);
      _state$.next({
        ...previous,
        rows: [...previous.rows, ...data],
        cursor: meta.next_cursor ?? null,
        loading: false,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _state$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  reset(): void { _state$.next(initial); },
};
