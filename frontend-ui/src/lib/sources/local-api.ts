import { createApi, type ApiClient } from '../api-base.js';
import { configService } from '../../services/config.service.js';
import type {
  CutsHistoryResponse,
  DelegatorEventsResponse,
  DelegatorIndexResponse,
  DelegatorResponse,
  GatewayAnalyticsSummaryResponse,
  GatewayBalanceHistoryResponse,
  GatewayBalanceRow,
  GatewayClaimantsResponse,
  GatewayFlowsResponse,
  GatewayProfileRow,
  GatewayRecipientsResponse,
  JobType,
  ListEnvelope,
  NetEconomicsResponse,
  NetworkStatsResponse,
  OrchDelegatorsResponse,
  OrchestratorProfileRow,
  PayoutLeaderboardResponse,
  PayoutSort,
  PayoutSummaryResponse,
  ProposalListResponse,
  ProposalRow,
  ProposalStatus,
  RoundSummaryResponse,
  RewardLeaderboardResponse,
  RewardSort,
  RewardSummaryResponse,
  StakeHistoryResponse,
  SummaryPeriod,
  TicketHistoryResponse,
  TicketsTimeseriesResponse,
  TranscoderLifecycleHistoryResponse,
  TranscoderLifecycleRow,
  TranscoderParamsHistoryResponse,
  TranscoderParamsRow,
  TranscoderProfileResponse,
  VoteListResponse,
} from '../../types/api.js';

let _client: ApiClient | null = null;
let _baseUrl = '';

function client(): ApiClient {
  const cfg = configService.value;
  if (!_client || cfg.baseApiUrl !== _baseUrl) {
    _baseUrl = cfg.baseApiUrl;
    _client = createApi({ baseUrl: cfg.baseApiUrl });
  }
  return _client;
}

