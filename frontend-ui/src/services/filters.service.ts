import { BehaviorSubject, type Observable } from 'rxjs';
import { STORAGE_KEYS, getItem, setItem } from '../lib/storage.js';
import type { JobType, PayoutSort, RewardSort } from '../types/api.js';

export interface PersistedFilters {
  jobType: JobType;
  payoutSort: PayoutSort;
  rewardSort: RewardSort;
  // Stored as ISO YYYY-MM-DD; "" means "default to today / 30-days-ago".
  rangeFrom: string;
  rangeTo: string;
}

const DEFAULT: PersistedFilters = {
  jobType: 'both',
  payoutSort: 'commission_usd',
  rewardSort: 'orch_tokens_usd',
  rangeFrom: '',
  rangeTo: '',
};

function readInitial(): PersistedFilters {
  const stored = getItem<Partial<PersistedFilters>>(STORAGE_KEYS.FILTERS, {});
  return { ...DEFAULT, ...stored };
}

const _state$ = new BehaviorSubject<PersistedFilters>(readInitial());

function persist(): void {
  setItem(STORAGE_KEYS.FILTERS, _state$.getValue());
}

export const filtersService = {
  state$: _state$.asObservable() as Observable<PersistedFilters>,
  get value(): PersistedFilters { return _state$.getValue(); },
  patch(p: Partial<PersistedFilters>): void {
    _state$.next({ ..._state$.getValue(), ...p });
    persist();
  },
  reset(): void {
    _state$.next(DEFAULT);
    persist();
  },
};
