import { format as dnumFormat, from as dnumFrom } from 'dnum';

export function shortAddress(address: string | null | undefined, head = 6, tail = 4): string {
  if (!address) return '—';
  const a = address.trim();
  if (a.length <= head + tail + 2) return a;
  return `${a.slice(0, head)}…${a.slice(-tail)}`;
}

export function formatNative(value: string | number | null | undefined, decimals = 18, opts: { digits?: number; compact?: boolean } = {}): string {
  if (value === null || value === undefined || value === '') return '—';
  const v = typeof value === 'number' ? String(value) : value;
  try {
    // The local API serializes chain values as decimal strings (e.g. "1252157.250..."),
    // while some upstreams ship raw scaled integers (e.g. "1252157250...18 zeros").
    // dnumFrom(string) handles the decimal form natively; the [bigint, decimals]
    // tuple covers the raw-scaled form.
    const dn = v.includes('.') ? dnumFrom(v) : dnumFrom([BigInt(v), decimals]);
    return dnumFormat(dn, {
      digits: opts.digits ?? 4,
      trailingZeros: false,
      compact: opts.compact ?? false,
    });
  } catch {
    const n = Number(v);
    if (!Number.isFinite(n)) return '—';
    return formatNumber(n, opts);
  }
}

export function formatDecimal(value: string | number | null | undefined, opts: { digits?: number; compact?: boolean } = {}): string {
  if (value === null || value === undefined || value === '') return '—';
  const n = Number(value);
  if (!Number.isFinite(n)) return '—';
  return formatNumber(n, opts);
}

export function formatNumber(n: number, { digits = 2, compact = false }: { digits?: number; compact?: boolean } = {}): string {
  return new Intl.NumberFormat(undefined, {
    notation: compact ? 'compact' : 'standard',
    maximumFractionDigits: digits,
    minimumFractionDigits: 0,
  }).format(n);
}

export function formatUsd(value: string | number | null | undefined, opts: { digits?: number; compact?: boolean } = {}): string {
  if (value === null || value === undefined || value === '') return '—';
  const n = Number(value);
  if (!Number.isFinite(n)) return '—';
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: 'USD',
    notation: opts.compact ? 'compact' : 'standard',
    maximumFractionDigits: opts.digits ?? (n < 1 ? 4 : 2),
    minimumFractionDigits: 0,
  }).format(n);
}

export function formatPercent(value: string | number | null | undefined, digits = 2): string {
  if (value === null || value === undefined || value === '') return '—';
  const n = Number(value);
  if (!Number.isFinite(n)) return '—';
  return `${n.toFixed(digits)}%`;
}

export function formatTimestamp(ts: string | Date | null | undefined): string {
  if (!ts) return '—';
  const d = typeof ts === 'string' ? new Date(ts) : ts;
  if (Number.isNaN(d.valueOf())) return '—';
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(d);
}

export function formatRelative(ts: string | Date | null | undefined, base: Date = new Date()): string {
  if (!ts) return '—';
  const d = typeof ts === 'string' ? new Date(ts) : ts;
  if (Number.isNaN(d.valueOf())) return '—';
  const seconds = Math.round((d.valueOf() - base.valueOf()) / 1000);
  const abs = Math.abs(seconds);
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
  if (abs < 60) return rtf.format(seconds, 'second');
  if (abs < 3600) return rtf.format(Math.round(seconds / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(seconds / 3600), 'hour');
  if (abs < 86400 * 30) return rtf.format(Math.round(seconds / 86400), 'day');
  if (abs < 86400 * 365) return rtf.format(Math.round(seconds / (86400 * 30)), 'month');
  return rtf.format(Math.round(seconds / (86400 * 365)), 'year');
}

export function todayIso(): string {
  const d = new Date();
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, '0');
  const day = String(d.getUTCDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}
