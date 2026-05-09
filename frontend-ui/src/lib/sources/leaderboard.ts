import { configService } from '../../services/config.service.js';
import type { LeaderboardResponse } from '../../types/external.js';

interface QueryParams {
  region?: string;
  pipeline?: string;
  model?: string;
}

function withQs(base: string, q: QueryParams): string {
  const params = new URLSearchParams();
  if (q.region && q.region !== 'GLOBAL') params.set('region', q.region);
  if (q.pipeline) params.set('pipeline', q.pipeline);
  if (q.model) params.set('model', q.model);
  const s = params.toString();
  return s ? `${base}?${s}` : base;
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: { Accept: 'application/json' } });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  return res.json() as Promise<T>;
}

export const leaderboardSource = {
  fetchTranscoding(region?: string): Promise<LeaderboardResponse> {
    return fetchJson<LeaderboardResponse>(
      withQs(configService.value.leaderboardStatsUrl, region ? { region } : {}),
    );
  },
  fetchAi(params: { pipeline: string; model: string; region?: string }): Promise<LeaderboardResponse> {
    const q: QueryParams = { pipeline: params.pipeline, model: params.model };
    if (params.region) q.region = params.region;
    return fetchJson<LeaderboardResponse>(withQs(configService.value.aiLeaderboardStatsUrl, q));
  },
};
