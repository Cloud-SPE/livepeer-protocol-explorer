export interface ApiOptions {
  baseUrl: string;
  getHeaders?: () => Record<string, string>;
  onUnauthorized?: () => void;
}

export interface ApiClient {
  get: <T>(path: string, query?: Record<string, unknown>) => Promise<T>;
  post: <T>(path: string, body?: unknown, init?: RequestInit) => Promise<T>;
  put: <T>(path: string, body?: unknown) => Promise<T>;
  del: <T>(path: string) => Promise<T>;
  url: (path: string, query?: Record<string, unknown>) => string;
}

export class ApiError extends Error {
  status: number;
  body: unknown;
  constructor(message: string, status: number, body: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
  }
}

function qs(query: Record<string, unknown> | undefined): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v === undefined || v === null || v === '') continue;
    params.set(k, String(v));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

export function createApi(opts: ApiOptions): ApiClient {
  const buildUrl = (path: string, query?: Record<string, unknown>): string => {
    const base = opts.baseUrl.replace(/\/$/, '');
    const p = path.startsWith('/') ? path : `/${path}`;
    return `${base}${p}${qs(query)}`;
  };

  async function request<T>(method: string, path: string, query?: Record<string, unknown>, body?: unknown, init?: RequestInit): Promise<T> {
    const url = buildUrl(path, query);
    const headers: Record<string, string> = {
      Accept: 'application/json',
      ...(opts.getHeaders?.() ?? {}),
      ...((init?.headers as Record<string, string>) ?? {}),
    };

    const finalInit: RequestInit = { method, headers, ...init };
    if (body !== undefined) {
      if (body instanceof FormData || body instanceof Blob) {
        finalInit.body = body;
      } else {
        headers['Content-Type'] = headers['Content-Type'] ?? 'application/json';
        finalInit.body = JSON.stringify(body);
      }
    }

    const res = await fetch(url, finalInit);
    if (res.status === 401 || res.status === 403) opts.onUnauthorized?.();

    const ct = res.headers.get('content-type') ?? '';
    let parsed: unknown;
    if (ct.includes('application/json')) {
      parsed = await res.json().catch(() => null);
    } else if (ct.includes('text/')) {
      parsed = await res.text();
    } else {
      parsed = await res.blob();
    }

    if (!res.ok) {
      const msg =
        (typeof parsed === 'object' && parsed !== null && 'error' in parsed
          ? String((parsed as { error: { message?: string } }).error?.message ?? '')
          : '') || `HTTP ${res.status}`;
      throw new ApiError(msg, res.status, parsed);
    }
    return parsed as T;
  }

  return {
    get: (path, query) => request('GET', path, query),
    post: (path, body, init) => request('POST', path, undefined, body, init),
    put: (path, body) => request('PUT', path, undefined, body),
    del: (path) => request('DELETE', path),
    url: buildUrl,
  };
}
