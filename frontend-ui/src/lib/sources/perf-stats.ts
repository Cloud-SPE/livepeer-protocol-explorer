import { configService } from '../../services/config.service.js';
import type { PerfStatsResponse } from '../../types/external.js';

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: { Accept: 'application/json' } });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  return res.json() as Promise<T>;
}

export const perfStatsSource = {
  fetchTranscoding(orchestrator: string): Promise<PerfStatsResponse> {
    const url = `${configService.value.perfStatsUrl}?orchestrator=${encodeURIComponent(orchestrator)}`;
    return fetchJson<PerfStatsResponse>(url);
  },
  fetchAi(params: { orchestrator: string; pipeline?: string; model?: string }): Promise<PerfStatsResponse> {
    const qs = new URLSearchParams({ orchestrator: params.orchestrator });
    if (params.pipeline) qs.set('pipeline', params.pipeline);
    if (params.model) qs.set('model', params.model);
    const url = `${configService.value.aiPerfStatsUrl}?${qs.toString()}`;
    return fetchJson<PerfStatsResponse>(url);
  },
};
