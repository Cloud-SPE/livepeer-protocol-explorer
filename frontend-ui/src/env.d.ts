/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_BASE_API_URL?: string;
  readonly VITE_GATEWAY_URL?: string;
  readonly VITE_GATEWAY_BEARER_TOKEN?: string;
  readonly VITE_BYOC_GATEWAY_URL?: string;
  readonly VITE_PERF_STATS_URL?: string;
  readonly VITE_AI_PERF_STATS_URL?: string;
  readonly VITE_LEADERBOARD_STATS_URL?: string;
  readonly VITE_AI_LEADERBOARD_STATS_URL?: string;
  readonly VITE_REGIONS_URL?: string;
  readonly VITE_PIPELINE_URL?: string;
  readonly VITE_EXPLORER_TX_BASE?: string;
  readonly VITE_EXPLORER_ADDRESS_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
