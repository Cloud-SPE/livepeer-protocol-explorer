import { localApi } from '../lib/sources/local-api.js';

export const stakeHistoryService = {
  fetchStakeHistory(address: string, fromRound?: number, toRound?: number) {
    const params: { fromRound?: number; toRound?: number } = {};
    if (fromRound !== undefined) params.fromRound = fromRound;
    if (toRound !== undefined) params.toRound = toRound;
    return localApi.getStakeHistory(address, params);
  },
};
