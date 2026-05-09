import { BehaviorSubject, type Observable } from 'rxjs';
import { STORAGE_KEYS, getItem, setItem } from '../lib/storage.js';
import type { HistoryEntry, Modality } from '../types/playground.js';

const MAX_PER_MODALITY = 10;

type HistoryMap = Partial<Record<Modality, HistoryEntry[]>>;

function readInitial(): HistoryMap {
  return getItem<HistoryMap>(STORAGE_KEYS.HISTORY, {});
}

const _state$ = new BehaviorSubject<HistoryMap>(readInitial());

function persist(): void {
  setItem(STORAGE_KEYS.HISTORY, _state$.getValue());
}

export const historyService = {
  state$: _state$.asObservable() as Observable<HistoryMap>,
  get state(): HistoryMap { return _state$.getValue(); },

  list(modality: Modality): HistoryEntry[] {
    return _state$.getValue()[modality] ?? [];
  },

  push(entry: Omit<HistoryEntry, 'id' | 'timestamp'>): HistoryEntry {
    const full: HistoryEntry = {
      ...entry,
      id: crypto.randomUUID(),
      timestamp: new Date().toISOString(),
    };
    const previous = _state$.getValue();
    const list = previous[entry.modality] ?? [];
    const next: HistoryMap = {
      ...previous,
      [entry.modality]: [full, ...list].slice(0, MAX_PER_MODALITY),
    };
    _state$.next(next);
    persist();
    return full;
  },

  remove(modality: Modality, id: string): void {
    const previous = _state$.getValue();
    const list = (previous[modality] ?? []).filter((e) => e.id !== id);
    _state$.next({ ...previous, [modality]: list });
    persist();
  },

  clear(modality: Modality): void {
    const previous = _state$.getValue();
    _state$.next({ ...previous, [modality]: [] });
    persist();
  },

  clearAll(): void {
    _state$.next({});
    persist();
  },
};
