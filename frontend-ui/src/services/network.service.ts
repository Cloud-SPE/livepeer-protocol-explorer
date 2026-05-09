import { localApi } from '../lib/sources/local-api.js';

export const networkService = {
  fetchNetworkStats() {
    return localApi.getNetworkStats();
  },

  fetchRound(roundId: number | string) {
    return localApi.getRound(roundId);
  },
};
