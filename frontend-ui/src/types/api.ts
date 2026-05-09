export interface ListMeta {
  chain_id: string;
  next_cursor?: string;
}

export interface ListEnvelope<T> {
  data: T[];
  meta: ListMeta;
}

export interface OrchestratorProfileRow {
  address: string;
  display_name?: string;
  avatar_url?: string;
  total_stake: string;
  fee_cut_percent: string;
  fee_share_percent: string;
  reward_cut_percent: string;
  is_active: boolean;
  service_uri?: string;
  last_lifecycle_event_at?: string;
  as_of_block: string;
  as_of_round?: string;
}

export interface GatewayProfileRow {
  address: string;
  display_name?: string;
  avatar_url?: string;
  kind: string;
  latest_deposit: string;
  latest_reserve: string;
  unlock_in_progress: boolean;
  as_of_block: string;
}

export interface TranscoderParamsRow {
  event_id: string;
  transcoder_address: string;
  block_number: string;
  block_timestamp: string;
  tx_hash: string;
  log_index: number;
  reward_cut_raw: string;
  reward_cut_percent: string;
  fee_share_raw: string;
  fee_share_percent: string;
  fee_cut_percent: string;
}

export interface TranscoderLifecycleRow {
  event_id: string;
  transcoder_address: string;
  block_number: string;
  block_timestamp: string;
  tx_hash: string;
  log_index: number;
  event_name: string;
  round: string;
  is_active: boolean;
}

export interface TranscoderProfileResponse {
  transcoder_address: string;
  block_number: string;
  params?: TranscoderParamsRow;
  lifecycle?: TranscoderLifecycleRow;
}

export interface TranscoderParamsHistoryResponse {
  data: TranscoderParamsRow[];
}

export interface TranscoderLifecycleHistoryResponse {
  data: TranscoderLifecycleRow[];
}

export interface GatewayBalanceRow {
  gateway_address: string;
  block_number: string;
  deposit: string;
  reserve_funds_remaining: string;
  reserve_claimed_in_current_round: string;
  withdraw_round: string;
  unlock_in_progress: boolean;
  source: string;
}

export interface GatewayBalanceHistoryResponse {
  gateway_address: string;
  data: GatewayBalanceRow[];
}

export interface GatewayClaimantRow {
  gateway_address: string;
  claimant_address: string;
  block_number: string;
  claimable_reserve: string;
  claimed_reserve: string;
  source: string;
}

export interface GatewayClaimantsResponse {
  gateway_address: string;
  data: GatewayClaimantRow[];
}

export interface GatewayFlowRow {
  event_id: string;
  event_name: string;
  flow_kind: string;
  block_number: string;
  block_timestamp: string;
  tx_hash: string;
  log_index: number;
  asset?: string;
  amount_native?: string;
  amount_usd?: string;
  from_address?: string;
  to_address?: string;
}

export interface GatewayFlowsResponse {
  gateway_address: string;
  semantics?: string;
  data: GatewayFlowRow[];
}

export interface GatewayRecipientRow {
  recipient_address: string;
  payout_event_count: string;
  total_amount_native: string;
  total_amount_usd: string;
  usd_rows_priced: string;
  ticket_redeemed_count: string;
  reserve_claimed_count: string;
  reserve_transfer_count: string;
  latest_block_number: string;
}

export interface GatewayRecipientsResponse {
  gateway_address: string;
  semantics: string;
  data: GatewayRecipientRow[];
}

export interface GatewayAnalyticsBucket {
  count: string;
  total_amount_native: string;
  total_amount_usd: string;
  usd_rows_priced: string;
}

export interface GatewaySummaryRow {
  event_name: string;
  flow_kind: string;
  count: string;
  total_amount_native: string;
  total_amount_usd: string;
  usd_rows_priced: string;
}

export interface GatewayAnalyticsSummaryResponse {
  gateway_address: string;
  days: string;
  semantics: string;
  from_timestamp: string;
  to_timestamp: string;
  funding: GatewayAnalyticsBucket;
  payouts: GatewayAnalyticsBucket;
  withdrawals: GatewayAnalyticsBucket;
  distinct_recipients: string;
  data: GatewaySummaryRow[];
}

export interface TicketHistoryRow {
  event_id: string;
  tx_hash: string;
  block_number: string;
  block_timestamp: string;
  gateway_address: string;
  orchestrator_address: string;
  face_value: string;
  face_value_usd: string;
  fee_share_percent: string;
  fee_cut_percent: string;
  valuation_version: string;
}

export interface TicketHistoryResponse {
  data: TicketHistoryRow[];
  next_cursor?: string;
}

export type JobType = 'both' | 'ai' | 'transcoding';

export interface VoteTally {
  against_weight: string;
  for_weight: string;
  abstain_weight: string;
  vote_count: string;
}

export interface ProposalRow {
  proposal_id: string;
  proposer?: string;
  vote_start?: string;
  vote_end?: string;
  description?: string;
  created_block: string;
  created_at: string;
  created_tx_hash: string;
  executed: boolean;
  executed_block?: string;
  executed_at?: string;
  vote_tally: VoteTally;
}

export interface ProposalListResponse {
  data: ProposalRow[];
}

export type ProposalStatus = 'executed' | 'not_executed' | 'active' | 'all';

export interface VoteRow {
  event_id: string;
  event_name: string;
  proposal_id: string;
  voter: string;
  support: string; // "0" Against, "1" For, "2" Abstain
  weight: string;
  reason?: string;
  block_number: string;
  block_timestamp: string;
  tx_hash: string;
}

