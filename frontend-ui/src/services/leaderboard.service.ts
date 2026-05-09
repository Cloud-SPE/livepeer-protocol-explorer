import { BehaviorSubject, type Observable } from 'rxjs';
import { leaderboardSource } from '../lib/sources/leaderboard.js';
import type { LeaderboardResponse } from '../types/external.js';

export type LeaderboardKind = 'transcoding' | 'ai';

interface LeaderboardState {
  kind: LeaderboardKind;
  region: string;
  pipeline: string;
  model: string;
  data: LeaderboardResponse | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const initial: LeaderboardState = {
  kind: 'transcoding',
  region: 'GLOBAL',
  pipeline: '',
  model: '',
  data: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _state$ = new BehaviorSubject<LeaderboardState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const leaderboardService = {
  state$: _state$.asObservable() as Observable<LeaderboardState>,
  get state(): LeaderboardState { return _state$.getValue(); },

  async refresh(params: {
    kind: LeaderboardKind;
    region?: string;
    pipeline?: string;
    model?: string;
  }): Promise<void> {
    const previous = _state$.getValue();
    const region = params.region ?? previous.region;
    const pipeline = params.pipeline ?? previous.pipeline;
    const model = params.model ?? previous.model;
    _state$.next({
      ...initial,
      kind: params.kind,
      region,
      pipeline,
      model,
      loading: true,
    });
    try {
      const data =
        params.kind === 'ai'
          ? await leaderboardSource.fetchAi(
              region === 'GLOBAL' ? { pipeline, model } : { pipeline, model, region },
            )
          : await leaderboardSource.fetchTranscoding(region === 'GLOBAL' ? undefined : region);
      _state$.next({
        kind: params.kind,
        region,
        pipeline,
        model,
        data,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _state$.next({ ..._state$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  reset(): void { _state$.next(initial); },
};

export interface LeaderboardOrchRow {
  orchestrator: string;
  avg_score: number;
  avg_success_rate: number;
  avg_round_trip: number;
  region_count: number;
  regions: string[];
}

/** Aggregate the response into per-orchestrator averages for a flat table view. */
export function summarize(data: LeaderboardResponse | null): LeaderboardOrchRow[] {
  if (!data) return [];
  const rows: LeaderboardOrchRow[] = [];
  for (const [orch, byRegion] of Object.entries(data)) {
    const regions = Object.keys(byRegion);
    if (regions.length === 0) continue;
    let sumScore = 0;
    let sumSuccess = 0;
    let sumRoundTrip = 0;
    for (const r of regions) {
      const s = byRegion[r];
      if (!s) continue;
      sumScore += s.score;
      sumSuccess += s.success_rate;
      sumRoundTrip += s.round_trip_score;
    }
    const n = regions.length;
    rows.push({
      orchestrator: orch,
      avg_score: sumScore / n,
      avg_success_rate: sumSuccess / n,
      avg_round_trip: sumRoundTrip / n,
      region_count: n,
      regions,
    });
  }
  rows.sort((a, b) => b.avg_score - a.avg_score);
  return rows;
}