export const localApi = {
  // ───────── orchestrators ─────────
  listOrchestrators(params: { cursor?: string; limit?: number; activeOnly?: boolean } = {}) {
    return client().get<ListEnvelope<OrchestratorProfileRow>>('/orchestrators', {
      cursor: params.cursor,
      limit: params.limit,
      active_only: params.activeOnly,
    });
  },

  getOrchestrator(address: string) {
    return client().get<OrchestratorProfileRow>(`/orchestrators/${address}`);
  },

  getStakeHistory(address: string, params: { fromRound?: number; toRound?: number } = {}) {
    return client().get<StakeHistoryResponse>(`/orchestrators/${address}/stake-history`, {
      from_round: params.fromRound,
      to_round: params.toRound,
    });
  },

  getCutsHistory(address: string) {
    return client().get<CutsHistoryResponse>(`/orchestrators/${address}/cuts-history`);
  },

  getNetEconomics(address: string, params: { periodDays?: number } = {}) {
    return client().get<NetEconomicsResponse>(`/orchestrators/${address}/net-economics`, {
      period_days: params.periodDays,
    });
  },

  getDelegator(address: string) {
    return client().get<DelegatorResponse>(`/delegators/${address}`);
  },

  listDelegators(params: { cursor?: string; limit?: number } = {}) {
    return client().get<DelegatorIndexResponse>('/delegators', {
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  getDelegatorEvents(
    address: string,
    params: { cursor?: string; limit?: number } = {},
  ) {
    return client().get<DelegatorEventsResponse>(
      `/delegators/${address}/events`,
      { cursor: params.cursor, limit: params.limit },
    );
  },

  getOrchestratorDelegators(
    orchestrator: string,
    params: { cursor?: string; limit?: number } = {},
  ) {
    return client().get<OrchDelegatorsResponse>(
      `/orchestrators/${orchestrator}/delegators`,
      { cursor: params.cursor, limit: params.limit },
    );
  },

  getNetworkStats() {
    return client().get<NetworkStatsResponse>('/network/stats');
  },

  getRound(roundId: number | string) {
    return client().get<RoundSummaryResponse>(`/rounds/${roundId}`);
  },

  // ───────── transcoders (orch params + lifecycle) ─────────
  getTranscoderParamsLatest(address: string) {
    return client().get<TranscoderParamsRow>(`/transcoders/${address}/params/latest`);
  },

  getTranscoderParamsHistory(
    address: string,
    params: { fromBlock?: number; toBlock?: number; limit?: number } = {},
  ) {
    return client().get<TranscoderParamsHistoryResponse>(
      `/transcoders/${address}/params/history`,
      { from_block: params.fromBlock, to_block: params.toBlock, limit: params.limit },
    );
  },

  getTranscoderLifecycleLatest(address: string) {
    return client().get<TranscoderLifecycleRow>(`/transcoders/${address}/lifecycle/latest`);
  },

  getTranscoderLifecycleHistory(
    address: string,
    params: { fromBlock?: number; toBlock?: number; limit?: number } = {},
  ) {
    return client().get<TranscoderLifecycleHistoryResponse>(
      `/transcoders/${address}/lifecycle/history`,
      { from_block: params.fromBlock, to_block: params.toBlock, limit: params.limit },
    );
  },

  getTranscoderProfileAtBlock(address: string, block: number | 'latest') {
    return client().get<TranscoderProfileResponse>(
      `/transcoders/${address}/profile/block/${block}`,
    );
  },

  // ───────── orchestrator tickets ─────────
  getOrchestratorTickets(
    address: string,
    params: { start?: string; end?: string; cursor?: string; limit?: number } = {},
  ) {
    return client().get<TicketHistoryResponse>(`/orchestrators/${address}/tickets/latest`, {
      start: params.start,
      end: params.end,
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  // ───────── gateways ─────────
  listGateways(params: { cursor?: string; limit?: number } = {}) {
    return client().get<ListEnvelope<GatewayProfileRow>>('/gateways', {
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  getGatewayProfile(address: string) {
    return client().get<GatewayProfileRow>(`/gateways/${address}/profile`);
  },

  getGatewayBalanceLatest(address: string) {
    return client().get<GatewayBalanceRow>(`/gateways/${address}/balance/latest`);
  },

  getGatewayBalanceHistory(
    address: string,
    params: { fromBlock?: number; toBlock?: number; limit?: number } = {},
  ) {
    return client().get<GatewayBalanceHistoryResponse>(
      `/gateways/${address}/balance/history`,
      { from_block: params.fromBlock, to_block: params.toBlock, limit: params.limit },
    );
  },

  getGatewayRecipients(
    address: string,
    params: {
      fromBlock?: number;
      toBlock?: number;
      limit?: number;
      semantics?: 'net' | 'gross';
    } = {},
  ) {
    return client().get<GatewayRecipientsResponse>(`/gateways/${address}/recipients`, {
      from_block: params.fromBlock,
      to_block: params.toBlock,
      limit: params.limit,
      semantics: params.semantics,
    });
  },

  getGatewayFlows(
    address: string,
    params: { fromBlock?: number; toBlock?: number; limit?: number } = {},
  ) {
    return client().get<GatewayFlowsResponse>(`/gateways/${address}/flows`, {
      from_block: params.fromBlock,
      to_block: params.toBlock,
      limit: params.limit,
    });
  },

  getGatewayAnalyticsSummary(
    address: string,
    params: { days?: number; semantics?: 'net' | 'gross' } = {},
  ) {
    return client().get<GatewayAnalyticsSummaryResponse>(
      `/gateways/${address}/analytics/summary`,
      { days: params.days, semantics: params.semantics },
    );
  },

  getGatewayClaimantsAtBlock(address: string, block: number | 'latest') {
    return client().get<GatewayClaimantsResponse>(
      `/gateways/${address}/claimants/block/${block}`,
    );
  },

  // ───────── governance ─────────
  listProposals(params: { status?: ProposalStatus; limit?: number } = {}) {
    return client().get<ProposalListResponse>('/governance/proposals', {
      status: params.status,
      limit: params.limit,
    });
  },

  getProposal(id: string) {
    return client().get<ProposalRow>(`/governance/proposals/${id}`);
  },

  listVotes(params: { proposal_id?: string; voter?: string; cursor?: string; limit?: number } = {}) {
    return client().get<VoteListResponse>('/governance/votes', {
      proposal_id: params.proposal_id,
      voter: params.voter,
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  // ───────── payouts ─────────
  getPayoutLeaderboard(params: {
    from: string;
    to: string;
    job_type?: JobType;
    sort?: PayoutSort;
    cursor?: string;
    limit?: number;
  }) {
    return client().get<PayoutLeaderboardResponse>('/payouts/leaderboard', {
      from: params.from,
      to: params.to,
      job_type: params.job_type,
      sort: params.sort,
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  getPayoutSummary(period: SummaryPeriod, date: string, jobType?: JobType) {
    return client().get<PayoutSummaryResponse>(`/payouts/summary/${period}/${date}`, {
      job_type: jobType,
    });
  },

  // ───────── rewards ─────────
  getRewardLeaderboard(params: {
    from: string;
    to: string;
    sort?: RewardSort;
    cursor?: string;
    limit?: number;
  }) {
    return client().get<RewardLeaderboardResponse>('/rewards/leaderboard', {
      from: params.from,
      to: params.to,
      sort: params.sort,
      cursor: params.cursor,
      limit: params.limit,
    });
  },

  getRewardSummary(period: SummaryPeriod, date: string) {
    return client().get<RewardSummaryResponse>(`/rewards/summary/${period}/${date}`);
  },

  // ───────── tickets ─────────
  getTicketsTimeseries(params: { start: string; end: string; job_type?: JobType }) {
    return client().get<TicketsTimeseriesResponse>('/tickets/timeseries/daily', {
      start: params.start,
      end: params.end,
      job_type: params.job_type,
    });
  },

  // ───────── reports (CSV URLs only — used as <a download>) ─────────
  reportPayoutsCsvUrl(orchestrator: string, start: string, end: string): string {
    return client().url('/reports/payouts.csv', { orchestrator, start, end });
  },
  reportRewardsCsvUrl(orchestrator: string, start: string, end: string): string {
    return client().url('/reports/rewards.csv', { orchestrator, start, end });
  },
  reportGatewayPayoutsCsvUrl(gateway: string, start: string, end: string): string {
    return client().url('/reports/gateway-payouts.csv', { gateway, start, end });
  },
};
