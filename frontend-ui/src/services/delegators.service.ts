import { localApi } from '../lib/sources/local-api.js';

export const delegatorsService = {
  fetchDelegator(address: string) {
    return localApi.getDelegator(address);
  },

  listDelegators(params: { cursor?: string; limit?: number } = {}) {
    return localApi.listDelegators(params);
  },

  fetchDelegatorEvents(
    address: string,
    params: { cursor?: string; limit?: number } = {},
  ) {
    return localApi.getDelegatorEvents(address, params);
  },

  fetchOrchestratorDelegators(
    orchestrator: string,
    params: { cursor?: string; limit?: number } = {},
  ) {
    return localApi.getOrchestratorDelegators(orchestrator, params);
  },
};
