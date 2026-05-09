const PREFIX = 'lp-tools:';

export function getItem<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (raw === null) return fallback;
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

export function setItem<T>(key: string, value: T): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch (err) {
    console.warn(`storage write failed for ${key}`, err);
  }
}

export function removeItem(key: string): void {
  try {
    localStorage.removeItem(PREFIX + key);
  } catch {
    /* ignore */
  }
}

export const STORAGE_KEYS = {
  THEME: 'theme',
  FILTERS: 'filters',
  HISTORY: 'history',
  GATEWAY_OVERRIDE: 'gateway-override',
} as const;
