import { BehaviorSubject, type Observable } from 'rxjs';
import { localApi } from '../lib/sources/local-api.js';
import { ensService } from './ens.service.js';
import type {
  GatewayAnalyticsSummaryResponse,
  GatewayBalanceHistoryResponse,
  GatewayBalanceRow,
  GatewayProfileRow,
  GatewayRecipientsResponse,
} from '../types/api.js';

interface ListState {
  rows: GatewayProfileRow[];
  cursor: string | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

interface DetailState {
  address: string | null;
  profile: GatewayProfileRow | null;
  balance: GatewayBalanceRow | null;
  balanceHistory: GatewayBalanceHistoryResponse | null;
  recipients: GatewayRecipientsResponse | null;
  analytics: GatewayAnalyticsSummaryResponse | null;
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
};

const detailInitial: DetailState = {
  address: null,
  profile: null,
  balance: null,
  balanceHistory: null,
  recipients: null,
  analytics: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _list$ = new BehaviorSubject<ListState>(listInitial);
const _detail$ = new BehaviorSubject<DetailState>(detailInitial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const gatewaysService = {
  list$: _list$.asObservable() as Observable<ListState>,
  detail$: _detail$.asObservable() as Observable<DetailState>,
  get list(): ListState { return _list$.getValue(); },
  get detail(): DetailState { return _detail$.getValue(); },

  async refreshList(): Promise<void> {
    _list$.next({ ...listInitial, loading: true });
    try {
      const { data, meta } = await localApi.listGateways({ limit: 100 });
      ensService.recordMany(data);
      _list$.next({
        rows: data,
        cursor: meta.next_cursor ?? null,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _list$.next({ ..._list$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  async loadMore(): Promise<void> {
    const previous = _list$.getValue();
    if (!previous.cursor || previous.loading) return;
    _list$.next({ ...previous, loading: true, error: null });
    try {
      const { data, meta } = await localApi.listGateways({ cursor: previous.cursor, limit: 100 });
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
      const [profile, balance, balanceHistory, recipients, analytics] = await Promise.all([
        localApi.getGatewayProfile(address).catch(() => null),
        localApi.getGatewayBalanceLatest(address).catch(() => null),
        localApi.getGatewayBalanceHistory(address, { limit: 50 }).catch(() => null),
        localApi.getGatewayRecipients(address, { limit: 25 }).catch(() => null),
        localApi.getGatewayAnalyticsSummary(address, { days: 7 }).catch(() => null),
      ]);
      if (profile) ensService.recordMany([profile]);
      // Recipients land with `recipient_address` rather than `address`; remap so
      // they slot into the cache too. Names/avatars only flow through if the
      // backend supplied them.
      if (recipients?.data) {
        ensService.recordMany(
          recipients.data.map((r) => ({
            address: r.recipient_address,
          })),
        );
      }
      _detail$.next({
        address,
        profile,
        balance,
        balanceHistory,
        recipients,
        analytics,
        loading: false,
        error: profile ? null : 'Failed to load gateway',
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _detail$.next({ ...detailInitial, address, loading: false, error: errMsg(err) });
    }
  },

  reset(): void {
    _list$.next(listInitial);
    _detail$.next(detailInitial);
  },
};