export interface VotesCoverage {
  domain: string;
  backfill_complete: boolean;
  last_processed_block?: string;
}

export interface VoteListResponse {
  data: VoteRow[];
  next_cursor?: string;
  meta: VotesCoverage;
}

export type PayoutSort = 'commission_usd' | 'ticket_count' | 'face_value_usd';

export interface PayoutLeaderboardRow {
  orchestrator_address: string;
  display_name?: string;
  avatar_url?: string;
  ticket_count: string;
  sum_face_value_native: string;
  sum_face_value_usd: string;
  sum_commission_native: string;
  sum_commission_usd: string;
  sum_delegators_share_native: string;
  sum_delegators_share_usd: string;
  distinct_gateways: string;
  usd_rows_priced: string;
}

export interface PayoutLeaderboardMeta {
  chain_id: string;
  from: string;
  to: string;
  valuation_version: string;
  job_type: string;
  sort: string;
  next_cursor?: string;
}

export interface PayoutLeaderboardResponse {
  data: PayoutLeaderboardRow[];
  meta: PayoutLeaderboardMeta;
}

export interface PayoutSummaryResponse {
  period_start: string;
  period_end: string;
  valuation_version: string;
  job_type: string;
  ticket_count: string;
  sum_face_value_native: string;
  sum_face_value_usd: string;
  sum_commission_native: string;
  sum_commission_usd: string;
  sum_delegators_share_native: string;
  sum_delegators_share_usd: string;
  distinct_gateways: string;
  usd_rows_priced: string;
}

export type RewardSort = 'orch_tokens_usd' | 'reward_event_count' | 'total_tokens_usd';

export interface RewardLeaderboardRow {
  orchestrator_address: string;
  display_name?: string;
  avatar_url?: string;
  reward_event_count: string;
  sum_total_tokens: string;
  sum_total_tokens_usd: string;
  sum_orch_tokens: string;
  sum_orch_tokens_usd: string;
  sum_delegators_tokens: string;
  sum_delegators_tokens_usd: string;
  usd_rows_priced: string;
}

export interface RewardLeaderboardMeta {
  chain_id: string;
  from: string;
  to: string;
  valuation_version: string;
  sort: string;
  next_cursor?: string;
}

export interface RewardLeaderboardResponse {
  data: RewardLeaderboardRow[];
  meta: RewardLeaderboardMeta;
}

export interface RewardSummaryResponse {
  period_start: string;
  period_end: string;
  valuation_version: string;
  reward_event_count: string;
  sum_total_tokens: string;
  sum_total_tokens_usd: string;
  sum_orch_tokens: string;
  sum_orch_tokens_usd: string;
  sum_delegators_tokens: string;
  sum_delegators_tokens_usd: string;
  usd_rows_priced: string;
}

export interface TicketSeriesRow {
  date: string;
  count: string;
}

export interface TicketsTimeseriesResponse {
  start: string;
  end: string;
  job_type: string;
  ai: TicketSeriesRow[];
  transcoding: TicketSeriesRow[];
}

export interface StakeHistoryPoint {
  round: string;
  block_number: string;
  block_timestamp: string;
  total_stake: string;
  fee_cut_percent: string;
  reward_cut_percent: string;
  fee_share_percent: string;
  is_active: boolean;
}

export interface StakeHistoryResponse {
  address: string;
  data: StakeHistoryPoint[];
  meta: ListMeta;
}

export interface CutsHistoryPoint {
  block_number: string;
  block_timestamp: string;
  fee_cut_percent: string;
  reward_cut_percent: string;
  fee_share_percent: string;
  event_id: string;
}

export interface CutsHistoryResponse {
  address: string;
  data: CutsHistoryPoint[];
  meta: ListMeta;
}

export interface NetEconomicsResponse {
  address: string;
  period_days: number;
  period_start: string;
  period_end: string;
  gross_payouts_usd: string;
  gross_rewards_usd: string;
  gas_cost_native_eth: string;
  gross_total_usd: string;
}

export interface DelegationRow {
  delegate_address: string;
  bonded_principal: string;
  pending_stake?: string;
  pending_fees?: string;
  pending_round?: string;
  as_of_block: string;
  as_of_timestamp: string;
}

export interface DelegatorResponse {
  delegator_address: string;
  is_active: boolean;
  first_bond_block: string;
  last_seen_block: string;
  delegations: DelegationRow[];
  chain_id: string;
}

export interface NetworkStatsResponse {
  chain_id: string;
  latest_round?: string;
  latest_round_started_block?: string;
  latest_round_started_at?: string;
  active_orchestrators: number;
  total_lpt_staked: string;
  gateways_known: number;
  payouts_usd_24h: string;
  rewards_usd_24h: string;
  gas_burned_eth_24h: string;
  orchestrator_profile_refreshed_at?: string;
  broadcaster_profile_refreshed_at?: string;
}

export interface RoundOrchSummary {
  address: string;
  total_stake: string;
  fee_cut_percent: string;
  reward_cut_percent: string;
  fee_share_percent: string;
  is_active: boolean;
}

export interface RoundSummaryResponse {
  round: string;
  round_started_block: string;
  round_started_at: string;
  active_orchestrators: number;
  total_lpt_staked: string;
  top_orchs: RoundOrchSummary[];
  payouts_usd_on_day: string;
  rewards_usd_on_day: string;
  new_round_events: number;
}

export type SummaryPeriod = 'daily' | 'weekly' | 'monthly';
