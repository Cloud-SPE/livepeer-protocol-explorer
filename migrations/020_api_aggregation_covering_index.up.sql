CREATE INDEX idx_valuations_api_event_version_asset_covering
ON event_valuations (event_id, valuation_version, asset)
INCLUDE (amount_native, amount_usd);
