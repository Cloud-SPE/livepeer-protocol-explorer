import { describe, it, expect, beforeEach } from 'vitest';
import { defineRoutes, matchRoute } from '../lib/router.js';

describe('router', () => {
  beforeEach(() => {
    defineRoutes([
      { pattern: '/' },
      { pattern: '/orchestrators' },
      { pattern: '/orchestrators/:address' },
      { pattern: '/governance/proposals/:id' },
      { pattern: '/reports/daily/:date' },
    ]);
  });

  it('matches a static path', () => {
    const m = matchRoute('/orchestrators');
    expect(m).not.toBeNull();
    expect(m?.pattern).toBe('/orchestrators');
    expect(m?.params).toEqual({});
  });

  it('extracts a single dynamic segment', () => {
    const m = matchRoute('/orchestrators/0xabc');
    expect(m?.pattern).toBe('/orchestrators/:address');
    expect(m?.params).toEqual({ address: '0xabc' });
  });

  it('decodes URL-encoded params', () => {
    const m = matchRoute('/orchestrators/0x%41%42');
    expect(m?.params).toEqual({ address: '0xAB' });
  });

  it('matches the longest defined route', () => {
    const m = matchRoute('/governance/proposals/42');
    expect(m?.pattern).toBe('/governance/proposals/:id');
    expect(m?.params).toEqual({ id: '42' });
  });

  it('returns null for unmatched paths', () => {
    expect(matchRoute('/totally/unknown')).toBeNull();
  });

  it('matches root path', () => {
    expect(matchRoute('/')?.pattern).toBe('/');
  });
});
