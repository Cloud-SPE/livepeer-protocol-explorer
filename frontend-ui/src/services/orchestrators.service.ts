import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import { ensService } from './ens.service.js';
import type {
  CutsHistoryResponse,
  NetEconomicsResponse,
  OrchestratorProfileRow,
  TicketHistoryResponse,
  TranscoderLifecycleHistoryResponse,
  TranscoderParamsHistoryResponse,
  TranscoderProfileResponse,
} from '../types/api.js';

interface ListState {
  rows: OrchestratorProfileRow[];
  cursor: string | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
  activeOnly: boolean;
}

interface DetailState {
  address: string | null;
  profile: OrchestratorProfileRow | null;
  blockProfile: TranscoderProfileResponse | null;
  paramsHistory: TranscoderParamsHistoryResponse | null;
  lifecycleHistory: TranscoderLifecycleHistoryResponse | null;
  tickets: TicketHistoryResponse | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const listInitial: ListState = {
  rows: [],
  cursor: null,
  loading: false,
  error: null,
  lastUpdated: null,
  activeOnly: false,
};

const detailInitial: DetailState = {
  address: null,
  profile: null,
  blockProfile: null,
  paramsHistory: null,
  lifecycleHistory: null,
  tickets: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _list$ = new BehaviorSubject<ListState>(listInitial);
const _detail$ = new BehaviorSubject<DetailState>(detailInitial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const orchestratorsService = {
  list$: _list$.asObservable() as Observable<ListState>,
  detail$: _detail$.asObservable() as Observable<DetailState>,
  get list(): ListState { return _list$.getValue(); },
  get detail(): DetailState { return _detail$.getValue(); },

  async refreshList(activeOnly?: boolean): Promise<void> {
    const previous = _list$.getValue();
    const ao = activeOnly ?? previous.activeOnly;
    _list$.next({ ...listInitial, activeOnly: ao, loading: true });
    try {
      const { data, meta } = await localApi.listOrchestrators({ activeOnly: ao, limit: 250 });
      ensService.recordMany(data);
      _list$.next({
        rows: data,
        cursor: meta.next_cursor ?? null,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
        activeOnly: ao,
      });
    } catch (err) {
      _list$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  async loadMore(): Promise<void> {
    const previous = _list$.getValue();
    if (!previous.cursor || previous.loading) return;
    _list$.next({ ...previous, loading: true, error: null });
    try {
      const { data, meta } = await localApi.listOrchestrators({
        cursor: previous.cursor,
        activeOnly: previous.activeOnly,
        limit: 250,
      });
      ensService.recordMany(data);
      _list$.next({
        ...previous,
        rows: [...previous.rows, ...data],
        cursor: meta.next_cursor ?? null,
        loading: false,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _list$.next({ ...previous, loading: false, error: errMsg(err) });
    }
  },

  async loadDetail(address: string): Promise<void> {
    _detail$.next({ ...detailInitial, address, loading: true });
    try {
      const [profile, blockProfile, paramsHistory, lifecycleHistory, tickets] = await Promise.all([
        localApi.getOrchestrator(address).catch(() => null),
        localApi.getTranscoderProfileAtBlock(address, 'latest').catch(() => null),
        localApi.getTranscoderParamsHistory(address, { limit: 25 }).catch(() => null),
        localApi.getTranscoderLifecycleHistory(address, { limit: 25 }).catch(() => null),
        localApi.getOrchestratorTickets(address, { limit: 25 }).catch(() => null),
      ]);
      if (profile) ensService.recordMany([profile]);
      _detail$.next({
        address,
        profile,
        blockProfile,
        paramsHistory,
        lifecycleHistory,
        tickets,
        loading: false,
        error: profile ? null : 'Failed to load orchestrator',
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _detail$.next({
        ...detailInitial,
        address,
        loading: false,
        error: errMsg(err),
      });
    }
  },

  /**
   * One-shot pull of the full orchestrators list into the ENS cache. Used by
   * views (governance votes / proposal detail) that render arbitrary
   * addresses and want names/avatars for any of them that happen to be
   * orchestrators. Does NOT mutate the list state — that stays paginated
   * and view-driven. Re-runs are cheap because of the `_warmed` flag.
   */
  async warmEnsCache(): Promise<void> {
    if (_ensWarmed || _ensWarming) return;
    _ensWarming = true;
    try {
      let cursor: string | undefined;
      let pages = 0;
      // Safety bound — at limit=1000 we expect ≤2 pages even on a busy network.
      while (pages < 5) {
        const params: { limit: number; cursor?: string } = { limit: 1000 };
        if (cursor) params.cursor = cursor;
        const { data, meta } = await localApi.listOrchestrators(params);
        ensService.recordMany(data);
        if (!meta.next_cursor) break;
        cursor = meta.next_cursor;
        pages += 1;
      }
      _ensWarmed = true;
    } catch {
      /* leave _ensWarmed false so a later call can retry */
    } finally {
      _ensWarming = false;
    }
  },

  reset(): void {
    _list$.next(listInitial);
    _detail$.next(detailInitial);
    _ensWarmed = false;
  },

  fetchCutsHistory(address: string): Promise<CutsHistoryResponse> {
    return localApi.getCutsHistory(address);
  },

  fetchNetEconomics(address: string, periodDays?: number): Promise<NetEconomicsResponse> {
    const params: { periodDays?: number } = {};
    if (periodDays !== undefined) params.periodDays = periodDays;
    return localApi.getNetEconomics(address, params);
  },
};

let _ensWarmed = false;
let _ensWarming = false;
