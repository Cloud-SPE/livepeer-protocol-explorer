import { BehaviorSubject, type Observable } from 'rxjs';
import { catalogSource } from '../lib/sources/catalog.js';
import type { PipelineEntry, RegionEntry } from '../types/external.js';

interface CatalogState {
  regions: RegionEntry[];
  pipelines: PipelineEntry[];
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const initial: CatalogState = {
  regions: [],
  pipelines: [],
  loading: false,
  error: null,
  lastUpdated: null,
};

const _state$ = new BehaviorSubject<CatalogState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const catalogService = {
  state$: _state$.asObservable() as Observable<CatalogState>,
  get state(): CatalogState { return _state$.getValue(); },

  async load(): Promise<void> {
    if (_state$.getValue().loading) return;
    _state$.next({ ..._state$.getValue(), loading: true, error: null });
    try {
      const [regions, pipelines] = await Promise.all([
        catalogSource.fetchRegions().catch(() => ({ regions: [] })),
        catalogSource.fetchPipelines().catch(() => ({ pipelines: [] })),
      ]);
      _state$.next({
        regions: regions.regions,
        pipelines: pipelines.pipelines,
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
