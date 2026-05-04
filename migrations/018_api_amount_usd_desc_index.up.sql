CREATE INDEX idx_valuations_version_amount_usd_event
ON event_valuations (valuation_version, amount_usd DESC, event_id DESC)
WHERE amount_usd IS NOT NULL;
