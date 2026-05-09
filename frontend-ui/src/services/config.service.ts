import { BehaviorSubject, type Observable } from 'rxjs';
import type { AppConfig, PartialAppConfig } from '../types/config.js';

const DEFAULT_CONFIG: AppConfig = {
  // Empty string = relative URLs, which go through the Vite dev proxy in
  // development. Production deployments override this in `public/config.json`.
  baseApiUrl: '',
  gatewayUrl: 'https://dream-gateway.livepeer.cloud',
  gatewayBearer: '',
  byocGatewayUrl: 'https://openai-gateway.livepeer.cloud/v1',
  perfStatsUrl: 'https://leaderboard-serverless.vercel.app/api/raw_stats',
  aiPerfStatsUrl: 'https://lpc-leaderboard-serverless.vercel.app/api/raw_stats',
  leaderboardStatsUrl: 'https://leaderboard-serverless.vercel.app/api/aggregated_stats',
  aiLeaderboardStatsUrl: 'https://lpc-leaderboard-serverless.vercel.app/api/aggregated_stats',
  regionsUrl: 'https://lpc-leaderboard-serverless.vercel.app/api/regions',
  pipelineUrl: 'https://lpc-leaderboard-serverless.vercel.app/api/pipelines',
  explorerTxBase: 'https://arbiscan.io/tx/',
  explorerAddressBase: 'https://arbiscan.io/address/',
};

function fromEnv(): PartialAppConfig {
  const e = import.meta.env;
  return {
    ...(e.VITE_BASE_API_URL ? { baseApiUrl: e.VITE_BASE_API_URL } : {}),
    ...(e.VITE_GATEWAY_URL ? { gatewayUrl: e.VITE_GATEWAY_URL } : {}),
    ...(e.VITE_GATEWAY_BEARER_TOKEN ? { gatewayBearer: e.VITE_GATEWAY_BEARER_TOKEN } : {}),
    ...(e.VITE_BYOC_GATEWAY_URL ? { byocGatewayUrl: e.VITE_BYOC_GATEWAY_URL } : {}),
    ...(e.VITE_PERF_STATS_URL ? { perfStatsUrl: e.VITE_PERF_STATS_URL } : {}),
    ...(e.VITE_AI_PERF_STATS_URL ? { aiPerfStatsUrl: e.VITE_AI_PERF_STATS_URL } : {}),
    ...(e.VITE_LEADERBOARD_STATS_URL ? { leaderboardStatsUrl: e.VITE_LEADERBOARD_STATS_URL } : {}),
    ...(e.VITE_AI_LEADERBOARD_STATS_URL ? { aiLeaderboardStatsUrl: e.VITE_AI_LEADERBOARD_STATS_URL } : {}),
    ...(e.VITE_REGIONS_URL ? { regionsUrl: e.VITE_REGIONS_URL } : {}),
    ...(e.VITE_PIPELINE_URL ? { pipelineUrl: e.VITE_PIPELINE_URL } : {}),
    ...(e.VITE_EXPLORER_TX_BASE ? { explorerTxBase: e.VITE_EXPLORER_TX_BASE } : {}),
    ...(e.VITE_EXPLORER_ADDRESS_BASE ? { explorerAddressBase: e.VITE_EXPLORER_ADDRESS_BASE } : {}),
  };
}

const _config$ = new BehaviorSubject<AppConfig>({ ...DEFAULT_CONFIG, ...fromEnv() });

export const configService = {
  config$: _config$.asObservable() as Observable<AppConfig>,
  get value(): AppConfig {
    return _config$.getValue();
  },
  apply(runtime: PartialAppConfig | null): AppConfig {
    const merged: AppConfig = { ...DEFAULT_CONFIG, ...fromEnv(), ...(runtime ?? {}) };
    _config$.next(merged);
    return merged;
  },
  patch(partial: PartialAppConfig): void {
    _config$.next({ ..._config$.getValue(), ...partial });
  },
};

export async function loadRuntimeConfig(): Promise<AppConfig> {
  try {
    const res = await fetch('/config.json', { cache: 'no-store' });
    if (!res.ok) throw new Error(`config.json HTTP ${res.status}`);
    const json = (await res.json()) as PartialAppConfig;
    return configService.apply(json);
  } catch (err) {
    console.warn('config.json missing or invalid, using build-time defaults', err);
    return configService.apply(null);
  }
}
