-- 017_event_valuations_terminal_failures
-- Option A: every valuable event produces an event_valuations outcome row,
-- including terminal failures where numeric USD price fields are unavailable.

ALTER TABLE event_valuations
    DROP CONSTRAINT event_valuations_status_check;

ALTER TABLE event_valuations
    ALTER COLUMN native_usd_price DROP NOT NULL,
    ALTER COLUMN amount_usd DROP NOT NULL;

ALTER TABLE event_valuations
    ADD CONSTRAINT event_valuations_status_check
    CHECK (
        status IN (
            'priced',
            'priced_with_warning',
            'failed_missing_pool',
            'failed_missing_oracle',
            'failed_sequencer_outage'
        )
    );

WITH latest_terminal AS (
    SELECT DISTINCT ON (a.event_id, a.valuation_version, a.asset)
        a.event_id,
        a.valuation_version,
        a.asset,
        a.result_status,
        a.error_detail
    FROM valuation_attempts a
    WHERE a.result_status IN (
        'failed_missing_pool',
        'failed_missing_oracle',
        'failed_sequencer_outage'
    )
    ORDER BY a.event_id, a.valuation_version, a.asset, a.attempt_number DESC
)
INSERT INTO event_valuations (
    event_id,
    valuation_version,
    asset,
    pricing_method,
    chain_id,
    block_number,
    amount_native,
    native_usd_price,
    amount_usd,
    pricing_chain,
    status,
    source
)
SELECT
    r.id AS event_id,
    lt.valuation_version,
    lt.asset,
    CASE
        WHEN lt.asset = 'ETH' THEN 'chainlink_eth_usd'
        WHEN lt.asset = 'LPT'
         AND lt.valuation_version LIKE '%_degraded_spot_pre_cardinality'
            THEN 'uniswap_v3_spot_x_chainlink_eth'
        ELSE 'uniswap_v3_twap_30min_x_chainlink_eth'
    END AS pricing_method,
    r.chain_id,
    r.block_number,
    CASE
        WHEN r.event_name = 'EarningsClaimed' AND lt.asset = 'LPT'
            THEN COALESCE((r.raw_event -> 'decoded' ->> 'rewards')::numeric / 1000000000000000000::numeric, 0)
        WHEN r.event_name = 'EarningsClaimed' AND lt.asset = 'ETH'
            THEN COALESCE((r.raw_event -> 'decoded' ->> 'fees')::numeric / 1000000000000000000::numeric, 0)
        ELSE r.amount_normalized
    END AS amount_native,
    NULL AS native_usd_price,
    NULL AS amount_usd,
    COALESCE(lt.error_detail, '{}'::jsonb) AS pricing_chain,
    lt.result_status AS status,
    CASE
        WHEN lt.asset = 'ETH' THEN 'chainlink_dual_rpc'
        ELSE 'uniswap_v3_dual_rpc'
    END AS source
FROM latest_terminal lt
JOIN raw_protocol_events r
  ON r.id = lt.event_id
LEFT JOIN event_valuations v
  ON v.event_id = lt.event_id
 AND v.valuation_version = lt.valuation_version
 AND v.asset = lt.asset
WHERE v.event_id IS NULL
ON CONFLICT (event_id, valuation_version, asset) DO NOTHING;
