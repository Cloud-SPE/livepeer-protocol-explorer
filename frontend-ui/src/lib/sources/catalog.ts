import { configService } from '../../services/config.service.js';
import type { PipelinesResponse, RegionsResponse } from '../../types/external.js';

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: { Accept: 'application/json' } });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  return res.json() as Promise<T>;
}

export const catalogSource = {
  fetchRegions(): Promise<RegionsResponse> {
    return fetchJson<RegionsResponse>(configService.value.regionsUrl);
  },
  fetchPipelines(): Promise<PipelinesResponse> {
    return fetchJson<PipelinesResponse>(configService.value.pipelineUrl);
  },
};
