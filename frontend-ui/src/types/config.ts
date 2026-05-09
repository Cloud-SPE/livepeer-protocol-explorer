export interface AppConfig {
  baseApiUrl: string;
  gatewayUrl: string;
  gatewayBearer: string;
  byocGatewayUrl: string;
  perfStatsUrl: string;
  aiPerfStatsUrl: string;
  leaderboardStatsUrl: string;
  aiLeaderboardStatsUrl: string;
  regionsUrl: string;
  pipelineUrl: string;
  explorerTxBase: string;
  explorerAddressBase: string;
}

export type PartialAppConfig = Partial<AppConfig>;

export const THEME_NAMES = ['auto', 'light', 'dark', 'midnight', 'solarized', 'high-contrast'] as const;
export type ThemeName = (typeof THEME_NAMES)[number];
