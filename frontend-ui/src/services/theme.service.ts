import { BehaviorSubject, type Observable } from 'rxjs';
import { THEME_NAMES, type ThemeName } from '../types/config.js';
import { STORAGE_KEYS, getItem, setItem } from '../lib/storage.js';

function isThemeName(v: unknown): v is ThemeName {
  return typeof v === 'string' && (THEME_NAMES as readonly string[]).includes(v);
}

function readInitial(): ThemeName {
  const v = getItem<string | null>(STORAGE_KEYS.THEME, null);
  return isThemeName(v) ? v : 'auto';
}

const _theme$ = new BehaviorSubject<ThemeName>(readInitial());

function apply(theme: ThemeName): void {
  document.documentElement.setAttribute('data-theme', theme);
}

apply(_theme$.getValue());

export const themeService = {
  theme$: _theme$.asObservable() as Observable<ThemeName>,
  themes: THEME_NAMES,
  get value(): ThemeName {
    return _theme$.getValue();
  },
  set(theme: ThemeName): void {
    if (!isThemeName(theme)) return;
    setItem(STORAGE_KEYS.THEME, theme);
    apply(theme);
    _theme$.next(theme);
  },
  cycle(): void {
    const idx = THEME_NAMES.indexOf(this.value);
    const next = THEME_NAMES[(idx + 1) % THEME_NAMES.length] ?? 'auto';
    this.set(next);
  },
};
