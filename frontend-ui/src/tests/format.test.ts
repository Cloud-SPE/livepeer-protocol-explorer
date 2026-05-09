import { describe, it, expect } from 'vitest';
import {
  shortAddress,
  formatNative,
  formatUsd,
  formatPercent,
  formatNumber,
  formatTimestamp,
  todayIso,
} from '../lib/format.js';

describe('shortAddress', () => {
  it('returns em-dash for null', () => {
    expect(shortAddress(null)).toBe('—');
    expect(shortAddress(undefined)).toBe('—');
  });

  it('shortens long addresses', () => {
    expect(shortAddress('0x1234567890abcdef1234567890abcdef12345678')).toBe('0x1234…5678');
  });

  it('passes through short strings', () => {
    expect(shortAddress('0xabc')).toBe('0xabc');
  });
});

describe('formatNative', () => {
  it('handles 18-decimal big integers', () => {
    const result = formatNative('1500000000000000000', 18);
    expect(result).toContain('1.5');
  });

  it('handles pre-formatted decimal strings from the local API', () => {
    const result = formatNative('1252157.250877878672656418', 18);
    expect(result).toMatch(/1[\s,.]?252[\s,.]?157/);
  });

  it('preserves precision for small values from a decimal string', () => {
    const result = formatNative('0.000123', 18, { digits: 6 });
    expect(result).toContain('0.000123');
  });

  it('returns em-dash for empty', () => {
    expect(formatNative(null)).toBe('—');
    expect(formatNative('')).toBe('—');
  });
});

describe('formatUsd', () => {
  it('formats USD with $ prefix', () => {
    const out = formatUsd('1234.56');
    expect(out).toContain('$');
    expect(out).toContain('1,234');
  });

  it('returns em-dash for null', () => {
    expect(formatUsd(null)).toBe('—');
  });
});

describe('formatPercent', () => {
  it('appends % sign', () => {
    expect(formatPercent('12.5')).toBe('12.50%');
  });
});

describe('formatNumber', () => {
  it('uses compact notation', () => {
    const out = formatNumber(1500000, { compact: true });
    expect(out).toMatch(/M$/);
  });
});

describe('formatTimestamp', () => {
  it('returns em-dash for invalid', () => {
    expect(formatTimestamp(null)).toBe('—');
    expect(formatTimestamp('not-a-date')).toBe('—');
  });

  it('formats a valid ISO date', () => {
    expect(formatTimestamp('2026-01-15T10:30:00Z')).not.toBe('—');
  });
});

describe('todayIso', () => {
  it('returns YYYY-MM-DD shape', () => {
    expect(todayIso()).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });
});
