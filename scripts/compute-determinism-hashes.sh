#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required" >&2
  exit 1
fi

OUT="${1:-}"
if [[ -z "$OUT" ]]; then
  echo "usage: $0 <output-json-path>" >&2
  exit 1
fi

psqlq() {
  psql "$DATABASE_URL" -At -c "$1"
}

raw_count=$(psqlq "SELECT COUNT(*) FROM raw_protocol_events;")
raw_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY id)), md5(''))
FROM (
  SELECT id,
         md5(concat_ws('§',
           chain_id::text,
           tx_hash,
           log_index::text,
           block_number::text,
           block_hash,
           block_timestamp::text,
           contract_address,
           contract_name,
           event_name,
           event_signature,
           COALESCE(asset, ''),
           COALESCE(amount_raw::text, ''),
           COALESCE(amount_normalized::text, ''),
           is_valuable::text,
           COALESCE(from_address, ''),
           COALESCE(to_address, ''),
           finality,
           is_canonical::text,
           COALESCE(l1_batch_tx_hash, ''),
           COALESCE(jsonb_strip_nulls(raw_event)::text, ''),
           abi_hash_used
         )) AS row_md5
  FROM raw_protocol_events
) s;")

decode_failures_count=$(psqlq "SELECT COUNT(*) FROM decode_failures;")
decode_failures_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, tx_hash, log_index)), md5(''))
FROM (
  SELECT chain_id, tx_hash, log_index,
         md5(concat_ws('§',
           chain_id::text,
           block_number::text,
           block_hash,
           tx_hash,
           log_index::text,
           contract_address,
           array_to_string(topics, '|'),
           encode(data, 'hex'),
           attempted_abi_hash,
           error_message,
           COALESCE(resolved_event_id::text, '')
         )) AS row_md5
  FROM decode_failures
) s;")

event_valuations_count=$(psqlq "SELECT COUNT(*) FROM event_valuations;")
event_valuations_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY event_id, valuation_version, asset)), md5(''))
FROM (
  SELECT event_id, valuation_version, asset,
         md5(concat_ws('§',
           chain_id::text,
           event_id::text,
           valuation_version,
           asset,
           pricing_method,
           block_number::text,
           amount_native::text,
           COALESCE(native_usd_price::text, ''),
           COALESCE(amount_usd::text, ''),
           COALESCE(jsonb_strip_nulls(pricing_chain)::text, ''),
           status,
           source
         )) AS row_md5
  FROM event_valuations
) s;")

valuation_attempts_count=$(psqlq "SELECT COUNT(*) FROM valuation_attempts;")
valuation_attempts_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY event_id, valuation_version, asset, attempt_number)), md5(''))
FROM (
  SELECT event_id, valuation_version, asset, attempt_number,
         md5(concat_ws('§',
           event_id::text,
           valuation_version,
           asset,
           attempt_number::text,
           result_status,
           COALESCE(jsonb_strip_nulls(error_detail)::text, '')
         )) AS row_md5
  FROM valuation_attempts
) s;")

token_prices_count=$(psqlq "SELECT COUNT(*) FROM token_prices_by_block;")
token_prices_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, asset, quote, block_number, source)), md5(''))
FROM (
  SELECT chain_id, asset, quote, block_number, source,
         md5(concat_ws('§',
           chain_id::text,
           asset,
           quote,
           block_number::text,
           block_hash,
           block_timestamp::text,
           price::text,
           source,
           COALESCE(pool_address, ''),
           COALESCE(oracle_address, ''),
           COALESCE(jsonb_strip_nulls(raw)::text, '')
         )) AS row_md5
  FROM token_prices_by_block
) s;")

stake_count=$(psqlq "SELECT COUNT(*) FROM stake_balances_by_block;")
stake_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, delegator_address, block_number)), md5(''))
FROM (
  SELECT chain_id, delegator_address, block_number,
         md5(concat_ws('§',
           chain_id::text,
           delegator_address,
           delegate_address,
           block_number::text,
           block_timestamp::text,
           block_hash,
           bonded_principal::text,
           COALESCE(pending_stake::text, ''),
           COALESCE(pending_fees::text, ''),
           COALESCE(pending_round::text, ''),
           source,
           COALESCE(jsonb_strip_nulls(raw_call)::text, ''),
           COALESCE(triggering_event_id::text, '')
         )) AS row_md5
  FROM stake_balances_by_block
) s;")

delegator_registry_count=$(psqlq "SELECT COUNT(*) FROM delegator_registry;")
delegator_registry_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, delegator_address)), md5(''))
FROM (
  SELECT chain_id, delegator_address,
         md5(concat_ws('§',
           chain_id::text,
           delegator_address,
           first_bond_block::text,
           first_bond_event_id::text,
           last_seen_block::text,
           last_seen_event_id::text,
           is_active::text
         )) AS row_md5
  FROM delegator_registry
) s;")

