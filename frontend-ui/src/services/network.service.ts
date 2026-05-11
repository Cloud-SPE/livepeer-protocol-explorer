import { localApi } from '../lib/sources/local-api.js';

export const networkService = {
  fetchNetworkStats() {
    return localApi.getNetworkStats();
  },

  fetchRound(roundId: number | string) {
    return localApi.getRound(roundId);
  },

  listRounds(params: { cursor?: string; limit?: number } = {}) {
    return localApi.listRounds(params);
  },

  fetchRoundEvents(
    roundId: number | string,
    params: { cursor?: string; limit?: number; kinds?: string } = {},
  ) {
    return localApi.getRoundEvents(roundId, params);
  },

  fetchRoundEventCounts(roundId: number | string) {
    return localApi.getRoundEventCounts(roundId);
  },
};
