import { localApi } from '../lib/sources/local-api.js';

export const delegatorsService = {
  fetchDelegator(address: string) {
    return localApi.getDelegator(address);
  },
};
