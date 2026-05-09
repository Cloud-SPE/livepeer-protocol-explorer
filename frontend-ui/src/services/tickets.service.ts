import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import type { JobType, TicketsTimeseriesResponse } from '../types/api.js';

interface TicketsState {
  data: TicketsTimeseriesResponse | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
  start: string;
  end: string;
  jobType: JobType;
}

const initial: TicketsState = {
  data: null,
  loading: false,
  error: null,
  lastUpdated: null,
  start: '',
  end: '',
  jobType: 'both',
};

const _state$ = new BehaviorSubject<TicketsState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const ticketsService = {
  state$: _state$.asObservable() as Observable<TicketsState>,
  get state(): TicketsState { return _state$.getValue(); },

  async refresh(params: { start: string; end: string; jobType?: JobType }): Promise<void> {
    const jobType = params.jobType ?? _state$.getValue().jobType;
    _state$.next({ ...initial, start: params.start, end: params.end, jobType, loading: true });
    try {
      const data = await localApi.getTicketsTimeseries({
        start: params.start,
        end: params.end,
        job_type: jobType,
      });
      _state$.next({
        data,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
        start: params.start,
        end: params.end,
        jobType,
      });
    } catch (err) {
      _state$.next({ ..._state$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  reset(): void { _state$.next(initial); },
};
