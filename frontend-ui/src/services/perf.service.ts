import { BehaviorSubject, type Observable } from 'rxjs';
import { perfStatsSource } from '../lib/sources/perf-stats.js';
import type { PerfStatsResponse } from '../types/external.js';
import type { LeaderboardKind } from './leaderboard.service.js';

interface PerfState {
  kind: LeaderboardKind;
  orchestrator: string;
  pipeline: string;
  model: string;
  data: PerfStatsResponse | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const initial: PerfState = {
  kind: 'transcoding',
  orchestrator: '',
  pipeline: '',
  model: '',
  data: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _state$ = new BehaviorSubject<PerfState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const perfService = {
  state$: _state$.asObservable() as Observable<PerfState>,
  get state(): PerfState { return _state$.getValue(); },

  async refresh(params: {
    kind: LeaderboardKind;
    orchestrator: string;
    pipeline?: string;
    model?: string;
  }): Promise<void> {
    if (!params.orchestrator) return;
    _state$.next({
      ...initial,
      kind: params.kind,
      orchestrator: params.orchestrator,
      pipeline: params.pipeline ?? '',
      model: params.model ?? '',
      loading: true,
    });
    try {
      const data =
        params.kind === 'ai'
          ? await perfStatsSource.fetchAi({
              orchestrator: params.orchestrator,
              ...(params.pipeline ? { pipeline: params.pipeline } : {}),
              ...(params.model ? { model: params.model } : {}),
            })
          : await perfStatsSource.fetchTranscoding(params.orchestrator);
      _state$.next({
        kind: params.kind,
        orchestrator: params.orchestrator,
        pipeline: params.pipeline ?? '',
        model: params.model ?? '',
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
