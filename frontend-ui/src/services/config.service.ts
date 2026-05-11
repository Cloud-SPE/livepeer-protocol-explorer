import { BehaviorSubject, type Observable } from 'rxjs';
import type { AppConfig, PartialAppConfig } from '../types/config.js';

// Build-time defaults, used until `/config.json` resolves at boot.
//
// In production the FE bundle and the API are served by the same axum
// process, so an empty `baseApiUrl` (relative URLs) is correct.
//
// All values are overridable per-deploy by the API's env-driven
// `/config.json` handler — see `crates/livepeer-api/src/routes/operational.rs`
// `frontend_config` for the env contract. Edit env vars on the host, restart
// the api container, no FE rebuild needed.
const DEFAULT_CONFIG: AppConfig = {
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

const _config$ = new BehaviorSubject<AppConfig>({ ...DEFAULT_CONFIG });

export const configService = {
  config$: _config$.asObservable() as Observable<AppConfig>,
  get value(): AppConfig {
    return _config$.getValue();
  },
  apply(runtime: PartialAppConfig | null): AppConfig {
    const merged: AppConfig = { ...DEFAULT_CONFIG, ...(runtime ?? {}) };
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
    console.warn('config.json unreachable, using build-time defaults', err);
    return configService.apply(null);
  }
}
