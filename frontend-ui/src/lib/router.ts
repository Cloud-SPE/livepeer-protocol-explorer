export type RouteParams = Record<string, string>;

export interface RouteMatch {
  pattern: string;
  path: string;
  params: RouteParams;
  query: URLSearchParams;
}

export interface RouteDef {
  pattern: string;
  redirect?: (params: RouteParams) => string;
}

const ROUTES: RouteDef[] = [];

export function defineRoutes(routes: RouteDef[]): void {
  ROUTES.length = 0;
  ROUTES.push(...routes);
}

function patternToRegex(pattern: string): { regex: RegExp; keys: string[] } {
  const keys: string[] = [];
  const regex = new RegExp(
    '^' +
      pattern
        .replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
        .replace(/:([a-zA-Z_]\w*)/g, (_m, k: string) => {
          keys.push(k);
          return '([^/]+)';
        }) +
      '/?$',
  );
  return { regex, keys };
}

export function matchRoute(path: string): RouteMatch | null {
  for (const def of ROUTES) {
    const { regex, keys } = patternToRegex(def.pattern);
    const m = regex.exec(path);
    if (!m) continue;
    const params: RouteParams = {};
    keys.forEach((k, i) => {
      params[k] = decodeURIComponent(m[i + 1] ?? '');
    });
    if (def.redirect) {
      const target = def.redirect(params);
      navigate(target, true);
      return null;
    }
    return { pattern: def.pattern, path, params, query: new URLSearchParams() };
  }
  return null;
}

export function getCurrentPath(): { path: string; query: URLSearchParams } {
  const hash = window.location.hash.slice(1) || '/';
  const [path, qs = ''] = hash.split('?');
  return { path: path || '/', query: new URLSearchParams(qs) };
}

export function navigate(path: string, replace = false): void {
  const target = `#${path.startsWith('/') ? path : `/${path}`}`;
  if (replace) {
    window.history.replaceState(null, '', target);
    window.dispatchEvent(new HashChangeEvent('hashchange'));
  } else {
    window.location.hash = target.slice(1);
  }
}

export type RouteHandler = (match: RouteMatch | null, query: URLSearchParams) => void;

export function startRouter(handler: RouteHandler): () => void {
  const fire = (): void => {
    const { path, query } = getCurrentPath();
    const m = matchRoute(path);
    handler(m ? { ...m, query } : null, query);
  };
  window.addEventListener('hashchange', fire);
  fire();
  return () => window.removeEventListener('hashchange', fire);
}

export function withViewTransition(fn: () => void): void {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => unknown };
  if (typeof doc.startViewTransition === 'function') {
    doc.startViewTransition(fn);
  } else {
    fn();
  }
}
