import { BehaviorSubject, type Observable } from 'rxjs';

export interface EnsEntry {
  name?: string;
  avatar?: string;
}

/** Any row shape with an `address` plus optional ENS-shaped fields. The local
 *  API ships these on profile rows (orchestrators, gateways, leaderboard rows,
 *  recipients, etc.). The cache pulls from whatever rows pass through. */
export interface EnsRow {
  address: string;
  display_name?: string | null;
  avatar_url?: string | null;
}

const _cache$ = new BehaviorSubject<Record<string, EnsEntry>>({});

function norm(addr: string | null | undefined): string {
  return (addr ?? '').toLowerCase();
}

function nonEmpty(v: string | null | undefined): v is string {
  return typeof v === 'string' && v.length > 0;
}

/**
 * Address → ENS-shaped {name, avatar} cache. Pure in-memory, no RPC, no
 * subgraph. The cache is fed by every list/profile service that already
 * receives `display_name` and `avatar_url` from the local Rust API. Address
 * chips subscribe to the cache and render the friendlier form when known.
 */
export const ensService = {
  cache$: _cache$.asObservable() as Observable<Record<string, EnsEntry>>,
  get cache(): Record<string, EnsEntry> { return _cache$.getValue(); },

  /** Sync lookup. Returns an empty entry when the address is unknown. */
  lookup(address: string | null | undefined): EnsEntry {
    if (!address) return {};
    return _cache$.getValue()[norm(address)] ?? {};
  },

  /** Record a single entry. Empty inputs are dropped. */
  record(address: string, entry: EnsEntry): void {
    if (!address || (!nonEmpty(entry.name) && !nonEmpty(entry.avatar))) return;
    const key = norm(address);
    const cur = _cache$.getValue();
    const prev = cur[key] ?? {};
    const name = nonEmpty(entry.name) ? entry.name : prev.name;
    const avatar = nonEmpty(entry.avatar) ? entry.avatar : prev.avatar;
    if (name === prev.name && avatar === prev.avatar) return;
    const merged: EnsEntry = {
      ...(name !== undefined ? { name } : {}),
      ...(avatar !== undefined ? { avatar } : {}),
    };
    _cache$.next({ ...cur, [key]: merged });
  },

  /** Bulk-record from any array of rows that have address + optional ENS fields. */
  recordMany(rows: readonly EnsRow[] | undefined | null): void {
    if (!rows || rows.length === 0) return;
    const cur = _cache$.getValue();
    const next = { ...cur };
    let changed = false;
    for (const r of rows) {
      if (!r?.address) continue;
      if (!nonEmpty(r.display_name) && !nonEmpty(r.avatar_url)) continue;
      const key = norm(r.address);
      const prev = next[key] ?? {};
      const name = nonEmpty(r.display_name) ? r.display_name : prev.name;
      const avatar = nonEmpty(r.avatar_url) ? r.avatar_url : prev.avatar;
      if (name === prev.name && avatar === prev.avatar) continue;
      next[key] = {
        ...(name !== undefined ? { name } : {}),
        ...(avatar !== undefined ? { avatar } : {}),
      };
      changed = true;
    }
    if (changed) _cache$.next(next);
  },

  /** Mark an avatar URL as broken so the chip stops trying to render it. */
  forgetAvatar(address: string): void {
    if (!address) return;
    const key = norm(address);
    const cur = _cache$.getValue();
    const prev = cur[key];
    if (!prev?.avatar) return;
    const stripped: EnsEntry = prev.name !== undefined ? { name: prev.name } : {};
    _cache$.next({ ...cur, [key]: stripped });
  },

  reset(): void { _cache$.next({}); },
};