orchestrator_profile_count=$(psqlq "SELECT COUNT(*) FROM orchestrator_profile;")
orchestrator_profile_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, address)), md5(''))
FROM (
  SELECT chain_id, address,
         md5(concat_ws('§',
           chain_id::text,
           address,
           total_stake::text,
           latest_fee_cut_percent::text,
           latest_reward_cut_percent::text,
           latest_fee_share_percent::text,
           is_active::text,
           COALESCE(last_lifecycle_event_at::text, ''),
           as_of_block::text,
           COALESCE(as_of_round::text, ''),
           last_event_id::text,
           COALESCE(service_uri, '')
         )) AS row_md5
  FROM orchestrator_profile
) s;")

broadcaster_profile_count=$(psqlq "SELECT COUNT(*) FROM broadcaster_profile;")
broadcaster_profile_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, address)), md5(''))
FROM (
  SELECT chain_id, address,
         md5(concat_ws('§',
           chain_id::text,
           address,
           latest_deposit::text,
           latest_reserve::text,
           unlock_in_progress::text,
           as_of_block::text,
           last_event_id::text
         )) AS row_md5
  FROM broadcaster_profile
) s;")

orch_payouts_daily_count=$(psqlq "SELECT COUNT(*) FROM orch_payouts_daily;")
orch_payouts_daily_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, day_utc, orchestrator_address, valuation_version, broadcaster_kind)), md5(''))
FROM (
  SELECT chain_id, day_utc, orchestrator_address, valuation_version, broadcaster_kind,
         md5(concat_ws('§',
           chain_id::text,
           day_utc::text,
           orchestrator_address,
           valuation_version,
           broadcaster_kind,
           ticket_count::text,
           sum_face_value_native::text,
           sum_face_value_usd::text,
           sum_commission_native::text,
           sum_commission_usd::text,
           sum_delegators_share_native::text,
           sum_delegators_share_usd::text,
           distinct_gateways::text,
           usd_rows_priced::text,
           source_max_event_id::text
         )) AS row_md5
  FROM orch_payouts_daily
) s;")

orch_rewards_daily_count=$(psqlq "SELECT COUNT(*) FROM orch_rewards_daily;")
orch_rewards_daily_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, day_utc, orchestrator_address, valuation_version)), md5(''))
FROM (
  SELECT chain_id, day_utc, orchestrator_address, valuation_version,
         md5(concat_ws('§',
           chain_id::text,
           day_utc::text,
           orchestrator_address,
           valuation_version,
           reward_event_count::text,
           sum_total_tokens::text,
           sum_total_tokens_usd::text,
           sum_orch_tokens::text,
           sum_orch_tokens_usd::text,
           sum_delegators_tokens::text,
           sum_delegators_tokens_usd::text,
           usd_rows_priced::text,
           source_max_event_id::text
         )) AS row_md5
  FROM orch_rewards_daily
) s;")

tickets_daily_count=$(psqlq "SELECT COUNT(*) FROM tickets_daily;")
tickets_daily_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY chain_id, day_utc, broadcaster_kind)), md5(''))
FROM (
  SELECT chain_id, day_utc, broadcaster_kind,
         md5(concat_ws('§',
           chain_id::text,
           day_utc::text,
           broadcaster_kind,
           ticket_count::text,
           distinct_orchestrators::text,
           distinct_gateways::text,
           source_max_event_id::text
         )) AS row_md5
  FROM tickets_daily
) s;")

reorg_events_count=$(psqlq "SELECT COUNT(*) FROM reorg_events;")
reorg_events_md5=$(psqlq "
SELECT COALESCE(md5(string_agg(row_md5, '' ORDER BY id)), md5(''))
FROM (
  SELECT id,
         md5(concat_ws('§',
           chain_id::text,
           divergence_block::text,
           depth::text,
           array_to_string(old_block_hashes, '|'),
           array_to_string(new_block_hashes, '|'),
           affected_event_count::text,
           COALESCE(notes, '')
         )) AS row_md5
  FROM reorg_events
) s;")

cat > "$OUT" <<EOF
{
  "raw_protocol_events": {"count": $raw_count, "md5": "$raw_md5"},
  "decode_failures": {"count": $decode_failures_count, "md5": "$decode_failures_md5"},
  "event_valuations": {"count": $event_valuations_count, "md5": "$event_valuations_md5"},
  "valuation_attempts": {"count": $valuation_attempts_count, "md5": "$valuation_attempts_md5"},
  "token_prices_by_block": {"count": $token_prices_count, "md5": "$token_prices_md5"},
  "stake_balances_by_block": {"count": $stake_count, "md5": "$stake_md5"},
  "delegator_registry": {"count": $delegator_registry_count, "md5": "$delegator_registry_md5"},
  "orchestrator_profile": {"count": $orchestrator_profile_count, "md5": "$orchestrator_profile_md5"},
  "broadcaster_profile": {"count": $broadcaster_profile_count, "md5": "$broadcaster_profile_md5"},
  "orch_payouts_daily": {"count": $orch_payouts_daily_count, "md5": "$orch_payouts_daily_md5"},
  "orch_rewards_daily": {"count": $orch_rewards_daily_count, "md5": "$orch_rewards_daily_md5"},
  "tickets_daily": {"count": $tickets_daily_count, "md5": "$tickets_daily_md5"},
  "reorg_events": {"count": $reorg_events_count, "md5": "$reorg_events_md5"}
}
EOF
